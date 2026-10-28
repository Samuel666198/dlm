/// 下载侧所有对外 HTTP 请求（探测、分段、整份、HLS）统一带的 User-Agent。
///
/// 不少 CDN/WAF 对**没有 UA** 的请求直接 403（ModelScope 的 LFS CDN 实测如此）；
/// 用自报家门的 `dlm/<版本>`，不冒充浏览器。
pub const USER_AGENT: &str = concat!("dlm/", env!("CARGO_PKG_VERSION"));
pub const PROBE_READ_BYTES: u64 = 236_032;
pub const MIN_SEGMENT_BYTES: u64 = 16_384;
pub const DEFAULT_CONNECTIONS: usize = 8;
pub const WINDOW_BYTES: u64 = 4_194_304;
pub const WINDOW_MIN_TIMEOUT_SECS: u64 = 15;
pub const WINDOW_MAX_TIMEOUT_SECS: u64 = 120;
pub const FAST_RETRY_MAX: u32 = 5;
pub const SLOW_RETRY_INTERVAL_SECS: u64 = 30;
pub const STATE_SAVE_MAX_INTERVAL_SECS: u64 = 5;
pub const FLAG_ADD_ONLY: u32 = 1 << 0;
pub const FLAG_IGNORE_MTIME: u32 = 1 << 1;
pub const FLAG_KEEP_STATE: u32 = 1 << 2;
/// B3a：`--hls`，把 URL 当作 HLS 播放列表（只影响这一次运行的分流，不参与续传比对）。
pub const FLAG_HLS: u32 = 1 << 3;
