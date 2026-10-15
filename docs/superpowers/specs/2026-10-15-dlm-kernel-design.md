# dlm 内核设计（v1）

日期 2026-10-15 · 状态：待用户复核 · 上游依据 `projects/tool/idm-recon/docs/50-idm-capability-spec.md`

一句话：一个 Rust 写的多连接分段下载内核 + CLI，把 IDM 已实测确认的分段机制实现出来，
并把"分段调度"这一层做成**不依赖网络与磁盘的纯状态机**，使其可被逐条钉死测试。

v1 只做内核与 CLI。不做守护进程、不做浏览器扩展、不做 GUI、不做站点抓取。

---

## 1. 已定前提

| 项 | 取值 | 理由 |
|---|---|---|
| 语言/运行时 | Rust（本机 rustc/cargo 1.95） | 定位为日常真用的工具，要单文件 exe、无运行时依赖 |
| I/O 与并发 | `reqwest::blocking` + `std::thread`，每段一条阻塞连接 | 与 IDM 实测同构（阻塞套接字 + 多线程）；并发上限只有个位数，异步买不到东西，反而拉低调度器的可读性 |
| v1 边界 | 内核 + CLI | 守护进程与扩展是独立子系统，同轮开工 v1 会膨胀到难验收 |
| 任务模型 | 照 IDM 类型库那份 9 项契约建模 | 不带 referer/cookie/POST/认证就下不了需要登录的站点，接口后补要动内核 |
| 项目名/路径 | `projects\tool\dlm\`，二进制 `dlm.exe` | 占位，随时可改 |

## 2. 模块划分

每个单元一个职责，跨单元只走下列接口，不共享可变状态（除 `Mutex<Scheduler>` 这一处）。

| 单元 | 职责 | 对外接口 | 允许依赖 |
|---|---|---|---|
| `cli` | 解析参数，产出 `Task`；斜杠别名在此层展开 | `cli::parse() -> Task` | clap |
| `task` | 9 项契约的数据体与合法性校验 | `Task`（`url, referer, cookies, data, user, password, dir, name, flags`） | 无 |
| `probe` | 无 Range 试探；读满预读量后掐断，保留已收字节 | `probe(&Task) -> ProbeResult` | reqwest |
| `scheduler` | **段表与劈分决策，纯状态机，零 I/O** | `on(event) -> [Action]` | 无 |
| `fetch` | 领一个 `Action`，发 `Range: start-`，流式交给 `store`，回报事件 | `fetch_one(action, &Task, &Sink) -> Event` | reqwest |
| `store` | 按偏移落盘；唯一持有文件句柄 | `write_at(offset, &[u8])` | std |
| `state` | 断点文件读写，原子 tmp+rename | `save(&Snapshot)` / `load() -> Option<Snapshot>` | serde |
| `resume` | 决定续传还是重来 | `decide(Snapshot, ProbeResult) -> ResumeKind` | 无 |
| `report` | 终端一行进度 | `tick(&Snapshot)` | 无 |

模块图：`cli → task → probe → resume → scheduler ⇄ fetch → store`，`state` 由主循环两侧读写，`report` 只读快照。

### 2.1 scheduler 的输入输出（核心）

事件（入）：`ConnectionAvailable`（空闲连接数 +1）、`SegmentProgress{seg, bytes}`、
`SegmentFinished{seg}`、`SegmentStalled{seg}`、`ServerSaysNoRanges`。

动作（出）：`Open{seg_id, start, end: Option<END>}`、`Retighten{seg_id, new_end}`、`Nothing`。

**它是可序列化的纯逻辑**：给定同样的事件序列，必须产出同样的动作序列。这是"行为可对照 IDM"的前提。

### 2.2 两条一致性约束（写死在设计里，避免实现时留坑）

- 带 `data`（POST 体）时**禁用分段**，段表退化为单段顺序下载。POST 语义下 Range 不成立。
- 无 `Accept-Ranges` 或首请求返回 `200` 而非预期的 `206` 时，段表同样退化为单段，且**不重试分段**。

## 3. 常量与来源强度

| 常量 | 默认值 | 来源强度 |
|---|---|---|
| `PROBE_READ_BYTES` | 236032 | **观测**：IDM 4 次独立运行落在 236030–236040，与文件总长无关。来源未证明（不是 64 KiB 整数倍），环回链路所测 |
| `MIN_SEGMENT_BYTES` | 16384 | **观测**：IDM 三次未限速运行分别停在 15913/14588/14028。"尺寸触底"与"最大连接数用完"两约束未分离 |
| `MAX_CONNECTIONS` | 8 | **观测**：环回实测 1 试探 + 7 分段。IDM 官方手册反而建议别超过 4 |
| `STALL_TIMEOUT` | 15 s 无数据即判卡 | **自定** |
| `STATE_SAVE_MIN_INTERVAL` | 1 s（事件驱动 + 每 5 s 兜底） | **自定**，IDM 说法是"每分钟多次" |

三个观测值都可用命令行覆盖。覆盖掉它们不影响正确性，只影响"像不像 IDM"。

**明确不继承 IDM 的两点**：① 不预分配文件全长，靠写入自然延长（中断时不留空洞，IDM 的全长行为不是正确性要求）；
② 续传请求带 `If-Range: <etag>`（IDM 实测没发，这是它的短板，会让"文件已变但仍按旧段续"变成静默错误）。

## 4. 断点文件

`<目标文件>.dlm.json`，与目标文件同目录，完成后删除（`--keep-state` 可留）。原子写：`*.tmp` → rename。
**写者唯一**：只有主循环线程写断点文件；`fetch` 线程只发事件，不直接碰 `state`（避免多把账）。
`--no-resume` 的语义＝无视已存在的断点文件、从 0 开始并覆盖它。

```json
{ "v": 1,
  "task": { "url": "...", "referer": null, "cookies": null, "data_file": null,
            "user": null, "dir": "...", "name": "...", "flags": 0 },
  "validator": { "etag": "...", "last_modified": "...", "total_len": 50331648,
                 "final_url": "...", "ignore_mtime": false },
  "segments": [ { "start": 0, "end": 236032, "written": 236032, "state": "done" } ],
  "stats": { "bytes_delivered": 0, "bytes_duplicated": 0, "retries": 0 },
  "needs_secret": false,
  "saved_at": "..." }
```

**安全边界（v1 硬规定）**：断点文件**不写口令**。带 `password` 的任务落盘时只记 `needs_secret: true`，
续传时必须重新给口令，否则拒绝续传并说明原因。IDM 用 `EncPassword` 位移混淆存了它——那是可还原等于没存，不复刻。
口令只走 HTTPS；检测到 `http://` + `password` 直接报错退出，不降级发送。

## 5. CLI 形状

POSIX 为主，IDM 斜杠风格作为别名在同一层展开（目的是让现成调用 IDM 的脚本不改就能接上）。

```
dlm --url U [--out-dir D] [--out-name N] [--connections N] [--referer R] [--cookie C]
    [--data-file F] [--user U] [--password P] [--ignore-mtime] [--no-resume]
    [--sha256 HEX] [--max-retries N] [--keep-state] [--quiet]
dlm /d U [/p D] [/f N] [/n]        # 别名，等价
```

`flags` 位义（**自定义，不兼容 IDM**，因为 IDM 的 `lFlags` 语义未取证）：bit0 = 只入队不下（等价 `--no-start`，v1 即"打印任务摘要后退出"）、
bit1 = 忽略 Last-Modified、bit2 = 完成后保留断点文件。

## 6. 错误处理与重试

1. **段级**：请求失败/读超时/连接被对端关闭 → 该段退回调度器；快速退避 1/2/4/8/16 s，最多 5 次。
2. **慢重试**：超过 5 次不判整任务失败，转入 **30 s 无限慢重试**（对齐 IDM 队列行为 A15），
   并在进度行显示 `slow-retry xN`；`--max-retries` 可封顶。
3. **续传冲突**：服务器 `ETag`/`Last-Modified` 与断点不符 → 默认**从头重来并告知原因**；
   `--ignore-mtime` 时仅 Last-Modified 变化可续（ETag 变了仍重来）。
4. **416 Range Not Satisfiable** → 该段直接标完成，不重试。
5. **重定向**：跟随（上限 10 跳），并把**最终 URL** 记进 `validator.final_url`，续传按它比对。
6. **Ctrl-C**：收到信号 → 停发新请求、等在途写盘落地、保存断点、非零码退出。不硬杀。
7. **完成后校验**：`--sha256` 给了就算，不匹配则保留文件与断点并报错退出（不自动删，让用户自己决定）。
   没给则只校验"已写字节数 == total_len"。

## 7. 测试策略

三层，前两层是门禁，第三层只是参考。

1. **scheduler 单测（表驱动，无网络无磁盘）**：给定事件序列断言动作序列。必测用例：
   空表首段 → 中点劈分序列 → 老段 `Retighten` 收紧终点 → 段长低于 `MIN_SEGMENT_BYTES` 不再劈 →
   `ServerSaysNoRanges` 后任何 `ConnectionAvailable` 都不产生 `Open` → 416 后该段标完成。
2. **集成测试（真发 HTTP，对端是自己人）**：把 `idm-recon/tmp/probe-server.mjs` 复制进
   `tests/harness/`，起 `127.0.0.1`，跑 4 个场景：48 MiB 全速、48 MiB 限速、`/flaky`（中途断）、
   无 `Accept-Ranges` 退化。断言：落盘文件 `sha256` 等于基准（blob 是确定性的）；
   **把这次运行里真实发生的事件序列（连接空闲、段进度、段完成/卡死）录下来，回放给 scheduler，
   断言其动作序列与实际发出的 Range 一致**——不能直接拿"预测序列"对比，因为劈分时机取决于真实时序；
   `/flaky` 场景断言已完成段不重下。
3. **IDM 行为对照（参考，不作门禁）**：同一台服务器同一文件，让本机 IDM 用 `/n /d` 下一遍，
   把两边的 Range 序列并排打印。标注：本机 IDM 构件被第三方改过、且环回链路的观测强度有限，
   所以这条只用来发现"明显不像"，不当验收条件。

成功判据（v1 交付定义）：1、2 两层全绿，且
① 48 MiB 落盘字节级正确；② Range 序列与调度器预测一致；③ 掐断后能续且不重下已完成段；
④ 无 Range 服务器正确退化为单连接。性能不设指标（并发个位数，瓶颈在网络）。

## 8. 明确不做

内核/驱动拦截、浏览器扩展与 Native Messaging、守护进程与本地监听、站点抓取与镜像、
限速配额、GUI、完成后动作链（杀软/关机等）。这些在 `50` 文档里是 P1/P2，各自要单独立项。

## 9. 依赖

`clap`（derive）、`reqwest`（blocking + rustls-tls + webpki-roots，不依赖系统证书栈）、
`serde` + `serde_json`、`anyhow`、`sha2`。edition 2024。除此之外不加依赖。
