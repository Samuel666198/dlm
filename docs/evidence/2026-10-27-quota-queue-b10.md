# A16 配额 / 队列动作 / B10 证据（2026-10-27）

**四闸**（收口复跑）：`--lib` **294/0**（3.01 s）· `--bins` **4/0** · `integration` **113/0**（64.24 s）·
`clippy --all-targets -- -D warnings` **0**。数字随发漂移，跑一遍看 `0 failed`。

## 1. 内核 `--max-bytes`

真二进制 + 本机 node 对端（`/sz/1048576`，`--connections 1 --max-bytes 524288`）：

```text
下载未完成  目标：…\budget.bin  全长 1048576 B，落盘 563387 B，归属已定 563387 B
说明：已达到本次运行字节预算（--max-bytes=524288）：断点已保留，再次运行会继续
code=3
```

再跑同一任务不带预算：

```text
下载已完成  目标：…\budget.bin  全长 1048576 B，落盘 1048576 B，归属已定 1048576 B
code=0
```

- 563387 B = 预算 524288 B 越过帽最多一块（64 KiB）的上界内；断点 `.dlm.json` 保留。
- 集成用例：`the_binary_stops_at_the_byte_budget_and_resumes_to_completion`、
  `max_bytes_with_data_file_is_a_parameter_error`；单元/装配层见 Task 报告。

## 2. 队列配额

集成用例 `gui_quota_stops_and_resumes_across_a_window`：`dlm-gui --quota 262144 --quota-window 2
--concurrency 1`，添加 `/sz/524288`；轮询到 `done`，落盘 524288 B，任务日志含
"已达到本次运行字节预算"。

同参数手动跑一次 `/sz/524288`（窗口 2 秒），日志保留了两轮：

```text
下载未完成  目标：…\quota.bin  全长 524288 B，落盘 496126 B，归属已定 327152 B
说明：已达到本次运行字节预算（--max-bytes=262144）：断点已保留，再次运行会继续
下载已完成  目标：…\quota.bin  全长 524288 B，落盘 524288 B，归属已定 524288 B
```

- 任务第一次退出码 **3**、状态回到 `queued`、`hold_until` 指向下一窗口；窗口重置后自动续传至 `done`。
- 配额按**断点覆盖量**记账（`quota.used` 采样 327152）；文件长度超过覆盖量是 8 个 worker
  各自"越过帽再落一块"的并集（spec §2.1/§2.7 的 N×64 KiB 已接受上界）。
- 最终文件 524288 B；队列文件 `quota.used` 在收尾窗口重置后为 0。

## 3. B10 真浏览器

Edge 156；无 Cookie 页面 `http://127.0.0.1:<port>/file.bin`：

```text
browser=Edg/156.0.4314.50
queue_status=dlm queue: 1793169971270-1
queue_file=…\queue-out\file.bin bytes=65536
direct_status=dlm job: 1793169974-1
direct_file=…\direct\file.bin ok=true
```

- GUI 开着：宿主走 `/api/add`，回执 `mode=queue`，落点由 `dlm-gui --out-dir` 决定。
- 杀掉 GUI：端点文件仍在但连不上 ⇒ 回退 B2 直连，落点回到宿主 `download_dir`。
- 截图：`docs/evidence/2026-10-27-quota-queue-b10.png`；完整日志：
  `docs/evidence/2026-10-27-quota-queue-b10-browser.log`。
- 队列文件里没有 Cookie 字段；带 Cookie 的投递由单元用例
  `host_keeps_cookie_downloads_on_the_direct_path` 钉住。

## 4. 未做 / 边界

- 分类规则、更新检查（外网需单独点头）、B3、A5、原生 GUI/Windows 服务、队列 Cookie。
- 配额滚动窗口/自然日/每任务配额不做；队列暂不透传 `--max-speed`。
- 端点文件含 token，位于用户目录，同用户进程可读，与"能直接 spawn dlm"同一信任边界。
