# C3 剩余项：分类规则与更新检查（设计）

**状态**：已实现（2026-10-28；四闸 297/4/114/clippy 0；Chrome 156 回归）
**范围**：只做产品层两项；不改下载落盘路径、不新增依赖、不改 B2/B10 的 Cookie 规则。

## 1. 分类规则

- 按**文件名扩展名**自动归类：`video` / `audio` / `archive` / `document` / `image` / `program` / `other`。
- 默认只作为 `TaskView.category` 暴露给 UI，不改变下载目录。
- `--category-dirs` 打开后，入队时把任务名写成 `<category>/<name>`（分类名只来自 `category_for`，
  用户不能传目录）；落点变成 `out-dir/<category>/<name>`，仍过 dlm 的相对子路径/落点闸。
- 规则住 `queue::category_for` 一处；`TaskView` 与 UI 共用；未知/无扩展名 ⇒ `other`。
- UI：任务行显示分类，顶部下拉筛选；不影响现有 API 数组形状（只加字段）。

## 2. 更新检查

- **默认关闭**：不给 `--update-url` 就一次外网请求都不发。
- `dlm-gui --update-url <URL>`：仅接受 `http://` / `https://`；启动时在后台 GET 一次 JSON：
  ```json
  {"version":"0.2.0","url":"https://example.com/download","notes":"..."}
  ```
  `version` 必填；`latest` 也可作同义键。比较按数字点段（`0.2.0` > `0.1.9`）。
- `GET /api/update`（token 必需）返回：
  `{enabled,current,latest,available,url,notes,checked_at,error}`；未启用时 `enabled=false`。
- UI 显示"有新版本 / 已是最新 / 未启用 / 检查失败"；只通知，不自动下载安装。
- 安全：无硬编码更新服务器；URL 由用户给；不把响应写进队列/任务日志；10 秒超时。
- 测试用本机 stub，不访问公网。

## 3. 验收

- `queue::category_for` 表驱动单测；`TaskView.category` 出现在 API；UI 筛选不影响任务操作。
- `check_update` 对本地 stub 解析版本并判定；`version_is_newer` 表驱动。
- 旧 `queue.json` 直接可读；四闸全绿；Chrome/Edge 真浏览器回归现有 UI。
- 不做：自动更新、DASH、A5、原生 GUI、队列 Cookie。
