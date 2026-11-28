//! 视频下载：把 yt-dlp 当作**外部下载后端**，而不是自己重写一遍站点解析。
//!
//! ## 为什么要它
//!
//! 2026-11-27 用户拿 B 站视频页地址来下，落盘的是一个网页（已由 `probe::looks_like_html` 挡住）。
//! 挡住的下一步得有出路 —— 视频站的分片、DASH、签名、防盗链每一条都不该由下载器自己啃。
//!
//! 这一层只做三件事：
//! 1. **找到 yt-dlp**（用户自己的，或同目录便携版，或 `DLM_YTDLP` 指定的）；
//! 2. 先问一次元信息（标题/容器/体积）—— 有了它，队列里才有像样的文件名；
//! 3. 跑下载并把它的进度读成我们的进度。
//!
//! ## 有意的取舍
//!
//! - **不随包分发 yt-dlp**：它有自己极快的更新节奏（站点一改就要升），
//!   随包等于把一个会过期的二进制焊进发布物里。
//! - **不做站点专用解析**：`video-knowledge-workbench` 那份 `remote.py` 里有抖音
//!   `RENDER_DATA`/`_ROUTER_DATA` 那类补丁，但那是因为它要**结构化内容**；
//!   下载器只要文件，yt-dlp 自己的站点支持已经足够。
//! - **进度只为好看**：`[download] xx%` 解析失败就当没有进度，不猜、不补。

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

/// 用它指定一个明确的 yt-dlp 路径（测试与便携用法）。**优先级最高**。
pub const ENV_YTDLP: &str = "DLM_YTDLP";

/// 元信息里我们真正用得上的三样。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoMeta {
    /// 标题（用来当文件名的主体）。
    pub title: String,
    /// 容器/扩展名，例如 `mp4`、`webm`。
    pub ext: String,
    /// 体积。yt-dlp 有时只给估算值（`filesize_approx`），那就标着是估算。
    pub filesize: Option<u64>,
    /// 体积是估算的吗（不含音轨的单流估算很常见）。
    pub approx: bool,
}

/// 找 yt-dlp：`DLM_YTDLP` → 同目录便携版 → PATH。
///
/// 顺序不能反：环境变量是"这次就用这个"的硬指定，同目录是"绿色版一起拷来的"，
/// PATH 才是"用户机器上装的"。前两者更具体。
pub fn find_ytdlp() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os(ENV_YTDLP) {
        let path = PathBuf::from(explicit);
        // **故意不检查是否为文件**：测试会用别的方式给一个存在的可执行体，
        // 而用户也可能给个别名/链接。真的跑不起来时错误会由 `Command` 报出来。
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        for name in ["yt-dlp.exe", "yt-dlp"] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        for name in ["yt-dlp.exe", "yt-dlp"] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// 找不到 yt-dlp 时给用户的那段话。
///
/// 单独成函数是因为**它要说的事比"缺个依赖"多**：为什么需要它、去哪儿装、
/// 以及"你也可以直接用 `--allow-html` 下那个网页"。用例钉住这三点。
pub fn missing_hint() -> String {
    [
        "没找到 yt-dlp —— 视频下载要靠它（本站不内置，因为它更新很勤）。",
        "安装任选一种：",
        "  winget install yt-dlp",
        "  pip install yt-dlp",
        "或把它放到 dlm.exe 同目录，或用环境变量 DLM_YTDLP 指定路径。",
    ]
    .join("\n")
}

/// 这个地址看起来是**视频页**吗（而不是一个文件或普通网页）。
///
/// 用途只有一个：新建任务时把「作为视频下载」**预先勾上**，省得用户先失败一次
/// 才知道要点它。所以判断要**宽松**（认站点），宁可多勾一次 —— 勾错了顶多是
/// yt-dlp 报一句"不支持的站点"，而不勾的一边是下回来一个网页。
///
/// 不判断"是不是以 .mp4 结尾"：那正好是直链，不该走 yt-dlp。
pub fn looks_like_video_page(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return false;
    }
    // 直链视频文件：交给普通下载（分段/续传都更好用）。
    const DIRECT: [&str; 6] = [".mp4", ".mkv", ".flv", ".webm", ".mov", ".m4v"];
    let path = lower.split(['?', '#']).next().unwrap_or(&lower);
    if DIRECT.iter().any(|ext| path.ends_with(ext)) {
        return false;
    }
    const HOSTS: [&str; 12] = [
        "bilibili.com",
        "b23.tv",
        "youtube.com",
        "youtu.be",
        "douyin.com",
        "iesdouyin.com",
        "xiaohongshu.com",
        "xhslink.com",
        "vimeo.com",
        "twitch.tv",
        "dailymotion.com",
        "ixigua.com",
    ];
    HOSTS.iter().any(|host| lower.contains(host))
}

/// 问元信息用的参数。
pub fn argv_info(url: &str) -> Vec<String> {
    vec![
        "--dump-single-json".into(),
        "--no-playlist".into(),
        "--no-warnings".into(),
        // 只问元信息，别真下：`--simulate` 让它只做提取。
        "--simulate".into(),
        "--".into(),
        url.to_string(),
    ]
}

/// 视频任务的落点名字：队列是按 `task.name` 找文件的，所以这个名字必须**确定**。
///
/// 踩过的坑：第一版这里只给 `%(title)s.%(ext)s`，yt-dlp 按标题自己起名，
/// 而队列按 `--out-name` 找 —— 下完了桌面端却认不出文件，看着就是"视频下载无效"。
pub fn force_out_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return "video.mp4".to_string();
    }
    // 强制合并成 mp4，所以没扩展名（或扩展名不是视频容器）时补 `.mp4`。
    let lower = trimmed.to_ascii_lowercase();
    const VIDEO_EXT: [&str; 6] = [".mp4", ".mkv", ".webm", ".mov", ".flv", ".m4v"];
    if VIDEO_EXT.iter().any(|ext| lower.ends_with(ext)) {
        trimmed.to_string()
    } else {
        format!("{trimmed}.mp4")
    }
}

/// 下载用的参数：先选最好的视频+音频再合并成 mp4。
///
/// `name` 给了就**钉死**输出文件名（队列要靠它认领文件）；没给才退回标题。
pub fn argv_download(url: &str, dir: &Path, name: Option<&str>) -> Vec<String> {
    let template = match name {
        Some(n) => dir.join(force_out_name(n)),
        None => dir.join("%(title)s.%(ext)s"),
    };
    vec![
        "-f".into(),
        "bv*+ba/b".into(),
        "--merge-output-format".into(),
        "mp4".into(),
        "--no-playlist".into(),
        "--newline".into(),
        "-o".into(),
        template.display().to_string(),
        // 钉死名字时 yt-dlp 仍可能给"临时后缀"，加这两条让它最后一定落到模板上。
        "--force-overwrites".into(),
        "--".into(),
        url.to_string(),
    ]
}

/// 把 `--dump-single-json` 的输出读成 [`VideoMeta`]。
pub fn parse_info(json: &str) -> Result<VideoMeta> {
    let value: serde_json::Value =
        serde_json::from_str(json).context("yt-dlp 交回的元信息不是合法 JSON")?;
    let title = value
        .get("title")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("video")
        .to_string();
    let ext = value
        .get("ext")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("mp4")
        .to_string();
    let exact = value.get("filesize").and_then(|v| v.as_u64());
    let guess = value.get("filesize_approx").and_then(|v| v.as_u64());
    Ok(VideoMeta {
        title,
        ext,
        filesize: exact.or(guess),
        approx: exact.is_none() && guess.is_some(),
    })
}

/// 从 yt-dlp 的一行输出里读进度（0..=100）。
///
/// 只认 `[download] 12.3%` 这种形状：
/// - `[download] Destination: …`、`[info] …` 等一律返回 `None`（不是进度）；
/// - `100%` 也走这里，所以"下完"就是 100.0；
/// - 解析不出数字就当没有进度 —— 不猜。
pub fn parse_progress(line: &str) -> Option<f64> {
    let rest = line.trim_start().strip_prefix("[download]")?.trim_start();
    let pct_part = rest.split_whitespace().next()?;
    let num = pct_part.strip_suffix('%')?;
    let value: f64 = num.parse().ok()?;
    if !(0.0..=100.0).contains(&value) {
        return None;
    }
    Some(value)
}

/// 跑一遍视频下载。返回给 `main` 的退出码（0 成功，1 失败）。
pub fn run(url: &str, dir: &Path, name: Option<&str>, quiet: bool, wants_json: bool) -> Result<u8> {
    let Some(tool) = find_ytdlp() else {
        return Err(crate::failure::Failure::new(
            crate::json_out::Code::ToolMissing,
            "未找到 yt-dlp，无法下载视频。",
        )
        .with_hint(
            "安装 yt-dlp（winget install yt-dlp 或 pip install yt-dlp），或把它放到 dlm.exe 同目录，或用环境变量 DLM_YTDLP 指定路径。",
        )
        .into());
    };

    // ① 元信息：先有名字，日志和落点才说得清。
    let info_out = spawn_capture(&tool, &argv_info(url))?;
    if !info_out.status.success() {
        bail!(
            "yt-dlp 提取信息失败（退出码 {:?}）：\n{}",
            info_out.status.code(),
            tail(&info_out.stderr)
        );
    }
    let info_json = String::from_utf8_lossy(&info_out.stdout);
    let meta = parse_info(&info_json)?;
    // **`--quiet` 只管进度行，不管"这是什么"。**
    //
    // 踩过的坑（2026-11-28）：GUI 派发永远带 `--quiet`，于是视频任务的日志
    // 全程零字节 —— 用户看到的是一条没有大小、没有进度、日志空白的任务，
    // 自然以为"视频下载无效"。
    if !wants_json {
        let size = match meta.filesize {
            Some(bytes) => format!("约 {}", human(bytes)),
            None => "体积未知".to_string(),
        };
        println!("视频下载：{}（{}，{}）", meta.title, meta.ext, size);
        println!("工具：{}", tool.display());
    }

    // ② 下载：进度逐行读出来。
    let mut child = Command::new(&tool);
    child
        .args(argv_download(url, dir, name))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    crate::proc::no_window(&mut child);
    // 让 Python 的 stdio 一律 UTF-8。
    //
    // 实测（2026-11-27）：yt-dlp 的 stdout 被接成管道时，Python 按系统本地编码
    // （中文 Windows 上是 GBK）写进度行；一旦输出里有它编码不了的东西就
    // `ERROR: Unable to download video: [Errno 22] Invalid argument`，
    // **整个下载被打断**，只留一个 1024 字节的 `.part`。中文标题是国内站点的常态，
    // 所以这不是边角情况 —— 用户报的「视频下载无效」有一半是它。
    child
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8");
    let mut child = child
        .spawn()
        .with_context(|| format!("启动 yt-dlp 失败（{}）", tool.display()))?;

    // stderr 必须**真的读出来**：只 pipe 不读有两个后果 ——
    // ① 管道缓冲区（约 64 KiB）填满后 yt-dlp 会阻塞在 write 上，下载假死；
    // ② 失败时手里一句原因都没有，只能报个退出码（2026-11-27 就是这样，退出码 120 无从查起）。
    let stderr_tail = child.stderr.take().map(|pipe| {
        std::thread::spawn(move || {
            let mut kept: Vec<String> = Vec::new();
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                if line.trim().is_empty() {
                    continue;
                }
                kept.push(line);
                if kept.len() > 20 {
                    kept.remove(0);
                }
            }
            kept
        })
    });

    let mut last_pct: Option<f64> = None;
    // quiet 模式下已经打过的那个百分数（用来去重，不是 5% 的格）。
    let mut shown_step: i64 = -1;
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(pct) = parse_progress(&line) {
                last_pct = Some(pct);
                if wants_json {
                    continue;
                }
                if quiet {
                    // 每 5% 一档、独立成行：这几行会落进任务日志，
                    // 用户翻日志就能看出"在下、下到哪了"。
                    // 按"四舍五入后的百分数"去重：99.9 与 100 都会印成 100，
                    // 不该连着打两行。
                    let shown = pct.round() as i64;
                    if shown != shown_step {
                        shown_step = shown;
                        println!("进度 {shown}%");
                    }
                } else {
                    print!("\r{:>5.1}%", pct);
                    use std::io::Write;
                    let _ = std::io::stdout().flush();
                }
            }
        }
    }
    let status = child.wait().context("等待 yt-dlp 结束失败")?;
    if !quiet && !wants_json && last_pct.is_some() {
        // 原地覆盖那一行要换行收尾；quiet 模式每档都是独立行，不需要。
        println!();
    }

    if !status.success() {
        let tail = stderr_tail
            .and_then(|h| h.join().ok())
            .map(|lines| lines.join("\n"))
            .unwrap_or_default();
        if tail.trim().is_empty() {
            bail!("yt-dlp 下载失败（退出码 {:?}）", status.code());
        }
        bail!("yt-dlp 下载失败（退出码 {:?}）：\n{}", status.code(), tail);
    }
    if wants_json {
        println!(
            "{}",
            crate::json_out::to_line(&crate::json_out::ok(serde_json::json!({
                "video": true,
                "title": meta.title,
                "ext": meta.ext,
                "filesize": meta.filesize,
                "dir": dir.display().to_string(),
                "path": dir.join(name.map(force_out_name).unwrap_or_else(|| format!("{}.{}", meta.title, meta.ext)))
                    .display().to_string(),
            })))
        );
    } else {
        // 结尾也**不看 quiet**：日志要能回答"下到哪了"，这是排查的第一手材料。
        match name {
            Some(n) => println!("下载完成  目标：{}", dir.join(force_out_name(n)).display()),
            None => println!("下载完成  目标目录：{}", dir.display()),
        }
    }
    Ok(0)
}

fn spawn_capture(tool: &Path, args: &[String]) -> Result<std::process::Output> {
    let mut cmd = Command::new(tool);
    cmd.args(args)
        .stdin(Stdio::null())
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8");
    crate::proc::no_window(&mut cmd);
    cmd.output()
        .with_context(|| format!("启动 yt-dlp 失败（{}）", tool.display()))
}

/// 错误只留最后几行：yt-dlp 的 stderr 动辄几十行，抄全文没人看。
fn tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(3)..].join("\n")
}

fn human(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 元信息解析：缺字段不许炸，估算体积要标出来。
    #[test]
    fn info_parsing_falls_back_instead_of_failing() {
        let full = r#"{"title":"示例 视频","ext":"mp4","filesize":1048576}"#;
        let meta = parse_info(full).unwrap();
        assert_eq!(meta.title, "示例 视频");
        assert_eq!(meta.ext, "mp4");
        assert_eq!(meta.filesize, Some(1_048_576));
        assert!(!meta.approx, "有精确值时不该标成估算");

        let guessed = r#"{"title":"x","filesize_approx":2048}"#;
        let meta = parse_info(guessed).unwrap();
        assert_eq!(meta.filesize, Some(2048));
        assert!(meta.approx, "只有估算值时要说清楚是估算");
        assert_eq!(meta.ext, "mp4", "没给 ext 时退回 mp4");

        let bare = r#"{}"#;
        let meta = parse_info(bare).unwrap();
        assert_eq!(meta.title, "video", "没标题也要有个能当文件名的东西");
        assert_eq!(meta.filesize, None);

        assert!(parse_info("不是 json").is_err());
    }

    /// **视频任务的输出名必须确定**（用户 2026-11-27 报「视频下载功能无效」）。
    ///
    /// 队列按 `task.name` 认领文件；yt-dlp 默认按标题自己起名 —— 名字对不上，
    /// 下完了桌面端也认不出，看着就是「无效」。所以这里把名字钉死。
    #[test]
    fn the_video_output_name_is_pinned_so_the_queue_can_claim_it() {
        // 有扩展名：原样保留
        assert_eq!(force_out_name("BV1atse6EEfx.mp4"), "BV1atse6EEfx.mp4");
        assert_eq!(force_out_name("影片.mkv"), "影片.mkv");
        // 没有扩展名（视频页 URL 推出来的名字通常这样）：补 mp4
        assert_eq!(force_out_name("BV1atse6EEfx"), "BV1atse6EEfx.mp4");
        // 空名字也得有个能落地的名字
        assert_eq!(force_out_name("   "), "video.mp4");

        // 参数里真的用上了这个名字，而且不再是 %(title)s 模板
        let pinned = Path::new("D:/out");
        let argv = argv_download("https://example.com/v/1", pinned, Some("BV1"));
        let template = argv
            .iter()
            .position(|a| a == "-o")
            .map(|i| argv[i + 1].clone())
            .expect("-o 必须有值");
        assert!(template.ends_with("BV1.mp4"), "输出名要被钉死：{template}");
        assert!(
            !template.contains("%(title)s"),
            "钉死名字时不该再留标题模板：{template}"
        );

        // 没给名字（命令行直下）才退回标题模板
        let free = argv_download("https://example.com/v/1", pinned, None);
        let free_template = free
            .iter()
            .position(|a| a == "-o")
            .map(|i| free[i + 1].clone())
            .unwrap();
        assert!(
            free_template.contains("%(title)s"),
            "没给名字就该按标题：{free_template}"
        );
    }

    /// 进度解析：只认 `[download] xx%`，别把别的行当进度。
    #[test]
    fn progress_only_comes_from_download_percentage_lines() {
        assert_eq!(
            parse_progress("[download]  42.3% of 1.00GiB at 2.00MiB/s ETA 00:01"),
            Some(42.3)
        );
        assert_eq!(parse_progress("[download] 100% of 1.00GiB"), Some(100.0));
        assert_eq!(parse_progress("[download]   0.0% of 1.00GiB"), Some(0.0));

        assert_eq!(parse_progress("[download] Destination: C:\\x\\a.mp4"), None);
        assert_eq!(parse_progress("[info] Writing video metadata"), None);
        assert_eq!(parse_progress("42.3%"), None, "没有前缀不算进度");
        assert_eq!(
            parse_progress("[download] 120% of x"),
            None,
            "超出 0..100 不算"
        );
        assert_eq!(parse_progress(""), None);
    }

    /// 参数形状：URL 一律放在 `--` 之后（避免被当成选项）。
    #[test]
    fn the_url_always_comes_after_a_double_dash() {
        let info = argv_info("https://example.com/v/-x");
        assert_eq!(info.last().unwrap(), "https://example.com/v/-x");
        assert_eq!(info[info.len() - 2], "--", "URL 前的 -- 不能少");

        let dir = Path::new("D:/out");
        let dl = argv_download("https://example.com/v/1", dir, None);
        assert_eq!(dl.last().unwrap(), "https://example.com/v/1");
        assert_eq!(dl[dl.len() - 2], "--");
        let template = dl
            .iter()
            .position(|a| a == "-o")
            .map(|i| dl[i + 1].clone())
            .expect("-o 必须有值");
        assert!(
            template.contains("%(title)s") && template.ends_with("%(ext)s"),
            "落点模板要带标题与扩展名占位：{template}"
        );
        assert!(
            template.starts_with("D:/out"),
            "要落在指定目录里：{template}"
        );
    }

    /// **识别视频页**：为了"预先帮你勾上"，所以宽松 —— 但直链视频不能误判。
    #[test]
    fn video_pages_are_recognised_but_direct_links_are_not() {
        // 常见的视频页（用户 2026-11-27 拿 BV 号来下就是这个形状）
        assert!(looks_like_video_page(
            "https://www.bilibili.com/video/BV1atse6EEfx"
        ));
        assert!(looks_like_video_page("https://b23.tv/abcd"));
        assert!(looks_like_video_page("https://youtu.be/xyz"));
        assert!(looks_like_video_page("https://www.douyin.com/video/1234"));
        assert!(looks_like_video_page(
            "https://www.bilibili.com/video/BV1atse6EEfx?spm_id_from=333.999"
        ));

        // **直链视频文件不该走 yt-dlp**：普通下载有分段与续传，比它好用
        assert!(!looks_like_video_page("https://cdn.example.com/a.mp4"));
        assert!(!looks_like_video_page(
            "https://cdn.example.com/a.mp4?token=1"
        ));
        assert!(!looks_like_video_page("https://cdn.example.com/a.mkv"));

        // 普通网页/文件、空串、非 http 一律不认
        assert!(!looks_like_video_page("https://example.com/file.zip"));
        assert!(!looks_like_video_page(""));
        assert!(!looks_like_video_page("D:\\a.mp4"));
    }

    /// 缺依赖时的话必须说清三件事，否则用户只知道"失败了"。
    #[test]
    fn the_missing_tool_hint_says_what_where_and_an_alternative() {
        let hint = missing_hint();
        assert!(hint.contains("yt-dlp"), "要说是缺 yt-dlp");
        assert!(hint.contains("winget") || hint.contains("pip"), "要给装法");
        assert!(hint.contains("DLM_YTDLP"), "要给「我自己已经有一个」的出路");
    }

    #[test]
    fn human_sizes_read_like_sizes() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(1_048_576), "1.0 MiB");
        assert_eq!(human(1024), "1.0 KiB");
    }
}
