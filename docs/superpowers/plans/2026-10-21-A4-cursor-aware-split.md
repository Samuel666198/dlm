# A4 块级光标劈分 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**状态**：已交付（2026-10-21；四闸 259/3/106/clippy 0）。

**Goal:** 把 `split_tail` 的切点建立在 worker 的**新鲜块级光标**上，使每次 `Retighten` 的重复流量不超过一块（64 KiB）。

**Architecture:** scheduler 的 `split_tail` 不动（仍是剩余尾巴的中点）。新增 `fetch::NO_PROGRESS` 与一个 per-worker `AtomicU64`：`run_window_with_limits` 每写一块存 `cursor - 1`；engine 在 `ping_free_workers` 前 `sync_in_flight_progress`，把新光标作为 `Event::Progress` 喂给 scheduler，再发 `Connected`。

**Spec:** [`../specs/2026-10-15-dlm-kernel-design.md`](../specs/2026-10-15-dlm-kernel-design.md) §10 **R21**。

## 基线（先量后改）

新增集成判据 `residual_duplicate_traffic_is_bounded_by_one_chunk_per_retighten`（48 MiB、8 worker、全速）：

| | duplicated | tightened | 上界 tightened×64 KiB |
|---|---:|---:|---:|
| 改动前 | **9,012,699 B**（17.2%） | 54 | 3,538,944 |
| 改动后 | **319,799 B**（0.63%） | 32 | 2,097,152 |

改动前超上界 2.55 倍，证明切点落到了 worker 已经读过的位置；改动后低于上界。

## Tasks（已完成）

- [x] `fetch::NO_PROGRESS` 哨兵；`run_window_with_limits` 加 `progress: Option<&AtomicU64>`，每写一块存 `cursor - 1`。
- [x] 旧入口 `run_window` / `run_window_with_if_range` 传 `None`，既有测试零改动。
- [x] `WorkerSpawn` / `Engine` 加 per-worker 光标；`dispatch` 前重置为 `NO_PROGRESS`。
- [x] `Engine::sync_in_flight_progress`：只喂新值，避免 trace 膨胀；`ping_free_workers` 先同步再 `Connected`。
- [x] 集成判据：`duplicated ≤ tightened × 64 KiB`，并要求 `tightened` 非空（防判据空转）。

## 验收

- 四闸：`--lib` **259/0**（3.01 s）· `--bins` **3/0** · `integration` **106/0**（63.87 s）· clippy **0**。
- 新增 1 条集成；既有 midpoint/floor 判据全部保留，未放宽。
- **未复量**：限速档、续传档的 dup；旧 12.8%–20.8% 只作 v1 历史。

## 提交

```bash
git add src/fetch.rs src/engine.rs tests/integration.rs README.md docs/superpowers/specs/2026-10-15-dlm-kernel-design.md docs/superpowers/plans/2026-10-15-dlm-kernel-STATUS.md docs/superpowers/plans/2026-10-21-三路路线图.md docs/superpowers/plans/2026-10-21-A4-cursor-aware-split.md
git commit -m "feat: 块级光标劈分，重复流量压到每笔≤一块"
```
