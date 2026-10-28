# C3 分类与更新检查证据（2026-10-28）

**四闸**（收口亲跑）：`--lib` **297/0**（3.01 s）· `--bins` **4/0** · `integration` **114/0**（64.63 s）·
`clippy --all-targets -- -D warnings` **0**。

## 1. 分类

- `queue::category_for` 按扩展名归 `video/audio/archive/document/image/program/other`；
  表驱动单测 `category_rules_are_extension_based_and_default_to_other`（含 `movie.MP4`、
  `pack.tar.gz`、无扩展名、尾点）。
- `TaskView.category` 出现在 `/api/tasks`；集成用例 `gui_queue_adds_and_finishes_a_download`
  断 `gui.bin` 的 `category=other`。
- UI 任务行显示分类（中文名），顶部下拉筛选；不改落盘目录。

## 2. 更新检查

- `version_comparison_is_numeric_by_dotted_segment`：`0.10.0 > 0.9.9`、远端更旧不算新版本。
- `update_check_reads_a_local_manifest`：本机 stub 返回
  `{"version":"99.0.0","url":"…","notes":"test"}`，判 `available=true`。
- `gui_update_check_exposes_a_local_manifest`：起真 `dlm-gui --update-url http://127.0.0.1:<stub>`，
  轮询 `/api/update` 直到 `checked_at`，断 `enabled/latest=99.0.0/available/notes`。
- 没有公网请求；不启用时 UI 显示“更新检查未启用”。

## 3. UI 重做（2026-10-28）

- 轻量单列任务流：去掉常驻侧栏/检查器；配额与更新收进顶栏；详情与日志由右侧抽屉承载。
- 接口与字段不变；`src/ui.html` 通过 `include_str!` 进入 `dlm-gui`，无前端依赖/CDN。
- Chrome 156 回归通过；截图/日志见 `2026-10-22-c3-gui.*`。

## 4. 真浏览器回归

Chrome 156，`verify-gui.mjs`：

```text
tasks="gui.bin — done\nhttp://127.0.0.1:61678/gui.bin\n65536 / 65536 B · exit 0 · 其他\n日志\n删除"
file=…\dl\gui.bin bytes=65536
```

截图/日志：`docs/evidence/2026-10-22-c3-gui.png` / `.log`（页面顶部有“更新检查未启用”和分类筛选）。

## 4. 边界

- 默认只打标签/筛选、不改落盘路径；`--category-dirs` 打开后任务名写成 `<category>/<name>`，
  落点进 `out-dir/<category>/`。手动实测：`movie.mp4` ⇒ `queue.name=video/movie.mp4`，
  文件落在 `out/video/movie.mp4`（1024 B）。
- `--open` 用 Edge/Chrome 的 `--app` 模式打开 Web UI（窗口外壳，不是原生控件）；四闸不启动浏览器，
  因此这一项只做编译/静态检查，未做真人窗口走查。
- 更新检查只通知，不下载/安装；无硬编码更新服务器；`--update-url` 只收 http/https。
- 外部请求 10 秒超时；本机 loopback 清单测试走 `no_proxy`。
- 未做：自动更新、B3b DASH/嗅探、A5、原生 GUI/Windows 服务、队列 Cookie。
