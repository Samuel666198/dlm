# dlm 内核 v1 — 执行状态与恢复说明

计划：[`2026-10-15-dlm-kernel.md`](./2026-10-15-dlm-kernel.md)（末尾 `## 勘误` E1–**E17** 为权威，与正文冲突时以勘误为准）
设计：[`../specs/2026-10-15-dlm-kernel-design.md`](../specs/2026-10-15-dlm-kernel-design.md)（§10 勘误 R1–R7，R5 已作废 R4）
分支 `sdd/dlm-kernel`，未 push、无远程。**2026-10-17 更新。**

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
| **T8 engine 主循环** | 🟡 **阶段1–5 全部已提交，任务级评审三发全回来、全判 Needs fixes ⇒ 修复轮 1 待发**（A `scheduler+state` 0C/3I、B `engine` 0C/2I、C `fetch+tests` 0C/4I） | `1c140e8..1e209a2`（实现）+ `bc8646d`/`b10ec51`/`7b368d2`/`acb3166`（阶段5B 与文档） |
| T9 CLI · T10 验收 | 未开始（T8 收口后按顺序走） | — |

计划文末 `## 勘误` 现有 **E1–E17**（E7–E17 都是执行期实测后才写进计划的，正文不回改）；设计稿 §10 现有 **R1–R7**。派发任何任务前先把与该任务相关的 E/R 条目抄进 brief。

**当前基线（2026-10-17 控制员实测于 `105fbf1`，撤销变异复原后复跑）**：`cargo test --lib` **162 passed / 0 failed**、`cargo test --test integration` **74 passed / 0 failed**（约 18 s）、`cargo clippy --all-targets -- -D warnings` **退出码 0**。下一任务的验收下限就是这个数：**只许增、不许删、不许改判据方向**。

## 下一个动作（2026-10-17 断点，照此顺序）

**T8 的修复轮按"能否独立交付"切两发，同任务任何时刻只允许一发在飞。**

1. **8F-1（不发新夹具、不动 `fetch` 契约）** —— 账本 `## Task 8` 评审 A/C 与裁定 **E17** 的落点：
   - **E17**：删 `engine.rs:457-461` 那笔 kill+续传重下账 + `overlap_with`（`:533`）+ 它的算术用例（`:1698`）；
     `state.rs::bytes_duplicated` 文档改写成"已知少报，上界 `WINDOW_BYTES × workers`（算术推论，写明出处）"。
   - **A1**：`scheduler.rs:395-402` `clamp_to_file` 会把**整条越过 EOF 的种子**夹成 `{999,999}` 这种幽灵已确认字节 ⇒
     照 `engine.rs:651` 那句 `.filter(|iv| iv.start <= last)` 修，并补字面用例；
     ⚠ **参照实现 `cover()` 不许再抄这条夹法**（E15 末尾的方法教训就是它）。
   - **A3**：`state.rs:194-195` 那句"与线上多打实测差 ≤ 7 KiB"**没有任何断言在这个量级上** ⇒ 改成实际断言强度。
   - **C-I-1…C-I-4**：账单用例补**量具活体针**（`conn_close_side` 条数 > 0 且 `written ≥ len`）并按它仪器的真实分辨率改写文档；
     `:2333` 那个 `8,206,900` 的**出处标错**（是实现者的 `duplicated`，不是线上 `excess`）与 `:2381` 那句没断言支撑的"3.1-3.5 MB"；
     `:2133`/`:2136` **两条不存在的用例名**（要么补用例要么改名）；`:2287`/`:2493` 两处**无守卫的线上读数**改用 `requests_at_least(n)`。
   - B 的 Minor 里**同源那三条**一并修：`engine.rs:1813` 用 `!is_empty()` 而精确条数可知、`:1815` 断的是自己两行前设的字面量（＝没断）、
     `:1512` 拿被测公式现算期望值。
2. **8F-2（这条要动 `fetch` 契约 + 新夹具，单独一发）** —— **E16** 退化模式：一枪下完剩余整条 body、预算按整条体量算（R7）、
   被掐如实 `Advanced`／未入账的前缀要么进账要么写明。**先写今天会红的用例**（需要能服务 > 一个窗口的"不认 Range"对端）。
3. 两发落地后发**一发 scoped 复审**（覆盖 8F-1 + 8F-2 的合并 diff）→ 记账 `Task 8: complete`。
4. **T9 CLI**（`FLAG_ADD_ONLY` 要在 `engine::run` 之前生效；`fresh_reason`/`message` 渲染时**不许再截断**，见裁定 9）、
   **T10 四场景验收**（`/drop/<n>` 与 `/sz/<n>s`，证据落 `docs/evidence/`；**多 worker 的真"杀了再续"用 spawn `dlm` 二进制 + kill 子进程**，
   评审 C 认可的就是这条路：断言 `fresh_reason.is_none()`、续传后每一枪线上起点 ≥ 覆盖集终点 + 1、最终 sha）。
5. 最后**全分支最终评审** + `.superpowers/sdd/` 清理 + 交一份"我替你做过的全部裁定"清单。
6. **仍等用户点头的一条**：`T8-R5`（残余 1.25×–1.74× 重复流量的唯一解法是改 spec §2.1 的劈分规则 —— 见下"待裁定"）。


## 恢复步骤（照做即可，别凭记忆）

```bash
cd /e/WorkSpace/projects/tool/dlm
SDD="C:/Users/ROG/.qoder-cn/plugins/cache/qoder-marketplace/superpowers/6.3.0/skills/subagent-driven-development/scripts"
git rev-parse --abbrev-ref HEAD          # 应为 sdd/dlm-kernel
git log --oneline -6                     # 顶端应是 docs（E16/E17）+ 105fbf1(E15) + acb3166 + 7b368d2 + b10ec51
git status --porcelain                   # 应为空（不空就是树里有在制品，先按 md5 备份再动）
cargo test --lib 2>&1 | tail -2          # 应为 162 passed
cargo test --test integration 2>&1 | tail -2   # 应为 74 passed
tail -40 .superpowers/sdd/2026-10-15-dlm-kernel/progress.md   # 末段是评审 C 的四条 Important 与 E16/E17 两条裁定
bash "$SDD/task-brief" docs/superpowers/plans/2026-10-15-dlm-kernel.md 8
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
8. **T8 的简报有三处断接口，一律按 E13 改**：`Scheduler` 没有续传播种门（要加 `resume(total, min_seg, done)` 并配"与 `new`＋逐个封存等价"的用例）；`Config.resume: Decision` 站不住 ⇒ **引擎自走 `load → probe → decide`**，`Config` 改 `{workers, min_seg, max_retries, verify_sha256, force_fresh, keep_state}`；Step 1 那条 `harness::replay(Range 序列)` 与 spec §7.2 相反 ⇒ 录**事件序列**回放给真 `Scheduler`，比"动作 vs 实际派发 vs 线上 Range"三道链接。
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

## 挂账的 minor（评审判"不阻塞"，最终评审 triage）

- **口令规则的残余绕过**：前导空格/Tab 的 URL（`" \thttp://…"`）仍过校验，而 reqwest 的 `Url::parse` 会剥掉它们 ⇒ 这类串真能明文发口令。修法二选一：`validate()` 改比 `Url::parse(…).scheme()`，或 T9 先拒带首尾空白的 url。
- 空 URL 规则无单测；`target_path` 只断言了 `file_name()`，没断言 `dir` 真被用上、没覆盖 `"download"` 兜底与 query/fragment 剥离。
- 计划字面要求的两个边角要人裁：无 path 的 URL（`https://example.com`）文件名取成 `example.com`；百分号编码名不解码（`a%20b.zip` 原样落盘）。
- `--cookie` / `--referer` 同样是凭据，明文 HTTP 下照发；spec §4 只管口令，本轮不自作规则。
- 依赖基线不可复现：`Cargo.toml` 用开区间，`.gitignore` 还忽略着 `Cargo.lock`（应用型 crate 通常该提交 lockfile）。
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

- `scheduler.rs:145-146` 公共文档指向一个**不存在的测试名**，且无条件承诺"逐条相同"（E15 已把这条承诺收窄）⇒ 随 A1 一起修（在 8F-1）。
- `scheduler.rs` 的 `resume` 面缺两行字面用例（`start > total_len`、`[0, total_len]`），`resume` + `NoRanges` 的组合没钉。
- 调度器测试开始读 `consts::MIN_SEGMENT_BYTES` ⇒ 破了它自己文档里"本模块不读 consts"的边界。
- `state.rs:1124` 残留旧的"账单"措辞；`:176` 说"上界"而 `:192` 说"既不是上界也不是下界"（自相矛盾），且"必然被别人再下"强于 E14 批准的"可能被重下"；跨续传累积那一格没写。
- `engine.rs`：`last_note` 是**全局一格**而失败按 worker 计 ⇒ 可能把甲的 transport 原因接到乙的"退避封顶"消息上；成功且不留断点时**先 save 再删**（白花一次 tmp+rename）；`StoreFailed`/`StateFailed` 直接 `Err` 把整个 `Report` 丢了（E13② 只规定 `Abort` 走 `Err`）；`stop_and_join` 最坏可阻塞 120 s 而文档没写；`min_seg = 0` 与 `workers` 无上限**没有下限闸**（`min_seg = 0` ⇒ 逐字节劈段 + 请求风暴）；`STATE_SAVE_MAX_INTERVAL_SECS` 那个读法让它实际成了**最小**间隔（名字会说谎）。
- `engine.rs:1512` 用被测公式现算期望值、`:1813` 用 `!is_empty()` 而精确条数可知、`:1815` 断的是自己两行前设的字面量（＝没断）⇒ 这三条**并入 8F-1**（与 C 那批"测试面"同源）。
- 评审 C 的 Minor 逐条**未随上下文保留**（那一发的原始输出没落盘）⇒ 上面 C 的部分只记了我自己复量到的四条 Important；最终评审时按 HEAD 的用例集重新扫一遍悬空引用与无守卫读数，别依赖这条账。

## 等用户点头的事（第 1、2 件跨项目、不动 dlm 之外任何文件；第 3 件是 dlm 自己的核心算法）

1. `idm-recon/tmp/probe-server.mjs` 原件带着同一个"不 `res.end()`"缺陷，且 `/flaky` 一样从不中断 body ⇒ `idm-recon/docs/25-blackbox-probe.md` 里基于它得到的时序/并发观测、以及"`/flaky` 中途断"这条取证说法，可能被污染。要不要我去复测并更正那份文档？
2. `IDMan.exe`（PID 6180）仍在后台运行 —— 上一会话经你批准启动的那次实测留下的，与 dlm 无关。要不要我关掉？
3. **`T8-R5`（v1 不自作主张）**：终点帽（E14）之后仍有 **1.25×–1.74×** 的残余重复流量，成因已量清：帽只能拦"帽落下之后还要读的字节"，拦不到"调度器决定收紧时那一枪早已读过去的字节"（调度器只在一枪**结束时**才从 `Progress` 知道它读到哪儿），同一条在途尾巴被反复劈（实测一段被收紧 4 次）就把这一截乘了出来。要压到"每笔 ≤ 一块"必须改 **spec §2.1 的劈分规则**（候选：已被收紧过的在途段不再作为劈分候选）。**这是 IDM 的观察行为，改不改由你点。** 我手上的对照证据：idm-recon 实测 IDM 自己的丢弃重取只有 **+5.5%** ⇒ 更好的策略存在，但不属于 v1 该偷的懒。
