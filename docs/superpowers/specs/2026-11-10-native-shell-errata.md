# 设计稿勘误（reviewer 反馈，2026-11-10）

本文件是 `2026-11-10-native-shell-full-design.md` 的**强制勘误**。
评审发现原稿有多处前提错误，实现前必读本文件；两者冲突时**以本文件为准**。

## E1 步骤 1 不是"纯搬家" —— 已规避

**原稿错**：把步骤 1 说成"纯搬家，不改逻辑"。

**事实**：`run()` 主线程阻塞在 `listener.incoming()`（`mod.rs:184`），
而 eframe 必须占住主线程。HTTP accept 循环必须挪到独立线程，
端点文件的写入/删除时机、`run()` 的返回路径都要重排。这是行为变更。

**实际处置**：步骤 1 已按"只拆文件、不动 `run()`"完成并提交
（`refactor: 拆分 gui.rs`），`listener.incoming()` 仍在主线程。
真正的线程重排放到步骤 2 单独做，**不再声称是搬家**。

## E2 集成测试会因真窗口红掉 —— 必须先加 `--headless`

**事实**：`tests/integration.rs` 有 11 处 `CARGO_BIN_EXE_dlm-gui` 真起进程。
步骤 2 后每次跑集成测试会弹 11 个真窗口；无 GPU 的机器直接
"HTTP 没起来"红掉。

**必做**：步骤 2 第一步就给 `dlm-gui` 加 `--headless`：
只起 runner + HTTP，**不初始化 eframe**。所有集成测试改用它。

## E3 依赖实测的前提是错的

**原稿错**：说"关默认特性掉的是 wgpu，换 glow 省 132 包"。

**事实**：
- eframe 的默认渲染后端**本来就是 glow**；wgpu 是 opt-in 特性
- `default-features = false` 真正掉的是 **`default_fonts`**（掉了所有文字空白）
  与 `accesskit`（无障碍树，`egui_kittest` 靠它找控件）
- 省下的 132 包主要是 **wayland/x11 + accesskit**，不是 wgpu
- `egui_kittest` 的 snapshot 默认要 wgpu，作为 dev-dependency 会把它拉回来

**修正后的数字**（仍需实测确认）：

| 场景 | 包数 |
|---|---|
| 默认 + tray-icon | 493 |
| 关 wayland/x11/accesskit（**保留 default_fonts**）+ tray-icon | 361 |
| 再加 egui_kittest（含 wgpu） | 440 |

**结论**：361 只在"非测试构建"成立。**必须保留 `default_fonts`**，
否则窗口里一个字都没有。若要 361，得自己嵌字体（本来也要嵌中文字体）。

## E4 按 host 限连接架构上不成立

**原稿错**：说改 `scheduler.rs` 就能限"同一服务器连接数"。

**事实**：每任务 spawn 自己的 `dlm` 进程，各有各的 scheduler。
改 `scheduler.rs` 只限**一个任务内**。默认 `connections=8` 且
`per_host=8` 时是恒等操作；同 host 两个任务照样 2×N。
`Event`/`Action` 没有 host 字段，要改 `Scheduler::new` 签名 + 34 条调度器测试。

**修正方案**（二选一，待定）：
- **A（推荐）**：队列层做跨进程预算。runner 知道每个任务的目标 host，
  派活时按 host 计数，超了就延后启动。不动引擎，不动 34 条测试。
- **B**：单进程内多任务共享 scheduler。改动巨大，否决。

**在定案前，步骤 7 不做。**

## E5 设置项与既有文档化限制冲突

**事实**：
- `child_args` 不传 `--connections` / `--max-speed`（已核实，`runner.rs:259`）
- README:334 明写"队列暂不透传限速"是**已接受的限制**
- 设置面板却列了这两项

**修正**：设置面板要么
- 把这两项透传进 `child_args`（等于解除那条限制，需同步改 README），或
- 从面板移除，只保留全局默认

**优先级规则必须写清**（原稿没有）：CLI > `settings.json` > 代码默认。
`out_dir` 同时来自 CLI 与设置 ⇒ 以 CLI 为准，设置面板显示"由命令行覆盖"。

## E6 关窗收进托盘没给实现路径

原稿只说"关窗 = 收进托盘"，没写怎么做。实际路径：

```
ctx.input(|i| i.viewport().close_requested())
  → 若 close_to_tray 且非"真退出"：ctx.send_viewport_cmd(ViewportCommand::CancelClose)
                                  + ViewportCommand::Visible(false)
托盘"显示窗口"：clone 的 egui::Context 发 Visible(true) + Focus + request_repaint
托盘"退出"：先置 quitting = true，再发 Close —— 否则被自己的 CancelClose 吞掉
--autostart：ViewportBuilder::with_visible(false)
```

## E7 单实例与托盘互斥

**问题**：托盘常驻后，双击 exe 命中 `probe_existing_instance` 只打印 URL 退 0，
窗口永远出不来。

**修正**：加 `POST /api/show` —— 已有实例收到后把窗口显示出来。
单实例分支改为：探测到已有实例 ⇒ 打 `/api/show` ⇒ 退 0。

## E8 退出路径缺握手

队列 store 归 runner 线程所有，eframe 退出不等它 ⇒ 丢最后一轮状态。

**修正**：`Ticket::Exit` + reply 通道 + join 超时（3 秒），
超时就放弃并记日志，不无限等。

## E9 休眠与既有裁定冲突

**事实**：`queue::validate_on_complete`（`queue.rs:361`）明确拒绝 shutdown 族，
理由"队列可能同时完成多个任务"。而设置面板的 `on_complete` 正是给队列任务用的。

**修正**：**队列不做休眠**。理由与 shutdown 相同。
休眠只保留在 CLI 的 `--on-complete`（单任务场景，用户明确知道只跑一个）。
设置面板的"完成后"选项里**不出现休眠**。

`rundll32 powrprof.dll,SetSuspendState` 也不可靠：三参数靠 rundll32 解析，
且 Hibernate 位取决于系统是否开着休眠（`powercfg /h`）。
若日后要做，用 `shutdown /h`（休眠没开就直接失败，行为可判定）。

## E10 标题栏要决策

demo 是自绘标题栏（34px），要 `with_decorations(false)` + 自己写拖拽/缩放/
最大化/贴边，并**失去 Win11 圆角、阴影、Snap Layouts**。

**这是 C 类问题，需要你定**：
- **A**：原生装饰（`with_decorations(true)`）—— 跟 demo 不一样，但白得 Win11 全套
- **B**：自绘 —— 长得像 demo，但要自己实现窗口管理，且观感不如系统原生

**默认按 A 实现**，因为下载器不是设计作品，系统一致性更重要。

## E11 UI 验收覆盖不到反目标

反目标是"托盘点不开 / 僵尸进程 / 启动变慢"，但 `egui_kittest` 测不到
winit/eframe/托盘/关窗/剪贴板 —— 它只把 `Ui` 渲到离屏纹理。

**修正**：补三层
| 层 | 手段 | 覆盖 |
|---|---|---|
| 逻辑 | 单测 | 设置读写、剪贴板匹配、限流决策 |
| 集成 | `--headless` + HTTP | 队列、扩展投递、reveal、自启、单实例 |
| 冒烟 | **agent-desktop 真驱动窗口** | 托盘菜单、关窗收托盘、窗口能显示 |
| 启动耗时 | 计时打点 | exe 启动 → 首帧，**阈值 300ms**（待实测校准） |

`egui_kittest` 只做三态渲染的像素回归，不承担交互验收。

## E12 布局常量必须按 point 算

原稿的 34/50/32/212/372 写的是 px，egui 用 point（`pixels_per_point`）。
150% 缩放下直接错位。

**修正**：全部改用 point，并在 `theme.rs` 里定义为常量；
窗口初始 1120×700 pt，最小 760×480 pt。

## E13 剪贴板提示链路是断的

**问题**：
- `tray-icon` 不暴露 balloon（要自己 `Shell_NotifyIcon` NIM_MODIFY），
  361 个包里没有通知 crate
- 气泡问"要下载吗"却没有可点入口，托盘菜单里也没有剪贴板项

**修正**：
- 不做气泡。命中后在**窗口内**弹一条非模态提示条（右下角），带"下载/忽略"
- 窗口不可见时（托盘态）用托盘图标 tooltip 计数提示（`tray-icon` 支持 set_tooltip）
- 托盘菜单加一项"剪贴板：N 条待处理"

## 其他已采纳的小修正

| 项 | 修正 |
|---|---|
| 速度/剩余时间无数据源 | `tick` 已有 `last_delivered`，推算速率与 ETA 进快照 |
| 快照 500ms 读一次断点文件 | 1000 行 = 每轮 1000 次文件读；改为 runner 内缓存，只对变化的任务读 |
| UI 跨帧持锁 | 改 `Arc<Mutex<Arc<Vec<TaskView>>>>`，读时 clone 指针 |
| `Ticket::Add` 的 reply | UI 线程不能阻塞等，用回调或轮询 |
| `FILE_LINK_RE` 套用会变义 | 用固定扩展名集合手写，不引入 regex |
| 剪贴板判变化 | 用 `GetClipboardSequenceNumber()`，变了才 `OpenClipboard` |
| 删 ui.html 会红门禁 | 同时删 `check-ui-script.mjs` 与 `gates.ps1` 那条 |
| 步骤 9 图标 | 需要 `build.rs` + `winresource`，仓库现在没有 build.rs |
| `tray_hint` 语义 | 拆成 `tray_hint`（开关）+ `tray_hint_shown`（已提示过） |
| 测试数字会漂 | 门禁描述写"全绿"，不写死条数 |

## C 类问题（需你定，已按默认值推进）

| # | 问题 | 我的默认 |
|---|---|---|
| C1 | 重写范围：只换壳 vs 补功能 | 按 D8：补剪贴板 + 休眠（CLI）+ 限流（待 E4 定案） |
| C2 | 队列是否支持休眠 | **不支持**（E9） |
| C3 | 二进制体积预算 | 待实测，先不设硬限 |
| C4 | 自绘 vs 原生标题栏 | **原生**（E10） |
| C5 | 剪贴板默认 | **关**（已确认） |
| C6 | "启动更快"的数 | 首帧 ≤ 300ms（待校准） |
| C7 | 人工截图能否当长期 UI 验收 | 不能，用 agent-desktop 冒烟替代 |

## E14 `--autostart` 隐藏窗口未解决（实测记录）

目标：`--autostart` 起的实例不弹窗（P3）。

**试过四条路，全部无效**（用 `IsWindowVisible` 实测）：

| 做法 | 结果 |
|---|---|
| `ViewportBuilder::with_visible(false)` | 窗口仍可见 |
| 首帧 `ViewportCommand::Visible(false)` | 命令发出，仍可见 |
| 第 20 帧再发 `Visible(false)` | 同上 |
| Win32 `ShowWindow(GetActiveWindow())` / `EnumWindows` 按 pid 找 | 调用执行，`MainWindowHandle` 仍可见 |

**判断**：eframe 在窗口事件循环里会重新显示窗口，外部隐藏被覆盖；
或者 `MainWindowHandle` 指向的是另一个隐藏窗口而真正可见的那个没被找到。

**现状**：`--autostart` 仍会短暂显示窗口，随后被 `hide_window()` 尝试隐藏。
**这是已知缺口，不是已完成项。** 后续可选方向：
1. 读 eframe 的 viewport 事件，在 `ViewportEvent::Visible` 之后立刻隐藏
2. 放弃 `--autostart` 的"完全不弹窗"，改成"启动后最小化到托盘"（`SW_MINIMIZE`）
3. 用 `winit` 的 `Window::set_visible`（绕过 eframe 的封装）

托盘本身的「关窗收托盘」「显示窗口」「退出」三条语义**已验证通过**，
不受此缺口影响。