# dlm

一个 Windows 本地下载器：**多连接分段下载内核 + 命令行 + 原生桌面窗口 + 浏览器扩展**。

把大文件切成多段并行下载，支持断点续传、限速、配额、完成后动作；
浏览器里右键或点悬浮球就能把下载交给它，桌面窗口统一看进度。

```
cargo build --release --bins
target/release/dlm.exe --url https://example.com/big.iso --out-dir D:/dl
```

## 它由四部分组成

| 部分 | 可执行文件 | 做什么 |
|---|---|---|
| 下载内核 | `dlm.exe` | 分段下载引擎：多连接、断点续传、限速、配额、HLS、完成后动作。同时是完整 CLI |
| 桌面窗口 | `dlm-gui.exe` | 原生窗口（eframe/egui）+ 系统托盘 + 设置页 + 任务队列。**同时是扩展的后端** |
| 浏览器宿主 | `dlm-native-host.exe` | Native Messaging 宿主，把扩展的请求转给内核或队列 |
| 浏览器扩展 | `browser/extension/` | 右键菜单、页内悬浮球、媒体/文件嗅探、HLS 直链 |

界面分三层，别混淆：`dlm-gui` 的 HTTP 服务是**产品接口**（扩展依赖它），不是"旧 Web UI 的残留"。
HTTP 只绑 `127.0.0.1`，所有路径都要 `?t=<token>`。

## 核心能力

**下载**
- 多连接分段（默认 8，可配 1–32），按最大剩余段中点劈分
- 断点续传：任意时刻被杀都能接着下（断点文件 `<目标>.dlm.json`，原子写）
- 限速：全任务共享一个速率上限
- 配额：固定窗口内的字节预算，到量暂停、下窗续跑
- 校验：`--sha256` 完成后校验
- HLS：明文 m3u8 分片顺序下载并合并（不支持加密、DASH/DRM）
- 完成后动作：`open` / `run:<exe>` / `shutdown:<秒>`

**浏览器**
- 右键「Download with dlm」
- 页内悬浮球：投递当前页、看进度、停止/继续
- 媒体与文件嗅探：自动识别页面里的 HLS 直链、音视频、附件
- 无 Cookie 的公开直链进队列，带 Cookie 的走直连

**桌面**
- 原生窗口 + 托盘（关窗不退出，收进托盘继续跑）
- 任务队列、全局配额、分类筛选
- 设置页：连接数、每 host 上限、限速、剪贴板监视、完成后动作
- 剪贴板监视：复制下载链接会弹提示（默认关）
- 开机自启（可选）

## 退出码

| 码 | 含义 |
|---|---|
| `0` | 下载完成；`--help`/`--version`；`--no-start`（只登记、不发请求） |
| `1` | 跑到一个结论但**没下完**（退避到顶、校验不符、对端把体掐断、整份那一枪没拿全） |
| `2` | 参数档：边界形状校验失败，**且** clap 自己的用法错误 |
| `3` | `--max-bytes` 到量且没下完：断点保留，再跑一次会继续；任务在预算内下完仍退 0 |

## 常用选项

```
--url <URL>                     下载 URL（必填，仅 http/https）
--out-dir <DIR>                 输出目录（默认当前目录）
--out-name <NAME>               输出文件名（不给取 URL 末段）
--connections <N>               并发连接数（默认 8）
--max-speed <BYTES_PER_SEC>     速率上限（0 = 不限）
--max-bytes <BYTES>             本次最多新交付多少字节（到量即停并保留断点）
--cookie <COOKIE>               原样发送 Cookie
--cookie-stdin                  从 stdin 读 Cookie（避免出现在进程列表）
--referer <URL>                 原样发送 Referer
--user / --password             Basic 认证（仅 https）
--sha256 <HEX>                  完成后校验
--hls                           把 URL 当 HLS 播放列表
--on-complete <ACTION>          open / run:<exe> / shutdown:<秒>
--no-resume                     无视断点，从头下载
--keep-state                    完成后保留断点文件
--quiet                         关掉逐拍进度行
--no-start                      只打摘要，不发请求
```

斜杠别名（`/d` `/p` `/f` `/n` `/a`）在进解析器之前就地替换成对应长选项，
所以必填性、值类型、报错文案两种写法完全一致。表以外的斜杠写法会直接报错。

## 版本号（两套，各自独立）

- **内核 / CLI / GUI**：`Cargo.toml` 的 `version`，当前 **0.1.0**。`dlm --version`、`User-Agent: dlm/<版本>` 与包名都用它。
- **浏览器扩展**：`browser/extension/manifest.json` 的 `version`，当前 **0.2.11**。Service Worker 文件名带版本号（`background-<version>.js`）。

两者**不要求相等**：扩展可以独立迭代，内核也可以独立发版。

## 构建

依赖 8 个 crate：内核侧 `clap` derive、`reqwest` blocking+rustls、`serde`、`serde_json`、
`anyhow`、`sha2`；原生外壳另加 `eframe 0.37`（glow + `default_fonts` + `clipboard`）与
`tray-icon 0.21`。两者都必须 `default-features = false`，否则会拉进 GTK/macOS 依赖。

`std::thread` 并发、每段一条阻塞连接、无 tokio。工具链由 `rust-toolchain.toml` 钉为 msvc。

## 打包（Windows x64）

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File packaging/build-package.ps1
```

产物 `target/package/dlm-<version>-win-x64.zip`，内含三个 exe、`browser/extension`、
`install.ps1` / `uninstall.ps1`（用户级，不需要管理员）、`START-HERE.txt`、说明文档。

扩展仍需在浏览器开发者模式里"加载已解压"。详见 [`packaging/README.md`](packaging/README.md)。

## 测试与门禁

一条命令跑全部门禁：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\gates.ps1
```

```
cargo fmt --check                            # 退出码 0
cargo test --lib                             # 350 通过
cargo test --bins                            # 4 通过
cargo test --test integration                # 121 通过（真发 HTTP，对端是自己写的 node 服务器）
cargo clippy --all-targets -- -D warnings     # 退出码 0
node browser/test/check-extension-static.mjs  # 扩展版本/SW 文件名/两份后缀正则一致
```

**门禁数字是地板不是天花板**：只许增，不许删测试、不许改判据方向。

`tests/harness/probe-server.mjs` 是测试用的对端，能演：诚实头 + 掐断 body、限速、
不认 `Range`、只服务试探那一枪。

## 已知限制

不是"偷偷没做"，都是做过裁定并写进勘误的：

| 限制 | 说明 |
|---|---|
| 不处理 Ctrl-C | 承诺是"任意时刻被杀都能续"（实测成立），不含优雅收尾 |
| HLS 不可续传 | 失败保留已完成的半截文件，重来从头 |
| 暂停/停止会丢约 5 秒 | 断点按枪收尾保存，被硬杀时最后几秒未确认的字节要重下 |
| 队列不透传限速 | 配额与 `--max-speed` 是两件事 |
| HTTPS 用本机证书存储 | 不是随二进制分发的根 |
| 站点抓取不做 | 整站爬取是独立产品级功能 |

## 文档

**完整索引见 [`docs/INDEX.md`](docs/INDEX.md)**；接手先读 [`docs/2026-11-11-交接-HANDOVER.md`](docs/2026-11-11-交接-HANDOVER.md)。

| 位置 | 是什么 |
|---|---|
| [`docs/INDEX.md`](docs/INDEX.md) | 文档总索引：阅读顺序、编号释义、每篇文档状态 |
| [`docs/2026-11-11-交接-HANDOVER.md`](docs/2026-11-11-交接-HANDOVER.md) | 当前交接：现状、五分钟跑起来、架构坑、欠账、硬纪律 |
| [`docs/superpowers/specs/`](docs/superpowers/specs/) | 设计稿（内核、产品层、配额队列、HLS、媒体嗅探、原生外壳） |
| [`docs/superpowers/plans/`](docs/superpowers/plans/) | 实现计划（含各阶段勘误，**与正文冲突时以勘误为准**） |
| [`docs/evidence/`](docs/evidence/) | 一次性实测记录（数字都来自真跑） |
| [`browser/README.md`](browser/README.md) | 扩展结构与 Native Messaging 协议 |
| [`packaging/README.md`](packaging/README.md) | 打包与安装器 |
| [`gates.ps1`](gates.ps1) | 一条命令跑全部门禁，失败退 1 |

## 设计取向

分段调度那一层是**不依赖网络与磁盘的纯状态机**，可以逐条钉死测试 ——
这是整个项目最容易出错也最值得测的部分，所以把它与 IO 彻底分开。

宁可自己定义并注明"不兼容"，也不假装复刻别人的私有行为：
取不到证的东西（某些位义、某些别名）一律显式声明，不猜。
