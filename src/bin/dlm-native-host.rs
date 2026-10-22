#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

//! dlm 的 Native Messaging 宿主（B1）：stdin/stdout 上的最小 ping/pong。
//! 浏览器会带 origin / parent-window 参数启动它；B1 不看参数。诊断只走 stderr，stdout 只出帧。

use std::process::ExitCode;

fn main() -> ExitCode {
    match dlm::host::Host::serve_stdio() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("dlm-native-host: {err}");
            ExitCode::FAILURE
        }
    }
}
