//! 下载完成后的病毒扫描（E3）。
//!
//! 用 Windows Defender 自带的 `MpCmdRun.exe`，不引第三方引擎、不联网。
//!
//! ## 三条纪律
//!
//! - **扫描失败绝不影响任务成败**：任务已经下完了，扫描只是补一道手续。
//!   进程起不来、Defender 被组策略关掉、路径找不到 —— 一律只记一条日志。
//! - **不阻塞 runner**：一次扫描可能几分钟，放到独立线程里跑，runner 照常派发下一个任务。
//! - **命令要能被测试**：拼命令行这件事抽成纯函数（[`scan_command`]），
//!   路径带空格的引号规则在测试里钉死，而不是靠"看着对"。

use std::path::{Path, PathBuf};

/// Defender 的扫描入口。找不到就返回 `None`（调用方只记日志）。
///
/// 两个位置都试：平台目录（版本号会变，取最高的那个）与老式的固定路径。
pub fn find_mpcmdrun() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(program_data) = std::env::var("ProgramData") {
        let platform = Path::new(&program_data)
            .join("Microsoft")
            .join("Windows Defender")
            .join("Platform");
        if let Ok(entries) = std::fs::read_dir(&platform) {
            let mut versions: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
            versions.sort();
            for dir in versions.into_iter().rev() {
                let exe = dir.join("MpCmdRun.exe");
                if exe.is_file() {
                    candidates.push(exe);
                }
            }
        }
    }
    candidates.push(Path::new(r"C:\Program Files\Windows Defender").join("MpCmdRun.exe"));

    candidates.into_iter().find(|p| p.is_file())
}

/// 拼扫描命令行：`MpCmdRun.exe -Scan -ScanType 3 -File <路径> -DisableRemediation`
///
/// `-ScanType 3` = 自定义扫描单个文件；`-DisableRemediation` = 只报告、不自动删改 ——
/// 删不删由用户在 Defender 里决定，下载器不该替人做这个决定。
pub fn scan_args(path: &Path) -> Vec<String> {
    vec![
        "-Scan".to_string(),
        "-ScanType".to_string(),
        "3".to_string(),
        "-File".to_string(),
        path.to_string_lossy().to_string(),
        "-DisableRemediation".to_string(),
        "-NoNewPrivs".to_string(),
    ]
}

/// 起一个后台线程扫描刚下完的文件，结果追加进任务日志。
///
/// **永远不返回错误给调用方**：这是收尾手续，不是下载的一部分。
pub fn spawn_scan(file: &std::path::Path, log: &std::path::Path) {
    let file = file.to_path_buf();
    let log = log.to_path_buf();
    std::thread::spawn(move || {
        let Some(exe) = find_mpcmdrun() else {
            append(&log, "扫描跳过：找不到 MpCmdRun.exe（Defender 可能被移除）");
            return;
        };
        match std::process::Command::new(exe)
            .args(scan_args(&file))
            .output()
        {
            Ok(out) => {
                let text = String::from_utf8_lossy(&out.stdout).to_string()
                    + &String::from_utf8_lossy(&out.stderr);
                let line = summarize(&text).unwrap_or_else(|| "无输出".to_string());
                append(&log, &format!("扫描：{line}"));
            }
            Err(err) => append(&log, &format!("扫描失败（不影响任务）：{err}")),
        }
    });
}

fn append(log: &std::path::Path, line: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
    {
        let _ = writeln!(f, "[dlm] {line}");
    }
}
/// 从 `MpCmdRun` 的输出里摘一句人话：优先威胁名，其次最后一行。
///
/// 它的输出格式随平台版本变，所以这里只做"尽力而为"的提取，取不到就返回 None。
pub fn summarize(output: &str) -> Option<String> {
    let lower = output.to_lowercase();
    if lower.contains("no threats") || lower.contains("found no") {
        return Some("未发现威胁".to_string());
    }
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let l = line.to_lowercase();
        if l.ends_with("information:") {
            continue; // 只是表头
        }
        if l.contains("threat") || line.contains("发现") {
            return Some(line.chars().take(160).collect());
        }
    }
    output
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .map(|l| l.chars().take(160).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_args_keep_the_path_as_one_argument() {
        let args = scan_args(Path::new(r"D:\dl\a b.zip"));
        assert_eq!(args[0], "-Scan");
        assert_eq!(args[2], "3", "ScanType 3 = 自定义扫描单个文件");
        assert_eq!(args[4], r"D:\dl\a b.zip", "带空格的路径必须是一个参数");
        assert!(
            args.contains(&"-DisableRemediation".to_string()),
            "只报不删"
        );
    }

    #[test]
    fn summarize_prefers_a_threat_line_then_falls_back_to_the_last_line() {
        assert_eq!(
            summarize("Scan starting...\nfound no threats.\n"),
            Some("未发现威胁".to_string())
        );
        assert_eq!(
            summarize("Threat information:\nThreat: EICAR-Test-File\n"),
            Some("Threat: EICAR-Test-File".to_string())
        );
        assert_eq!(
            summarize("Scan starting...\nScan finished.\n"),
            Some("Scan finished.".to_string())
        );
        assert_eq!(summarize("   \n\n"), None);
    }
}
