# C3 分类规则与更新检查 Implementation Plan

**状态**：已交付（2026-10-28；四闸 297/4/114/clippy 0；Chrome 156 回归）。
**Spec**：[`../specs/2026-10-28-c3-categories-update-design.md`](../specs/2026-10-28-c3-categories-update-design.md)

## Task 1 分类

- `src/queue.rs`：`category_for(name)` + 表驱动单测。
- `src/gui.rs`：`TaskView.category`；UI 任务行显示分类、顶部筛选下拉；`load()` 按筛选过滤。
- `tests/integration.rs`：GUI 端到端任务 JSON 含 `category`；旧队列无新字段可读由 queue 单测覆盖。

## Task 2 更新检查

- `src/gui.rs`：`GuiConfig.update_url`、`UpdateView`、`version_is_newer`、`check_update`、
  `/api/update`、启动后台检查、UI 状态行。
- `src/bin/dlm-gui.rs`：`--update-url`；只收 http/https。
- `gui::tests`：版本比较表；本地 stub 解析。
- `tests/integration.rs`：不改；用 `gui::tests` 的 stub 覆盖解析。
- 无外部网络请求（默认未启用）。

## Task 3 文档与验收

- README `dlm-gui` 选项、分类/更新边界；STATUS/路线图/能力表/证据。
- 四闸；Chrome 真浏览器回归（`verify-gui.mjs`）；更新截图/日志。
- 提交 `feat: 队列分类与更新检查`（本地，不 push）。
