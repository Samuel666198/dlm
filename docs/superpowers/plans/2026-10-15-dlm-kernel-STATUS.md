# dlm 内核 v1 — 执行状态与恢复说明

计划：[`2026-10-15-dlm-kernel.md`](./2026-10-15-dlm-kernel.md)（末尾 `## 勘误` E1–**E33** 为权威，与正文冲突时以勘误为准；**E28＝T9 评审四发的实测复核（含撤销 E26 附记那条假前提），E29＝9B-4 两发修复轮的账，E30＝T10 的落地账（含"名字像 ≠ 覆盖到"）＋ E30 附记（撤销"seq 与 conn 差一"那条我自己照抄的假结论），E31＝用户第二批裁定 ＋ T11 把 `slow-retry xN` 端到端买到，E33＝最终评审之后整个修复波（G／H／J 三族）的账——一条 Critical 被我实测否证、两条我自己下的裁定被推翻、一行号引用腐坏族**；原"预留 E18"未发出，编号已顺延，不回填空位）
设计：[`../specs/2026-10-15-dlm-kernel-design.md`](../specs/2026-10-15-dlm-kernel-design.md)（§10 勘误 R1–**R16**，R5 已作废 R4；R8–R13＝T9 评审换来的，**R14＝T10 量到的"保存粒度＝收尾粒度"那一格**）
分支 `sdd/dlm-kernel`，未 push、无远程。**2026-10-20 更新。**

## 2026-10-20 收口断点（控制员；**恢复时先读这一节；下面 2026-10-19 那节已被本节取代，保留是为了让"当时怎么判断的"可读**）

**四闸真账（控制员在 `099af0b` 亲跑，文档收口发之后复跑逐格相同）**：`cargo test --lib` **240 passed / 0 failed**（3.01 s）· `cargo test --bins` **3 / 0** · `cargo test --test integration` **97 / 0**（约 63.9 s）· `cargo clippy --all-targets -- -D warnings` **退出码 0**。
下限口径照旧：**只许增、不许删、不许改判据方向**。
提交链（源码面，`4fe6a7c` 之后）：`b1de2e1`（**H1** 落点闸两支并联）⇒ `cb18d98`（**H2** 内容判据六形＋下限＋三字段）⇒ `110bc81`（**H3** 对面剥头行为钉成进程级两臂针＋帮助面交叉针＋四面禁坐标）⇒ `bb90e02`（**J1** 第二组 base64 解出 `:<口令>` 结构位＋读侧那道闸＋措辞不再假指认）⇒ `099af0b`（**J2** `fetch` 四处实参针＋跨端口那一臂＋帮助"默认 8"交叉上实现常数）。
**恢复的第一件事**＝读本节 ＋ `git log --oneline -8` ＋ `git status --porcelain`；**不要再读 G5 那一档**（G5/G5b/G5c/G5d/G6a/G6b/H1/H2/H3/J1/J2 全部已交付，上面 10-19 那节里"恢复第一件事＝读 G5 报告"已成历史）。

**凭据面今天的不变量（用户可见，按这一节读）**：
① **装填口（写侧）**：`probe::landed_url_is_refused(url, password)` 三支合取——userinfo／规范形状／**内容**。内容那一支的形状是**两组**：第一组＝把**我们手上那份**口令编成六形（原样／百分号解码后／std 与 url-safe × 有无 padding），第二组＝**把待判串里每段连续 base64 符号用两张表 × 四个起始偏移各解一次**、认解出字节里的 `:<口令>` 结构位（真正上过线的那份 `base64("<user>:<口令>")`／`base64(":<口令>")`）。
② **下限**：口令短于 **4 字节**（UTF-8 **字节**数）两组都不比内容 ⇒ 那是**文档化边界**，且对新支也有直接格子钉着（`base64("u:abc")` 在 `"ab"`／`"abc"`／`""` 三档必须放行；把那道早退挪到第二组之后 ⇒ `--lib` 239/1，红的就是这一格）。
③ **读侧**：磁盘上那份断点是**历史写的**，写侧闸守不住它 ⇒ `resume::decide` 打"从头重来的原因"之前，对断点里的落地 URL／`ETag`／`Last-Modified` 三份值各过**同一份**判据，命中就换成两个值都不回显的措辞（**结论仍是"从头重来"**），未命中三条原因逐字照打。
④ **对面剥凭据头**那一维今天由进程级**三臂**针钉着（跨 host／同 host 带／**跨端口**），只在 reqwest 0.13.5／url 2.5.8／tower-http 0.6.11／http 1.5.0 上成立 ⇒ 与 G1e 那张十条 `Location` 表**同一族升级欠账**。
⑤ 措辞：落地/验证子那两句拒绝消息**不断言发生过重定向、不点 userinfo、两种来源（服务器交回的／你写进 `--url` 的）都摆出、一个字都不回显**；旧那句措辞现在就是它自己的负臂（六发针盯着）。

**本节取代掉的两条旧登记**（原文留在下面那节当证据，别再按它们行动）：
- 10-19 那节里"`probe::run` 里 `e.url()` 那条错误支**至今无活体夹具**" ⇒ **已作废**：载体是 `--url https://127.0.0.1:<明文桩>/x/<blob>` ＋ `--password`（scheme 允许口令 ⇒ `validate()` 放行，而本机无 TLS ⇒ 握手失败走错误支，那条带凭据形状的串只活在 `e.url()` 里）。针：`a_landing_url_carrying_the_wire_credential_pair_is_refused_on_the_error_branch_and_keeps_the_stage`（四臂＋一反臂）。仍未买到的只有**成功路径**（要活体 https ⇒ 得往本机证书存储装本地 CA，那是有主的既有裁定）。
- "内容闸不管你自己写进 `--url` 的那份" ⇒ **读反了**：管，而且整条拒（无重定向时落地串就是 `--url` 本身）。⚠ 证据是那句**不回显的消息文本**，**不是退出码**——本机无 TLS 时判据不开火也退 1（README 与 `probe.rs` 那两处措辞已按此改准）。

**行号引用这一族今天清了**（KR 的 Important 2）：`src/`、`tests/`、`README.md` 里**指向自家文件**的行号引用全部换成符号指针，共**九处**（`facts_from_response` 的落地支／`Task::validate` 里"明文 HTTP 不发送口令"那一支／`with_contract_headers`／`landed_url_is_refused` 的文档段（两处引用它的）／`report::summary` 那两格 ＋ `main` 里 `--no-start` 那一路／选项表里 `--connections` 那条 `#[arg(help = …)]`／`probe.rs` 模块文档"对端无视 `Range` ⇒ 连接被拆掉"那一条），复验口径＝`grep -rnE "(src|tests)/[a-z_]+\.rs:[0-9]+" src/ tests/ README.md` **零命中**。**对面 crate 里带版本号的行号保留**（`reqwest-0.13.5/src/redirect.rs:239-251`、`url 2.5.8 parser.rs:448-459` 那一族）——那是钉住对面版本的写法，不是会漂移的承诺。⚠ **本文件下面那些已冻结的历史小节里还有若干行号引用**（`src/probe.rs:84-85`、`src/engine.rs:996`、`tests/integration.rs:1947-1951` 等），按本项目"历史小节原文照留、不回改"的纪律**不动**；读它们时按消息文本/符号名去找，别按行号（这条正是 KR 量到的腐坏：写下时的 `probe.rs:362` 今天落在一只解码器的文档行上）。

**还剩什么（2026-10-20 用户三条已答，逐条按答案办的结果）**：
- **G4＝已做**（`2a5b6ce`）：`.gitignore` 删掉 `Cargo.lock` 那一行 ＋ 锁入库；入库前 `cargo metadata --locked` 退出码 **0** ⇒ 锁与 `Cargo.toml` 一致，锁里逐字读到那四个对面版本（R15 信任锚结论与两张实测表的共同地基从此随仓库走）。
- **`.superpowers/` 归档＝按"先搬 zip 进 `archive/`，**原件暂不删**"办**：归档件 `E:\WorkSpace\archive\2026_retired\dlm-sdd-2026-10-15-dlm-kernel.zip`（建件后按 `testzip()` 全 CRC ＋ 逐文件 md5 与盘上原件三方对账；件内带 `MANIFEST.json`）。
  原件留在 `dlm\.superpowers\sdd\2026-10-15-dlm-kernel\`，理由是你当场选的就是这一档：仓库里 **39 处**引用（README 1／`src/` 5／本文件 18／计划 8／代裁清单 6／设计稿 1）继续指得到东西。
  ⚠ 两条随之成立的注意事：**① 原件在 git 里是未跟踪的**（`.superpowers/sdd/.gitignore` 写着 `*` ⇒ 账本不入库；`git status` 之所以还显示 `?? .superpowers/`，是我自己放在 `.superpowers/tmp-rr/` 的那只一次性 node 对端，它**此刻被一个活进程占着搬不走**，我没有强搬、也没有去杀那个进程）；**② `git clean -fdx` 会连账一起清** ⇒ 恢复源＝本文件 ＋ `git log` ＋ 归档件。
- **清单＝已交**：`docs/superpowers/plans/2026-10-18-控制员代裁清单.md`（第六节是 10-19／10-20 这一波的 39 项，含**撤回我自己定的两条**）。
- **`git push`＝你说先不推** ⇒ 分支 `sdd/dlm-kernel` 继续只在本地、无远程；下一次要推仍然要你单独点头。
- **`idm-recon` 那份取证文档＝你说不动** ⇒ 那边仓库继续一个字不改；这条从"欠你一句话"改成"**已答：不办**"，dlm 这边不受影响（自己的 harness 早已修，场景 3 走 `/drop/<n>`）。

**没有别的欠账了**：这一波之后仓库侧只剩"要改产品行为/加仪器"的活，而它们全在第 4 节那批**已登记的用户可见限制**里（端到端 TLS 那一格、`T8-R5` 的残余重复流量、Ctrl-C、`--data-file` 形状）。

## 2026-10-21 追加（交接；本节仍是最上面两节的读法，这里只补两条新事实）

- **真公网第一次验收**（此前全部验收跑在本机自写的 node 对端上）：`dlm --url https://registry.npmjs.org/left-pad --connections 4` ⇒ 退 **0**、`全长＝落盘＝归属已定＝22573 B`，且与 `curl` 单独取回的同一文件**逐字节同 sha256**（`bf67de9a…f297b1`）；`--sha256` 给正确值退 0、给 64 个 0 退 **1** 且**目标文件与断点文件都保留**（README 那句"不自动删"第一次在真服务器上看到）。原始输出留在 `target/live-check/`。⚠ 同一天量到的环境事实：`cdn.jsdelivr.net` 在这台机器上被连接重置（`curl` 同报 reset ⇒ **不是 dlm 的问题**），而 dlm 那一枪打出的失败文案**整条抄了那条 URL** ⇒ 边界 ④ 第一次在真实失败场景里现身（已登记，不改判据）。
- **交接稿**＝[`./2026-10-21-下一任交接-HANDOFF.md`](./2026-10-21-下一任交接-HANDOFF.md)（接手三十秒／硬纪律十六条／已裁定不要重做／具名欠账／下一阶段三条路带前置／权威源与归档表／动手前自检）。**下一任先读那份，再读本文件的 2026-10-20 节。**
**一处同源文本我没补针**（照实登记，不是漏）：README 选项表里那格"默认 8"（`README.md` 的 `--connections` 那一行）与 `DEFAULT_CONNECTIONS`、`--help` 那句是**三处同源文本**，现在只有后两处绑成了交叉针（`number_from_help("connections", "默认") == DEFAULT_CONNECTIONS`）；README 那一格**不绑**——它是给人读的表，为它开一根"扫 README 文本"的针＝钉措辞而不是钉行为（与本节下面"已判掉的三条"里第 ② 条同一条立场）。代价：改实现默认值时 README 会漂，靠人复查。

## 2026-10-21 下一阶段（路线图与阶段 0）

- **用户裁定**：A/B/C 三条路线**全做**，按从简单到困难分阶段；顺序与每阶段前置见 [`./2026-10-21-三路路线图.md`](./2026-10-21-三路路线图.md)。交接稿 §5 的"三选一"自此只当历史分析读。
- **阶段 0（能力对账）已交付**：[`../../evidence/2026-10-21-idm能力对照表.md`](../../evidence/2026-10-21-idm能力对照表.md)。49 行状态计数：已覆盖 14／部分 3／本机可验 12／未取证 1／不做或不复刻 19。
- **阶段 0 结论**：P0 九项里 8 项有落点、P0-9 是"不复刻 IDM 落后项"；A 路线真缺口只有速率/配额（A16→A2/A3）与队列（A15/P1-15→C1）；B/C 为绿地，B 开工前先用 B0 量浏览器 Native Messaging 契约，不复刻 IDM 私有操作码/管道/端口。
- **执行方式**：按路线图逐阶段走 spec → plan → 实现；四闸为 floor；不 push。

## 2026-10-21 A1 交付（If-Range）

- **R17**：续传窗口带 `If-Range`；强 ETag 优先，Last-Modified 仅在未开 `--ignore-mtime` 时用；对端回 `200` ⇒ `ValidatorChanged`，一个字节不写；Fresh、不认 Range、`--data-file` 都不发。R9 的"不发 If-Range"半句作废。
- **落点**：`resume::if_range_value`（取值）／`engine::if_range_for`（发送面）／`fetch::run_window_with_if_range`（带头的窗口入口；原 `run_window` 为 `None` 包装）／`Plan.resume_validator`（断点验证器进引擎）。
- **新用例**：3 条集成（`a_window_request_carries_the_if_range_value_it_was_given`、`an_if_range_shot_that_gets_200_is_validator_changed_and_writes_nothing`、`a_resumed_run_fails_cleanly_when_if_range_gets_200_after_the_probe`）＋6 条单元。
- **四闸**：`--lib` **246/0**（3.01 s）· `--bins` **3/0** · `integration` **100/0**（63.94 s）· clippy **0**。
- **边界**：弱 ETag／不带引号的 ETag 不发（退 Last-Modified 或干脆不发）；对端忽略 If-Range 回 200 按"验证器变了"保守处理；`ToEof` 不发。

## 2026-10-21 A2 交付（`--max-speed`）

- **R18**：`--max-speed <BYTES_PER_SECOND>` 是单任务所有连接**合计**的速率上限；0 = 不限；探针预读、窗口请求、`--data-file` 整份请求都过同一只漏桶。
- **落点**：`src/rate.rs`（`RateLimiter`）／`probe::run_with_limiter`／`fetch::run_window_with_limits`、`run_whole_with_limiter`／`Config.max_speed` ＋ CLI `--max-speed`。
- **Windows 粒度**：低于 16 ms 的等待不睡（债务留在 `next`），避免 ~15.6 ms 的 sleep 粒度把小块拖慢；长期平均仍由 `next` 约束。
- **新用例**：6 条 `rate::tests` 单元 ＋ 2 条集成（`a_shared_rate_cap_spans_all_workers`、`the_data_file_route_obeys_the_rate_cap`）。
- **四闸**：`--lib` **252/0**（3.01 s）· `--bins` **3/0** · `integration` **102/0**（63.71 s）· clippy **0**。
- **已知边界**：低于约 35 KiB/s 时 4 MiB 窗口会被 120 s 预算切开；`Windowed` 以 `Advanced` 继续，退化 `ToEof` 可能在跳前缀时被判 `Stalled`。配额/择时恢复仍归 A3。

## 2026-10-21 A3a 交付（完成后动作）

- **R19**：`--on-complete open|run:<exe>`；只在下完且校验通过后执行；动作失败只警告、不改退出码；`!ok` 只打"未执行"。
- **落点**：`src/actions.rs`（`CompletionAction`／`ActionPlan`／`plan`／`execute`）／CLI `--on-complete`／`main` 在 `finish` 后执行。
- **新用例**：5 条 `actions::tests` 单元 ＋ 1 条 CLI 解析 ＋ 2 条二进制（成功＋动作失败仍退 0；失败下载不执行动作）。
- **四闸**：`--lib` **258/0**（3.01 s）· `--bins` **3/0** · `integration` **104/0**（64.04 s）· clippy **0**。
- **未做**：`shutdown`（参数层直接拒绝，需单独点头）、计划任务、阻止休眠、多动作链；归 A3b。

## 2026-10-21 A3b 交付（shutdown；计划任务移到 C1）

- **R20**：`--on-complete shutdown:<seconds>`（最少 60 秒）；计划 `shutdown.exe /s /t N`；执行前打
  `警告：将在 N 秒后关机；取消请运行 shutdown /a`；R19 的"shutdown 拒绝"半句作废。
- **计划任务移到 C1**：前台 CLI 进程内等待占住终端，系统计划任务写系统状态；C1 的队列项才是起始时间/时间窗的落点。
- **阻止休眠不做**：要 Win32 FFI，且不是 CLI 内核的核心。
- **新用例**：`actions::tests` 增 shutdown 解析/计划；二进制 `bare_shutdown_is_a_parameter_error_and_never_reaches_the_wire`（退 2、不上网、不关机）。
- **四闸**：`--lib` **259/0**（3.01 s）· `--bins` **3/0** · `integration` **105/0**（63.96 s）· clippy **0**。

## 2026-10-21 A4 交付（块级光标劈分）

- **R21**：`split_tail` 的切点基于 worker **每写一块**更新的光标；engine 在 `Connected` 前同步，每次 `Retighten` 的重复 ≤ 一块（64 KiB）。
- **实测**（48 MiB、8 worker、全速）：`duplicated` 9,012,699 B（17.2%）/ 54 次收紧 → **319,799 B（0.63%）/ 32 次**；上界 2,097,152 B。
- **新用例**：`residual_duplicate_traffic_is_bounded_by_one_chunk_per_retighten`；既有 midpoint/floor 判据全部保留。
- **四闸**：`--lib` **259/0**（3.01 s）· `--bins` **3/0** · `integration` **106/0**（63.87 s）· clippy **0**。
- **未复量**：限速档、续传档的 dup；旧 12.8%–20.8% 只作 v1 历史。

## 2026-10-22 B0 交付（Native Messaging 取证）

- **注册表**：Chrome HKCU/HKLM、Edge HKCU、Firefox HKCU；Chrome/Edge 共用 `IDMMsgHost.json`，Firefox 用 `IDMMsgHostMoz.json`。
- **宿主**：`IDMMsgHost.exe` 6.38.20.9；`IDMan.exe` 6.42.42.2；`IDMIntegrator64.exe` 6.41.23.1。
- **现役扩展**：Chrome **6.42.64 / MV3**、Edge **6.43.1 / MV2**；均 `connectNative("com.tonec.idm")`、1500 ms 退避、opcode 14–19。
- **本地监听**：动态 `127.0.0.1:51122` / `50627`，属主 `IDMIntegrator64.exe`；`1001` 只是扩展里的固定候选。
- **未取证**：Q-B1 操作码语义（不阻塞自建协议）；Q-B2 的端口已量到，`POST /open/1` 协议帧未取证。
- **证据**：[`../../evidence/2026-10-22-native-messaging-recon.md`](../../evidence/2026-10-22-native-messaging-recon.md)。无代码改动；四闸 **259/3/106/clippy 0**（63.92 s，复跑）。

## 今日断点（2026-10-19 控制员；**已被上面 2026-10-20 那节取代，保留是为了让"当时怎么判断的"可读**）

**四闸真账（控制员在 `d7092e7` 这棵源码树上亲跑）**：`cargo test --lib` **215 passed / 0 failed**（3.01 s）· `cargo test --bins` 3 · `cargo test --test integration` **90 / 0**（63.69 s）· `cargo clippy --all-targets -- -D warnings` 零命中。工作树干净、未 push。
提交链（源码面）：`4fe6a7c`（G1b）⇒ `d32b3e3`（G1c 反转默认）⇒ `aa1af55`（G1c-R1 三条裁定）⇒ `e2647f2` + `524eaf1`（**G1d** 四裁定）⇒ `d7092e7`（README 补真）；其间 `1344fea`/`714083a` 只动文档。

**凭据闸今天的不变量（用户可见，别再按 10-18 那节读）**：本 CLI 只接受 `http://`/`https://` 的**规范写法**——scheme 之后紧跟恰好 `://`；主机段（终止符 `/` `\` `?` `#`）**非空、不以 `:` 开头**、段内无 `@`；任何其它形状当场以"参数写错了"拒 ⇒ 退出码 **2**。
形状表现 **60 格＝18 放行 + 42 拒**（T99-H1 前是 59＝19＋40：H1 加进 I6 那一格，并把 `http://[::1]:8080/u:p@x` 从放行侧挪到落点闸拒侧——那一格的**形状裁决 `Some(false)` 一字未动**，变的只是 `validate()` 走哪一支），进程级零泄漏判据现 **16 臂**。控制员亲跑九条形状（含前导空白、C0、scheme 内插 Tab、`http:/`、`http:/\`、`http:////`）⇒ 八条退 2、`https://` 空 host 退 2（G1d 之后），本机一次性对端 **0 请求 / 0 个 `authorization`**。
⇒ **下面 10-18 那节里"README 那两句今天对畸形写法仍不成立"一句已作废**：G1c＋G1c-R1＋G1d 三发把它修掉了。README 里那句承诺也**今天才真存在**（`d7092e7`；此前 `src/task.rs` 两处代码文档声称"与 README 同一句话"而 README 一个字都没有＝假交叉引用，控制员自查）。

**落点名字闸（T99-H1，2026-10-19 收口）**：闸改成**两支并联**——`.`／`..` 切段判据 ＋ 按 `Path::join` 真语义判"拼出去还在不在目录里"的背板（绝对、盘符、`u:p@x`、`\` 打头五族，旧闸实测全退 0；背板对 `..` 盲，实测 `E:/o/..` 前缀仍是 `E:/o` ⇒ 并联不是替换）。两支宽度不同的理由段随此改写：裁决差集实测只剩 `--out-dir` 本身写成盘相对 `C:x` 配同名末段那一格与含 `/` 的输入域；形状闸对 `http://[::1]:8080/u:p@x` 的"规范写法"裁决一字未动，落点闸现在才真拦它。账见 `.superpowers/sdd/2026-10-15-dlm-kernel/t99-H1-report.md`。

**恢复的第一件事＝读 `G5` 的报告**（`.superpowers/sdd/2026-10-15-dlm-kernel/t99-G5-report.md`；简报 `t99-G5-brief.md`，BASE `1d061ac`，**G5 在飞**）。
**四闸现值（控制员亲跑于 `1d061ac`）＝`--lib` 218 / `--bins` 3 / `--test integration` 92 / clippy 命中 0**（下面那档 215/3/90 是 `d7092e7` 的账；G1f 净增三臂）。
**G1e 与 G1f 的结论（两发已回，必须一起读——它们把本节原来那条"按实测二选一"的待裁问题关掉了）**：
- **G1e（量具发，`src/` 零改动，`7312af8`+`c6f7fb5`）**实测十条服务器给的 `Location` ⇒ **一支都造不出"无 `@` 但非规范"的落地串** ⇒ 原计划"把形状检查接进 `src/fetch.rs` 那四处"＝**恒不触发的死分支，不做**。
  这张表只在 **reqwest 0.13.5 / url 2.5.8 / tower-http 0.6.11 / http 1.5.0** 上成立 ⇒ **升级对面任一版本必须复量**（已登记成欠账）。
- **T99-H3 把同一族欠账的第二根针并进来**（不另立一族）：`tests/integration.rs::a_cross_host_redirect_drops_the_cookie_header_while_a_same_host_one_keeps_it`
  钉的是对面 `redirect.rs::remove_sensitive_headers` 的覆盖面（跨 host／跨端口／跨 scheme 时剥凭据头），**同一组四个对面版本**上才成立 ⇒ 复量清单现在＝那张十条 `Location` 表 ＋ 这一发两臂。
  射程照实登记：针实测得到 `Cookie`，`Authorization` 那一支本机走不到（明文 http 配口令先被 `Task::validate()` 拒、本机对端不做 TLS）⇒"降级时凭据不上明文线"＝**机制实测 ＋ 对面源码**两半，端到端那一格照旧挂在那条 TLS 欠账上。
- 同一批测量量出一格真口子：url 2.5.8 的"special relative or authority"分支把 `http:/alice:s3cret@h/landed` 按**相对引用**解析（`@` 连同口令**折进 path**、authority 取本次 origin）⇒ 落地串形状完全规范、两道旧闸都看不见 ⇒ `probe::run` 实测 `Ok`。
  危害＝**我们自己的手把口令形状的东西写回那份号称永不含口令的断点文件**，并被 `src/resume.rs:205-209` 打进 ② 原因串回显终端；`src/state.rs:14-16` 自己写着这一格靠闸兜，可那句话管的只是"用户自己写的 URL"。
- **G1f（本体发，`006a673`+`1d061ac`）**据此新立第三判据：**落地串含本任务 `--password` 字面值（且非空串）即拒、不回显**；七个落地口收成一条 `probe::landed_url_is_refused(url, password)`，`src/fetch.rs` 四处不再自用半支 1。
  ⚠ **闸序救了一次、也欠一格**：`Task::validate()` **先**拒"明文 HTTP ＋ 口令"（`src/task.rs:262`，**空串也算给了**）⇒ 第三判据真正可达的那一半在 **https** 侧，本机造不出"带口令的任务走到落地闸"的夹具（反臂与空口令臂因此住单元层）；
  `src/fetch.rs:1030` 退回旧判据**本机量不出来**（变异 m3 全绿）⇒ 补的是**结构针**（"数次数＋禁字"两根一起写），**不是**不真实的夹具；该针在 **G5 第 0 条**。
- 两条具名欠账（不许悄悄消失）：**第一条由 T99-H2 关掉**（旧账："落地串若以 base64／百分号编码形式回显口令，第三判据检测不到，仓库无 base64 工具、依赖 6 个钉死"）：本仓库自己写了一只 base64 编码器（与 `hex_lower`／手写 sha256 同先例，零新依赖），内容判据从"只认字面值"扩到**六种形状**（原样／百分号解码后撞中／std 与 url-safe × 带与不带 padding），同一份判据另接进 `etag` 与 `last_modified` 两个装填口。换上来的是**三条新边界**，按实测写准、不写成"什么都看得见"：① 口令短于 **4 字节**不比内容（下限理由在 `probe::PASSWORD_CONTENT_FLOOR`）；② 只认"按标准编码我们这份口令"，二次编码／`+` 当空格／大小写打乱／加盐看不见；③ **用户自己写进 `--url` 的那份字面值仍原样落盘**（Minor 5，本发不加判据）。**第二条照旧挂着**：`probe::run` 里"重定向中途失败再用 `e.url()` 判"那条错误支（按符号名指，别再写行号），G1e 十条与 G1f 三臂**都没触发过**，T99-H2 也没给它造出活体夹具 ⇒ 仍是欠账。

**之后还剩（顺序固定，各发全都碰 `src/task.rs`/`src/cli.rs`/`src/engine.rs`/`src/fetch.rs`/`tests/` ⇒ 不许并发派发实现者）**：
**G5（在飞）**＝第 0 条那根"落地口只许走统一判据"的结构针 ＋ `--out-name` 不许越出 `--out-dir`〔用户 2026-10-18 裁"拒掉"，相对子路径 `sub/x.bin` 仍允许〕＋ `src/engine.rs` 两份重复抄写文案里的 `（spec §6.7）` 删掉提成一处 ＋ 把那根针从"只扫 `--help`"扩成"扫一次真跑的 stdout+stderr、带一发 `--sha256` 不符的正臂"；
⚠ 本发还**预先裁掉**了一发必然翻红的夹具碰撞（`src/cli.rs` 那发 `a_slash_token_in_value_position_stays_a_value`：新闸一加，它的 `into_task()` 那一臂就会被拒 ⇒ 改法已写进简报，处置＝**降到 parse 层钉性质、另起一小臂钉新闸**，不许放宽闸）
⇒ **G5b**（`t99-G5b-brief.md` 已写好：`--help` 十六条选项历史上从来没有过一句说明 ⇒ 补 16 句 ＋ 别名映射进帮助 ＋ 两根针，m1/m2 专证针不是装饰）
⇒ **G6 措辞收口**（G1d 报告 R2.8 交出**十条**待裁过期措辞，全在 `src/` 注释里：`task.rs:64-65`/`:97-98`/`:211-213`/`:499`/`:649` 那句"已经打了三发"的计数、`probe.rs:947-948`/`:188-190` 枚举未点名"空 host 带端口"、`STATUS.md:22`）
⇒ **一发 scoped 复审覆盖整个修复波**（BASE `4fe6a7c` ⇒ 修复波末发；不是一发一个复审）⇒ **E33** ＋ STATUS 收口 ⇒ **G4**（`Cargo.lock` 入库：`.gitignore` 删那行 + `git add Cargo.lock`）⇒ `.superpowers/` 归档（用户已裁"打包归档再删原件"，**必须放在所有报告都写完之后**，先验归档件完整再删原件）。

**已撤销/已判掉的三条**（免得恢复时重做）：① 评审建议"统一 `authority_has_userinfo` 与 `canonical_shape_of` 的终止符集合"**判为不做**——两条判据是刻意的**并集**（`src/probe.rs:196-201`），统一＝把半支 1 唯一更严的那面削掉，等于把安全针改钝去迁就整齐；② `http://:8443/a.bin`（明文侧同族）不加形状格——它退 2 但走"明文 HTTP 不发送口令"那一支，钉它＝钉措辞而不是钉行为；③ G1d 在并行负载下见过的两红（`the_harvested_facts…`、`scenario_a_fullspeed_48mib…`）**单飞复跑 90/0** ⇒ 归因今天才算证真。

**仍欠用户一句话的两条**（不要自动执行）：**`git push`**；**`idm-recon` 那台 probe-server 的同源缺陷要不要复测并更正 `idm-recon/docs/25-blackbox-probe.md`**（那边仓库至今一个字没动，我当时给的是提问而不是推荐 ⇒ 不在任何一次"按推荐"的覆盖范围内）。

---

## 收工断点（2026-10-18 控制员；**已被上面 10-19 那节取代，保留是为了让"当时怎么判断的"可读**）

**工作树干净，未 push。最后一发**动源码**的提交是 `4fe6a7c`（G1b），其后 `1344fea` 只动文档 ⇒ 四闸数字是对 `4fe6a7c` 这棵源码树亲跑的（`--lib` 214 / `--bins` 3 / `--test integration` 89，63.36 s / `clippy --all-targets -- -D warnings` 退出码 0）。

今天落了六件事：补 `README.md`（工作区规则要求，之前缺）；T99 全分支最终评审三发（`t99-final-F1-obligations.md` / `-F2-surface.md` / `-F3-triage.md`，均在 `.superpowers/sdd/2026-10-15-dlm-kernel/`）；
勘误 **E32**（凭据闸被"同一意图的畸形写法"绕过：六种形状实测表 + 三轮修复 + "这条其实第一天就登记过、被我降级成挂账"那条自查）；
spec **R15**（信任锚＝本机证书存储）与 **R16**（段表 `end` 是闭区间含端点，R6 与 §4 示例那个 `236032` 差一字节）；
STATUS 自己那四处自相矛盾（E/R 计数、恢复步骤的 166/76、下游裁定第 7 条的 `/slow` 154 s、第 8 条的 `Config` 清单与"逐个封存等价"）；
账本 `progress.md` 末尾 append 撤销那条根本不存在的"node 对端 `seq` 与 `conn` 差一"。

**⚠ 恢复的第一件事＝按现成简报重派 G1c，不要重新发明**：`.superpowers/sdd/2026-10-15-dlm-kernel/t99-G1c-brief.md`。
这一发在今日收工时被控制员**主动中止**；中止前复量 `git diff src/ tests/` **为空** ⇒ **没有半成品、没有遗留变异**，简报与实测记录都在，直接重派即可。
为什么它必须第一个做：**凭据闸至今仍有第三族未修**——`http:/u:p@h`、`http:/\u:p@h`、`http:////u:p@h` 三形状实测各退 0，
一次性本机对端各收到 `authorization: Basic …`（**明文 http**），而规范写法 `http://u:p@h` 退 2（对照）。
⇒ **README 里"`http://` 配 `--password` 直接失败"与"断点文件里永远没有口令"这两句，今天对畸形写法仍不成立**（E32 已写明，别当已修）。
G1c 的设计是**反转默认**：只放行规范 `http(s)://`（authority 的终止符是斜杠、反斜杠、问号、井号这四种，段内不许有 `@`），其余当场以"参数写错了"拒绝 ⇒ 我们放行的集合是 reqwest 接受集的**真子集**，不再逐形状追对面；
并把同一套严格判据也给 `probe.rs` 那条**服务器控制的落地 URL**（E9 的理由：凭据会原样进 `validator.final_url`）。
⚠ 简报里要求这一发必须有**进程级零泄漏判据**（每支带凭据的 http 形状退 2 且对端 `authorization` 命中 0）——前两发都只在库级判据里打转，所以每发都漏一族。
已登记的代价：`ftp://` 与怪写法从"交给 reqwest 报错"变成"我们当场拒"＝**用户可见行为变更**（要落进 README 与 E33）。

**之后还剩四步（顺序固定，都碰同一批文件，不许并发派发）**：
① **G5**（`.superpowers/…/t99-G5-brief.md` 已写好）＝`--out-name` 不许越出 `--out-dir`（用户 2026-10-18 裁"拒掉"，相对子路径 `sub/x.bin` 仍允许）＋ 派生名四格现状钉死
＋ `src/engine.rs:996` 与 `:1594` 那两份**重复抄写**的用户文案里的 `（spec §6.7）` 删掉、提成一处，并把那根针从"只扫 `--help`"扩成"扫一次真跑的 stdout+stderr、且带一发 `--sha256` 不符的正臂"。
② **G5b**（`.superpowers/…/t99-G5b-brief.md` 已写好）＝`--help` 十六条选项**说明全空**（复量：`git log -S "help = "` 与 `-S "help="` 均为空、`pub struct Cli` 体内 `///` 数为 0 ⇒ 历史上从来没有过，不是哪一发修泄漏时删的）；
补 16 句用户面一行说明 ＋ 别名映射进帮助 ＋ 两根针（结构针断"每条 `get_help()` 非空"、进程针断帮助里含 `/d` 与 `/onboot`），三发变异里 **m1/m2 就是为了证明这两根针不是装饰**。
③ **勘误 E33** ＝三发最终评审的分诊账（F3 分诊 40 行：12 现在修 / 17 作 v1 限制交付 / 11 撤销，另有 8 组重复·矛盾·腐坏）
＋ 一条我已复量的更正：`src/engine.rs` 里 `Report::message` 文档那句"`ok = true` + `message = Some(..)` 今天没有用例钉住"**是假的**
—— `src/main.rs` 的退出档用例第①支就是这一格（⇒ 那笔"具名欠账"应登记为**已半交付**：装配层钉住、生产路径仍造不出）。
＋ E28③ 那一族的另一半：内部文档坐标进了**失败文案**，而当年那根针只管 `--help`。
④ **G4**（`Cargo.lock` 入库：`.gitignore` 删那行 + `git add Cargo.lock`——`reqwest 0.13.5` 这一档与 R15 的信任锚结论**只活在这个没入库的文件里**）
与 `.superpowers/` 归档（用户已裁"打包归档再删原件"；落点建议 `projects/.temporary/`＝工作区规则里的"清理前暂存"位。**这一步必须放在所有报告都写完之后**，
且要"先验归档件完整、再删原件"，因为 STATUS 第 172 行那一处引用的 `progress.md` / `closureA-review.md` 是全树唯一副本）。

**仍欠用户一句话的两条**（不要自动执行）：**`git push`**（分支全程只在本地、无远程）；
**`idm-recon` 那台 probe-server 的同源缺陷要不要复测并更正 `idm-recon/docs/25-blackbox-probe.md`**——那边仓库至今一个字没动，
这一条我当时给的是提问而不是推荐 ⇒ 不在任何一次"按推荐"的覆盖范围内。

## 进度

| 任务 | 状态 | 提交 |
|---|---|---|
| T0 工具链 / 依赖 / 416 根因 | ✅ 评审通过（3 轮修复 + 1 发合并复审） | `bc376eb..e73b848` |
| T1 常量与任务模型 | ✅ 评审通过（0C/0I/6 minor） | `e73b848..fef292d` |
| T2 Scheduler 纯状态机（核心） | ✅ 评审通过（1 轮修复：补 `SegmentComplete`、退化模式改起点优先、删死变体 `Halt`） | `89a0eb2..0ee1d5b` |
| T3 store 按偏移落盘 | ✅ 评审通过（1 轮修复：`flush` 改走 `sync_all`，`len` 守卫收紧到 500 µs） | `0ee1d5b..b1da691`（+ `0d18e96` docs E8） |
| T4 state 断点文件 | ✅ 评审通过（1 轮修复：`stub_for_test` 共用段表闸、`validate()` 拒 userinfo URL、`Task` 的 `Debug` 掩码口令） | `0d18e96..f241ed5`（含 `1d0c0af` docs R6/E9） |
| **T5 resume 判定** | ✅ 评审通过（1 轮修复：补 ①b「请求本身变了」闸；另修两条**恒真假绿夹具**——关键词 `"URL"` 被两条路的串同时满足） | `23e39fd..8b2921f`（含 `ec78d25`/`4353719` docs E10） |
| **T6 probe 试探** | ✅ 评审通过（1 轮修复 + **补发**的 scoped 复审全绿：`accept_ranges` 改带否决的合取、206 不许回落 `Content-Length`） | `4353719..840b0e5` |
| **T7 fetch 窗口化子请求** | ✅ 评审通过（1 轮修复 + scoped 复审：钉住 206 起点 / `Complete` 边界 / 416 分母三格存活变异，`FailKind::Transport(String)`） | `840b0e5..1c140e8`（含 `ba906f4` docs R7/E11/E12） |
| **T8 engine 主循环** | ✅ **收口**：实现（阶段1–5）+ 窄面评审 A/B/C 全 Needs fixes → 修复 8F-1/8F-2 → 收口复审 A′/B′ → 修复 8F-3a/3b/4a/4b → **任务级评审两发（E＝E12–E17 义务 38 条：落实 35/部分 1/未落实 2；F＝对外契约 1C/3I/3M）判决全部处置**。基线 166/76/clippy 0，判据方向零放宽，无产品回归 | 实现 `1c140e8..acb3166`；修复 `4a105e4`…`cced137`（8F-1/2/3a/3b）、`923b743`/`02e73a6`（4a）、`0d847f0`/`5f69db3`/`c433995`（4b）、`8758e46`/`1bfdd45`（8F-5）、`4073005`/`b29b7ee`（控制员自裁两条缝）；勘误 `ee3310f`（E19）、`6b7af6a`（E20） |
| T9 CLI · T10 验收 | **T9 ✅ 代码面 + 评审面双收口**（`472a23f..0cdce1b`）：**9A** `56e998c` / **9B-1** `c677b9d` / **9B-2** `9703950`（第一发死于会话中断⇒第二发接半成品；八条关切⇒**E26**⇒**门禁改四闸**含 `cargo test --bins`）/ **9B-3** `6ccdfe8` + 控制员自收过时注释 `c6ac42e`（纪律"注释里不写数量与行号"）/ **评审四发（两对）**⇒`E27`+`E28`（**两条评审主张我复量后不成立、已撤销**；**两条是真缺陷**；**一条推翻我自己的勘误**⇒`--data-file` 端到端可达、被撤的 C2 判据恢复）/ **9B-4a** `4e60200`（help 泄漏 + 别名值位两支修复，加 `--` 与 `--min-seg` 两根追认针）/ **9B-4b** `0cdce1b`（恢复的 C2 + 它当场量出的**第三处真缺陷**＝带体那一路多余的前导空行）⇒ 落地账见 **E29** | T9 收口于 `c889ea0` |
| **T10 spec §7 四场景** | ✅ **收口**（`c889ea0..de4eab3`，账见计划 **E30**）：三发新用例 ＝ `scenario_a_fullspeed_48mib_is_byte_exact`（全速 48 MiB 字节级正确）、`scenario_b_throttled_splits_at_midpoints_and_respects_the_floor`（`/slow` 全尺寸：真的劈了＋劈点真上线＋中点算术＋下限＋劈在**飞行中的响应**上）、`scenario_c_real_process_kill_then_resume_does_not_redownload_covered_bytes`（真 spawn ＋ 真 kill ＋ 同 URL 再续，判据③"覆盖集内的字节一个都不许多要"）＋ `docs/evidence/2026-10-18-spec7-四场景验收.md`（判据→用例对照表在其 §8，偏差登记在 §6）。⚠ **执行者死于写报告之前**（129 次调用 / 65 分钟）⇒ 控制员独立验收：四闸亲跑、M-C1 亲跑复现红、`md5sum -c` 全等、工作树 **640 增 / 0 删**（既有判据一字未动）。⚠ **我那条"场景 A 早有活体"的错账已进 E30**：既有那发跑的是 **8 MiB**、oracle 是 `expected_sha` ⇒ 只 grep 函数名就断言它覆盖什么；**新纪律：凡"某一发用例覆盖了 X"，落笔前读它的入参尺寸＋期望值来源——名字像 ≠ 覆盖到。** | 四条成功判据全部有载体（对照表在 evidence §8）＋ §6.2 由 T11 补上 ⇒ **下一发＝全分支最终评审** |
| **T11 买仪器（用户批准的 §6.2）** | ✅ **`91010fc`**：`tests/harness/probe-server.mjs` 新增 `/probe-only`（**只**服务试探的 `bytes=0-236031` 走原正常路径、其余 503 空 body ＋ 新事件 `response_refuse`、拒绝照样 `res.end()`；该文件 **0 行删除**）＋ 真二进制程级用例 `the_binary_prints_slow_retry_rounds_when_every_worker_shot_is_refused`。**抓到的字面行** `0.4%  0.2 MiB/48.0 MiB  0.0 MiB/s  x0/8  slow-retry x1`——同一行还是 **E25⑤／E25①／E25②** 三条口径的活样本（`x0` 与 `slow-retry` 同拍不绑、速率不足一档印 `0.0`、0.468% 截成 `0.4%`）。对端账 1 正常 / 56 拒，退 1，断点保留，目标是 `236032 B` 的字节正确前缀。⇒ **spec §6.2 的 `slow-retry xN` 端到端不再是"说得出、验不了"** | 账见计划 **E31**；四闸 **202 / 3 / 89 / 0**（集成 63.2 s） |

计划文末 `## 勘误` 现有 **E1–E32**（2026-10-18 最终评审换来的更正一路涨到 E32；本节只报**范围**、不抄内容 ⇒ 抄具体条数正是上一版腐坏的方式。E7 起都是执行期实测后才写进计划的，正文不回改；**E19 ＝ T8 任务级评审第 1 发的三条**：E16 判据操作化成"枪数 + 线上区间 + sha"并附变异证据、§6.2 进度行缺数据面待裁、E8① 的"唯一调用点"误读已纠正；**E20 ＝ T9 派发前的九处更正 + 一处具名欠账**；**E21 ＝ 用户八项裁定的落点 + 甲的 `RunSnapshot` 契约（10 条判据 + 9A 的验收下限）**；**E27 ＝ 9B-3 六条关切**；**E28 ＝ T9 评审四发逐条实测后的十条裁定，含一条撤销我自己 E26 附记前提的**）；设计稿 §10 现有 **R1–R16**（R14＝保存节奏是合取闸；R15＝**v1 的信任锚是本机证书存储**、§9 那句"自带根"是假的；R16＝断点段表的 `end` 是**闭区间含端点**、R6 与 §4 示例那个 `236032` 差一字节。R8–R13＝T9 评审换来的：预读/下限无 CLI 旋钮、`If-Range` 不存在、416 是条件分支、Ctrl-C 不做、§5 计数与 `x4/8`、`--data-file` 的真实形状）。派发任何任务前先把与该任务相关的 E/R 条目抄进 brief。

**当前基线（2026-10-18 控制员实测于 `4fe6a7c`）**：`cargo test --lib` **214 passed / 0 failed**（3.01 s）、`cargo test --bins` **3 passed / 0 failed**（0.00 s；**第四闸**，E26：只有它看得见装配层读错退出档）、`cargo test --test integration` **89 passed / 0 failed**（**63.2 s**——T11 买 `slow-retry xN` 端到端那一发独占 ~61 s，与其余并行 ⇒ 净增 ~22 s，这是**买判据的价格**）、`cargo clippy --all-targets -- -D warnings` **退出码 0**；`git status` 干净；分支 `sdd/dlm-kernel`，**未 push、无远程**。验收面是**这四闸**（E26）；下限＝**只许增、不许删、不许改判据方向**（**简报里一律写 floor，不写 exact**——E26 之后那条措辞换成"exact"曾被合理读成上限，导致执行者有一发判据没敢加）。

## 下一个动作（2026-10-18 断点；**当前下一发＝最上面「今日收工断点」里的那一发 G1c（重派现成简报），不是下面任何一项**；第 1、2、2b、3 项均已收口；**第 4 项只收了一半**：最终评审三发已回来并处置、代裁清单已成文，但 `.superpowers/` 归档与 `git push` 仍未做；原文留着是因为第 1–3 项是论证与实测记录）

1. **T8 任务级收口评审（进行中，按"条数 ≤ 4、只读、报告必须落文件"切发）**：
   - **第 1 发 评审 E ＝ E12–E17 义务逐条对账** ✅ 已回来并处置：38 条义务 **落实 35 / 部分 1 / 未落实 2**，无实现面回归；
     报告 `.superpowers/sdd/2026-10-15-dlm-kernel/t8-review-E12-E17.md`，三条结论已落 **E19**（E16 判据操作化并附变异证据、
     §6.2 进度行缺数据面待用户裁、E8① 的"唯一调用点"误读已由 `4073005` 纠正）。
   - **第 2 发 ＝ 引擎对外契约 vs T9/CLI 所需**（评审 F）✅ 已回来并处置：13 行"承诺→数据来源"对账 + **1C/3I/3M**，
     四条主张控制员逐条读代码复量**全真**。C1（`Task::limits()` 死面 + "供 scheduler 初始化用"的假承诺，
     照它填会把 `--connections` 静默吞成 8）由 8F-5 的 `8758e46` 删掉、控制员再补刀 `b29b7ee`
     （连 `probe_prelude_bytes()` 一起删，`Task` 上一个常数访问器都不留）；I1/I2/I3 由 `1bfdd45` 写成双向契约。
     三条 Minor（sha256 形状校验归 CLI、`Report` 无最终路径字段、`trace` 不许打 stdout）与另六处 T9 相关更正
     一并落 **E20**（`6b7af6a`）。
   - ⇒ **`Task 8: complete` 已记账**（本行上面那张进度表）。T8 全程：**7 轮修复 + 4 轮评审**，判据方向零放宽。

⚠ **切法纪律（本轮换来的）**：派发一律按"**条数 ≤ 4、单一问题、只读、报告必须写成文件**"切，不按文件面切；
按文件面切的四发（5A、8F-2、8F-3b、8F-4b）里死了两发。评审对象优先选**当前状态**而不是 diff 历史
（diff 已被多轮窄包覆盖，重读一遍等于重付一遍）。
2. **T9 CLI** —— **已解锁**（用户 2026-10-17 把 E20① 裁成**甲** ⇒ 快照契约见 **E21**）。E20（`6b7af6a`）把九处更正逐条写死了（别名表按 recon C3、`--quiet` 不住 `Task.flags`、
   bit0 在 `run` 之前短路、`Task` 上没有常数访问器可填 `Config`、`prelude_max` 无 CLI 选项、退出码两档且判据只有 `!ok`、
   `retries`/`duplicated` 是跨轮账、`trace` 不打 stdout、sha256 形状校验归 CLI）。
   ⚠ **E20① 已关闭 ⇒ 派发拆两发，仍按"条数 ≤ 4、不按文件面切"**：**9A**＝引擎面 `RunSnapshot` + `Config.on_tick`（判据＝E21 的 1–10 与"三格新用例"，
   验收下限＝三闸 **173 / 76 / clippy 0**（9A 之后 lib 从 166 涨到 173）且**既有断言一条都不许改**）；**9B**＝`src/cli.rs` + `src/report.rs` + `main.rs`（照 E20 九处 + 下面那条裁定 9）。
   ⚠ 另带上**裁定 9**：`FailKind::Transport` 的载荷生产侧已是单行 ≤200 字符 ⇒ **`fresh_reason`/`message` 渲染时不许再截断、不许再拼接**（两处截断会互相吃掉信息）。
2b. **9B-4 修复轮 ✅ 已交付**（两发：`4e60200` 的 cli 层两支真缺陷修复 + 两根追认针，`0cdce1b` 的恢复 C2 +
   它当场量出的前导空行缺陷 + `--quiet` 两支形状；**账与裁定见计划 `E29`**）。下面六件事的原文留着当**证据记录**，
   其中"评审报了、我复量后不成立的两条"那一栏**不要**再拿去修。
   ① **`--help` 泄漏内部论证**（`Cli` 的结构体 `///` 成了 clap 的 `long_about`，实测打印 `**E23②**`/"暗道"/"那 14 条"）
   ⇒ 论证移到 `//`，并补一根**禁字针**扫 `--help` 的输出（禁 `E`+数字、`§`、`勘误`、`clap`、"[数字] 条长选项"）
   —— 这是 `c6ac42e` 那条纪律**第一次有牙**；② **别名展开不吃自己吐出的长选项的值位**
   （实测 `dlm /d U /f /n` 退 2、`dlm --url U --out-name /n` 退 0）⇒ 一行修 + 表驱动等价判据；
   ③ **恢复 C2**：`--data-file` 的进程级判据（不带 `--quiet` 跑，断 stdout 零 `\r`、结果行与摘要照打、退 0、对端只有一条 POST；
   对照臂用可分段那一路，证明进程级读得到 `\r`）；④ `--` 之后不展开别名那一支补一发带对照的；
   ⑤ `--min-seg` / `--probe-bytes` **反向拒绝针**；⑥ `--quiet` 收尾空行那一格**要么补针要么改名**，
   外加 `report::tests` 里那条夹具自证（`assert_eq!` 断的是同发上方自己写的字面量）改成读产品出口或删掉。
   ⚠ **简报里必须写的环境事实**：Git Bash 会把 `/d` 这类前导斜杠 token 改写成 `D:/` 再交给进程
   ⇒ 别名/值位相关的复量与用例一律 `MSYS2_ARG_CONV_EXCL='*'`，否则假红或假绿。
   ⚠ **评审报了、我复量后不成立的两条不要修**：`Err` 支退出档已有进程级判据（在
   `the_binary_maps_ok_to_the_exit_code_and_quiet_to_never_installing_the_sink` 第④支）、
   `slow_retry_rounds` 的 max-vs-sum 已有针（`engine::tests` 那一格期望串自带"不是各链相加"）。
3. **T10 四场景验收 ✅ 已交付**（`de4eab3`；交付账、独立复量与"名字像 ≠ 覆盖到"那条新纪律见计划 **E30**；
   下面这段留着是因为它是**仪器真账**，最终评审与以后任何要重跑场景的人都还得读它）：
   `/drop/<n>`（不是 `/flaky`，裁定 7）与 `/sz/<n>s`（n 取 65536 整数倍），证据落 `docs/evidence/`；
   **限速场景用 `/slow` 全尺寸（48 MiB），实测 24 s** ⇒ 原先写进账本的"≈150 s、要缩到 1 MiB"作废（**E28⑤**：
   那个 150 s 是**按单连接**算的，而限速按连接计、8 条 worker 并发 ⇒ 聚合 ≈ `N × 320 KiB/s`）。
   ⚠ **两处必须写进断言的例外，否则会做成恒绿或恒红**（E28⑫）：
   `STATE_SAVE` 那个"1 s ＋ 5 s 兜底"是**合取闸**⇒ 兜底事实上不存在，SIGKILL 时断点里往往只有"种子"那一格
   ⇒ 场景 3 的"续传不重下已完成段"不许假定断点比种子更细；
   §6.2 的 `slow-retry xN` 端到端今天**拿不到**（`/drop/0` 把预读一起砸 ⇒ 走 `Err` 不进重试链）
   ⇒ 要钉它需要一条新对端形状"预读能过、worker 枪必挂"，那是**仪器变更**，T10 若要买就单独登记。
   ⚠ **本条的"拿不到"已于同日作废**：那把新对端形状真被造出来并量到了（`/probe-only` ＋ 进程级用例，见计划 **E31** 与进度表 T11 行）
   ⇒ 留着这段是因为它记的是"**当时为什么判不到**"——那一档缺的是仪器而不是判据，这个诊断现在被实测背书了。
   多 worker 的真"杀了再续"＝spawn `dlm` 二进制 + kill 子进程，断言 `fresh_reason.is_none()`、续传后每一枪线上起点
   ≥ 覆盖集终点 + 1、最终 sha256。**可选加固**（复审 A′ 的 R7 配方）：给预算表补两格"两支都不落在 15/120 上"的行，
   关掉"下界夹平"盲区——它原先的立论前提（`stop.max(WINDOW_BYTES)` 隐身）已被我实测推翻，所以只算可选。
4. **全分支最终评审** + `.superpowers/sdd/` 清理 + 交一份"我替用户做过的全部裁定"清单。
5. **等用户点头的六条见文末**；没点的按现状继续，不擅自改 spec 形状或扩依赖。


## 恢复步骤（照做即可，别凭记忆）

```bash
cd /e/WorkSpace/projects/tool/dlm
SDD="C:/Users/ROG/.qoder-cn/plugins/cache/qoder-marketplace/superpowers/6.3.0/skills/subagent-driven-development/scripts"
git rev-parse --abbrev-ref HEAD          # 应为 sdd/dlm-kernel
git log --oneline -6                     # 顶端是最近几发 docs/test 提交 ⇒ 本文件**不维护**这条清单（旧版在这里抄了 6 个 sha，三轮之后整条成假）
git status --porcelain                   # 应为空（不空就是树里有在制品，先按 md5 备份再动）
cargo test --lib 2>&1 | tail -2           # 期望值＝上面「当前基线」那一行；这里**故意不复制数字**（抄过的 166 就是腐坏源）
cargo test --bins 2>&1 | tail -2          # 同上（第四闸，E26：只有它看得见装配层读错退出档）
cargo test --test integration 2>&1 | tail -2  # 同上；这发要跑约一分钟，它起真对端
cargo clippy --all-targets -- -D warnings # 期望退出码 0
tail -40 .superpowers/sdd/2026-10-15-dlm-kernel/progress.md   # ⚠ 这个文件被 .gitignore 忽略、全树唯一副本；它的末段带着一条**已被实测撤销**的仪器缺陷，2026-10-18 已在文末 append 更正 ⇒ 读末段时先看那条 dated 更正
# （旧版这一行写的是"末段是 8F-4b 接手账"；账本会被后续每一发继续追加，别把它的形状抄死）
# 计划里的 Task 正文到 **Task 10** 为止（T11 与最终评审的修复轮都**没有** `### Task` 正文，只有文末勘误）
# ⇒ 派修复轮时**自己写简报文件**（`.superpowers/sdd/2026-10-15-dlm-kernel/t99-*-brief.md` 那一族），
#   不要再跑 `task-brief <plan> 8` 那种"从计划里切一发"的命令——它切出来的是已完成任务的旧正文。
```

账本在 `.superpowers/sdd/`（被 `.superpowers/sdd/.gitignore` 整目录忽略 ⇒ `git clean -fdx` 会连账一起清，届时以本文件 + `git log` 为恢复源）。每任务循环：`task-brief` → 派实现者（记 BASE）→ `review-package PLAN BASE HEAD` → 派评审 → 需要则修复轮（≤5）→ 记账 `Task N: complete`。

## 下游任务必须遵守的裁定（都已进账/进勘误，派发时逐条抄进 dispatch）

1. **`src/lib.rs` 是模块树的唯一宿主**（E6）。每个新模块加 `pub mod`，`main.rs` 只当二进制。别把它改回 bin-only。
2. **常量 12 个，没有 `PROBE_USES_CONNECTION_CLOSE`**（E2）。计划 `:717` Task 6 的 `Consumes` 仍写着它 —— 那是断链，派发 T6 时必须显式声明作废。
3. **全 crate 禁止 `Connection: close` 与 `pool_max_idle_per_host(0)`**（R5）。416 卡死的真因是 harness 推流循环没 `res.end()`，reqwest 在这台仪器上无罪 —— 且"无罪"只对这台仪器成立，不许外推。
4. **`ProbeFacts` 住 `src/resume.rs`**（T5），T6 的 `probe.rs` `use crate::resume::ProbeFacts`；`probe → resume` 是允许的边，依赖方向行 `:46` 描述的是 main 的组装，不是全部 crate 内边。
5. **T9 的两处计划缺陷预先改掉**：`parse_from` 签名泛化为 `<I, S> where S: Into<OsString> + Clone`（否则计划自己的测试字面量 `["dlm", "--url", …]` 编译不过，断言不许改）；`ProgressLine` 无人定义 ⇒ 由 `src/report.rs` 自造，字段反推自 `:885` 那行渲染格式。
6. **T6 要补一条守卫用例**（E3）：同一连接上"读满一个 206 → 再发一个 416"必须都完成 —— 这是唯一守住 `res.end()` 修复不被回退的东西。
7. **T10 的"掐断后能续、且不重下已完成段"必须用 `/drop/<n>`**（E5），不是 `/flaky` —— `/flaky` 从来只把长度改成 4 MiB，不会中断 body。限速场景用 `GET /sz/<n>s`（n 取 65536 的整数倍，约 n/65536×200 ms），不用 `/slow`（48 MiB 真限速后要跑约 154 秒）。
   ⚠ **本条后半句已被实测推翻（E28⑤，2026-10-18 复量）：`/slow` 全尺寸 48 MiB 实测 24 秒**，那个 154 s 是**按单连接**算出来的，而限速按连接计、8 条 worker 并发 ⇒ 聚合 ≈ `N × 320 KiB/s`。
   ⇒ **T10 场景 B 用的就是 `/slow` 全尺寸**（`scenario_b_throttled_splits_at_midpoints_and_respects_the_floor`）；`/sz/<n>s` 仍是量"劈分节奏"的便宜档。
   这张表是"派发时逐条抄进 brief"的那张 ⇒ 抄之前先看这一行括注，别把 154 s 抄进下一发。
8. **T8 的简报有三处断接口，一律按 E13 改**：`Scheduler` 没有续传播种门（要加 `resume(total, min_seg, done)` 并配"与 `new`＋逐个封存等价"的用例）；`Config.resume: Decision` 站不住 ⇒ **引擎自走 `load → probe → decide`**，`Config` 改 `{workers, min_seg, max_retries, verify_sha256, force_fresh, keep_state}`；Step 1 那条 `harness::replay(Range 序列)` 与 spec §7.2 相反 ⇒ 录**事件序列**回放给真 `Scheduler`，比"动作 vs 实际派发 vs 线上 Range"三道链接。
   ⚠ **本条有两处落后于它自己引用的勘误，抄进简报之前按这两句改**（2026-10-18 控制员对码复量）：
   ① `Config` 的字段清单**不在本文维护**（抄一遍就是下一个腐坏源；现状＝`src/engine.rs` 的 `pub struct Config`，已实测确认**里面没有 `keep_state`**——它在 `Task::flags` 的 `FLAG_KEEP_STATE` 那一格，**E13②** 的自纠是对的）；
   ② "与 `new`＋逐个封存等价"这一句按字面**做不到、不许钉**（**E15** 已裁定：动作序列逐条相同不成立 ⇒ 只钉可观测的等价面）。
9. **`FailKind::Transport(String)`**（T7 裁定 D）⇒ `Outcome`/`FailKind` 从 `Copy` 降为 **`Clone`**（worker 线程发结论走克隆）。原因串在生产侧已保证**单行、≤200 字符、不含 body 字节** ⇒ **T9 打印时不许再截断或再拼接**（两处截断会互相吃掉信息）。
10. **`probe::run` 对带 `data_file` 的任务在上网前硬拒**（E11①）⇒ 引擎必须跳过 probe、用 `fetch::run_whole`，且**这一枪只发一次**（重发＝二次提交表单，spec R7 已封顶）；该形状**既不读也不写断点文件**（E13④：存一份段表等于造一条永远执行不了的假 `Resume`）。
11. **完成判定分两本账**（E7 追加段 + E13④）：`sched.is_done()` **且** `store.len() == total_len` 才算完；`snapshot_intervals()`/`confirmed_bytes()` 是"归属已定的字节"，**永远不许**当"已写字节数"去过 spec §6.7 的闸。给了 `--sha256` 就读真文件算（`sha2` 早在依赖里，不是新增），不匹配则**保留文件与断点**并报错退出。
12. **进程内窄桩的天花板是 `MAX_CONNECTIONS = 8` × `PER_CONNECTION = 4`** ⇒ 多 worker 的引擎用例一律用 node 对端（无上限）；已**授权把 `PER_CONNECTION` 抬到 64**（E13⑤），抬完既有断言一条不许改。
13. **终点帽的形状（E14① 乙 + 附记）**：每 worker 一格 `Arc<AtomicU64>`，**只降不升** ⇒ 原语是 `fetch_min`（我在派发里写 `fetch_max` 是**反的**，实现者纠正，单调性已钉两条用例）。`run_window` 的 64 KiB 循环**每块只查一次**帽：不 sleep、不自起轮询线程、不引 `tokio`。
14. **开放终点的判据是"窗口够不够得着 EOF"**（E14② 戊）：`min(iv.end, iv.start + WINDOW_BYTES - 1) + 1 >= total_len` 才许发 `bytes=start-`，不是看 `iv.end`。
15. **`bytes_duplicated` 的口径只许两种存在方式**（E14③ + **E17**）：要么有**线上证据**的真账单，要么**如实标注**成上界/少报。**禁止拿覆盖集倒推** —— "盘上有、覆盖集里没有"绝大多数是**从没下载过的洞**（多 worker 中点劈分天生制造洞），不是重复。
16. **线上字节量具只有一个诚实口径**：`Σ conn_close_side.sent`。`Σ response_done.bytes` **漏**半路被丢弃的响应；两者相加**重复计数**（一条正常响应两条都记）。我用两代坏尺子写过 E14 正文，附记已更正 ⇒ 任何"多打多少字节"的断言都要写明口径。
17. **退化模式一枪下完剩余整条 body**（**E16**）：不许按窗重请求（那是平方级流量），预算按整条体量算（R7 的整请求期限语义），丢弃的前缀要么进账要么写明没进账。
18. **续传播种只走既有的 `seal()`**（E15/E13①）：`Scheduler::resume` 不许开第二本覆盖账；等价性判据钉"**洞的集合相等 + 种子字节一次不重派 + `pending_bytes`/`is_done`/`snapshot_intervals` 逐步一致**"，**不许**钉"动作序列逐条相同"（按字面做不到，多洞种子的队列粒度天然不同）。

## 实测才知道的坑

- `rust-toolchain.toml` 不生效时报的是 `dlltool.exe: program not found`（本机全局默认是 gnu）。判据用 `rustup show`，**不动全局默认**。
- reqwest 的 feature 名是 `rustls`，不是 `rustls-tls`；写错会静默不加依赖、`Cargo.toml` 里什么都没有。
- `cargo … | tail -15; echo $?` 取到的是 `tail` 的退出码 —— 上一轮就这样误报过"编译成功"。
- Git Bash 把 `/x` 这类开关改写成盘符；给原生 node/curl 传路径时用 `MSYS2_ARG_CONV_EXCL='*'`，日志路径写仓库内 `target/`，别写 `/tmp`。
- harness 限速生效后有条尾巴：同一条 keep-alive 连接上的**第二个**请求要等一拍（实测 201/203 ms）。T7 的窗口超时判据若贴着 200 ms 量纲会被它干扰 —— 时序预算要算进去。
- `pipeline` 里取退出码要单独跑；`node --check` 只验语法不验行为。
- **`File::open` 一个目录在本机是直接失败的**（`PermissionDenied, os error 5`），不是"打开成功、读到才炸"。我给 T7 的 drive-by 写过的前提是错的，实现者实测推翻并只保留了"消息更准确"这一条价值。⇒ **派发里写的"库行为/症状成因"和数字一样要先复量。**
- **评审者报的观察也要复量**：T6 复审说发包 diff "被截断"，我未测就落了账；实测 `tail -n +13 包文件` 与 `git diff -U10 BASE..HEAD` **逐字节相同**（626 行对 626 行）⇒ 发包无罪，真因是最后一个 hunk 只有 10 行尾上下文、恰好停在一段中文文档中间。以后怀疑缺内容就拿 `git diff --stat` 的行数对账。
- **一发子代理撑不住一个整任务**：T8 第一发跑了 43 分钟 / 117 次工具调用后被 "Model response interrupted" 打断，且**没写报告文件** ⇒ 现场只能靠 `git log` + `git diff --numstat` 重建。按阶段拆发之后（第 2 发只做阶段4）同样 115 次调用走完并留下了报告。**判据：任务里有 2 个以上"可独立评审"的落点，就先拆发。**
- **被打断时"保 WIP"必须先于"量基线"**：那份 WIP（`src/engine.rs` +339/−74，编译不过）唯一的存在处就是工作树 ⇒ 先 `cp` 进 `.superpowers/sdd/<plan>/` 并**两侧 md5 复验**，再临时换成 HEAD 版本跑 `cargo test --lib` 取真实下限（这次量出 144），最后放回并复验 md5 与 numstat 未变。全程没碰 `git restore`/`checkout --`。
- 派发里引用的基线数字要**当场重测**：我给阶段4 的 dispatch 写 integration 下限 57，实际是 59（阶段3 自己加了两条 harness 测试）—— 实现者纠正了我。
- **量具错一代，结论就整个反掉**（2026-10-17，本轮最贵的一课）：我用 `Σ response_done.bytes` 就敢写"实际只有 1.51×，实现者报的 2× 是错的"，换成唯一诚实的 `Σ conn_close_side.sent` 之后是 **2.009×** ⇒ **实现者原本是对的，被我的坏尺子判成了错**。两条纪律：① 任何"字节数/比率"进文档之前先证明这把尺子**能看见它声称看见的东西**（漏计、重复计、跨组求和这三种错法我都犯过）；② **拿自己的量具去推翻别人的实测时，先复量自己的**。
- **变异结论不许过夜**：账本记的"M3 存活、集成层看不见 1 字节的洞"是实现者在**它那一发当时的树**上量的；5B 又往同一棵树加了 10 条用例 ⇒ 我到 HEAD 复量，两个方向（多报/少报一字节）各有 **3 条单测 + 3 条集成用例**红 ⇒ 结论作废。**规矩：任何"某变异存活／某层看不见"的登记，都必须在将要评审的那棵树上重跑一次才落账。**
- **子代理的原始输出不在盘上，压缩上下文后会丢**（评审 C 的 Minor 逐条就这么没了）⇒ **评审一发回来，第一件事就是把 Important/Minor 逐条原文抄进账本或一个文件**，不要只抄我消化过的结论。
- **`requests_at_least(n)` 存在的理由会被忘**：凡是"读线上日志再断言"的用例，先守卫条数再读（5B 已在 Abort 用例修过一发，C-I-4 又抓到两处）。裸 `srv.requests()?`/`ranges()?`/`events()?` 在引擎返回后立刻读，会因为日志落盘晚于响应而**碰巧为真**。
- **中文路径 + 一次性取证文件**：临时 `tests/tmp_probe.rs` 这类探针跑完即删、**不许进提交**（本轮两次都做到了）；提交信息里写明"实现来自被打断的那一发、控制员只改了哪几行"。

### 2026-10-17 收口复审批（工具与派发形状，跨会话有效）

- **子代理的长文交付必须落文件**：这台机上后台任务的 `tasks/*.output` **全是 0 字节**，唯一通道是完成通知里的 `result` 字段，**约 600 字硬截断**。一发 8.1 M tokens 的评审因此只回来半条＝白付。⇒ 简报里写死"全文写成指定绝对路径的文件，返回消息只给路径 + ≤ 60 字"；"只读"只约束被测树，报告文件要单独列路径。
- **同一件事，窄派单便宜一个数量级**：同一个面，开放式评审花 11.57 M tokens / 55 次调用，按"≤25 次调用 + 指定问题 + 不许跑 cargo（会撞构建锁）+ 报告落文件"重派的那发只花 1.16 M / 23 次，**还抓出前者漏掉的三处恒真判据**。
- **负向断言不许用 Grep 工具下结论**：它走 ripgrep 的 ignore 规则，**看不见 `.superpowers/`**（账本就在里面）与 `node_modules/` ⇒ "某名字全树不存在"是假阴性。要么 `grep -rn`（Bash），要么先拿一个明知存在的串在同一路径自测这根针能不能红。我差点用假阴性把"裁定 16 悬空"这条登记错。
- **别人报的变异结论要复跑，而且复跑到"哪一格先红"**：评审 A 预测全绿的 `stop.max(WINDOW_BYTES)` 实测**红 1**（它把 `stop` 自己 44 MB 这件事算反了）；预测红 2 的 `rate==0` 变异实测**红 3**；8F-4b 留下的注释说某变异"⇒ 这里红"，实测**首红在同一用例前一格**、那句针根本轮不到说话 ⇒ 方向对、落点错也要改口（`c433995`）。
- **判据想有牙，`const _: () = assert!(CONST > 2)` 比运行时 `assert!` 更早**：把"构造常量"与"守卫常量"写成同一式子必然恒真（`3W > 2W`）；断常量本身、并且放进 `const` ⇒ 窄化常量**编译期红**（实测 `error[E0080]`，还顺带满足 `clippy::assertions_on_constants`）。
- **四发死在 150 轮上限的共同形状**＝"N 条判据改造 + 每条变异自证 + 写报告 + 提交"堆一发。⇒ 下一刀**按条数（≤4）切，不按文件面切**；简报里写"接近上限先把 commit 打上、报告可以短"，本轮 8F-4a 就是靠这句带回了完整提交。
- **接手死掉的在制品，顺序不许倒**：`git log`（它有没有提交）→ `git status`/`diff --stat`（停在哪儿）→ **先 `cp` 备份 + 两侧 md5** → 再扫遗留变异 → 才跑闸。8F-4b 那发是"代码全做完、三闸全绿、只差报告"⇒ 我自己代提交三枚即可，**重派才是浪费**。


## 挂账的 minor（评审判"不阻塞"，最终评审 triage）

- **口令规则的残余绕过**：前导空格/Tab 的 URL（`" \thttp://…"`）仍过校验，而 reqwest 的 `Url::parse` 会剥掉它们 ⇒ 这类串真能明文发口令。修法二选一：`validate()` 改比 `Url::parse(…).scheme()`，或 T9 先拒带首尾空白的 url。
  ⚠ **已修（2026-10-18，G1 `9cf00e6` + G1b），并且这一行本身要读成一条纪律而不是一条待办**：它从 `89a0eb2`（**T0/T1 收工那天**）就登记在这里、诊断完全正确，
  却被我自己归进"挂账的 minor（不阻塞）"跟着项目走了 **90 发提交**。⇒ ① **凭据能上明文线／能落盘的形状不是 minor**，分诊先按后果分档、不按"评审有没有说阻塞"分档；
  ② 上面那两支修法**都没采纳**：`Url::parse` 要把 `url` 提成直接依赖（破你定的 6 个 crate 上限），"先拒带首尾空白"只治第一步（G1 做的就等于它，仍被 C0 前导与 scheme 内插 Tab 绕过）；
  ③ 实际落的是第三支＝**自己实现 WHATWG 的两步预处理**（先删全文 tab/LF/CR，再删首尾 `U+0000`–`U+0020`），见计划 **E32**。
- 计划字面要求的两个边角要人裁：无 path 的 URL（`https://example.com`）文件名取成 `example.com`；百分号编码名不解码（`a%20b.zip` 原样落盘）。
  ⇒ **2026-10-18 控制员裁**：**保留现状**＋钉用例＋写进 README（百分号解码要引第三方 crate ⇒ 破 6 个 crate 上限；把 host 当文件名无害、改成报错无收益）。
  同日实测五种形状的落点名（`https://example.com`⇒`example.com`、`…/`⇒`download`、`a%20b.zip`⇒原样、`?x=1#f`⇒剥掉、`…/dir/`⇒`download`）。
- `--cookie` / `--referer` 同样是凭据，明文 HTTP 下照发；spec §4 只管口令，本轮不自作规则。
  ⇒ **2026-10-18 用户裁："只补文档边界"**（不改代码、不折进 `needs_secret`）；已写进 README「凭证与边界」。**这一条从此不是挂账，是设计。**
- 依赖基线不可复现：`Cargo.toml` 用开区间，`.gitignore` 还忽略着 `Cargo.lock`（应用型 crate 通常该提交 lockfile）。
  ⇒ **2026-10-18 裁"入库"**（G4）：`.gitignore` 删那行 + 提交 `Cargo.lock`。⚠ `reqwest 0.13.5` 这一档、以及 R15 那条信任锚结论，**只活在这个没入库的文件里**。
- 空 URL 规则无单测；`target_path` 只断言了 `file_name()`，没断言 `dir` 真被用上、没覆盖 `"download"` 兜底与 query/fragment 剥离。
- 计划勘误文案两处错标：E2 把"判定段"标成 Task 6 Step 3（实为 Task 0 Step 3）；E4 被取代那段把"停在 body 中间"写成 §7 场景 2（应是场景 3，E5 用的编号才对）。

### T6/T7 收口后新挂的 minor（2026-10-16，同样只进最终评审 triage，本轮不动）

- `src/probe.rs:84-85` 文档第 10 条写"发过 Range 而响应不是 206 时**无条件**为假"，但保留的 416 分支（`:216-231`，F3 批准过）在此之前把 `accept_ranges` 硬置 `true` ⇒ 行为无误、**文档与代码相反**，措辞要收成"走 `peer_accepts_ranges` 的那一支"。
- `src/probe.rs:614-621` 一条真值表断言比的是**两个生产输出**（只能抓一类变异）；承重方向在 `:599-608`/`:622-623` 按字面钉着 ⇒ 可接受，但别把"比两个产物"当模板。
- `src/fetch.rs`（T7 修复轮）三处措辞/断言强度：`:180-188` 与 `:757-761` 那句"这一支在本仓库的夹具下打不进去"**是过度声明**（复审给了配方：`200` + `Content-Length: 0` + `Content-Range: bytes 0-4194303/50331648` 走 `spawn_stub` ⇒ `:763` 与兄弟 `:768-777` 可达且可测，只是没做）；`tests/integration.rs:1793-1794` 把两条 cell 各杀哪个变异**说反了**；`:1081` 与 `:2007` 的关键字断言（`响应体`＋`干净结束`）被两条路各自自造的常量同时满足 ⇒ 钉不住来源分支（判别词是 `响应头` vs `声明了长度`）。
- `tests/integration.rs:1947-1951` 用"绑端口→放开→再连"造 connection refused，有 **TOCTOU 偶发红**风险（要真修得换 accept-then-close 的确定性载体）。`take_transport_note`（`:1925`）把 200 硬写在测试里而不是引用 `TRANSPORT_NOTE_MAX_CHARS`（跨 crate 边界看不见私有常量）⇒ 上限被调会脱钩。
- `task-7-report.md` §2 的 RED 行号锚点 `integration.rs:2039` 已漂到 `:2046`（文档块长行）。**报告里的行号会腐烂，复验时按消息文本找。**
- 越界观察（评审判非阻塞）：`content_range_total`（`src/probe.rs:386-392`）**不核对 206 的窗口与我们请求的 Range 是否一致**；窄桩 `read_request_target` 只记 target、丢头 ⇒ 没有任何 probe 用例在**线上**断言 Range 真到了对端；`tests/harness/mod.rs` 顶部仍是文件级 `#[allow(dead_code)]`（下一个未用 helper 不会警告）。
- `FailKind::ValidatorChanged` 不带消息 ⇒ 引擎/T9 只能说"验证子不符"，说不出是四格里哪一格（长度变了／起点不对／416 分母不对／…）。是未来的候选裁定，不是本轮缺陷。

### T8 评审 A/B/C 新挂的 minor（2026-10-17；除已并入 8F-1 的三条，其余进最终评审 triage）

⚠ **本节以下条目的现状（2026-10-17 收口复审后）**：`scheduler.rs` 悬空测试名与"逐条相同"过度声明 ⇒ **8F-1 已修**（E15 收窄已写进文档）；`state.rs` 那句"必然被别人再下"与 `:176`/`:192` 自相矛盾 ⇒ **8F-3a / 8F-4a 已修**（统一成"可能"，并按 E14 附记拆开三批字节）；`engine.rs:1512` 拿被测公式现算期望、`:1813` 用 `!is_empty()`、`:1815` 断自己两行前设的字面量 ⇒ **8F-1 已修**（孪生表行 + 字面量期望值，我 2026-10-17 复量 `budget_secs:` 七格全是字面量）；`resume` + `NoRanges` 那一格 ⇒ **8F-3b 已补**（`scheduler.rs` 的 `no_ranges_on_a_multi_gap_seed_…`）；"评审 C 的 Minor 逐条未落盘"这条**已消解**——那一发的 Important/Minor 已逐条抄进 `.superpowers/…/progress.md` 与 `closureA-review.md`。**剩下的才是真挂账**（`last_note` 全局一格、`min_seg = 0` 无下限闸、`STATE_SAVE_MAX_INTERVAL_SECS` 名字会说谎、调度器测试读 `consts::MIN_SEGMENT_BYTES` 等）。

- `scheduler.rs:145-146` 公共文档指向一个**不存在的测试名**，且无条件承诺"逐条相同"（E15 已把这条承诺收窄）⇒ 随 A1 一起修（在 8F-1）。
- `scheduler.rs` 的 `resume` 面缺两行字面用例（`start > total_len`、`[0, total_len]`），`resume` + `NoRanges` 的组合没钉。
- 调度器测试开始读 `consts::MIN_SEGMENT_BYTES` ⇒ 破了它自己文档里"本模块不读 consts"的边界。
- `state.rs:1124` 残留旧的"账单"措辞；`:176` 说"上界"而 `:192` 说"既不是上界也不是下界"（自相矛盾），且"必然被别人再下"强于 E14 批准的"可能被重下"；跨续传累积那一格没写。
- `engine.rs`：`last_note` 是**全局一格**而失败按 worker 计 ⇒ 可能把甲的 transport 原因接到乙的"退避封顶"消息上；成功且不留断点时**先 save 再删**（白花一次 tmp+rename）；`StoreFailed`/`StateFailed` 直接 `Err` 把整个 `Report` 丢了（E13② 只规定 `Abort` 走 `Err`）；`stop_and_join` 最坏可阻塞 120 s 而文档没写；`min_seg = 0` 与 `workers` 无上限**没有下限闸**（`min_seg = 0` ⇒ 逐字节劈段 + 请求风暴）；`STATE_SAVE_MAX_INTERVAL_SECS` 那个读法让它实际成了**最小**间隔（名字会说谎）。
- `engine.rs:1512` 用被测公式现算期望值、`:1813` 用 `!is_empty()` 而精确条数可知、`:1815` 断的是自己两行前设的字面量（＝没断）⇒ 这三条**并入 8F-1**（与 C 那批"测试面"同源）。
- 评审 C 的 Minor 逐条**未随上下文保留**（那一发的原始输出没落盘）⇒ 上面 C 的部分只记了我自己复量到的四条 Important；最终评审时按 HEAD 的用例集重新扫一遍悬空引用与无守卫读数，别依赖这条账。

## 等用户点头的事（1、5 跨项目，2、3、4 是 dlm 自己的形状，6 是对外动作，8、9 是 T9 评审换来的）

> **2026-10-17 已裁定**（用户回"按推荐"⇒ 照下面每条**我原先写的建议**办）。逐条处置：
> **1 `T8-R5` ＝ v1 不动**，登记为已知残余，T10 实测后再判；**2 `T8-R6` ＝ 接受**，不为它开 `fetch` 每块上报；
> **3 `T8-R7` ＝ 接受**；**4 `ctrlc` ＝ 不加**，spec §6.6 的"非零码退出"照旧留白（本文件那条偏差声明即裁定后的口径）；
> **6 `git push` ＝ 不动**，等全分支最终评审之后再单独点头；
> **5（idm-recon 那台 probe-server 的同源缺陷）＝ 仍未答** —— 这一条我给的是"要不要我去复测并更正那份文档"而不是推荐，
> "按推荐"不覆盖它 ⇒ **挂着，那边仓库一个字都不动**。
> 另加一条 **T10 不跑 `IDMan.exe` 并排对照**（默认不做）。
> ⚠ 第 7 条（进度行数据面）**就是 E20① 那一裁，用户选了甲** ⇒ 全文见 **E21**，T9 已解锁。
>
> **2026-10-18 第二批已裁定**（用户回"**按照你的最优方案来**"⇒ 照我当天带的推荐办）。逐条处置：
> **1 `T8-R5` ＝ 维持 v1 不动**（"T10 实测后再判"那一判已判完：全速三跑 dup 12.8% / 14.3% / 20.8%、限速档 ≈ 20%、
> 续传第二轮 20.7%，对照 IDM 实测 +5.5% ⇒ 更好的策略存在，但改它要动 spec §2.1 的劈分规则，属 v2）。
> **2 ＝ 买那件仪器**（`slow-retry xN` 端到端）⇒ **T11 已派出**：给 `probe-server.mjs` 加一条"预读正常、其余全拒"的路由，
> 真二进制非静默跑一趟，断"真进度行里出现 `slow-retry x` 且数字 ≥ 1"。
> **3 ＝ 整条撤销，没有可修的**：我照抄执行者报的"node 对端 `socket_open.seq` 恒等于请求行 `conn` 减一"
> **实测不存在**（两种形状：单轮、以及杀掉再续的双轮，同一 `remotePort` 上两号**相等**）⇒ 详见计划 **E30 附记**
> 与提交 `f77bb45`。`/noranges` 的 1 MiB 封顶按我推荐**不动**（无 Range 的退化本就不该拿 48 MiB 去试）。
> **5 ＝ 都按推荐**：`--data-file` 的形状**留在 spec §10 R13**、不进 §5 正文；恢复的那发 C2 判据**不钉** `retries` 恒 0
> （那是裁定的结果不是观测，交付的用例也确实没钉）。
> **6 ＝ 维持"重复即报错"**，不动 `Cli` 的 `#[arg]` 属性面。
> ⚠ **仍缺一句的第 4 条：idm-recon 那份取证文档要不要我去复测更正**（第 5 条老账，同一个东西）——
> 我给的从来是提问不是推荐，所以两次"按推荐"都不覆盖它 ⇒ **那边仓库至今一个字没动**。
>
> 下面各条**原文照留**（它们是论证与实测记录，裁定只改状态、不改证据）。

1. **`T8-R5`（v1 不自作主张）**：终点帽（E14）之后仍有 **1.25×–1.74×** 的残余重复流量，成因已量清：帽只能拦"帽落下之后还要读的字节"，拦不到"调度器决定收紧时那一枪早已读过去的字节"（调度器只在一枪**结束时**才从 `Progress` 知道它读到哪儿），同一条在途尾巴被反复劈（实测一段被收紧 4 次）就把这一截乘了出来。要压到"每笔 ≤ 一块"必须改 **spec §2.1 的劈分规则**（我的建议：只把"已被收紧过的在途段"从**劈分候选集**里摘出，不动"剩余尾更长就动它"的排序；代价＝真卡死的尾巴多等一轮才被再劈）。**这是 IDM 的观察行为，改不改由你点。** 我手上的对照证据：idm-recon 实测 IDM 自己的丢弃重取只有 **+5.5%** ⇒ 更好的策略存在，但不属于 v1 该偷的懒。
2. **`T8-R6`（建议接受不修）**：`Report::trace` 里没有"收到帽时已经读到哪儿"这个观测量 ⇒ 引擎层"每笔收紧 ≤ 一块"的判据写不出来，除非让 `fetch` 每块上报进度（契约变更）。建议＝登记为已知缺口，不为它加仪器。
3. **`T8-R7`（建议接受不修）**：`window_budget` 是**整请求期限**、带 120 s 上界 ⇒ "极慢线 + `rate == 0`"那一格在任何预算分子修法下都拆不掉（8F-4a 已把它写成 `src/fetch.rs` 里的已知残余，而不是伪装成已修）。动它等于动 spec R7 的形状。
4. **`ctrlc` 要不要为 spec §6.6 破"只许 6 个依赖"**：任意时刻被杀都能续**已经证过**，缺的只是"非零码退出"那半句。建议＝不加，我把偏差写进计划勘误，而不是让 spec 继续声称做得到。
5. `idm-recon/tmp/probe-server.mjs` 原件带着同一个"不 `res.end()`"缺陷，且 `/flaky` 一样从不中断 body ⇒ `idm-recon/docs/25-blackbox-probe.md` 里基于它得到的时序/并发观测、以及"`/flaky` 中途断"这条取证说法，可能被污染。要不要我去复测并更正那份文档？（**dlm 自己的 harness 已修**；T10 场景 3 走 `/drop/<n>`，见裁定 7，这条不阻塞。）
6. **`git push`**：`sdd/dlm-kernel` 全程只在本地、无远程。要不要推、推到哪，由你单独点头。

**已自动消解的一条**（原记在这里）：`IDMan.exe` 上一会话留下的后台进程，2026-10-17 实测进程已不在，无需处理。另：Task 10 与 IDM 并排对照那一步要启动 `IDMan.exe`（本机构件被第三方改过、且环回链路观测强度有限），它只是参考、不作门禁——要不要跑也由你说一句。

7. **T9 的进度行要不要给引擎加"运行态只读快照"（E19②，评审 E 第 1 条 Important）**：spec §6.2 后半句要求进度行显示 `slow-retry xN`，而**全树没有任何数据面**（只有 `consts.rs:8` 的常量与 `engine.rs` 的退避表）；`Report` 只在结束时交回，`retries` 是全程总数、不含"第几轮慢重试"，运行中唯一外界观察是断点文件、而 `data_file` 那一支按 E13④ **不写断点**。⇒ 计划 `:885` 那行 `\r 45.2%  21.7 MiB/48.0 MiB  3.1 MiB/s  x4  dup 5.5%` 里的 **`dup 5.5%` 与 `slow-retry xN` 现在拿不到**（与"进度行没有 transport"是同一件事的两侧：**先要有出口，出口里还得有数**）。两支：**甲**＝给引擎加只读快照（共享原子量：已交付字节、`bytes_duplicated`、慢重试轮次；由已有的轮询那一拍更新，约一个窄面小轮）；**乙**＝把这两格从进度行砍掉并改 spec §5 的格式。我倾向**甲**（§5 那行是用户会看见的产品表面，砍格子等于偷偷改掉承诺；甲的成本可控）。**这条不阻塞 T8 收口**（四场景都不看进度行），但 **T9 派发前必须有裁定** ⇒ **用户 2026-10-17 选甲**（契约与判据已写成 **E21**，T9 解锁）。

8. **`--data-file` 换一个问题问法**（原先那组"CLI 收、产品行为不存在 ⇒ 三支选项"的问法**作废**——那条前提是我写错的，
   实测推翻过程见计划 **E28①**：真二进制 `dlm --url U --data-file F` 打出一条**无 `Range` 的 POST**、
   48 MiB 全落盘、退 **0**，试探零发、断点不读写 ⇒ 产品行为**完整**）。现在真的两问，都不影响正确性、只影响承诺面：
   ⑴ spec §5 要不要在**正文**写明带体形状走"整份单发、不试探、不读写断点、不可续"？
   我已按既有纪律只写进 **§10 R13**（正文不回改）；若你要它进正文，那是**改你复过的 spec**，需要你点头。
   ⑵ 恢复的那一发进程级判据（9B-4 的第③件）要不要顺带钉"重试链一次都不走"（即 `Report.retries` 恒 0）？
   **建议不钉**：`retries = 0` 是**裁定的结果**（那条路结构上没有重试，重发＝二次提交表单，spec R7 / E12⑧），
   把裁定写成观测判据是本项目的老毛病。⇒ 判据只钉**观察得到的**：对端只收到一条 POST、stdout 零 `\r`、退 0。
9. **重复给同一条长选项/别名的语义**。裁定 **E23 附记② / E24④** 的原句"`dlm /d A /d B` ⇒ `url == \"B\"` 且不报错"
   **在 clap 4 上不存在**（实测两种写法都退 2：`the argument '--url <URL>' cannot be used multiple times`；
   **等价性本身成立**，别名层没给自己单开一条结论）。我已裁**保持"重复即报错"**——理由与"`flags` 里本版不认识的位
   不许静默丢掉"同族：**"后者覆盖前者"等于吞掉用户给的一个值**。若你要的是"后写的算"，要动的是 `Cli` 的 `#[arg]`
   属性面（不是别名层的事），那是一次**行为变更** ⇒ 说一声我另开一轮。
