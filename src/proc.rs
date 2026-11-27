//! 起子进程时的 Windows 小工具。
//!
//! ## 为什么值得单独一个模块
//!
//! `dlm.exe` 是**控制台子系统**（PE subsystem = 3），而 `dlm-gui.exe` 是 **GUI 子系统**（= 2）。
//! GUI 进程起一个控制台子进程、又不加 `CREATE_NO_WINDOW` 时，Windows 会给它**分配一个新的
//! 控制台** —— 用户看到的就是一个黑色终端窗口突然冒出来。
//!
//! 2026-11-27 用户报的「下载时弹出终端窗口」就是这条：`gui/runner.rs` 起 `dlm.exe` 时漏了
//! 这个标志，而 `host.rs` / `gui/mod.rs` / `mcp_install.rs` 三处都写了 —— 于是同一个常量在
//! 四个地方各写了一遍，漏一处就出一个只有用户能看见的 bug。
//!
//! 所以收拢到这里：**凡是起 `dlm*.exe` 的地方都调它**，别再各自复制那个魔数。

use std::process::Command;

/// `CREATE_NO_WINDOW`：不为子进程分配控制台。
///
/// 注意它**不影响** stdin/stdout/stderr 的重定向 —— 管道照旧可用。
/// （`--cookie-stdin` 依赖子进程能读 stdin，这条不能因为加标志而坏掉。）
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 让子进程不弹控制台窗口。
///
/// Windows 之外是空操作 —— 那边的进程模型里没有这个问题。
pub fn no_window(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 这个用例证明不了"窗口没弹"（那要靠 conhost 探测），但它钉住一件更要紧的事：
    /// **加了标志之后管道仍然能用**。`--cookie-stdin` 全靠这条，不能因为不弹窗就坏掉。
    #[cfg(windows)]
    #[test]
    fn a_no_window_child_can_still_talk_over_pipes() {
        use std::io::{Read, Write};
        use std::process::Stdio;

        // `more` 会把 stdin 原样回显到 stdout，正好用来证明管道是通的。
        // （别用 `set /p` + `%var%`：批处理的变量在**解析时**就展开了，`set /p` 之后
        // 再 echo 拿不到值 —— 那是脚本的坑，不是管道的问题。）
        let mut cmd = Command::new("cmd.exe");
        cmd.args(["/C", "more"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = no_window(&mut cmd).spawn().expect("起 cmd.exe 失败");

        if let Some(mut pipe) = child.stdin.take() {
            pipe.write_all(b"hello\r\n").expect("写 stdin 失败");
        }
        let mut out = String::new();
        child
            .stdout
            .take()
            .expect("应有 stdout")
            .read_to_string(&mut out)
            .expect("读 stdout 失败");
        let _ = child.wait();

        assert!(
            out.contains("hello"),
            "加了 CREATE_NO_WINDOW 之后管道必须照旧可用，实际输出：{out:?}"
        );
    }

    /// 非 Windows 下必须是空操作，不能因此编不过。
    #[cfg(not(windows))]
    #[test]
    fn no_window_is_a_noop_off_windows() {
        let mut cmd = Command::new("echo");
        no_window(&mut cmd);
    }
}
