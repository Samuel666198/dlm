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
                match raw.strip_prefix("run:") {
                    Some(exe) if !exe.trim().is_empty() => Ok(Self::Run(PathBuf::from(exe))),
                    Some(_) => Err("完成后动作 run: 后面必须跟一个程序路径".into()),
                    None => Err("完成后动作只认 open、run:<exe> 或 shutdown:<秒数>".into()),
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
    }
}

/// 启动后**不等待**：`open`/`run` 是旁路动作，dlm 的退出码与下载结果不由子进程决定。
pub fn execute(plan: &ActionPlan) -> anyhow::Result<()> {
    Command::new(&plan.program)
        .args(&plan.args)
        .spawn()
        .with_context(|| format!("启动 {} 失败", plan.program.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
