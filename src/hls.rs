//! B3a：最小 HLS 直链下载（m3u8 master/media、相对地址、拒绝加密）。
//!
//! 只在 `--hls` 时走这条路：不接队列/浏览器嗅探，不做 DASH/DRM，也不写 `.dlm.json`。
//! 分片顺序写进同一个输出文件；失败保留半截文件，下次从头开始（不是断点续传）。

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result, bail, ensure};
use reqwest::Url;
use reqwest::blocking::Client;

use crate::consts::USER_AGENT;
use crate::engine::{self, Config, Report};
use crate::probe;
use crate::rate::RateLimiter;
use crate::task::Task;

const READ_CHUNK: usize = 64 * 1024;
const HLS_TIMEOUT_SECS: u64 = 60;
const HLS_CLIENT_TIMEOUT_SECS: u64 = 120;
const HLS_CONNECT_TIMEOUT_SECS: u64 = 15;
const SEGMENT_ATTEMPTS: u32 = 3;
const MAX_SEGMENT_BYTES: usize = 256 * 1024 * 1024;

/// HLS 顺序下载不依赖连接池；明确禁用空闲复用，避免"服务端关连接、客户端复用旧池"的竞态。
fn build_hls_client() -> Result<Client> {
    Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(HLS_CLIENT_TIMEOUT_SECS))
        .connect_timeout(std::time::Duration::from_secs(HLS_CONNECT_TIMEOUT_SECS))
        .tcp_nodelay(true)
        .pool_max_idle_per_host(0)
        .build()
        .context("建 HLS HTTP 客户端失败")
}

#[derive(Debug, PartialEq, Eq)]
enum Playlist {
    Master(Vec<Variant>),
    Media(MediaPlaylist),
}

#[derive(Debug, PartialEq, Eq)]
struct Variant {
    bandwidth: u64,
    uri: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ByteRange {
    offset: u64,
    length: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MediaSegment {
    uri: String,
    range: Option<ByteRange>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct MediaPlaylist {
    init: Option<MediaSegment>,
    segments: Vec<MediaSegment>,
}

/// `--hls` 且没给 `--out-name` 时的默认落点名：`playlist.m3u8` → `playlist.ts`。
fn default_name(url: &str) -> String {
    let tail = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .rsplit('/')
        .next()
        .unwrap_or("");
    let lower = tail.to_ascii_lowercase();
    if let Some(stem) = lower.strip_suffix(".m3u8")
        && !stem.is_empty()
    {
        return format!("{}.ts", &tail[..stem.len()]);
    }
    "download.ts".to_string()
}

fn attr(line: &str, key: &str) -> Option<String> {
    let (_, rest) = line.split_once(':')?;
    for part in rest.split(',') {
        let (k, v) = part.split_once('=')?;
        if k.trim().eq_ignore_ascii_case(key) {
            return Some(v.trim().trim_matches('"').to_string());
        }
    }
    None
}

fn parse_range(raw: &str) -> Result<ByteRange> {
    let (length, offset) = match raw.split_once('@') {
        Some((len, off)) => (len, Some(off)),
        None => (raw, None),
    };
    let length: u64 = length.trim().parse().context("HLS BYTERANGE 长度不合法")?;
    let offset: u64 = match offset {
        Some(off) => off.trim().parse().context("HLS BYTERANGE 偏移不合法")?,
        None => 0,
    };
    ensure!(length > 0, "HLS BYTERANGE 长度必须大于 0");
    Ok(ByteRange { offset, length })
}

fn parse_playlist(text: &str) -> Result<Playlist> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut variants: Vec<Variant> = Vec::new();
    let mut media = MediaPlaylist::default();
    let mut pending_bandwidth: Option<u64> = None;
    let mut pending_range: Option<ByteRange> = None;
    let mut last_range_end = 0u64;
    let mut expect_segment = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("#EXT-X-STREAM-INF:") {
            let bandwidth = attr(line, "BANDWIDTH")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            pending_bandwidth = Some(bandwidth);
            continue;
        }
        if line.starts_with("#EXT-X-KEY:") {
            let method = attr(line, "METHOD").unwrap_or_default();
            ensure!(
                method.eq_ignore_ascii_case("NONE"),
                "加密 HLS 不支持（#EXT-X-KEY METHOD={method}）；B3a 只做明文分片"
            );
            continue;
        }
        if line.starts_with("#EXT-X-MAP:") {
            if let Some(uri) = attr(line, "URI") {
                let range = match attr(line, "BYTERANGE") {
                    Some(raw) => Some(parse_range(&raw)?),
                    None => None,
                };
                media.init = Some(MediaSegment { uri, range });
            }
            continue;
        }
        if line.starts_with("#EXT-X-BYTERANGE:") {
            let raw = line.trim_start_matches("#EXT-X-BYTERANGE:").trim();
            let mut range = parse_range(raw)?;
            if !raw.contains('@') {
                range.offset = last_range_end;
            }
            pending_range = Some(range);
            continue;
        }
        if line.starts_with("#EXTINF:") {
            expect_segment = true;
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        if let Some(bandwidth) = pending_bandwidth.take() {
            variants.push(Variant {
                bandwidth,
                uri: line.to_string(),
            });
            continue;
        }
        if expect_segment {
            let range = pending_range.take();
            if let Some(range) = range {
                last_range_end = range.offset.saturating_add(range.length);
            }
            media.segments.push(MediaSegment {
                uri: line.to_string(),
                range,
            });
            expect_segment = false;
        }
    }
    if !variants.is_empty() {
        return Ok(Playlist::Master(variants));
    }
    ensure!(!media.segments.is_empty(), "HLS 播放列表里没有分片");
    Ok(Playlist::Media(media))
}

fn fetch_text(
    client: &Client,
    task: &Task,
    url: &Url,
    limiter: &RateLimiter,
) -> Result<(String, Url)> {
    let req = probe::with_contract_headers(client.get(url.as_str()), task)?
        .timeout(std::time::Duration::from_secs(HLS_TIMEOUT_SECS));
    ensure!(
        !probe::landed_url_is_refused(url.as_str(), task.password.as_deref()),
        "HLS 清单 URL 未通过凭据闸（不回显值）"
    );
    let resp = req.send().context("请求 HLS 清单失败")?;
    ensure!(
        resp.status().is_success(),
        "HLS 清单返回 HTTP {}",
        resp.status().as_u16()
    );
    let final_url = resp.url().clone();
    let text = resp.text().context("读 HLS 清单失败")?;
    limiter.consume(text.len() as u64);
    Ok((text, final_url))
}

fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

fn fetch_segment_once(
    client: &Client,
    task: &Task,
    url: &Url,
    range: Option<ByteRange>,
    limiter: &RateLimiter,
) -> Result<Vec<u8>> {
    let mut req = probe::with_contract_headers(client.get(url.as_str()), task)?
        .timeout(std::time::Duration::from_secs(HLS_TIMEOUT_SECS));
    if let Some(range) = range {
        req = req.header(
            reqwest::header::RANGE,
            format!(
                "bytes={}-{}",
                range.offset,
                range.offset.saturating_add(range.length).saturating_sub(1)
            ),
        );
    }
    ensure!(
        !probe::landed_url_is_refused(url.as_str(), task.password.as_deref()),
        "HLS 分片 URL 未通过凭据闸（不回显值）"
    );
    let mut resp = req.send().context("请求 HLS 分片失败")?;
    if range.is_some() {
        ensure!(
            resp.status() == reqwest::StatusCode::PARTIAL_CONTENT,
            "HLS 分片没有按 Range 返回（HTTP {}）",
            resp.status().as_u16()
        );
    } else {
        ensure!(
            resp.status().is_success(),
            "HLS 分片返回 HTTP {}",
            resp.status().as_u16()
        );
    }
    let mut data = Vec::new();
    let mut buf = vec![0u8; READ_CHUNK];
    loop {
        let n = resp.read(&mut buf).context("读 HLS 分片失败")?;
        if n == 0 {
            break;
        }
        ensure!(
            data.len().saturating_add(n) <= MAX_SEGMENT_BYTES,
            "HLS 分片超过 {} MiB 上限",
            MAX_SEGMENT_BYTES / (1024 * 1024)
        );
        limiter.consume(n as u64);
        data.extend_from_slice(&buf[..n]);
    }
    Ok(data)
}

/// 一个分片**完整读进内存后再写输出**：中途超时不会把半片留在成片里，重试也不会重复写。
fn append_body(
    client: &Client,
    task: &Task,
    url: &Url,
    range: Option<ByteRange>,
    file: &mut File,
    limiter: &RateLimiter,
) -> Result<u64> {
    let mut last = None;
    for attempt in 1..=SEGMENT_ATTEMPTS {
        match fetch_segment_once(client, task, url, range, limiter) {
            Ok(data) => {
                file.write_all(&data).context("写 HLS 输出失败")?;
                return Ok(data.len() as u64);
            }
            Err(err) => {
                last = Some(err);
                if attempt < SEGMENT_ATTEMPTS {
                    std::thread::sleep(std::time::Duration::from_secs(u64::from(attempt)));
                }
            }
        }
    }
    Err(last.expect("至少跑过一次分片请求"))
}

fn report(started: Instant, delivered: u64, ok: bool, message: Option<String>) -> Report {
    Report {
        // HLS 清单不告诉总字节数；失败时不要把"已落盘"冒充成"全长"。
        bytes_total: if ok { delivered } else { 0 },
        delivered,
        confirmed: delivered,
        duplicated: 0,
        retries: 0,
        elapsed: started.elapsed(),
        ok,
        budget_reached: false,
        trace: vec![],
        fresh_reason: None,
        message,
    }
}

fn ensure_parent_dir(target: &Path) -> Result<()> {
    let dir = target.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir).with_context(|| format!("建目标目录失败：{}", dir.display()))
}

fn run_inner(
    client: &Client,
    task: &Task,
    url: &Url,
    file: &mut File,
    limiter: &RateLimiter,
) -> Result<u64> {
    let (text, base) = fetch_text(client, task, url, limiter)?;
    // 凭据只按**原始任务 URL** 的 origin 判；playlist 可能被重定向到 CDN，
    // 拿最终 base 当同源会把原始 Cookie 送到 CDN。
    let origin = Url::parse(&task.url).context("HLS 原始 URL 解析失败")?;
    let mut total = 0u64;
    let playlist = parse_playlist(&text)?;
    let media = match playlist {
        Playlist::Master(variants) => {
            let chosen = variants
                .into_iter()
                .max_by_key(|v| v.bandwidth)
                .context("master 播放列表没有可用变体")?;
            let media_url = base
                .join(&chosen.uri)
                .context("master 播放列表里的 URI 解析失败")?;
            let (text, media_base) = fetch_text(client, task, &media_url, limiter)?;
            let Playlist::Media(media) = parse_playlist(&text)? else {
                bail!("master 指向的仍是一个 master 播放列表（B3a 只支持一层）");
            };
            (media, media_base)
        }
        Playlist::Media(media) => (media, base),
    };
    let (media, base) = media;
    if let Some(init) = &media.init {
        let init_url = base.join(&init.uri).context("初始化分片 URI 解析失败")?;
        let filtered = filter_cross_origin(task, &origin, &init_url);
        total = total.saturating_add(append_body(
            client, &filtered, &init_url, init.range, file, limiter,
        )?);
    }
    for segment in &media.segments {
        let segment_url = base.join(&segment.uri).context("分片 URI 解析失败")?;
        let filtered = filter_cross_origin(task, &origin, &segment_url);
        total = total.saturating_add(append_body(
            client,
            &filtered,
            &segment_url,
            segment.range,
            file,
            limiter,
        )?);
    }
    Ok(total)
}

fn filter_cross_origin(task: &Task, base: &Url, target: &Url) -> Task {
    let mut filtered = task.clone();
    if !same_origin(base, target) {
        filtered.referer = None;
        filtered.cookies = None;
        filtered.user = None;
        filtered.password = None;
    }
    filtered
}

pub fn run(task: &Task, cfg: &Config, started: Instant) -> Result<Report> {
    ensure!(
        !probe::landed_url_is_refused(&task.url, task.password.as_deref()),
        "HLS URL 未通过凭据闸（不回显值）"
    );
    let mut out_task = task.clone();
    if out_task.name.is_none() {
        out_task.name = Some(default_name(&task.url));
    }
    out_task.validate()?;
    let target = out_task.target_path();
    ensure_parent_dir(&target)?;
    let mut file = File::create(&target)
        .with_context(|| format!("建 HLS 输出文件失败：{}", target.display()))?;
    let client = build_hls_client()?;
    let limiter = RateLimiter::new(cfg.max_speed);
    let url = Url::parse(&task.url).context("HLS URL 解析失败")?;
    let result = run_inner(&client, task, &url, &mut file, &limiter);
    if let Err(err) = file.flush() {
        return Ok(report(
            started,
            0,
            false,
            Some(format!("HLS 输出 flush 失败：{err}")),
        ));
    }
    let delivered = fs::metadata(&target).map_or(0, |m| m.len());
    match result {
        Ok(_) => {
            if let Some(want) = &cfg.verify_sha256 {
                match engine::sha256_of_path(&target) {
                    Ok(got) if !engine::hash_matches(&got, want) => {
                        return Ok(report(
                            started,
                            delivered,
                            false,
                            Some(format!(
                                "sha256 不符：算得 {got}，期望 {want}。目标文件保留，不自动删"
                            )),
                        ));
                    }
                    Err(err) => {
                        return Ok(report(started, delivered, false, Some(format!("{err:#}"))));
                    }
                    Ok(_) => {}
                }
            }
            Ok(report(started, delivered, true, None))
        }
        Err(err) => Ok(report(started, delivered, false, Some(format!("{err:#}")))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn master_playlists_choose_the_highest_bandwidth_and_resolve_relative_uris() {
        let text = "#EXTM3U\n\
            #EXT-X-STREAM-INF:BANDWIDTH=100,CODECS=\"avc1\"\nlow/index.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=900,RESOLUTION=1920x1080\nhigh/index.m3u8\n";
        let Playlist::Master(variants) = parse_playlist(text).unwrap() else {
            panic!("master")
        };
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0].bandwidth, 100);
        assert_eq!(variants[1].uri, "high/index.m3u8");
        let base = Url::parse("https://x/a/master.m3u8").unwrap();
        assert_eq!(
            base.join(&variants[1].uri).unwrap().as_str(),
            "https://x/a/high/index.m3u8"
        );
    }

    #[test]
    fn media_playlists_keep_segments_in_order_and_accept_method_none() {
        let text = "#EXTM3U\n#EXT-X-KEY:METHOD=NONE\n#EXTINF:1,\na.ts\n#EXTINF:2,\nsub/b.ts\n";
        let Playlist::Media(media) = parse_playlist(text).unwrap() else {
            panic!("media")
        };
        let uris: Vec<&str> = media.segments.iter().map(|s| s.uri.as_str()).collect();
        assert_eq!(uris, vec!["a.ts", "sub/b.ts"]);
        assert!(media.segments.iter().all(|s| s.range.is_none()));
        assert!(media.init.is_none());
    }

    #[test]
    fn encrypted_playlists_are_refused() {
        let err =
            parse_playlist("#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"k\"\n#EXTINF:1,\na.ts\n")
                .unwrap_err()
                .to_string();
        assert!(err.contains("加密 HLS"), "{err}");
    }

    #[test]
    fn byte_range_segments_and_maps_are_parsed() {
        let text = "#EXTM3U\n\
            #EXT-X-MAP:URI=\"main.mp4\",BYTERANGE=\"719@0\"\n\
            #EXTINF:6,\n#EXT-X-BYTERANGE:274201@719\nmain.mp4\n\
            #EXTINF:6,\n#EXT-X-BYTERANGE:280451\nmain.mp4\n";
        let Playlist::Media(media) = parse_playlist(text).unwrap() else {
            panic!("media")
        };
        let init = media.init.expect("map");
        assert_eq!(init.uri, "main.mp4");
        assert_eq!(
            init.range,
            Some(ByteRange {
                offset: 0,
                length: 719
            })
        );
        assert_eq!(
            media.segments[0].range,
            Some(ByteRange {
                offset: 719,
                length: 274201
            })
        );
        assert_eq!(
            media.segments[1].range,
            Some(ByteRange {
                offset: 274_920,
                length: 280_451
            }),
            "省略 @offset 时从上一片末尾续"
        );
    }

    #[test]
    fn default_names_strip_the_playlist_suffix() {
        assert_eq!(default_name("https://x/path/movie.m3u8?t=1"), "movie.ts");
        assert_eq!(default_name("https://x/path/"), "download.ts");
        assert_eq!(default_name("https://x/path/master.M3U8"), "master.ts");
    }

    #[test]
    fn byte_range_requests_send_range_and_require_206() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase();
            assert!(request.contains("range: bytes=719-1018"), "{request}");
            write!(
                stream,
                "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 719-1018/2000\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc"
            )
            .unwrap();
        });
        let client = build_hls_client().unwrap();
        let task = Task {
            url: format!("http://127.0.0.1:{port}/main.mp4"),
            referer: None,
            cookies: None,
            data_file: None,
            user: None,
            password: None,
            dir: ".".into(),
            name: Some("x.mp4".into()),
            flags: crate::consts::FLAG_HLS,
        };
        let url = Url::parse(&task.url).unwrap();
        let body = fetch_segment_once(
            &client,
            &task,
            &url,
            Some(ByteRange {
                offset: 719,
                length: 300,
            }),
            &RateLimiter::new(0),
        )
        .unwrap();
        handle.join().unwrap();
        assert_eq!(body, b"abc");
    }

    #[test]
    fn cross_origin_segments_drop_credentials_and_referer() {
        let task = Task {
            url: "https://origin.example/master.m3u8".into(),
            referer: Some("https://ref.example/".into()),
            cookies: Some("sid=1".into()),
            data_file: None,
            user: Some("u".into()),
            password: Some("p".into()),
            dir: ".".into(),
            name: Some("x.ts".into()),
            flags: crate::consts::FLAG_HLS,
        };
        let base = Url::parse("https://origin.example/master.m3u8").unwrap();
        let same = filter_cross_origin(
            &task,
            &base,
            &Url::parse("https://origin.example/a.ts").unwrap(),
        );
        assert_eq!(same.cookies.as_deref(), Some("sid=1"));
        let cross = filter_cross_origin(
            &task,
            &base,
            &Url::parse("https://cdn.example/a.ts").unwrap(),
        );
        assert!(cross.cookies.is_none());
        assert!(cross.referer.is_none());
        assert!(cross.user.is_none());
        assert!(cross.password.is_none());
    }
}
