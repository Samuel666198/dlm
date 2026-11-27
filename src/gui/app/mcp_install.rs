//! `dlm-mcp` 的安装：拼直链 → 下载 → 解压 → 功能校验 → 落盘。
//!
//! ## 为什么要有这个文件
//!
//! 0.7.0 的「获取 dlm-mcp」只做了一件事：`open_external(发布页)` —— 把"下载、解压、
//! 放到同目录"三步手工活丢给用户，还要用户自己判断解压到哪。而这三步的底层能力
//! `updater.rs` 里早就有了（`extract_zip` 会用 `[System.IO.Compression.ZipFile]`）。
//!
//! ## 直链是怎么来的（**没有**改发布流程）
//!
//! 更新清单 `latest.json` 里只有主包的 `url`/`sha256`，没有 MCP 那一份。所以直链由
//! 清单里的**发行版号**拼出来：
//!
//! ```text
//! https://github.com/<owner>/<repo>/releases/download/v<rel>/dlm-mcp-<rel>-win-x64.zip
//! ```
//!
//! 代价是**拿不到摘要**（清单没有 `mcp_sha256`）。于是这里的校验是**功能校验**：
//! 装之前先跑一次 `dlm-mcp.exe --version`，跑不起来或输出认不出版本就不装。
//! 这挡不住"被替换成另一个能跑的 exe"，只挡得住"下到了坏包/截断的包/404 的 HTML"。
//! 把清单加 `mcp_sha256` 记为后续项，动发布流程前先问用户。
//!
//! ## 本版没打包怎么办：回退到向下兼容的旧版（2026-11-18）
//!
//! 0.7.1 / 0.7.2 发布时都忘了附带 MCP 包（当时那份包是手工打的，没有脚本），
//! 于是组件页「下载并安装」直接死在 404 上。两处补救：
//!
//! 1. **发布侧**：`packaging/build-mcp-package.ps1` 把这份包做成脚本，不再靠记性。
//! 2. **客户端侧**：见 [`mcp_asset_candidates`] —— 本版 404 就退到同一个 minor 的
//!    下一个补丁版。这能成立是因为兼容性是**单向**的：`dlm-mcp` 只要求界面
//!    `api >= 本程序的 REQUIRED_API`，而接口是加法演进的，**旧桥接配新界面永远可用**。
//!
//! 注意回退**只对 404 生效**：网络错误、解压失败、不可写都立刻上报 ——
//! 那些换版本重试没有意义，只会让人多等几轮。
//!
//! ## 副作用全部走 [`Host`]
//!
//! 下载、解压、探测、落盘四件事都从参数进来：用例因此不需要联网、不需要往真实安装
//! 目录写文件，也能覆盖「404」「包里没有 exe」「不可写」这些分支。

use std::path::{Path, PathBuf};

/// MCP 桥接的可执行文件名。与 GUI 同目录即被识别。
pub(crate) const MCP_BINARY: &str = "dlm-mcp.exe";

/// 仓库 slug。改仓库名只改这一处。
pub(crate) const REPO_SLUG: &str = "Samuel666198/dlm";

/// 发布页（不是某个具体包）：直链会随版本变，页面永远是最新的。
pub(crate) fn releases_page_url() -> String {
    format!("https://github.com/{REPO_SLUG}/releases/latest")
}

/// 由发行版号拼 MCP 包的直链。
///
/// 只接受 `0.7.0` 这种形状：版本号来自更新清单，是**我们自己的**数据；但仍然校验，
/// 免得一个被改坏的清单把任意 URL 塞进下载器。
pub(crate) fn mcp_asset_url(release: &str) -> Option<String> {
    let rel = release.trim();
    if rel.is_empty() {
        return None;
    }
    let shaped = rel
        .split('.')
        .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
        && rel.matches('.').count() >= 1;
    if !shaped {
        return None;
    }
    Some(format!(
        "https://github.com/{REPO_SLUG}/releases/download/v{rel}/dlm-mcp-{rel}-win-x64.zip"
    ))
}

/// 装一次的结果。界面直接拿 [`Self::message`] 显示，不再另写一套文案。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum McpInstallOutcome {
    Installed {
        version: String,
        path: String,
        /// 装的是**回退版本**（本发行版没带 MCP 包）。
        ///
        /// 为什么值得单独标记：用户该知道"这个包里没有它，我装的是近一个兼容版本"，
        /// 否则下次看到版本号对不上会以为装错了。
        fell_back: bool,
    },
    /// 清单里的发行版号拼不出直链。
    UnknownRelease(String),
    /// 这个发行版**以及它向下的几个补丁版**都没带 MCP 包（404）。
    NoAsset {
        url: String,
        /// 试过哪些版本。界面上要说清"试到哪儿为止"，别让用户以为还能自动搞定。
        tried: Vec<String>,
    },
    DownloadFailed(String),
    ExtractFailed(String),
    /// 包里没有 `dlm-mcp.exe`。
    NoBinary,
    /// 有 exe，但跑不起来 / 输出认不出版本。
    VerifyFailed(String),
    /// 目标目录不可写（装到 Program Files 且没提权）。
    PermissionDenied(String),
    /// 其他写入失败（磁盘满、被占用等）。
    WriteFailed(String),
    /// 被开关抑制（门禁 / 离线 / 用户关掉）。
    Suppressed,
}

impl McpInstallOutcome {
    pub(crate) fn installed(&self) -> bool {
        matches!(self, McpInstallOutcome::Installed { .. })
    }

    /// 给用户看的一句话。**每种失败都要说清"下一步做什么"**，不能只说失败了。
    pub(crate) fn message(&self) -> String {
        match self {
            McpInstallOutcome::Installed {
                version,
                path,
                fell_back,
            } => {
                if *fell_back {
                    format!(
                        "已安装 dlm-mcp {version}（{path}）—— 本版没有附带 MCP 包，\
                         装的是近一个兼容版本（接口是加法演进的，旧桥接配新界面可用）"
                    )
                } else {
                    format!("已安装 dlm-mcp {version}（{path}）")
                }
            }
            McpInstallOutcome::UnknownRelease(rel) => {
                format!("更新清单里的发行版号 `{rel}` 拼不出下载地址，请用「打开发布页」手动下载")
            }
            McpInstallOutcome::NoAsset { url, tried } => {
                if tried.len() <= 1 {
                    format!("这个发行版没有附带 MCP 包（404）。可以去发布页手动下载：{url}")
                } else {
                    format!(
                        "这个发行版和它向下的 {} 个补丁版（{}）都没有附带 MCP 包。\
                         可以去发布页手动下载：{url}",
                        tried.len(),
                        tried.join(" / ")
                    )
                }
            }
            McpInstallOutcome::DownloadFailed(err) => {
                format!("下载失败：{err}。检查网络或代理后重试")
            }
            McpInstallOutcome::ExtractFailed(err) => format!("解压失败：{err}"),
            McpInstallOutcome::NoBinary => {
                "包里没有 dlm-mcp.exe：这份包不完整，请用「打开发布页」重新下载".to_string()
            }
            McpInstallOutcome::VerifyFailed(err) => {
                format!("装好了但跑不起来（{err}）：已放弃，原来的文件没动")
            }
            McpInstallOutcome::PermissionDenied(err) => {
                format!("没有写入本程序目录的权限（{err}）：把 dlm-mcp.exe 手动放到程序同目录即可")
            }
            McpInstallOutcome::WriteFailed(err) => {
                format!("写入失败：{err}。确认没有别的程序正占用 dlm-mcp.exe 后重试")
            }
            McpInstallOutcome::Suppressed => "已跳过下载（离线 / 门禁模式）".to_string(),
        }
    }
}

/// 下载失败的两种：404 要单独认出来（"这个版本没带 MCP 包"不是网络问题）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FetchError {
    NotFound,
    Other(String),
}

/// 落盘失败的两种：不可写要单独认出来（要提示用户手动放，而不是让他反复重试）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlaceError {
    PermissionDenied(String),
    Other(String),
}

/// 安装需要的四件外部能力。用例实现一个假的，覆盖每个失败分支。
pub(crate) trait Host {
    fn fetch(&self, url: &str, dest: &Path) -> Result<u64, FetchError>;
    fn extract(&self, zip: &Path, dest: &Path) -> Result<(), String>;
    /// 跑 `exe --version` 并解析出版本号；跑不起来就 `None`。
    fn probe(&self, exe: &Path) -> Option<String>;
    fn place(&self, src: &Path, dst: &Path) -> Result<(), PlaceError>;
}

/// 真实实现：网络 + PowerShell 解压 + 起子进程 + 文件系统。
pub(crate) struct RealHost;

impl Host for RealHost {
    fn fetch(&self, url: &str, dest: &Path) -> Result<u64, FetchError> {
        // 为什么不用 `updater::download`：那个函数在非 2xx 时只回一句
        // "HTTP xxx" 的字符串，而这里**必须区分 404**（"这个版本没带 MCP 包"
        // 要给出完全不同的下一步）。自己发一次请求比回来猜字符串可靠。
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .connect_timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| FetchError::Other(format!("建下载客户端失败：{e}")))?;
        let mut response = client
            .get(url)
            .send()
            .map_err(|e| FetchError::Other(format!("请求失败：{e}")))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(FetchError::NotFound);
        }
        if !response.status().is_success() {
            return Err(FetchError::Other(format!("HTTP {}", response.status())));
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| FetchError::Other(format!("建临时目录失败：{e}")))?;
        }
        let mut file = std::fs::File::create(dest)
            .map_err(|e| FetchError::Other(format!("创建临时文件失败：{e}")))?;
        let bytes = response
            .copy_to(&mut file)
            .map_err(|e| FetchError::Other(format!("写文件失败：{e}")))?;
        Ok(bytes)
    }

    fn extract(&self, zip: &Path, dest: &Path) -> Result<(), String> {
        crate::updater::extract_zip(zip, dest).map_err(|e| format!("{e:#}"))
    }

    fn probe(&self, exe: &Path) -> Option<String> {
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--version");
        // 别在用户屏幕上闪一个黑框：这是后台探测，不是用户启动的程序。
        crate::proc::no_window(&mut cmd);
        let out = cmd.output().ok()?;
        if !out.status.success() {
            return None;
        }
        parse_version(&String::from_utf8_lossy(&out.stdout))
    }

    fn place(&self, src: &Path, dst: &Path) -> Result<(), PlaceError> {
        place_binary(src, dst)
    }
}

/// 从 `dlm-mcp --version` 的输出里取版本号。
///
/// 真实输出形如 `Download Manager MCP 桥接 0.7.0（内核 0.1.0，接口 api 2）`：
/// 第一段 `数字.数字.数字` 就是桥接自己的版本。中文文案会变，所以不按位置取。
pub(crate) fn parse_version(text: &str) -> Option<String> {
    let bytes: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        let mut dots = 0;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == '.') {
            if bytes[i] == '.' {
                dots += 1;
            }
            i += 1;
        }
        if dots >= 1 {
            let candidate: String = bytes[start..i].iter().collect();
            let trimmed = candidate.trim_end_matches('.');
            if trimmed.contains('.') && !trimmed.ends_with('.') {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// 把解压出来的 exe 放到目标位置。
///
/// 顺序：**先拷到旁边的临时名，再替换**。直接覆盖的话，一旦拷到一半失败，
/// 原来那份好的 `dlm-mcp.exe` 已经被删了 —— 用户就从"有但旧"变成"彻底没有"。
fn place_binary(src: &Path, dst: &Path) -> Result<(), PlaceError> {
    let tmp = dst.with_extension("exe.new");
    std::fs::copy(src, &tmp).map_err(map_place_err)?;
    // Windows 上 rename 不能覆盖，所以先删旧的；此时新文件已经完整落地。
    if dst.exists() {
        std::fs::remove_file(dst).map_err(map_place_err)?;
    }
    std::fs::rename(&tmp, dst).map_err(map_place_err)?;
    Ok(())
}

fn map_place_err(err: std::io::Error) -> PlaceError {
    if err.kind() == std::io::ErrorKind::PermissionDenied {
        PlaceError::PermissionDenied(err.to_string())
    } else {
        PlaceError::Other(err.to_string())
    }
}

/// 在解压目录里找 `dlm-mcp.exe`。zip 里常常套一层目录，所以往下找两层。
pub(crate) fn find_binary(root: &Path) -> Option<PathBuf> {
    fn walk(dir: &Path, depth: u32) -> Option<PathBuf> {
        let entries = std::fs::read_dir(dir).ok()?;
        let mut dirs = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.eq_ignore_ascii_case(MCP_BINARY) {
                    return Some(path);
                }
            } else if path.is_dir() {
                dirs.push(path);
            }
        }
        if depth == 0 {
            return None;
        }
        dirs.into_iter().find_map(|d| walk(&d, depth - 1))
    }
    walk(root, 2)
}

/// 门禁 / 离线时不要真去下载。与 `DLM_NO_UPDATE_CHECK` 同一类约定。
pub(crate) fn download_suppressed() -> bool {
    std::env::var_os("DLM_NO_MCP_DOWNLOAD").is_some()
}

/// 回退最多往下降几个补丁版。
///
/// 为什么要有个上限：每次回退都是一次真正的 HTTP 往返，网络差的时候会让人以为
/// 程序卡住。3 个（含本版）足够覆盖"这一版忘了打包、上一版有"的情形。
const MCP_FALLBACK_SPAN: u32 = 3;

/// 装的时候按顺序试哪些版本。
///
/// ## 为什么可以回退（2026-11-18 用户报「这个发行版没有附带 MCP 包（404）」）
///
/// 兼容性是**单向**的：`dlm-mcp` 只要求「界面 api >= 本程序的 REQUIRED_API」
/// （见 `src/bin/dlm-mcp.rs` 的 `check_compat`），而接口是**加法演进**的 ——
/// 所以**旧桥接配新界面永远可用**，只是用不上新工具。反方向才危险，而回退
/// 只会往下走，不会拿到"更新"的东西。
///
/// 于是规则很简单：先试本版；本版 404 就退到同一个 minor 的下一个补丁版，
/// 最多退 [`MCP_FALLBACK_SPAN`] 个。**不跨 minor**：跨 minor 意味着桥接协议
/// 可能真变过，与其猜，不如让用户去发布页手动挑。
///
/// 返回空 `Vec` 表示版本号形状就不对（调用方据此回 [`McpInstallOutcome::UnknownRelease`]）。
pub(crate) fn mcp_asset_candidates(release: &str) -> Vec<String> {
    let rel = release.trim();
    // 先过一遍成形校验：与 `mcp_asset_url` 用同一把尺子，别的形状一律不认。
    if mcp_asset_url(rel).is_none() {
        return Vec::new();
    }
    let mut parts: Vec<&str> = rel.split('.').collect();
    let Some(last) = parts.pop().and_then(|p| p.parse::<u32>().ok()) else {
        return vec![rel.to_string()];
    };
    let prefix = parts.join(".");
    let mut out = Vec::new();
    for patch in (0..=last).rev() {
        out.push(format!("{prefix}.{patch}"));
        if out.len() as u32 >= MCP_FALLBACK_SPAN {
            break;
        }
    }
    out
}

/// 装一次。`work_root` 是临时目录（调用方给，失败留痕便于排查），
/// `dest_dir` 是 GUI 所在目录。
///
/// 内部会按 [`mcp_asset_candidates`] 依次尝试：**只有 404 才继续往下退**，
/// 别的失败（网络、解压、写入）立刻停下 —— 那种情况换版本重试没有意义，
/// 只会让用户多等三轮。
pub(crate) fn install_release(
    release: &str,
    work_root: &Path,
    dest_dir: &Path,
    suppressed: bool,
    host: &dyn Host,
) -> McpInstallOutcome {
    if suppressed {
        return McpInstallOutcome::Suppressed;
    }
    let candidates = mcp_asset_candidates(release);
    if candidates.is_empty() {
        return McpInstallOutcome::UnknownRelease(release.to_string());
    }

    if let Err(err) = std::fs::create_dir_all(work_root) {
        return McpInstallOutcome::DownloadFailed(format!("建临时目录失败：{err}"));
    }

    let mut tried: Vec<String> = Vec::new();
    let mut last_url = String::new();
    for candidate in &candidates {
        let outcome = install_one(candidate, work_root, dest_dir, host);
        match outcome {
            // 这一版没带包：退到下一个补丁版。
            McpInstallOutcome::NoAsset { url, .. } => {
                last_url = url;
                tried.push(candidate.clone());
            }
            McpInstallOutcome::Installed {
                version,
                path,
                fell_back: _,
            } => {
                return McpInstallOutcome::Installed {
                    version,
                    path,
                    fell_back: candidate != release.trim(),
                };
            }
            // 其它失败：换版本重试没有意义，原样上报。
            other => return other,
        }
    }
    McpInstallOutcome::NoAsset {
        url: last_url,
        tried,
    }
}

/// 试**一个**具体版本。404 由调用方解读为"退到下一版"。
fn install_one(
    release: &str,
    work_root: &Path,
    dest_dir: &Path,
    host: &dyn Host,
) -> McpInstallOutcome {
    let Some(url) = mcp_asset_url(release) else {
        return McpInstallOutcome::UnknownRelease(release.to_string());
    };

    let zip = work_root.join("dlm-mcp.zip");
    let staging = work_root.join("staging");
    // 每轮清一次：上一轮解压出来的 exe 留着，会让人误以为这一轮也下到了东西。
    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_file(&zip);

    match host.fetch(&url, &zip) {
        Ok(0) => return McpInstallOutcome::DownloadFailed("下载到 0 字节".to_string()),
        Ok(_) => {}
        Err(FetchError::NotFound) => {
            return McpInstallOutcome::NoAsset {
                url,
                tried: Vec::new(),
            };
        }
        Err(FetchError::Other(err)) => return McpInstallOutcome::DownloadFailed(err),
    }

    if let Err(err) = host.extract(&zip, &staging) {
        return McpInstallOutcome::ExtractFailed(err);
    }
    let Some(binary) = find_binary(&staging) else {
        return McpInstallOutcome::NoBinary;
    };

    // **装之前**先跑一次：确认这是个能启动、且会自报版本的 dlm-mcp。
    let Some(version) = host.probe(&binary) else {
        return McpInstallOutcome::VerifyFailed(
            "解压出来的 dlm-mcp.exe 跑不起来或没有输出可识别的版本号".to_string(),
        );
    };

    let target = dest_dir.join(MCP_BINARY);
    match host.place(&binary, &target) {
        Ok(()) => McpInstallOutcome::Installed {
            version,
            path: target.display().to_string(),
            fell_back: false,
        },
        Err(PlaceError::PermissionDenied(err)) => McpInstallOutcome::PermissionDenied(err),
        Err(PlaceError::Other(err)) => McpInstallOutcome::WriteFailed(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dlm-mcp-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 假 Host：每个分支的行为由字段决定，用例不联网、不碰真实安装目录。
    struct FakeHost {
        fetch: Result<u64, FetchError>,
        extract: Result<(), String>,
        /// 解压后在 staging 里放什么：`Some(true)` = 放 dlm-mcp.exe。
        put_binary: bool,
        probe: Option<String>,
        place: Result<(), PlaceError>,
        /// 这些版本号的直链一律 404（模拟"那几版忘了打包 MCP"）。
        ///
        /// 别的错误由 `fetch` 字段统一给，用来区分"该继续回退"与"该立刻停"。
        missing: Vec<&'static str>,
    }

    impl Default for FakeHost {
        fn default() -> Self {
            Self {
                fetch: Ok(1024),
                extract: Ok(()),
                put_binary: true,
                probe: Some("0.7.0".to_string()),
                place: Ok(()),
                missing: Vec::new(),
            }
        }
    }

    impl Host for FakeHost {
        fn fetch(&self, url: &str, dest: &Path) -> Result<u64, FetchError> {
            if self.missing.iter().any(|v| url.contains(v)) {
                return Err(FetchError::NotFound);
            }
            if self.fetch.is_ok() {
                std::fs::write(dest, b"PK fake zip").unwrap();
            }
            self.fetch.clone()
        }

        fn extract(&self, _zip: &Path, dest: &Path) -> Result<(), String> {
            if self.extract.is_ok() {
                // 真实 zip 常常套一层目录：这里也套一层，顺便验证 find_binary 会往下找。
                let inner = dest.join("dlm-mcp-0.7.0-win-x64");
                std::fs::create_dir_all(&inner).unwrap();
                if self.put_binary {
                    std::fs::write(inner.join(MCP_BINARY), b"MZ fake").unwrap();
                }
            }
            self.extract.clone()
        }

        fn probe(&self, _exe: &Path) -> Option<String> {
            self.probe.clone()
        }

        fn place(&self, src: &Path, dst: &Path) -> Result<(), PlaceError> {
            self.place.clone()?;
            std::fs::copy(src, dst).unwrap();
            Ok(())
        }
    }

    #[test]
    fn the_asset_url_is_built_from_the_release_number() {
        assert_eq!(
            mcp_asset_url("0.7.0").as_deref(),
            Some(
                "https://github.com/Samuel666198/dlm/releases/download/v0.7.0/dlm-mcp-0.7.0-win-x64.zip"
            )
        );
        assert_eq!(
            mcp_asset_url(" 0.7.0 ").as_deref(),
            mcp_asset_url("0.7.0").as_deref()
        );
        // 清单被改坏时不许把任意 URL 塞进下载器。
        assert_eq!(mcp_asset_url(""), None);
        assert_eq!(mcp_asset_url("../../etc"), None);
        assert_eq!(mcp_asset_url("0.7.0/evil"), None);
        assert_eq!(mcp_asset_url("https://evil.invalid/x.zip"), None);
        assert_eq!(mcp_asset_url("7"), None, "至少要有两段");
    }

    #[test]
    fn a_good_package_installs_and_reports_the_probed_version() {
        let work = tmp_dir("ok-work");
        let dest = tmp_dir("ok-dest");
        let host = FakeHost::default();
        let out = install_release("0.7.0", &work, &dest, false, &host);
        assert!(out.installed(), "{out:?}");
        assert_eq!(
            out,
            McpInstallOutcome::Installed {
                version: "0.7.0".to_string(),
                path: dest.join(MCP_BINARY).display().to_string(),
                fell_back: false,
            }
        );
        assert!(
            dest.join(MCP_BINARY).is_file(),
            "装完必须真的在同目录有一份 exe"
        );
        let _ = std::fs::remove_dir_all(&work);
        let _ = std::fs::remove_dir_all(&dest);
    }

    /// 404 不是"网络问题"：这个版本根本没带 MCP 包，下一步是去发布页手动下载。
    #[test]
    fn a_missing_asset_is_reported_apart_from_a_network_failure() {
        let work = tmp_dir("404-work");
        let dest = tmp_dir("404-dest");
        let host = FakeHost {
            fetch: Err(FetchError::NotFound),
            ..FakeHost::default()
        };
        let out = install_release("0.7.0", &work, &dest, false, &host);
        match &out {
            McpInstallOutcome::NoAsset { url, tried } => {
                assert!(url.contains("dlm-mcp-0.7.0-win-x64.zip"), "{url}");
                assert_eq!(
                    tried.as_slice(),
                    ["0.7.0".to_string()],
                    "0.7.0 已经是最低补丁版"
                );
            }
            other => panic!("404 要单独认出：{other:?}"),
        }
        assert!(!dest.join(MCP_BINARY).exists());

        let host = FakeHost {
            fetch: Err(FetchError::Other("connection refused".into())),
            ..FakeHost::default()
        };
        let out = install_release("0.7.0", &work, &dest, false, &host);
        assert!(
            matches!(out, McpInstallOutcome::DownloadFailed(_)),
            "{out:?}"
        );
        let _ = std::fs::remove_dir_all(&work);
        let _ = std::fs::remove_dir_all(&dest);
    }

    /// 包里没有 exe、exe 跑不起来、目录不可写：三种失败都不能动目标文件。
    #[test]
    fn a_broken_package_never_replaces_the_existing_binary() {
        let work = tmp_dir("bad-work");
        let dest = tmp_dir("bad-dest");
        let existing = dest.join(MCP_BINARY);
        std::fs::write(&existing, b"OLD").unwrap();

        let host = FakeHost {
            put_binary: false,
            ..FakeHost::default()
        };
        assert_eq!(
            install_release("0.7.0", &work, &dest, false, &host),
            McpInstallOutcome::NoBinary
        );
        assert_eq!(
            std::fs::read(&existing).unwrap(),
            b"OLD",
            "失败不许动旧文件"
        );

        let host = FakeHost {
            probe: None,
            ..FakeHost::default()
        };
        let out = install_release("0.7.0", &work, &dest, false, &host);
        assert!(
            matches!(out, McpInstallOutcome::VerifyFailed(_)),
            "跑不起来就不许装：{out:?}"
        );
        assert_eq!(
            std::fs::read(&existing).unwrap(),
            b"OLD",
            "校验失败也不许动"
        );

        let host = FakeHost {
            place: Err(PlaceError::PermissionDenied("拒绝访问".into())),
            ..FakeHost::default()
        };
        assert!(matches!(
            install_release("0.7.0", &work, &dest, false, &host),
            McpInstallOutcome::PermissionDenied(_)
        ));
        assert_eq!(std::fs::read(&existing).unwrap(), b"OLD", "不可写也不许动");

        let host = FakeHost {
            extract: Err("坏 zip".into()),
            ..FakeHost::default()
        };
        assert!(matches!(
            install_release("0.7.0", &work, &dest, false, &host),
            McpInstallOutcome::ExtractFailed(_)
        ));
        let _ = std::fs::remove_dir_all(&work);
        let _ = std::fs::remove_dir_all(&dest);
    }

    #[test]
    fn suppression_and_a_bad_release_short_circuit_before_any_io() {
        let work = tmp_dir("suppress-work");
        let dest = tmp_dir("suppress-dest");
        let host = FakeHost::default();
        assert_eq!(
            install_release("0.7.0", &work, &dest, true, &host),
            McpInstallOutcome::Suppressed,
            "门禁/离线时一个请求都不许发"
        );
        assert!(matches!(
            install_release("bad", &work, &dest, false, &host),
            McpInstallOutcome::UnknownRelease(_)
        ));
        let _ = std::fs::remove_dir_all(&work);
        let _ = std::fs::remove_dir_all(&dest);
    }

    /// 版本号从真实输出里取：文案变了也要取得到。
    #[test]
    fn the_version_is_read_out_of_the_real_version_line() {
        assert_eq!(
            parse_version("Download Manager MCP 桥接 0.7.0（内核 0.1.0，接口 api 2）").as_deref(),
            Some("0.7.0"),
            "取第一段 x.y.z，不能取内核那一段"
        );
        assert_eq!(parse_version("0.6.0").as_deref(), Some("0.6.0"));
        assert_eq!(parse_version("no digits here"), None);
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("api 2"), None, "单个整数不算版本号");
    }

    /// 每个失败都要带"下一步做什么"，不能只说失败了。
    #[test]
    fn every_failure_says_what_to_do_next() {
        let cases = [
            McpInstallOutcome::UnknownRelease("bad".into()),
            McpInstallOutcome::NoAsset {
                url: "https://example.invalid/x.zip".into(),
                tried: vec!["0.7.2".into()],
            },
            McpInstallOutcome::DownloadFailed("timeout".into()),
            McpInstallOutcome::ExtractFailed("bad zip".into()),
            McpInstallOutcome::NoBinary,
            McpInstallOutcome::VerifyFailed("exit 1".into()),
            McpInstallOutcome::PermissionDenied("拒绝访问".into()),
            McpInstallOutcome::WriteFailed("被占用".into()),
            McpInstallOutcome::Suppressed,
        ];
        for case in cases {
            let msg = case.message();
            assert!(!msg.trim().is_empty(), "{case:?} 没有文案");
            assert!(!msg.contains('{'), "{case:?} 文案里有没替换的占位符：{msg}");
        }
        // 不可写这一条必须告诉用户手动放，否则他只会反复点重试。
        assert!(
            McpInstallOutcome::PermissionDenied("x".into())
                .message()
                .contains("手动放"),
            "不可写要给出替代动作"
        );
    }

    /// **回退候选的规则**：本版优先，然后同一个 minor 往下退，最多 3 个。
    ///
    /// 这条钉的是"退到哪儿为止"。不跨 minor 是刻意的：跨 minor 意味着桥接协议可能
    /// 真变过，与其猜，不如让用户去发布页手动挑。
    #[test]
    fn the_fallback_candidates_walk_down_the_same_minor_only() {
        assert_eq!(
            mcp_asset_candidates("0.7.2"),
            vec![
                "0.7.2".to_string(),
                "0.7.1".to_string(),
                "0.7.0".to_string()
            ],
            "0.7.2 应当先试自己，再连续退两个补丁版"
        );
        // 补丁号为 0 时没有更低的同 minor 版本，只试自己。
        assert_eq!(mcp_asset_candidates("0.7.0"), vec!["0.7.0".to_string()]);
        // 补丁号很大也只试 3 个，不会打十几个 404 让人干等。
        assert_eq!(
            mcp_asset_candidates("1.4.99"),
            vec![
                "1.4.99".to_string(),
                "1.4.98".to_string(),
                "1.4.97".to_string()
            ]
        );
        // 形状不对的版本号不产生候选（拼不出直链）。
        assert!(mcp_asset_candidates("").is_empty());
        assert!(mcp_asset_candidates("bad").is_empty());
        assert!(mcp_asset_candidates("https://evil.invalid/x.zip").is_empty());
    }

    /// **本版没带 MCP 包时，真的会退到上一个补丁版并装上**。
    ///
    /// 这是用户 2026-11-18 报的那个 404 的直接回归：0.7.1 / 0.7.2 发布时都忘了
    /// 带 MCP 包，而旧代码只认本版，于是「下载并安装」直接死在一个红色提示上。
    ///
    /// 兼容性依据见 `mcp_asset_candidates` 的文档：接口是加法演进的，
    /// 旧桥接配新界面可用，所以往下退是安全的。
    #[test]
    fn a_missing_asset_falls_back_to_an_older_patch_and_installs_it() {
        let work = tmp_dir("fb-work");
        let dest = tmp_dir("fb-dest");
        let host = FakeHost {
            // 只有 0.7.2 这一版没打包
            missing: vec!["dlm-mcp-0.7.2-win-x64.zip"],
            probe: Some("0.7.1".to_string()),
            ..FakeHost::default()
        };
        let out = install_release("0.7.2", &work, &dest, false, &host);
        assert!(out.installed(), "应当退到 0.7.1 并装上：{out:?}");
        match &out {
            McpInstallOutcome::Installed {
                version, fell_back, ..
            } => {
                assert_eq!(version, "0.7.1");
                assert!(
                    *fell_back,
                    "装的是回退版本，必须标记出来 —— 否则用户看到版本号对不上会以为装错了"
                );
            }
            other => panic!("{other:?}"),
        }
        assert!(dest.join(MCP_BINARY).is_file(), "回退也要真的落盘");
        // 提示语要说清"本版没带包、装的是兼容版本"。
        let msg = out.message();
        assert!(msg.contains("没有附带 MCP 包"), "{msg}");
        assert!(msg.contains("兼容"), "{msg}");

        let _ = std::fs::remove_dir_all(&work);
        let _ = std::fs::remove_dir_all(&dest);
    }

    /// **网络错误不许触发回退** —— 换版本重试没有意义，只会让人多等三轮。
    #[test]
    fn a_network_error_stops_instead_of_silently_retrying_older_versions() {
        let work = tmp_dir("net-work");
        let dest = tmp_dir("net-dest");
        let host = FakeHost {
            fetch: Err(FetchError::Other("connection refused".into())),
            ..FakeHost::default()
        };
        let out = install_release("0.7.2", &work, &dest, false, &host);
        assert!(
            matches!(out, McpInstallOutcome::DownloadFailed(_)),
            "网络错误要立刻上报，不能当成 404 继续退：{out:?}"
        );
        assert!(!dest.join(MCP_BINARY).exists());
        let _ = std::fs::remove_dir_all(&work);
        let _ = std::fs::remove_dir_all(&dest);
    }
}
