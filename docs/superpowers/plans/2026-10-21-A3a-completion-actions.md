# A3a 完成后动作 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**状态**：已交付（2026-10-21；四闸 258/3/104/clippy 0）。

**Goal:** `--on-complete open` 或 `--on-complete run:<exe>`：下载成功且校验通过后，打开落点或启动程序（落点作为第一个参数）；动作失败只警告，不改下载结果与退出码。

**Architecture:** 新增 `src/actions.rs`（纯解析/计划 + 一行 spawn）。`Cli` 用 `FromStr` 在参数层校验 `open` / `run:<exe>`；`main` 在 `report::finish` 打出结果后执行，`!ok` 只打"未执行"。`shutdown`、计划任务、阻止休眠明确不在本发。

**Tech Stack:** Rust 2024、`std::process::Command`、clap derive、现有二进制测试。

**Spec:** [`../specs/2026-10-15-dlm-kernel-design.md`](../specs/2026-10-15-dlm-kernel-design.md) §10 **R19**。

## Global Constraints

- 四闸是 floor；不推送；不 `checkout`/`reset`/`stash`；报告先写、提交第二。
- **不在测试里真启动 `explorer.exe`、不执行任何关机/休眠**。动作的单元测试只测解析与 `ActionPlan`；
  成功路径不 spawn，错误路径用不存在的程序名。
- 动作失败不改下载结果/退出码（R19）。
- `--on-complete` 不进 `Task`/`Config`，不参与续传比对。
- 不经 shell；`run:<exe>` 只接受一个程序路径，落点作为唯一附加参数。
- 不新增依赖。

## File Structure

- Create: `src/actions.rs` —— `CompletionAction`、`ActionPlan`、`plan`、`execute`、单测。
- Modify: `src/lib.rs` —— `pub mod actions;`
- Modify: `src/cli.rs` —— `--on-complete` 字段、`action()` 访问器、测试。
- Modify: `src/main.rs` —— `dispatch` 取 action；`finish` 后执行/警告。
- Modify: `tests/integration.rs` —— 两条二进制级用例（成功+动作失败仍退 0；失败下载不执行动作）。
- Modify: `README.md`、STATUS、路线图、对照表。

---

### Task 1: `actions` 模块

**Files:**
- Create: `src/actions.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Produces: `pub enum CompletionAction { Open, Run(PathBuf) }`
- Produces: `pub struct ActionPlan { pub program: PathBuf, pub args: Vec<PathBuf> }`
- Produces: `pub fn plan(action: &CompletionAction, file: &Path) -> ActionPlan`
- Produces: `pub fn execute(plan: &ActionPlan) -> anyhow::Result<()>`
- `impl FromStr for CompletionAction { type Err = String; }`

- [ ] **Step 1: 写失败测试**

```rust
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
```

- [ ] **Step 2: 跑测试确认红**

Run: `cargo test --lib actions::`
Expected: 模块不存在。

- [ ] **Step 3: 写实现**

```rust
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

use anyhow::Context;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionAction {
    Open,
    Run(PathBuf),
}

impl FromStr for CompletionAction {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "open" => Ok(Self::Open),
            "shutdown" => Err("shutdown 需要单独点头，本版未启用；可用 open 或 run:<exe>".into()),
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

pub fn execute(plan: &ActionPlan) -> anyhow::Result<()> {
    Command::new(&plan.program)
        .args(&plan.args)
        .spawn()
        .with_context(|| format!("启动 {} 失败", plan.program.display()))?;
    Ok(())
}
```

`src/lib.rs` 加 `pub mod actions;`。

- [ ] **Step 4: 跑测试**

Run: `cargo test --lib actions::`
Expected: 5 passed。

### Task 2: CLI 的 `--on-complete`

**Files:**
- Modify: `src/cli.rs`

- [ ] **Step 1: 字段与访问器**

```rust
use crate::actions::CompletionAction;

#[arg(long, value_name = "ACTION",
      help = "完成后动作：open 或 run:<exe>（程序收到落点路径作为第一个参数）")]
on_complete: Option<CompletionAction>,
```

`Debug` 加 `.field("on_complete", &self.on_complete)`；加：

```rust
pub fn action(&self) -> Option<&CompletionAction> {
    self.on_complete.as_ref()
}
```

- [ ] **Step 2: 测试**

```rust
#[test]
fn on_complete_parses_open_and_run_and_rejects_bad_values() -> anyhow::Result<()> {
    assert_eq!(
        parse(&["dlm", "--url", "https://x/a", "--on-complete", "open"])?.action(),
        Some(&CompletionAction::Open)
    );
    let run = parse(&["dlm", "--url", "https://x/a", "--on-complete", "run:C:\\tools\\scan.exe"])?;
    assert_eq!(run.action(), Some(&CompletionAction::Run("C:\\tools\\scan.exe".into())));

    let err = parse(&["dlm", "--url", "https://x/a", "--on-complete", "wat"]).unwrap_err();
    assert!(format!("{err:#}").contains("open"), "错误要指可用值：{err:#}");
    let shutdown = parse(&["dlm", "--url", "https://x/a", "--on-complete", "shutdown"]).unwrap_err();
    assert!(format!("{shutdown:#}").contains("单独点头"));
    Ok(())
}
```

- [ ] **Step 3: 跑 CLI 测试**

Run: `cargo test --lib cli::`
Expected: 全绿。

### Task 3: main 执行时机

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: 在消费 `Cli` 前取 action**

`dispatch`：

```rust
let quiet = cli.quiet();
let no_start = cli.no_start();
let action = cli.action().cloned();
let mut cfg = cli.into_config();
let task = match cli.into_task() { ... };
```

- [ ] **Step 2: `finish` 之后执行**

```rust
let rep = engine::run(&task, cfg)?;
let code = finish(&rep, &task, quiet);
if let Some(action) = action.as_ref() {
    if rep.ok {
        if let Err(err) = dlm::actions::execute(&dlm::actions::plan(action, &task.target_path())) {
            eprintln!("警告：完成后动作失败：{err:#}");
        }
    } else {
        eprintln!("完成后动作未执行：下载未完成");
    }
}
Ok(code)
```

- [ ] **Step 3: 跑 bins**

Run: `cargo test --bins`
Expected: 全绿。

### Task 4: 二进制级用例

**Files:**
- Modify: `tests/integration.rs`

- [ ] **Step 1: 成功下载 + 动作失败仍退 0**

```rust
#[test]
fn a_completion_action_failure_does_not_change_a_successful_exit_code() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let dir = harness::tmp_dir("a3-action-fail")?;
    let out = run_dlm(&[
        "--url", &format!("{base}/sz/65536"),
        "--out-dir", dir.path().to_str().unwrap(),
        "--name", "a3.bin",
        "--quiet",
        "--on-complete", "run:dlm-no-such-completion-action-exe",
    ])?;
    assert_eq!(out.status.code(), Some(0), "动作失败不许改下载成功档：{}", stderr_of(&out));
    assert!(stderr_of(&out).contains("完成后动作失败"), "{}", stderr_of(&out));
    drop(srv);
    Ok(())
}
```

- [ ] **Step 2: 失败下载不执行动作**

```rust
#[test]
fn a_failed_download_does_not_run_the_completion_action() -> anyhow::Result<()> {
    let (srv, base) = harness::spawn()?;
    let dir = harness::tmp_dir("a3-action-skip")?;
    let zeros = "0".repeat(64);
    let out = run_dlm(&[
        "--url", &format!("{base}/sz/65536"),
        "--out-dir", dir.path().to_str().unwrap(),
        "--name", "a3.bin",
        "--quiet",
        "--sha256", &zeros,
        "--on-complete", "run:dlm-no-such-completion-action-exe",
    ])?;
    assert_eq!(out.status.code(), Some(1), "校验不符仍是下载失败档");
    assert!(stderr_of(&out).contains("完成后动作未执行"), "{}", stderr_of(&out));
    assert!(!stderr_of(&out).contains("完成后动作失败"), "动作根本没该启动：{}", stderr_of(&out));
    drop(srv);
    Ok(())
}
```

- [ ] **Step 3: 跑二进制用例**

Run: `cargo test --test integration completion_action`
Expected: 2 passed。

### Task 5: 文档、四闸、提交

- [ ] **Step 1: README**

选项表加：

```markdown
| `--on-complete <ACTION>` | — | 完成后动作：`open` 或 `run:<exe>`；程序收到落点作为第一个参数；失败只警告、不改退出码 |
```

"不做"那行把"完成后动作链"改成"动作链/关机/计划任务（`open`／`run:` 已做）"。
加一句边界：`shutdown` 明确未启用，需单独点头。

- [ ] **Step 2: STATUS/路线图/对照表**

STATUS 追加 A3a 交付；路线图 A3 标"部分交付（A3a run/open）"，A3b 未做；
对照表 E4 改成"已覆盖（部分）"，E6 仍"本机可验"（阻止休眠未做）。

- [ ] **Step 3: 四闸**

```bash
cd "E:/WorkSpace/projects/tool/dlm"
export MSYS2_ARG_CONV_EXCL='*'
cargo test --lib
cargo test --bins
cargo test --test integration
cargo clippy --all-targets -- -D warnings
```

- [ ] **Step 4: 报告先落盘**

写 `.superpowers/sdd/2026-10-15-dlm-kernel/t99-A3a-report.md`：四闸、动作面、退出码独立性、没有买到的格子（`open` 成功路径未 spawn、shutdown 未做）。

- [ ] **Step 5: 提交**

```bash
git add src/actions.rs src/lib.rs src/cli.rs src/main.rs tests/integration.rs README.md docs/superpowers/specs/2026-10-15-dlm-kernel-design.md docs/superpowers/plans/2026-10-15-dlm-kernel-STATUS.md docs/superpowers/plans/2026-10-21-三路路线图.md docs/evidence/2026-10-21-idm能力对照表.md docs/superpowers/plans/2026-10-21-A3a-completion-actions.md
git commit -m "feat: 完成后动作 --on-complete open/run"
```
