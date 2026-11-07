# v2.1 产品手感：四项交付与验收（2026-11-08）

按 [`../superpowers/plans/2026-11-07-v21-product-polish.md`](../superpowers/plans/2026-11-07-v21-product-polish.md)
执行完 P2 / P1 / P4 / P3 四发。托盘 P5 按计划本轮不做。

## 一句话结论

四发全部落地，门禁从四道扩到七道并全绿：`fmt` · `--lib` **322/0** · `--bins` **4/0** ·
`integration` **121/0** · `clippy` · 扩展静态检查 · UI 脚本语法。

## P2 打开文件 / 打开所在文件夹

新增 `POST /api/reveal`，body `{id, mode}`，`mode` 只认 `open` | `select`。

**安全形状**：接口只收 task id。落点由服务端按 `queue::target_path(out_dir, task.name)` 自己算，
任务名在入队时已禁 `/ \ :`。客户端塞进来的 `path` / `name` 字段**一律不看** ——
否则这个带 token 的本地接口就变成一个通用的"打开任意文件"入口。

**`select` 的 argv 形状**：`explorer.exe /select,<落点>`，切与路径必须在**同一个**参数里。
拆成两个参数时资源管理器会把路径当成第二件要打开的东西。

真浏览器回执（Edge 157）：

```json
{"ok":true,"spawned":true,
 "path":"C:\\Users\\ROG\\AppData\\Local\\Temp\\dlm-c3-RLR0zK\\dl\\gui.bin",
 "program":"explorer.exe",
 "args":["/select,C:\\Users\\ROG\\AppData\\Local\\Temp\\dlm-c3-RLR0zK\\dl\\gui.bin"]}
```

四条反臂（全部真跑）：

| 反臂 | 期望 | 实测 |
|---|---|---|
| 未知 id | 拒绝、不启动进程 | `{"error":"任务不存在","ok":false}` |
| 客户端伪造 `path: C:\Windows\System32\calc.exe` | 不采纳 | 回执里的 path 仍是队列那个 `gui.bin` |
| 未完成任务 | 拒绝并说明原因 | `任务还没下完` |
| 非法 mode（`delete`） | 400，不投递 ticket | 400 |

## P1 队列失败自动重试

只对退出码 **1** 重试。`--max-task-retries`（默认 **2**，`0` = 关）。
退避 **30 s → 120 s**；表尾之后判 `failed`，把决定权交回用户。

**退出码 → 状态这张表**（`finish_verdict`，逐格有用例）：

| 退出码 | 判定 | 为什么 |
|---|---|---|
| `0` | `done` | 下完了 |
| `3` | `queued`（配额等待） | 配额到量，等下一个窗口；**不计入重试轮数**，否则配额轮会吃掉重试额度 |
| `2` | `failed` | 参数错，重试必然同样失败 |
| `1` | `queued` + 退避，或 `failed` | 唯一值得自动再试的一档 |
| 其它 / 无码 | `failed` | 契约外，不猜 |

**真进程验收**（假 `dlm` 按调用次数发退出码，`DLM_FAKE_CODES=1,1,0`）：

```
gui_retries_a_failed_task_until_it_succeeds   ok  (3.64 s)
  → 子进程恰好起 3 次（计数文件 = 3），attempts 到过 1、终值 2，任务 done，落点 4 B

gui_does_not_retry_when_the_cap_is_zero       ok  (1.41 s)
  → --max-task-retries 0：判 failed，attempts 保持 0，子进程只起 1 次
```

**已知代价（照实登记）**：404/403 这类永久失败也是退出 1，会白重试满次数（默认 2 轮）。
要区分得给 `dlm` 新增退出档，那是契约变更，本轮不做。

## P4 UI 收尾

- 成功行不再印 `exit 0`，改显示「已校验」；
- 弹窗 Esc 关闭；关闭后焦点回到触发按钮；Tab 焦点圈；
- 空态补一句"也可以在浏览器里右键链接交给 dlm"。

真浏览器断言（同一次 `verify-gui.mjs`）：

```
p4_done_text="...64.0 KiB\n已校验"      ← 没有 exit 0
p4_modal={"inModal":true,"closed":true,"back":true}
```

## P3 开机自启与单实例

**自启**：`dlm-gui --autostart-install` 写 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\dlm-gui`，
值为 `"<路径>" --autostart`；路径含空格时整体加引号。`--autostart-remove` 按同名删。
安装器 `install.ps1 -Autostart` 也能开，默认**关**。`--autostart` 起的实例不弹浏览器窗口。

真注册表实测：

```
before: 不存在
after : E:\WorkSpace\projects\tool\dlm\target\debug\dlm-gui.exe --autostart
final : 已删除
```

**单实例**：启动时读端点文件并打一次 `/api/tasks?t=<token>`。能连上且是 JSON ⇒ 复用。
判据是"端口上真有人认得这个 token"，不是"文件存在" —— GUI 被硬杀会留下陈旧端点文件。

真进程实测：

```
first pid=42772   endpoint port=54469 pid=42772
second exit=0
dlm-gui already running (pid 42772): http://127.0.0.1:54469/?t=e2e-token
endpoint still pid=42772        ← 第二个没抢端口
```

反臂 `gui_starts_when_the_endpoint_file_is_stale`：端点指向一个没人听的端口 ⇒ 照常启动，
并把端点文件重写成本次监听的端口。

## 新增的两道门禁

| 门禁 | 为什么加 |
|---|---|
| `browser/test/check-ui-script.mjs` | `ui.html` 用 `include_str!` 嵌进二进制，**脚本语法错误不会让 cargo 失败**，只会白屏。本轮真实踩到（`const done` 与既有变量重声明），页面完全静默，只有真浏览器验收才发现。现在秒级检出，负向验证过：注入语法错误 ⇒ 退 1 |
| `gates.ps1` 工具链解析 | 这台机器 `cargo` 不在系统 PATH 上，原先门禁会以"cargo 不是命令"报红，看起来像代码坏了 |

## 打包

```
target/package/dlm-0.1.0-win-x64.zip
bytes=6667940
sha256=986d7f16c5bbe8f2851b0c63a6b9d5b448419621dda9ad93171ef2024f20a272
```

包内 15 个文件；`fake-dlm`（测试用假 dlm）不随包分发。
`dlm-gui --help` 在包内能看到 `--max-task-retries` / `--autostart` / `--new-instance` /
`--autostart-install` / `--autostart-remove`。

## 已知缺口（本轮不做，登记在计划里）

- **托盘 P5**：零新 crate 要裸写 Win32 FFI（约 400–600 行），且无法在无头环境验证；
  先让 `--autostart` 跑一段，看"看不到进度"是否真的困扰再定。
- 队列优先级、失败重排排序、剪贴板监控、系统计划任务、DASH/DRM：仍不在本轮范围。
- `integration` 偶发一次 `hls_downloads_master_playlist_segments_in_order` 抖动（并发下 stub 的
  5 ms accept 轮询）；随后连续三次全绿（119/0、119/0、121/0），未复现，登记为观察项。
