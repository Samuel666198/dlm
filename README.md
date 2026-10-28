# dlm

多连接分段下载器**内核 + CLI**。把 IDM 已实测确认的分段机制（开放终点请求、按最大剩余段中点劈分、
窗口化子请求、丢弃重取）自己实现一遍，其中"分段调度"这一层是**不依赖网络与磁盘的纯状态机**，可逐条钉死测试。

v1 是内核与命令行；v2 已加 B1/B2/B10 浏览器投递（无 Cookie 的公开直链可进队列）与 C1–C3 本地 Web UI
（`dlm-gui`：任务队列、全局固定窗口配额、完成后动作、环回 HTTP + 内嵌单页）。不做原生 GUI、Windows 服务、守护进程、站点抓取。

```
cargo build                     # 工具链由 rust-toolchain.toml 钉为 msvc
target/debug/dlm.exe --url https://example.com/big.iso --out-dir D:/dl
```

依赖 6 个 crate（`clap` derive、`reqwest` blocking+rustls、`serde`、`serde_json`、`anyhow`、`sha2`），
`std::thread` 并发、每段一条阻塞连接、无 tokio。

## 退出码

| 码 | 含义 |
|---|---|
| `0` | 下载完成；`--help`/`--version`；`--no-start`（只登记、不发请求） |
| `1` | 跑到一个结论但**没下完**（退避到顶、校验不符、对端把体掐断、整份那一枪没拿全） |
| `2` | 参数档：本 CLI 的边界形状校验失败，**且** clap 自己的用法错误（同码不同源，措辞不同：前者是"参数写错了"，后者由 clap 交回） |
| `3` | `--max-bytes` 到量且没下完：断点保留，再跑一次会继续；任务在预算内下完仍退 0 |

## 选项

POSIX 长选项为主，IDM 的斜杠别名在**进解析器之前就地替换成同一条长选项**（于是必填性、值类型、
报错文案两种写法完全一致），目的是让现成调 IDM 的脚本不改就能接上。

| 长选项 | 别名 | 说明 |
|---|---|---|
| `--url <URL>` | `/d` | 必填 |
| `--out-dir <DIR>` | `/p` | 默认当前目录 |
| `--out-name <NAME>` | `/f` | 不给则取 URL 末段 |
| `--connections <N>` | — | 默认 8 |
| `--max-speed <BYTES/S>` | — | 所有连接**合计**的速率上限（字节/秒；0 = 不限） |
| `--max-bytes <BYTES>` | — | 本次运行最多新交付的字节数（0 = 不限；到量即停并保留断点；与 `--data-file` 互斥） |
| `--referer <URL>` / `--cookie <C>` | — | 原样带上 |
| `--cookie-stdin` | — | 从 stdin 读 Cookie 头（与 `--cookie` 互斥；避免出现在命令行/进程列表） |
| `--data-file <FILE>` | — | 请求体从文件读，见下面"带请求体那条路" |
| `--user <U>` / `--password <P>` | — | `Authorization: Basic`；**只在 https 上发** |
| `--ignore-mtime` | — | 仅 `Last-Modified` 变了可续；`ETag` 变了仍从头重来 |
| `--no-resume` | — | 无视断点，从头下 |
| `--sha256 <HEX>` | — | 64 个十六进制字符；不匹配则**保留文件与断点**并报错退出，不自动删 |
| `--max-retries <N>` | — | 给"无限慢重试"封顶 |
| `--keep-state` | — | 完成后保留断点文件 |
| `--quiet` | `/n` | 关掉逐拍进度行；**最终摘要与错误照打** |
| `--no-start` | `/a` | 打印任务摘要后退出 0，一个字节都不许多打 |
| `--on-complete <ACTION>` | — | 完成后动作：`open`、`run:<exe>`、`shutdown:<秒数>`（至少 60）；失败只警告、不改退出码 |
| — | `/q` `/s` `/onboot` | 接受并忽略（v1 没有对应子系统） |

`flags` 位义是**自定义的**（IDM 的 `lFlags` 语义未取证）：bit0 只入队不下、bit1 忽略 Last-Modified、
bit2 完成后保留断点。摘要按**位名**列举，掩码数字不印；本版不认识的位若被置上，那一格会明说"另有本版不认识的位"，
不静默丢掉。

## 进度行

```
0.4%  0.2 MiB/48.0 MiB  0.0 MiB/s  x0/8  slow-retry x1
```

`\r` 原地覆盖的一拍，字段固定为：百分比（整数千分位**向下截断**，不四舍五入）、已覆盖/总长（单位只有 MiB 一档、
一位小数、不足 0.1 印 `0.0`）、速率、`x{活跃连接}/{连接总数}`。后两格**互不绑定**：慢重试期间活跃数合法地为 0，
此时 `x0/8` 说的是"8 条里此刻 0 条在途"。`dup X%`（本次运行内归属之外**可能**被重下的比例）与 `slow-retry xN`
在 0 时**整格不出现**。总长为 0（真·空文件）时定死印 `100.0%`。

收尾那一行给结论、落点、跨断点文件的账（"这个断点文件从建立起一共重试 N 次，归属之外可能被重下 X B"——
那是**跨轮累计**，不是本次运行数，且强度只到"可能"）、从头重来的原因（若有）、说明（若有）。

## 凭证与边界

- **口令只走 HTTPS**；`http://` 配 `--password` **直接失败**，不降级发送。`--user` 单独给也一样：明文上不发凭据。
- 断点文件里**永远没有口令字段**（连 `[redacted]` 这种占位也没有——那个字段根本不存在）。
  这条靠两层保证：结构上 `TaskView` 是 `Task` 九个字段**去掉 `password`** 之后的那个视图（少一个字段 ⇒ 任何后来新加的写盘路径都带不出去）；
  内容上，服务器交回的**三个自由文本字段**——落地 URL（**没有发生重定向时就是你写的那条 `--url` 本身**，见下面 ③）、
  `ETag`、`Last-Modified`——里若出现**本任务 `--password`** 的任一形状，
  这份事实会被当场拒（拒绝消息既不回显那条地址、也不回显那两个值）。
  后者存在的理由：那三个字段都是**服务器可控的自由文本**，会写进断点文件、并在续传冲突的原因里打回终端——
  "结构里没有这个字段"挡不住别人把口令写进自由文本里。
  看得见的是**两组**形状（⚠ **别把形状数当覆盖面积**——下面把"永远不会是唯一决定者"的那几支也点出来了）：
  **第一组＝编我们手上这份**：原样、百分号编码（把待判串解码成字节再比原样口令）、以及把**我们这份**口令编成
  `std` / `url-safe` 的 base64（各带与不带 padding 共四形）。其中**真正能独立开火的只有四个写法**：
  原样那一格同时被百分号那一支覆盖（没编码时两条都命中），而带 padding 的两形恒等于去 padding 那一形
  **加尾点**（`contains(带 pad)` 为真时 `contains(去 pad)` 必为真）⇒ 形 3 与形 5 永远不是唯一决定者，
  留着它们是把"对面把带 padding 的整串写进 URL"按字面枚举清楚，不是多两维面积。
  **第二组＝解对面那一份**：把待判串里每一段连续的 base64 符号试解一次（两张字母表 × 四个起始偏移），
  解出的字节里出现 `:<口令>` 就算命中。这一组管的是**真正上过线的那份凭据** `base64("<user>:<口令>")`
  （只给 `--password` 时是 `base64(":<口令>")`）：只编我们那一份时，那份凭据对要在
  `(len(user)+1) % 3 == 0` 那一档才恰好把口令的 base64 露成子串，`aliceX`（余 1）／`a`（余 2）／
  不给用户名（余 1）这三档**旧判据全盲**——现在这三档各有格子（含一格进程级活体针）。
  ⚠ 第二组带回来的**新代价**也要照实写：它把待判串里**任意**一段 base64 符号都当候选解，所以"误杀面"
  不再只由"字面撞中"决定——一段与凭据无关的长 base64 只要解出 `:<口令>` 那三个字节就整条拒。
  挡住它的是同一道**下限**（<4 字节不比内容），而且这一刀对新支也有直接格子钉着
  （`base64("u:abc")` 在 `"ab"`／`"abc"` 两档必须放行；把下限那道早退挪到第二组**之后**，当场红）。
  ⚠ 第二组的代价是**线性**的：每一段 run × 四个相位 × 两张表，长度由对端交回的那条串决定，
  本仓库不设长度上限，也**不拿墙钟做判据**（判据不许依赖计时）⇒ 登记为已接受的代价，不是量过的结论。
  比对一律在**字节**平面做（非 ASCII 口令与它 `%E4%B8%AD` 那种形式落在同一格），而且是
  **大小写敏感的字节比对**：改成大小写不敏感会把十六进制摘要、英文单词、`%20` 样式整片误拒，
  代价远大于"对面把口令打了乱序大小写"那一族的收益 ⇒ 那一族登记为看不见。
  判据**只比口令、不比用户名**，这是有意为之（三条支撑）：用户名本来就按 spec §4 落盘
  （`task.user`，用例 `username_is_persisted_by_ruling` 钉着）⇒ 对面把它回显进落地串**不增加**新暴露；
  `--user` 单独给**不产生** `Authorization`（实测对端收到的 `authorization` 为空）；换用户名由续传的
  ①b 那一臂管（`用户名（--user）`）。
  ⚠ 边界五条，照实写、不写成"什么都看得见"：
  ① 口令**短于 4 字节**（按 UTF-8 字节数）时**两组都不比内容**，否则几乎每条落地 URL 都会被误拒——那种任务只剩"URL 里带 userinfo"与
  "URL 写法非规范"两道形状闸在管；这个下限是文档化的边界，不是藏起来的宽松。
  ② 看得见的前提是"对面（或我们自己的 HTTP 客户端在序列化时）按**标准**编码/书写我们这份凭据"；对面**任意变形**看不见
  （二次百分号编码只解一层、`+` 当空格的表单编码不解、大小写打乱、加盐、base64 之后再百分号编码、
  顺序颠倒的 `base64("<口令>:<user>")`、以及引号包着的 `"hunter2"` 那一族——第二组只认 `:` 这一结构位）。
  ③ **内容闸管你自己写进 `--url` 的那份**（旧那句"不管"在 HEAD 是假的）：落地闸读的是**最终** URL，
  而**没有发生重定向时那条串就是你写的 `--url` 本身** ⇒ 同一个口令写在自己的 path/query 里会让这一枪被**整条拒**
  （实测 `--url https://<对端>/x/hunter2 --password hunter2` 打的正是那句**不回显地址的拒绝**；
  ⚠ 别拿"退 **1**"当证据——本机无 TLS，那一枪不判据也退 1。字面 `hunter2` 那一格由单元层钉着，
  进程级那发活体针钉的是 base64 那三档）。方向是"多拒"（安全侧），
  不是放行；而 `task.url` 这个字段本身没人改写，照 spec 原样落盘。
  ④ **放行路径上 reqwest 的错误文本会整条抄那条 URL**（这是 ① 那一侧的直接后果，不是新通路）：实测
  `--url https://<对端>/x/YWJjZA --user aliceX --password hunter2` ⇒ 判据不开火 ⇒ stderr 打
  `error sending request for url (https://…/x/YWJjZA)` 整条；同一形状 `--url …/x/12 --password 12`
  （4 字节以下的口令撞在自己的 query 上）⇒ 同样不拒、同样整条打出来。这一发**不改判据**，只把它从"看不见"
  升成具名边界（针：`…_is_refused_on_the_error_branch_and_keeps_the_stage` 的反臂）。
  ⑤ **读侧也过同一份判据**：磁盘上那份断点是**历史写的**，写侧闸守不住它。所以 `resume` 打"从头重来的原因"
  之前，对断点里的落地 URL／`ETag`／`Last-Modified` 三份值各过**同一个**判据，命中就换成一句**两个值都不回显**
  的原因（结论仍是"从头重来"，任务照跑）；未命中时那三条原因逐字照打——诊断一口都不许吃掉。
- **重定向换了 host，凭据头不带进第二跳**：这一条由对面 HTTP 客户端在跨 host／跨端口／跨 scheme 时剥掉
  `Authorization`、`Cookie` 等敏感头实现，本项目把它钉成**进程级两臂针**（`tests/integration.rs` 的
  `a_cross_host_redirect_drops_the_cookie_header_while_a_same_host_one_keeps_it`：跨 host 臂断第二跳收不到 `Cookie`，
  **同 host 臂断它必须收到** —— 后一臂防的是"把针写成任何重定向都不许带凭据"那种恒剥装饰）。
  ⚠ 射程照实写：针实测得到的是 `Cookie` 那一支；`Authorization` 走不到这只共享函数（明文 http 配口令被我们自己的
  校验闸先拒，本机对端不做 TLS）⇒「https 被降级到 http 时凭据不上明文线」那一格**本机端到端造不出夹具**，
  今天的真值是**机制实测 ＋ 对面源码**两半。这条与那张十条 `Location` 表**同一族版本依赖**：只在
  `reqwest 0.13.5 / url 2.5.8 / tower-http 0.6.11 / http 1.5.0` 上成立，升级对面任一版本必须两发一起复量。
  本项目**不**为此自建重定向策略（要防的那一面实测为零，而替掉默认策略的代价覆盖所有请求）。
- 落点不许越出目录：无论名字是你显式给的 `--out-name`，还是不给时由 URL 末段推导，**最终生效的那个名字**都要过闸（⇒ 越出去就退出码 **2**）。
  闸现在是**两支并联**的判据，谁也替代不了谁：一支查"越目录"——按 `/` 与 `\` 切段后出现 `.` 或 `..` 段就拒；
  另一支按 `Path::join` 的真语义判落点——名字与 `--out-dir` 拼出来的结果不再位于 `--out-dir` 底下就拒。
  后者一次夹住这些形状：绝对路径、盘符与盘相对写法（`C:x`、`C:\…`，以及 `u:p@x` 这种「字母＋冒号」打头的末段——
  `join` 把它们当成**新路径**而不是文件名，整份丢掉 `--out-dir` 那半边），和 `\` 打头换根的写法（Windows 语义下挂到当前盘根）。
  它管不到 `.`／`..`（`E:\o\..` 仍然以 `E:\o` 开头），所以前一支必须留着：两支是并联，不是替换。
  | 名字来源 | 拒 `.` / `..` 段 | 拒"拼出去离开目录"的形状 |
  |---|---|---|
  | 显式 `--out-name` | 拒 | 拒（全判据与 `join` 语义并联） |
  | URL 末段推导 | **拒**（背板对它盲，见上） | **拒**（末段可以含 `\` 与冒号——旧文档那句"不可能出现/不出目录"是假话，实测 `…/C:\evil\p.bin` 的落点就是 `C:\evil\p.bin`） |
  两支今天能差出来的只有一格：`--out-dir` 本身写成盘相对不带根的 `C:x`、名字也叫 `C:x` 时推导支放行、显式支拒；
  外加显式支多管**含 `/` 的名字**（如 `C:/o/x.bin`，拼回去其实仍在目录内，但那是绝对写法，闸不猜你的意图）——那种串推导支永远收不到。
  形状闸那句 `http://[::1]:8080/u:p@x` 是**规范写法**没有变；现在落点闸会拒它——"这条 URL 写得规范"与"它的末段当文件名会把落点带出目录"两句同时为真，各管各的。
  相对子路径 `sub/x.bin` 照旧允许（它只可能来自显式 `--out-name`——推导名是末段，不含 `/`，但**可以**含 `\`，所以背板对它同样生效）。**不做 sanitize**：宁可当场拒绝，也不静默改写你的落点。
- URL 里带 userinfo（`http://user:pass@host/`）一律拒绝。
- **本 CLI 只接受 `http://` 或 `https://` 的规范写法**：scheme 之后必须紧跟恰好 `://`，
  主机段（authority，终止符是 `/` `\` `?` `#` 之一）**非空、不以 `:` 开头**且里面不许出现 `@`。
  任何其它形状一律当场以"参数写错了"拒绝（⇒ 退出码 **2**）：`http:/host`、`http:\host`、`http:////host`
  这类分隔符变体，http/https 之外的 scheme，不写 scheme 的串，以及空 host（`https://`、`https://:8443/a.bin`）。
  为什么把默认反过来（不去逐个认坏写法）：`reqwest` 走 WHATWG 解析器，它对 special scheme 的分隔符容忍
  （`http:/host`、`http:\host`、`http:////host` 都照样解析成功），**逐形状清单追不上对面实现的接受集**；
  而判据读的形状一旦和解析出的形状分家，口令就能在明文线上线、随 `task.url` 落进断点文件
  （实测过程与两轮修复见 `docs/superpowers/plans/2026-10-15-dlm-kernel.md` 文末勘误 **E32**）。
  放行集因此是 `reqwest` 接受集的**真子集**：代价是**多拒**（`ftp://` 这类以前由对面报错，现在由我们当场拒），
  那是用户可见的行为变更；少认才有安全后果。
- 错误消息、摘要、进度行都不抄请求体的任何一截；`--data-file` 指向的文件内容不会进任何输出。
- `Cli` 与 `Task` 的手写 `Debug` 掩掉口令，是为了日志防线，不是产品文案。
- **会话 Cookie 是第二类凭据，v1 不给它划凭据那样的边界**（2026-10-18 定）：`--cookie` 与 `--referer` 原样发送、
  **不看 scheme**（明文 http 上照发，这是刻意设计），会出现在 `--no-start` 摘要与断点文件 `task.cookies` 里，
  且 `needs_secret` **只看 `password`** ⇒ 用 Cookie 认证的任务在续传时会静默复用盘上断点里那把钥匙。
  要这条变严＝一次设计变更（把 `cookies` 折进 `needs_secret`），不是 bug。
- **HTTPS 的信任锚＝你本机的证书存储**（不是随二进制分发的根，见设计稿 §10 **R15**）：
  `reqwest` 的 `rustls` 特性在这棵依赖图里落在 `rustls-platform-verifier`，`webpki-roots` 一个都没进锁。
  ⇒ 上面那条"口令只走 HTTPS"防的是**明文把口令裸奔**，它**不防**"系统存储里有一根你不想信的根"（企业 MITM CA、误装的根照样被信任）。

## 带请求体那条路（`--data-file`）

这条路**不试探、不读也不写断点文件、不可续**：整份一次请求拿完，线上只有**一条 POST**、不带 `Range`。
理由是不能拿"一条无体 GET 试探确认过的字节"去给带体的下载背书，而"试探也发 POST"意味着把同一份表单交第二次
（下单、投票、发信都是真副作用），两头都不接受。跑不完就如实报"未完成"并以非零码退出，**不重发**。

## 完成后动作（`--on-complete`）

只在下完且 `--sha256`（若给）通过后执行：

- `open`：用 `explorer.exe <落点>` 打开；
- `run:<exe>`：直接启动程序，落点作为**第一个参数**；不经 shell，不支持额外参数；
- `shutdown:<秒数>`：`shutdown.exe /s /t <N>`，**最少 60 秒**；执行前打
  `警告：将在 N 秒后关机；取消请运行 shutdown /a`。

动作失败只打 `警告：完成后动作失败：…` 到 stderr，**不改下载结果与退出码**；下载未完成时只打
`完成后动作未执行：下载未完成`。计划任务不做在前台 CLI：它移到 C1 队列（进程内等待会占住终端，
系统计划任务会写系统状态）；阻止休眠也不做。

## 浏览器 Native Messaging（B1/B2）

B1 的最小闭环：宿主 `com.dlm.host` + 一个 MV3 扩展，只做 `ping` → `pong`，不接下载。
安装/卸载/手动加载见 [`browser/README.md`](browser/README.md)。宿主 `dlm-native-host` 走 stdio，
4 字节本机序帧、1 MiB 上限；扩展固定 key/ID（B1 阶段权限只有 `nativeMessaging` + `storage`，
B2 扩到 `contextMenus` + `cookies` + `<all_urls>`）。

浏览器验收（不在 Rust 四闸内）：

```bash
node browser/test/native-ping.mjs
node browser/test/verify-native-ping.mjs --browser "C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
```

Edge 155 实测通过；Chrome 155 的 `--load-extension` 被忽略，需在 `chrome://extensions` 手动加载
`browser/extension`。

**B2**：右键链接/图片/音视频或页面右下角 `dlm ↓` 把 `url/referer/cookie` 交给宿主；宿主 spawn
一个 `dlm`（一任务一进程，不排队），Cookie 走 `--cookie-stdin`，不写命令行、不写任务日志。
B2 **没有**本地监听/令牌，也不是队列（队列与环回 HTTP 在 C 的 `dlm-gui`，见下节）。浏览器验收：

```bash
node browser/test/verify-native-download.mjs
```

页面挂钩、DNR、代理读取、媒体嗅探不在 B2。

**B10**：无 Cookie 的公开直链，在 `dlm-gui` 开着时优先交给队列（回执 `mode:"queue"` + `id`）；
带 Cookie 的下载仍走 B2 直连（队列不存 Cookie）；GUI 不可达或端点文件不存在 ⇒ 回退直连；
队列明确拒绝 ⇒ 报错、不重复下载；请求已发出但响应超时 ⇒ 报
"队列响应超时，未重复下载"（不直连，避免重复）。端点文件是
`%LOCALAPPDATA%\dlm\gui\endpoint.json`（`port/token/pid/started_at`），由 `dlm-gui` 原子写，
可用 `--endpoint` 覆盖；它含 token，和"能直接 spawn dlm"是同一信任边界。浏览器验收：

```bash
node browser/test/verify-queue-download.mjs --browser "C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
```

证据：[`docs/evidence/2026-10-27-quota-queue-b10.md`](docs/evidence/2026-10-27-quota-queue-b10.md)。

## 产品层：本地 Web UI（C0–C3，`dlm-gui`）

**C0 裁定**：本地 Web UI，零新 crate（默认依赖仍是 6 个）。没有原生窗口；浏览器打开页面即可。

```bash
cargo build --bins
target/debug/dlm-gui.exe                 # 打印 http://127.0.0.1:<port>/?t=<token>
```

| 选项 | 默认 | 说明 |
|---|---|---|
| `--port <PORT>` | `0` | `0` 选临时端口；只绑 `127.0.0.1` |
| `--queue <FILE>` | `%LOCALAPPDATA%\dlm\queue.json` | 队列持久化（临时文件 + rename） |
| `--out-dir <DIR>` | `%USERPROFILE%\Downloads` | 落盘目录 |
| `--log-dir <DIR>` | `%LOCALAPPDATA%\dlm\gui\logs` | 每个任务一份日志 |
| `--dlm <EXE>` | 与 `dlm-gui` 同目录的 `dlm.exe` | 子进程 |
| `--concurrency <N>` | `2` | 同时跑几个任务 |
| `--token <T>` | 随机 | 只用于测试；正常用打印出来的 URL |
| `--quota <BYTES>` | 沿用队列 | 每窗口全局字节上限（0 = 关闭；至少 236032，首轮预读要计入） |
| `--quota-window <SECS>` | `3600`（或队列里的值） | 固定窗口秒数；给 `--quota-window` 前必须先有额度 |
| `--endpoint <FILE>` | `%LOCALAPPDATA%\dlm\gui\endpoint.json` | B10 端点文件；写失败只警告、浏览器回退直连 |

- **队列**：`queued/running/paused/done/failed`；`start_at`（Unix 秒）到点才启动；重启时 `running` 改回 `paused`，不假装进程还在。
- **配额**：全局固定窗口，所有任务共享一份额度；按断点覆盖量增量记账（新建任务首轮预读计入，续传预读不计，`Retighten` 重复流量不计）。到量任务退 3、回队列并显示"配额等待至 …"，窗口重置后自动续传。`/api/quota` 给 `{enabled,limit,window_secs,window_started_at,used,resets_at}`。超出上界最多并发数 × 64 KiB。
- **完成后动作**：队列只接 `open` 与 `run:<exe>`（与 CLI 同形，由子进程执行）；`shutdown` 在 API 层拒绝——队列可能同时完成多个任务，关机副作用不可控。
- **暂停/恢复/删除**：暂停 = kill 子进程（断点保留），恢复 = 重新入队并续传；删除保留目标文件，删任务与日志（日志跨轮 append）。
- **一任务一进程**：每个任务 spawn 一个 `dlm --quiet`，stdout/stderr 进该任务日志。队列**不存 Cookie**，认证/带 Cookie 的下载继续走 B2。
- **安全**：只绑 `127.0.0.1`；所有页面/API 都要求 `?t=<token>`；UI 用 `textContent` 插入数据；日志 API 只读 log 目录内文件、最多 64 KiB 尾。
- **进程活着 = 调度器活着**：没有 Windows 服务/开机自启/托盘；`start_at` 与配额等待都要让 `dlm-gui` 一直开着。
- **不做**：原生 GUI、分类规则、更新检查（要访问外网，需单独点头）、队列 Cookie。

验收（不在 Rust 四闸内，需真浏览器）：

```bash
cargo test --test integration              # 含 gui_queue_adds_and_finishes_a_download
node browser/test/verify-gui.mjs            # Chrome/Edge，添加→done→截图
```

证据：[`docs/evidence/2026-10-22-c3-gui.md`](docs/evidence/2026-10-22-c3-gui.md)（真浏览器截图与日志）。

## v1 已知限制（都不是"偷偷没做"，做过裁定并写进勘误）

| 限制 | 出处 |
|---|---|
| 不处理 Ctrl-C 信号。承诺是"**任意时刻被杀都能续**"（实测成立），不含"优雅收尾、非零码退出" | spec §10 R11 |
| 续传窗口带 `If-Range`（R17）：强 ETag 优先，`Last-Modified` 只在未开 `--ignore-mtime` 时用；对端回 `200` 判验证器变了、一个字节不写 | spec §10 R17 |
| 速率上限低于约 35 KiB/s 时，一个 4 MiB 窗口会被 120 s 预算切开；`Windowed` 以 `Advanced` 继续，退化 `ToEof` 可能在跳前缀时被判 `Stalled` | spec §10 R18 |
| **HTTPS 证书信的是本机证书存储**，不是随二进制分发的根（`webpki-roots` 没进锁）；设计稿 §9 旧句"不依赖系统证书栈"是假的 | spec §10 R15 |
| 416 不是无条件"标完成"：只有 `Content-Range: bytes */<len>` 里的 `len` 等于我们手里的总长才认 | spec §10 R10 |
| 断点保存的节奏**等于一枪收尾的节奏**（那句"每 5 秒兜底"是合取条件，从不单独生效）⇒ 开跑十几秒内被硬杀，断点里只有初始那一格种子 | spec §10 R14 |
| v2 起每次 `Retighten` 的重复 ≤ 一块（64 KiB）：48 MiB、8 worker、全速实测 `duplicated` **319,799 B（0.63%）/ 32 次收紧**，上界 2,097,152 B；限速/续传档未复量，旧 12.8%–20.8% 只作 v1 历史 | spec §10 R21 / 计划 E14、E31 |
| `--max-bytes` 按本次运行的新交付字节计：新建任务首轮预读计入，续传预读不计；到量最多再落一块（64 KiB），并发 N 时上界 N×64 KiB | spec §2.2/R24 |
| 队列配额按断点覆盖量增量采样（500 ms 一拍）：暂停/崩溃前最后一拍之后在途、尚未写进断点的字节不计入本窗，续传时重新计入；系统时钟回拨会延后固定窗口 | spec §2.1/§2.7 |
| 队列配额与 `--max-speed` 是两件事；队列暂不透传限速 | spec §2.7 |
| B10 端点文件含 token，位于用户目录；同用户进程可读，与"能直接 spawn dlm"同一信任边界；GUI 被硬杀会留下陈旧端点文件，宿主按"连不上 ⇒ 回退"容忍 | spec §4.5 |
| 重跳上限靠 HTTP 客户端默认继承，本 CLI 未显式设置 | — |
| `\r` 覆盖不擦掉上一发的行尾；终端窄于进度行会折行 | 计划 E26/C6 |
| 耗时只印 `ms` 一档，不做自适应 | 计划 E26/C7 |

不做且不在 v1 计划：内核/驱动拦截、浏览器页面挂钩/媒体嗅探/站点抓取（B1/B2 已有直接 URL 投递，B3 未做）、
原生 GUI/Windows 服务/开机自启、常驻守护进程、配额式限速（到量暂停/择时恢复）、分类规则/更新检查、
动作链/系统计划任务/阻止休眠（`open`／`run`／`shutdown` 已做，队列的 `start_at` 已做）。
C 的 `dlm-gui` 是本地环回 HTTP，不是常驻服务。

## 测试与验收

四道门禁，全绿才算交付（**数字随发漂移，这里的值是收口那一发亲跑的实测数**，跑一遍看的是"0 failed"而不是这个数）：

```
cargo test --lib                            # 294 通过（含队列/配额/UI 纯函数）
cargo test --bins                           # 4 通过（装配层的退出档只有这一层看得见）
cargo test --test integration               # 113 通过（真发 HTTP，对端是自己写的 node 服务器；含 dlm-gui 队列/配额/宿主投递端到端）
cargo clippy --all-targets -- -D warnings    # 退出码 0
```

`tests/harness/probe-server.mjs` 是自己人的对端，能演：诚实头 + 掐断 body（`/drop/<n>`）、限速（`/slow`）、
不认 `Range`（`/noranges`）、只服务试探那一枪（`/probe-only`，用来观测慢重试）。
spec §7 的四场景（48 MiB 全速、限速劈分、掐断再续、无 Range 退化）与四条成功判据的对照记录在
[`docs/evidence/2026-10-18-spec7-四场景验收.md`](docs/evidence/2026-10-18-spec7-四场景验收.md)。

## 文档地图

| 位置 | 是什么 |
|---|---|
| [`docs/superpowers/specs/2026-10-15-dlm-kernel-design.md`](docs/superpowers/specs/2026-10-15-dlm-kernel-design.md) | 设计稿（模块划分、常量、断点结构、CLI、错误处理、测试策略）。**§10 是勘误节：实测推翻的东西在这里，正文不回改** |
| [`docs/superpowers/specs/2026-10-22-dlm-product-ui-design.md`](docs/superpowers/specs/2026-10-22-dlm-product-ui-design.md) | C0–C3 产品层设计（本地 Web UI、队列、HTTP/UI、安全边界） |
| [`docs/superpowers/plans/2026-10-22-C1-C3-product-ui.md`](docs/superpowers/plans/2026-10-22-C1-C3-product-ui.md) | C1–C3 实现计划（已交付） |
| [`docs/superpowers/specs/2026-10-27-dlm-quota-queue-b10-design.md`](docs/superpowers/specs/2026-10-27-dlm-quota-queue-b10-design.md) | A16 配额 / 队列动作 / B10 设计（已交付） |
| [`docs/superpowers/plans/2026-10-27-dlm-quota-queue-b10.md`](docs/superpowers/plans/2026-10-27-dlm-quota-queue-b10.md) | A16 / 队列动作 / B10 实现计划 |
| [`docs/evidence/2026-10-27-quota-queue-b10.md`](docs/evidence/2026-10-27-quota-queue-b10.md) | 本轮证据（真浏览器 queue/direct、四闸） |
| [`docs/superpowers/plans/2026-10-15-dlm-kernel.md`](docs/superpowers/plans/2026-10-15-dlm-kernel.md) | 实现计划（T0–T10 逐任务；执行期追加的 T11 只作为 E31 的账存在，没有 `### Task` 正文）。**文末 `## 勘误` E1–E31 是权威**，与正文/设计稿冲突时以它为准 |
| [`docs/superpowers/plans/2026-10-15-dlm-kernel-STATUS.md`](docs/superpowers/plans/2026-10-15-dlm-kernel-STATUS.md) | 执行状态：进度表、当前基线、恢复步骤、下游必守的裁定、挂账的 minor、等用户点头的事 |
| `docs/evidence/` | 一次性实测记录（数字都来自真跑） |
| `.superpowers/sdd/2026-10-15-dlm-kernel/` | 逐任务的执行报告、评审原文、变异账（被 `.gitignore` 忽略，不随仓库走） |

上游依据是 `projects/tool/idm-recon/docs/50-idm-capability-spec.md`——那份里每条机制都带黑盒取证编号，
本项目的常量与规则按它取值，取不到证的（`lFlags` 位义、`/h` 别名）宁可自己定义并注明不兼容，也不假装复刻。
