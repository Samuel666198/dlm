# dlm 内核 v1 — 执行状态与恢复说明

计划：[`2026-10-15-dlm-kernel.md`](./2026-10-15-dlm-kernel.md)（末尾 `## 勘误` E1–E6 为权威，与正文冲突时以勘误为准）
设计：[`../specs/2026-10-15-dlm-kernel-design.md`](../specs/2026-10-15-dlm-kernel-design.md)（§10 勘误 R1–R5，R5 已作废 R4）
分支 `sdd/dlm-kernel`，未 push、无远程。**2026-10-15 收工断点。**

## 进度

| 任务 | 状态 | 提交 |
|---|---|---|
| T0 工具链 / 依赖 / 416 根因 | ✅ 评审通过（3 轮修复 + 1 发合并复审） | `bc376eb..e73b848` |
| T1 常量与任务模型 | ✅ 评审通过（0C/0I/6 minor） | `e73b848..fef292d` |
| **T2 Scheduler 纯状态机** | **下一个，未派发** —— 本项目的核心 | — |
| T3 store · T4 state · T5 resume · T6 probe · T7 fetch · T8 engine · T9 CLI · T10 验收 | 未开始 | — |

## 恢复步骤（照做即可，别凭记忆）

```bash
cd /e/WorkSpace/projects/tool/dlm
SDD="C:/Users/ROG/.qoder-cn/plugins/cache/qoder-marketplace/superpowers/6.3.0/skills/subagent-driven-development/scripts"
git rev-parse --abbrev-ref HEAD          # 应为 sdd/dlm-kernel
cat .superpowers/sdd/2026-10-15-dlm-kernel/progress.md | head -3   # 首行须指向本计划，否则不是本计划的账
bash "$SDD/task-brief" docs/superpowers/plans/2026-10-15-dlm-kernel.md 2
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

## 实测才知道的坑

- `rust-toolchain.toml` 不生效时报的是 `dlltool.exe: program not found`（本机全局默认是 gnu）。判据用 `rustup show`，**不动全局默认**。
- reqwest 的 feature 名是 `rustls`，不是 `rustls-tls`；写错会静默不加依赖、`Cargo.toml` 里什么都没有。
- `cargo … | tail -15; echo $?` 取到的是 `tail` 的退出码 —— 上一轮就这样误报过"编译成功"。
- Git Bash 把 `/x` 这类开关改写成盘符；给原生 node/curl 传路径时用 `MSYS2_ARG_CONV_EXCL='*'`，日志路径写仓库内 `target/`，别写 `/tmp`。
- harness 限速生效后有条尾巴：同一条 keep-alive 连接上的**第二个**请求要等一拍（实测 201/203 ms）。T7 的窗口超时判据若贴着 200 ms 量纲会被它干扰 —— 时序预算要算进去。
- `pipeline` 里取退出码要单独跑；`node --check` 只验语法不验行为。

## 挂账的 minor（评审判"不阻塞"，最终评审 triage）

- **口令规则的残余绕过**：前导空格/Tab 的 URL（`" \thttp://…"`）仍过校验，而 reqwest 的 `Url::parse` 会剥掉它们 ⇒ 这类串真能明文发口令。修法二选一：`validate()` 改比 `Url::parse(…).scheme()`，或 T9 先拒带首尾空白的 url。
- 空 URL 规则无单测；`target_path` 只断言了 `file_name()`，没断言 `dir` 真被用上、没覆盖 `"download"` 兜底与 query/fragment 剥离。
- 计划字面要求的两个边角要人裁：无 path 的 URL（`https://example.com`）文件名取成 `example.com`；百分号编码名不解码（`a%20b.zip` 原样落盘）。
- `--cookie` / `--referer` 同样是凭据，明文 HTTP 下照发；spec §4 只管口令，本轮不自作规则。
- 依赖基线不可复现：`Cargo.toml` 用开区间，`.gitignore` 还忽略着 `Cargo.lock`（应用型 crate 通常该提交 lockfile）。
- 计划勘误文案两处错标：E2 把"判定段"标成 Task 6 Step 3（实为 Task 0 Step 3）；E4 被取代那段把"停在 body 中间"写成 §7 场景 2（应是场景 3，E5 用的编号才对）。

## 等用户点头的两件跨项目事（不动 dlm 之外任何文件）

1. `idm-recon/tmp/probe-server.mjs` 原件带着同一个"不 `res.end()`"缺陷，且 `/flaky` 一样从不中断 body ⇒ `idm-recon/docs/25-blackbox-probe.md` 里基于它得到的时序/并发观测、以及"`/flaky` 中途断"这条取证说法，可能被污染。要不要我去复测并更正那份文档？
2. `IDMan.exe`（PID 6180）仍在后台运行 —— 上一会话经你批准启动的那次实测留下的，与 dlm 无关。要不要我关掉？
