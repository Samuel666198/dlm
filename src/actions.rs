//! 完成后动作（R19）：`open` 或 `run:<exe>`。只在下完且校验通过后由 `main` 调用。
//!
//! 这一层不做下载决策，也不看 `Report`：它只回答"给一个动作和一个落点，要启动什么进程"。
//! 不经 shell；`run:<exe>` 的程序收到落点路径作为**唯一**附加参数。`shutdown` 明确不在本发。

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
}

impl FromStr for CompletionAction {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "open" => Ok(Self::Open),
            "shutdown" => {
                Err("shutdown 需要单独点头，本版未启用；可用 open 或 run:<exe>".into())
            }
            _ => match raw.strip_prefix("run:") {
                Some(exe) if !exe.trim().is_empty() => Ok(Self::Run(PathBuf::from(exe))),
                Some(_) => Err("完成后动作 run: 后面必须跟一个程序路径".into()),
                None => Err("完成后动作只认 open 或 run:<exe>".into()),
            },
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
    fn parses_open_and_run() {
        assert_eq!("open".parse::<CompletionAction>().unwrap(), CompletionAction::Open);
        assert_eq!(
            "run:C:\\tools\\scan.exe".parse::<CompletionAction>().unwrap(),
            CompletionAction::Run("C:\\tools\\scan.exe".into())
        );
    }

    #[test]
    fn rejects_empty_run_unknown_values_and_shutdown() {
        assert!("run:".parse::<CompletionAction>().unwrap_err().contains("run:"));
        assert!("wat".parse::<CompletionAction>().unwrap_err().contains("open"));
        let shutdown = "shutdown".parse::<CompletionAction>().unwrap_err();
        assert!(shutdown.contains("单独点头"), "shutdown 要明说未启用，不许静默忽略：{shutdown}");
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
    fn execute_of_a_missing_program_returns_an_error_without_spawning_anything() {
        let plan = ActionPlan {
            program: PathBuf::from("dlm-no-such-completion-action-exe"),
            args: vec![PathBuf::from("D:/dl/a.bin")],
        };
        assert!(execute(&plan).is_err());
    }
}
