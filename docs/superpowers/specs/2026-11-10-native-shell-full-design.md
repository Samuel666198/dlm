# dlm 原生外壳设计

日期：2026-11-10 · 状态：**待批准**（批准后才写产品代码）
讨论落点：`2026-11-09-native-shell-design.md`（草稿，含 D1–D9 决策记录）

## 一、意图与理由

现在 `dlm-gui` 是「本地 HTTP 服务 + 用系统浏览器 `--app` 开无地址栏窗口」，
4.75 MB 纯 Rust、无 WebView。这条路拿到了"不做 GUI 又要有界面"，但拿不到三件事：

1. **真实窗口与托盘** —— 下载器要常驻，托盘是标配
2. **不依赖浏览器** —— 现在没有 Edge/Chrome 就没有界面
3. **启动快** —— 现在要先起浏览器再加载页面

三条都要，所以换原生。**浏览器扩展不动**：它是快捷入口，走
`扩展 → 宿主 → POST /api/add`，与 UI 怎么画无关。HTTP 服务留在进程里当扩展的入口。

## 二、目标 / 非目标 / 反目标

**目标**
- 双击即出窗口，不经过浏览器
- 托盘常驻；关窗 = 收进托盘
- 设置面板可关剪贴板监视（D9）
- 补三项能力：剪贴板监视、按 host 限连接数、完成后休眠（D8）
- 视觉按 demo 的设计语言（暖纸底 / 陶土橙 / 等宽数字），但允许先糙后调（D3）

**非目标**
- macOS（D2：之后再说）
- 站点抓取、FTP、拨号、DASH/DRM（recon P2）
- 界面语言 i18n（字段先留，本轮只做中文）

**反目标（这样就算失败）**
- 换完之后启动比现在慢
- 托盘图标点不开窗口，或关窗后进程还在但托盘没图标（僵尸进程）
- 为了做 UI 把引擎改坏 —— 引擎的 322 条单测必须全绿

## 三、约束（都是硬的）

| 约束 | 实测值 | 影响 |
|---|---|---|
| 依赖膨胀 | 见下节，**361 包**（glow 后端） | 编译时间从秒级到分钟级 |
| rustc 版本 | 本机 **1.95.0**，eframe 0.37 要求 **1.95** | 零余量，升级工具链前不能动 |
| 现有 C0 裁定 | "零新 crate、无原生窗口" | **作废**，README 要改写 |
| UI 验收 | `verify-gui.mjs` 用真浏览器驱动 | egui 无等价工具，**要另建** |
| 图标 | 项目里一枚都没有 | 托盘 + 窗口 + exe 图标都要从零做 |

### 依赖实测

用临时工程跑 `cargo generate-lockfile` 实测，不是估算：

| 组合 | 包数 |
|---|---|
| `eframe 0.37` 默认（含 wgpu）+ `tray-icon 0.21` | **493** |
| 同上但改 `glow`、关 wgpu | **361** |
| 再**加上** `egui_kittest`（`snapshot` 特性，仅 dev-dependency） | **440** |

**选 glow**：少 132 个包。wgpu 对"一个任务列表"是杀鸡用牛刀，且它在部分
老显卡/虚拟机上初始化更慢。glow 走 OpenGL，Windows 上更稳。

> 三组都是 `cargo generate-lockfile` 实测，含少量非 Windows 目标；Windows 侧实际编译数略低，
> 精确值要等真正引入后 `cargo build` 才知道。这里不把粗估写成实测。

## 四、进程形状

```
dlm-gui.exe（单进程常驻）
├─ 主线程      winit 事件循环 + egui 绘制（约 60 fps，空闲降到 10 fps）
├─ UI 层       读 Arc<Mutex<Vec<TaskView>>>；发 Ticket 给 runner
├─ HTTP 线程   只服务扩展（/api/add /api/pause /api/reveal ...）
├─ runner      队列 + 子进程（现有代码，不改）
├─ 剪贴板线程   监视 + 匹配（可关）
└─ 托盘         tray-icon 事件循环（独立线程，Windows 要求消息泵）
```

**关键决定：UI 直接读快照，不绕 HTTP 打自己。**

`snapshot_of()` 已是纯函数，runner 每轮更新
`Arc<Mutex<Vec<TaskView>>>`。UI 和 HTTP 各读一份，互不阻塞。
UI 的操作（暂停/继续/删除/打开文件）走现有 `mpsc::Sender<Ticket>`，
与 HTTP 同一条路 —— 这样**两条入口共用一套状态机**，不会出现
"UI 里暂停了但 HTTP 看还是 running"。

## 五、模块划分

新增文件，`gui.rs` 拆开：

```
src/gui/
  mod.rs         run() 编排：起线程、建窗口、跑事件循环
  snapshot.rs    TaskView / QuotaView / snapshot_of（从 gui.rs 搬）
  runner.rs      runner / apply / tick / finish_verdict（从 gui.rs 搬，逻辑不动）
  http.rs        handle_http / Request / respond（从 gui.rs 搬）
  settings.rs    Settings 结构 + 读写 + 默认值
  clipboard.rs   剪贴板监视（可关）
  app/           ← 新：原生 UI
    mod.rs       App 结构体，impl eframe::App
    list.rs      任务列表（表头 / 行 / 进度条）
    drawer.rs    详情抽屉
    dialog.rs    新建任务弹窗
    settings_ui.rs  设置面板
    theme.rs     配色、字体、间距常量
  tray.rs        托盘图标与菜单
```

**为什么拆 `gui.rs`**：它现在 1,785 行，混了队列、HTTP、UI 三件事。
加原生 UI 会到 3,000 行以上，改一处要读半天。拆分是这次的必要前置。

## 六、UI 设计（按 demo 落地）

demo：`docs/mockups/2026-11-09-native-ui.html`（三态可切）。

**布局**：标题栏 34px / 工具条 50px / 主体弹性 / 状态栏 32px；
左侧 212px 分类栏（D6 保留）；详情抽屉 372px 从右滑出。

**配色**（`theme.rs` 里的常量）：

| 语义 | 值 | 用途 |
|---|---|---|
| 纸底 | `#F6F3EC` | 窗口背景 |
| 面板 | `#FFFDF8` | 列表、抽屉 |
| 侧栏 | `#F1EDE4` | 分类栏、状态栏 |
| 描边 | `#E4DFD2` | 1px 分隔 |
| 正文 | `#2A2720` | 主文字 |
| 次文 | `#6C6860` | 副标题 |
| 弱文 | `#9C978D` | 占位、表头 |
| 强调 | `#C06340` | 主按钮、进度、选中 |
| 成功 | `#4E7A5C` | 已完成 |
| 警告 | `#A9762A` | 等待、暂停 |
| 失败 | `#A2443A` | 失败 |

**字体**：正文用系统无衬线（`Segoe UI Variable` / `PingFang SC`），
标题用衬线，数字用等宽。egui 需要显式加载字体文件 —— 挑 **思源黑体 /
Noto Sans SC** 的子集（SIL OFL，可商用），随二进制嵌入。字体体积实测后填。

**egui 做得到 / 做不到**（写清楚，免得画出兑现不了的东西）：

| 想要 | egui |
|---|---|
| 纯色、1px 边框、圆角 | ✅ |
| 等宽数字对齐 | ✅（`FontId::monospace` + 右对齐） |
| 自定义字体 | ✅（`FontDefinitions`） |
| 进度条 / 圆环 | ✅（自绘 `Painter`） |
| 滑出动画 | ✅（`ctx.animate_bool`） |
| 毛玻璃 / 模糊 | ❌ |
| 复杂投影 | ⚠️ 只有单层 |
| 原生菜单栏 | ❌（自绘，或用 `muda` 另加 crate） |

## 七、托盘与关闭语义

**关闭 = 收进托盘，不退出**（P1）。首次收进时提示一次，可在设置里关掉提示。

托盘菜单：
```
显示窗口
─────
全部暂停
新建任务
─────
退出
```
双击托盘图标 = 显示窗口。

**退出路径只有一条**：托盘菜单的「退出」→ 停 runner → kill 子进程 → 写队列 → 退出。
`Alt+F4` 与标题栏 × 都是"收进托盘"。

**`--autostart`** = 纯托盘，不弹窗（P3）。开机时没人看屏幕，只保证队列在跑。

## 八、设置面板

存 `%LOCALAPPDATA%\dlm\settings.json`，与 `queue.json` 平级。
格式：`serde_json`，字段全部带 `#[serde(default)]`，缺字段用默认值（老文件能读）。

| 分组 | 项 | 字段 | 默认 |
|---|---|---|---|
| 下载 | 默认连接数 | `connections` | 8 |
| 下载 | 同一服务器最大连接数 | `per_host_connections` | 8 |
| 下载 | 默认保存目录 | `out_dir` | `Downloads` |
| 下载 | 全局限速（KiB/s） | `max_speed` | 0（不限） |
| 行为 | **监视剪贴板** | `watch_clipboard` | **false** |
| 行为 | 完成后 | `on_complete` | `None` |
| 行为 | 开机自启 | `autostart` | false |
| 界面 | 关闭时收进托盘 | `close_to_tray` | true |
| 界面 | 收进托盘时提示 | `tray_hint` | true |

**剪贴板默认关是我按隐私定的**（IDM 默认开）。理由：剪贴板里有密码、验证码、
私钥，一个默认开着的程序持续读剪贴板，用户通常不知道。开启时给一句说明，
讲清只匹配"看起来像下载链接"的文本。**这条你还没表态，批准时可以否。**

## 九、三项能力

### 9.1 剪贴板监视（可关）

- 独立线程，`SetClipboardViewer` 或 100ms 轮询（轮询更简单，CPU 可忽略）
- 匹配：`^https?://` + 扩展名白名单（复用 `content.js` 那份 `FILE_LINK_RE`）
- 命中后：托盘气泡提示"发现链接，要下载吗"，**不自动开始**
- 相同 URL 60 秒内不重复提示
- 关闭时线程直接不启动，**不占任何资源**

### 9.2 按 host 限连接数

引擎层。现在 `--connections` 是全局的，一个任务 8 条连接全打同一 host。
IDM 的 `226 = 同一服务器的最大连接数` 说的就是这个。

实现：`scheduler.rs` 派活时，检查该段 URL 的 host 当前在途连接数，
超过 `per_host_connections` 就不派，等有连接空闲。
纯状态机改动，可单测（给调度器喂 host 标签）。

⚠ 这会改 `scheduler.rs` 的派发规则 —— 那是**有 322 条单测保护的核心**。
必须新增用例而不是改现有断言。

### 9.3 完成后休眠

`actions.rs` 加 `CompletionAction::Hibernate`，与 `Shutdown` 同形：

```
rundll32.exe powrprof.dll,SetSuspendState 0,1,0
```

⚠ 这条命令在不同 Windows 版本行为不一致（可能变成睡眠而非休眠）。
设计上只承诺"尝试休眠"，失败不报错。要精确控制得改电源策略，不做。

## 十、验收策略（关键，现有手段会失效）

`verify-gui.mjs` 作废。替代方案分三层：

| 层 | 手段 | 覆盖 |
|---|---|---|
| 逻辑 | 现有 322 条单测 + 新增 | 引擎、队列、设置读写、剪贴板匹配、host 限流 |
| 集成 | 真起 `dlm-gui`，走 HTTP 打 | 队列端到端、扩展投递、reveal、自启、单实例 |
| UI | **快照测试**：`egui_kittest` 或自建 | 三态渲染 + 交互 |

**UI 快照**：`egui_kittest 0.37.0` 确实存在（与 eframe 同版本线，MIT OR Apache-2.0，
仓库在 emilk/egui 下），`snapshot` 特性提供 `dify` 像素比对。要点：

- 它是**测试期依赖**（`[dev-dependencies]`），不进发布二进制
- `snapshot` 特性会拉 `dify` / `image` / `tempfile`，属额外测试依赖
- 它通过 AccessKit 找控件，所以 UI 代码里要给关键控件设 `accessibility_label`，
  否则测试只能做整图比对、没法点具体按钮

**替代方案**（若 kittest 不可用）：
这是唯一能替代浏览器驱动的东西。**但它本身是又一个依赖**，要实测可行性。
若不可用，退到"人工看截图 + 逻辑层全覆盖"，并明确写进文档这是已知缺口。

## 十一、实施顺序

| 步 | 内容 | 依赖 |
|---|---|---|
| 1 | 拆 `gui.rs` 为子模块（**纯搬家，不改逻辑**，322 单测必须全绿） | 无 |
| 2 | 引入 eframe(glow) + tray-icon，最小窗口能开 | 1 |
| 3 | 列表 + 抽屉 + 弹窗（读快照，发 Ticket） | 2 |
| 4 | 托盘 + 关闭语义 + `--autostart` 纯托盘 | 3 |
| 5 | 设置面板 + `settings.json` | 3 |
| 6 | 剪贴板监视（可关） | 5 |
| 7 | 按 host 限连接（引擎层，独立） | 无（可并行） |
| 8 | 完成后休眠 | 无 |
| 9 | 图标（托盘/窗口/exe）、安装器更新 | 4 |
| 10 | 删 `ui.html`、改写 README C0 节、UI 验收落地 | 全部 |

**第 1 步是关键**：先纯搬家、跑绿，再动 UI。否则一旦出问题分不清是搬家错还是 UI 错。

## 十二、留给实现者的

- 字体子集怎么裁（只保留常用汉字还是全量）—— 影响二进制体积
- `egui_kittest` 是否真能用（要实测）
- 托盘图标用什么图形（需要设计，我建议一个简单的下载箭头 + 陶土橙）
- `tray-icon` 在 Windows 上是否需要独立消息泵线程（Windows 要求托盘宿主有消息循环，
  而 eframe 的主线程被 winit 占着）—— 实现时必须先验证，这条写进第 4 步的验收
- 列表超过 1,000 行时的虚拟滚动（egui 的 `ScrollArea::show_rows` 够用）

## 十三、批准记录

待填。
