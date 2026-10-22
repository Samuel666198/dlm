# A3b shutdown 动作 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**状态**：已交付（2026-10-21；四闸 259/3/105/clippy 0）。

**Goal:** `--on-complete shutdown:<seconds>`：下载成功且校验通过后，用 `shutdown.exe /s /t <N>` 安排关机；最少 60 秒；测试只验计划，绝不真关机。

**Architecture:** 扩 `actions::CompletionAction` 的 `Shutdown { delay_secs }`；`plan` 交 `shutdown.exe /s /t N`；`main` 在执行前打取消提示。计划任务不塞进前台 CLI，移到 C1；阻止休眠不做。

**Spec:** [`../specs/2026-10-15-dlm-kernel-design.md`](../specs/2026-10-15-dlm-kernel-design.md) §10 **R20**（R19 的"shutdown 拒绝"半句作废）。

## 决策

- **shutdown 做**：显式 `shutdown:<秒数>`，最少 60 秒（防误触）。真实关机只能由用户显式给出；
  测试只验解析与 `ActionPlan`。
- **计划任务移到 C1**：进程内等待会占住终端；系统计划任务会写系统状态。C1 的队列项才是"起始时间/时间窗"
  的正确落点，不在 A3b 造一个一次性 sleep。
- **阻止休眠不做**：要 Win32 FFI，且不是 CLI 内核的核心；用户可用系统电源计划。

## Tasks（已完成）

- [x] `actions::CompletionAction` 加 `Shutdown { delay_secs }`；`FromStr` 接受 `shutdown:<N>`（N ≥ 60），
      拒绝裸 `shutdown`、非数字、`< 60`。
- [x] `actions::plan` 对 `Shutdown` 交 `shutdown.exe /s /t N`（三个 argv，不带落点）。
- [x] `main` 在执行前打 `警告：将在 N 秒后关机；取消请运行 shutdown /a`；动作失败仍只警告、不改退出码。
- [x] 单测：解析、计划、裸 shutdown/59 秒/非数字拒绝。
- [x] 二进制：`bare_shutdown_is_a_parameter_error_and_never_reaches_the_wire`（退 2、不上网、不关机）。

## 验收

- 四闸：`--lib` **259/0**（3.01 s）· `--bins` **3/0** · `integration` **105/0**（63.96 s）· clippy **0**。
- 未执行任何真实 `shutdown.exe`；`shutdown:60` 的端到端没有用例（会关机）。
- 计划任务/阻止休眠未做，已写 R20/README。

## 提交

```bash
git add src/actions.rs src/cli.rs src/main.rs tests/integration.rs README.md docs/superpowers/specs/2026-10-15-dlm-kernel-design.md docs/superpowers/plans/2026-10-15-dlm-kernel-STATUS.md docs/superpowers/plans/2026-10-21-三路路线图.md docs/evidence/2026-10-21-idm能力对照表.md docs/superpowers/plans/2026-10-21-A3b-shutdown.md
git commit -m "feat: shutdown 完成后动作（最少 60 秒）"
```
