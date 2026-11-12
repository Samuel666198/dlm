//! 完成后动作（R19/R20）：`open`、`run:<exe>` 或 `shutdown:<秒数>`。只在下完且校验通过后由 `main` 调用。
//!
//! 这一层不做下载决策，也不看 `Report`：它只回答"给一个动作和一个落点，要启动什么进程"。
//! 不经 shell；`run:<exe>` 的程序收到落点路径作为**唯一**附加参数。
//! `shutdown` 由 A3b 加入（至少 60 秒，见 [`CompletionAction::Shutdown`]）；本层只负责拼
//! `shutdown.exe /s /t <N>`，不判断时机、也不替用户取消。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

use anyhow::Context;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionAction {
    /// `explorer.exe <落点>`（Windows 的默认打开方式）。
    Open,
    /// `<exe> <落点>`。
    Run(PathBuf),
    /// `shutdown.exe /s /t <delay_secs>`；最少 60 秒（防误触）。
    Shutdown { delay_secs: u64 },
    /// `shutdown.exe /h`（休眠）。
    ///
    /// 用 `shutdown /h` 而不是 `rundll32 powrprof.dll,SetSuspendState`：
    /// 后者的三个参数靠 rundll32 解析、不可靠，且 Hibernate 位取决于系统是否
    /// 开着休眠（`powercfg /h`），关着就静默变成睡眠，退出码还是 0。
    /// `shutdown /h` 在休眠没开时**直接失败**，行为可判定。
    ///
    /// **队列不接受这个动作**（见 `queue::validate_on_complete`）：队列可能同时
    /// 完成多个任务，一次休眠会打断别的任务。只在 CLI 的 `--on-complete` 用。
    Hibernate,
}

impl FromStr for CompletionAction {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "open" => Ok(Self::Open),
            "shutdown" => Err("shutdown 必须带延迟，例如 shutdown:60（至少 60 秒）".into()),
            _ => {
                if let Some(secs) = raw.strip_prefix("shutdown:") {
                    let delay_secs = secs
                        .parse::<u64>()
                        .map_err(|_| "shutdown:<秒数> 必须是十进制整数".to_string())?;
                    if delay_secs < 60 {
                        return Err("shutdown 延迟至少 60 秒（防误触）；例如 shutdown:60".into());
                    }
                    return Ok(Self::Shutdown { delay_secs });
                }
                if raw == "hibernate" {
                    return Ok(Self::Hibernate);
                }
                match raw.strip_prefix("run:") {
                    Some(exe) if !exe.trim().is_empty() => Ok(Self::Run(PathBuf::from(exe))),
                    Some(_) => Err("完成后动作 run: 后面必须跟一个程序路径".into()),
                    None => {
                        Err("完成后动作只认 open、run:<exe>、shutdown:<秒数> 或 hibernate".into())
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionPlan {
    pub program: PathBuf,
    pub args: Vec<PathBuf>,
}

pub fn plan(action: &CompletionAction, file: &Path) -> ActionPlan {
    match action {
        CompletionAction::Open => ActionPlan {
            program: PathBuf::from("explorer.exe"),
            args: vec![file.to_path_buf()],
        },
        CompletionAction::Run(exe) => ActionPlan {
            program: exe.clone(),
            args: vec![file.to_path_buf()],
        },
        CompletionAction::Shutdown { delay_secs } => ActionPlan {
            program: PathBuf::from("shutdown.exe"),
            args: vec![
                PathBuf::from("/s"),
                PathBuf::from("/t"),
                PathBuf::from(delay_secs.to_string()),
            ],
        },
        CompletionAction::Hibernate => ActionPlan {
            program: PathBuf::from("shutdown.exe"),
            args: vec![PathBuf::from("/h")],
        },
    }
}

/// 环境变量：置上就**不真的**启动外壳动作，只记一行日志。
///
/// ## 为什么需要它
///
/// 门禁里有一批用例会走到「打开落点」/「在资源管理器里选中」，于是**真的**拉起
/// `explorer.exe`，指向 `target/harness-scratch/<pid>-<tag>-<n>/dl` —— 那是**用例的临时目录**。
/// 用例一结束 `TmpDir::drop` 就把目录删了，屏幕上那个资源管理器窗口随即弹出
///
/// ```text
/// 位置不可用  …\target\harness-scratch\44048-gui-queue-25\dl 不可用
/// ```
///
/// 也就是每跑一次门禁就在用户桌面上留一个这种对话框（实测 2026-11-12：跑几次攒几个）。
/// 而且这类窗口**不是**被测行为的一部分：真正要钉的是「回执里的程序名与参数」，
/// 那条断言留在用例里，启动进程这一步在门禁里没有必要。
///
/// ## 置上之后发生什么
///
/// `execute` 什么都不启动，调用方**仍然拿到真实的 [`ActionPlan`]**（程序名、参数都不变），
/// 只是回执里的 `spawned` 说 `false` —— 不骗人：确实没启动。
pub const NO_SHELL_ENV: &str = "DLM_NO_SHELL_OPEN";

/// 这个进程要不要真的启动外壳动作（见 [`NO_SHELL_ENV`]）。
pub fn shell_suppressed() -> bool {
    std::env::var_os(NO_SHELL_ENV).is_some()
}

/// 启动后**不等待**：`open`/`run` 是旁路动作，dlm 的退出码与下载结果不由子进程决定。
pub fn execute(plan: &ActionPlan) -> anyhow::Result<()> {
    Command::new(&plan.program)
        .args(&plan.args)
        .spawn()
        .with_context(|| format!("启动 {} 失败", plan.program.display()))?;
    Ok(())
}

/// 队列 UI 的“打开文件 / 打开所在文件夹”（v2.1 P2）。
///
/// 只回答“给一个落点和一个模式，要启动什么进程”，与 [`plan`] 同一姿态：不做 IO、不查队列。
/// 落点**必须由调用方从队列任务自己算出来**（`queue::target_path`），不接受客户端交来的路径 ——
/// 否则这个接口就变成一个通用的“打开任意文件”入口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevealMode {
    /// 用默认程序打开这个文件：`explorer.exe <落点>`。
    Open,
    /// 在资源管理器里选中它：`explorer.exe /select,<落点>`。
    Select,
}

impl RevealMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Select => "select",
        }
    }
}

impl FromStr for RevealMode {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw.trim() {
            "open" => Ok(Self::Open),
            "select" => Ok(Self::Select),
            other => Err(format!("打开方式只认 open 或 select，收到 {other:?}")),
        }
    }
}

/// `/select,<路径>` 是**一个**参数：逗号后面直接跟路径，中间不许有空格把它拆开。
/// 拆成 `["/select,", path]` 时资源管理器会把路径当成第二个“要打开的东西”，行为不对（实测见证据）。
pub fn reveal_plan(mode: RevealMode, file: &Path) -> ActionPlan {
    let args = match mode {
        RevealMode::Open => vec![file.to_path_buf()],
        RevealMode::Select => vec![PathBuf::from(format!("/select,{}", file.display()))],
    };
    ActionPlan {
        program: PathBuf::from("explorer.exe"),
        args,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hibernate_parses_and_plans_shutdown_h() {
        let action: CompletionAction = "hibernate".parse().unwrap();
        assert_eq!(action, CompletionAction::Hibernate);
        let plan = plan(&action, Path::new("D:/dl/a.bin"));
        assert_eq!(plan.program, PathBuf::from("shutdown.exe"));
        assert_eq!(plan.args, vec![PathBuf::from("/h")], "休眠不接受落点参数");
    }

    #[test]
    fn parses_open_run_and_shutdown() {
        assert_eq!(
            "open".parse::<CompletionAction>().unwrap(),
            CompletionAction::Open
        );
        assert_eq!(
            "run:C:\\tools\\scan.exe"
                .parse::<CompletionAction>()
                .unwrap(),
            CompletionAction::Run("C:\\tools\\scan.exe".into())
        );
        assert_eq!(
            "shutdown:60".parse::<CompletionAction>().unwrap(),
            CompletionAction::Shutdown { delay_secs: 60 }
        );
    }

    #[test]
    fn rejects_empty_run_unknown_values_and_bad_shutdown() {
        assert!(
            "run:"
                .parse::<CompletionAction>()
                .unwrap_err()
                .contains("run:")
        );
        assert!(
            "wat"
                .parse::<CompletionAction>()
                .unwrap_err()
                .contains("open")
        );
        let bare = "shutdown".parse::<CompletionAction>().unwrap_err();
        assert!(bare.contains("延迟"), "shutdown 必须明说要带延迟：{bare}");
        let too_soon = "shutdown:59".parse::<CompletionAction>().unwrap_err();
        assert!(
            too_soon.contains("60"),
            "低于 60 秒要拒绝并写出下限：{too_soon}"
        );
        let not_a_number = "shutdown:soon".parse::<CompletionAction>().unwrap_err();
        assert!(not_a_number.contains("十进制"), "{not_a_number}");
    }

    #[test]
    fn open_plan_is_explorer_with_the_landing_path() {
        let plan = plan(&CompletionAction::Open, Path::new("D:/dl/a.bin"));
        assert_eq!(plan.program, PathBuf::from("explorer.exe"));
        assert_eq!(plan.args, vec![PathBuf::from("D:/dl/a.bin")]);
    }

    #[test]
    fn run_plan_is_the_program_with_the_landing_path_as_its_only_argument() {
        let plan = plan(
            &CompletionAction::Run("C:\\tools\\scan.exe".into()),
            Path::new("D:/dl/a.bin"),
        );
        assert_eq!(plan.program, PathBuf::from("C:\\tools\\scan.exe"));
        assert_eq!(plan.args, vec![PathBuf::from("D:/dl/a.bin")]);
    }

    #[test]
    fn shutdown_plan_is_shutdown_exe_with_the_delay() {
        let plan = plan(
            &CompletionAction::Shutdown { delay_secs: 60 },
            Path::new("D:/dl/a.bin"),
        );
        assert_eq!(plan.program, PathBuf::from("shutdown.exe"));
        assert_eq!(
            plan.args,
            vec![
                PathBuf::from("/s"),
                PathBuf::from("/t"),
                PathBuf::from("60")
            ]
        );
        assert_eq!(plan.args.len(), 3, "shutdown 不接受落点参数");
    }

    #[test]
    fn execute_of_a_missing_program_returns_an_error_without_spawning_anything() {
        let plan = ActionPlan {
            program: PathBuf::from("dlm-no-such-completion-action-exe"),
            args: vec![PathBuf::from("D:/dl/a.bin")],
        };
        assert!(execute(&plan).is_err());
    }

    #[test]
    fn reveal_open_is_explorer_with_the_landing_path_only() {
        let plan = reveal_plan(RevealMode::Open, Path::new("D:/dl/a.bin"));
        assert_eq!(plan.program, PathBuf::from("explorer.exe"));
        assert_eq!(plan.args, vec![PathBuf::from("D:/dl/a.bin")]);
        assert_eq!(plan.args.len(), 1);
    }

    /// `/select,<路径>` 必须是**一个** argv：拆开时资源管理器会把路径当成另一件要打开的东西。
    #[test]
    fn reveal_select_keeps_the_switch_and_the_path_in_one_argument() {
        let plan = reveal_plan(RevealMode::Select, Path::new("D:/dl/a.bin"));
        assert_eq!(plan.program, PathBuf::from("explorer.exe"));
        assert_eq!(
            plan.args,
            vec![PathBuf::from("/select,D:/dl/a.bin")],
            "`/select,` 与路径必须在同一个参数里"
        );
        assert_eq!(plan.args.len(), 1, "拆成两个参数就不是语义那件事了");
        let only = plan.args[0].to_string_lossy().to_string();
        assert!(only.starts_with("/select,"), "{only}");
        assert!(
            !only.starts_with("/select, "),
            "多一个空格会让资源管理器把路径读成第二件东西：{only}"
        );
    }

    #[test]
    fn reveal_mode_parses_only_the_two_documented_words() {
        assert_eq!("open".parse::<RevealMode>().unwrap(), RevealMode::Open);
        assert_eq!("select".parse::<RevealMode>().unwrap(), RevealMode::Select);
        assert_eq!(
            " select ".parse::<RevealMode>().unwrap(),
            RevealMode::Select
        );
        let err = "delete".parse::<RevealMode>().unwrap_err();
        assert!(err.contains("open") && err.contains("select"), "{err}");
        assert_eq!(RevealMode::Open.as_str(), "open");
        assert_eq!(RevealMode::Select.as_str(), "select");
    }
}
