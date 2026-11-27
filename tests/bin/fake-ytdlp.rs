//! 假的 `yt-dlp`：只为了让"视频下载"这条路能在**不碰网络**的前提下被验到。
//!
//! 它照真实 yt-dlp 的两个关键约定说话：
//! 1. `--dump-single-json` ⇒ 往 stdout 吐一条 JSON（标题/容器/体积）；
//! 2. 下载时往 stdout 吐 `[download]  xx.x% of …` 这类进度行。
//!
//! 另外听两个开关，用来验失败那两支：
//! - `FAKE_YTDLP_FAIL=1` ⇒ 下载阶段以退出码 3 失败；
//! - `FAKE_YTDLP_INFO_FAIL=1` ⇒ 元信息阶段就以退出码 4 失败。

use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // 元信息那一趟
    if args.iter().any(|a| a == "--dump-single-json") {
        if std::env::var_os("FAKE_YTDLP_INFO_FAIL").is_some() {
            eprintln!("ERROR: [generic] Unable to extract initial state");
            std::process::exit(4);
        }
        // `title` 里故意带空格与中文：真实的标题就长这样，落点模板要能吃住。
        println!(
            r#"{{"title":"示例 视频","ext":"mp4","filesize":2097152,"webpage_url":"https://example.com/v/1"}}"#
        );
        return;
    }

    if std::env::var_os("FAKE_YTDLP_FAIL").is_some() {
        eprintln!("ERROR: unable to download video data: HTTP Error 403");
        std::process::exit(3);
    }

    // 下载那一趟：先一条"目的地"，再几条进度（第一条故意不是百分比，验解析器别乱认）
    println!("[download] Destination: 示例 视频.mp4");
    for pct in ["  0.0", " 25.0", " 50.0", " 99.9", "100.0"] {
        println!("[download] {pct}% of 2.00MiB at 1.00MiB/s ETA 00:01");
    }
    println!("[download] 100% of 2.00MiB in 00:02");
    let _ = std::io::stdout().flush();
}
