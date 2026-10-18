//! 终端那一行的渲染层（Task 9B-2）：**纯函数、只依赖 std、一个字节的 IO 都不做**。
//!
//! 依据：spec §5 的进度行样例（计划 `:885` 那行 `\r 45.2%  21.7 MiB/48.0 MiB  3.1 MiB/s  x4  dup 5.5%`）、
//! 勘误 **E25** 的七条渲染口径（单位/取整/零分母/整格缺席/`x{active}`/措辞/最终行取路径）、
//! **E22** 里管渲染的那几条（①那对 `MiB/MiB` 用 `covered_bytes/total_len`、②`total_len == 0` 是真·空文件
//! 而且不许造假分母、⑤`slow-retry` 与 `active_workers` 各自独立、⑥慢重试那一格的口径、⑦`dup %` 的分母
//! 是 `run_delivered`）、**E20⑦⑧⑨**（退出码两档且判据只有 `!ok`、`retries`/`duplicated` 是跨断点文件的账、
//! `trace` 不许进 stdout）、**E23⑥/E24③**（摘要逐字段打、口令那一格连 `[redacted]` 都不许出现）。
//!
//! ## 为什么交回 `String` 而不是直接往屏幕上打（**E25** 开头那句）
//! `report` 在 spec §2 的模块表里"允许依赖＝无"。把 IO 留在 `main` 换来两件事：每一格都能拿**字面量**
//! 断言（不必截 stdout、不必为"渲染对不对"再造一条进程内管道），以及渲染器可以在**引擎给不起的输入组合**
//! 上被断言 —— `ok = true` + `message = Some(..)` 那一格引擎侧今天没有确定性夹具（**E20⑦** 末尾的具名
//! 欠账），但它是退出码判据唯一会被读错的方向，只有纯函数能把它钉住（见 [`outcome`] 交回第二个值的原因）。
//!
//! ## 三个入口的分工
//! - [`tick`]：**本次运行**的瞬时量。速率与 `dup` 的分子分母都是同一拍取的增量（**E21③④**），
//!   与 `Report` 那两本跨断点文件的账**故意不同寿命**，两处不许互相"对齐"。
//! - [`summary`]：`--no-start` / `/a` 的任务摘要（**E20④**：引擎明写不看 bit0，所以打印摘要这一步之后
//!   调用方必须收工；摘要里"口令"这个话题整个不出现）。
//! - [`outcome`]：`Report` 的最终结果 —— 跨断点文件那两本账在这里才有归宿（**E20⑧/E25⑥**），
//!   带 `--data-file` 的那一支恒 0 因而整格不出现。

use crate::consts::{FLAG_ADD_ONLY, FLAG_IGNORE_MTIME, FLAG_KEEP_STATE};
use crate::engine::{Report, RunSnapshot};
use crate::task::Task;

/// 一 MiB 的字节数（**E25①** 换算口径：`1 MiB = 1_048_576 B`，单位只有这一档，不做自适应）。
const MIB: u64 = 1_048_576;

/// 摘要里"这个字段没给"那一格的写法（**E23⑥**：逐字段打 ⇒ 缺字段要显式说"未给"，
/// 否则"这一格不存在"与"值就是空串"读起来一样）。
const ABSENT: &str = "（未给）";

/// spec §5 的 flags 表**只定义了这三位**（bit0/bit1/bit2，**E20③** 数得出来就是三个）。
/// 摘要那一格要靠它把"本版不认识但确实置起了"的位区分出来 —— 静默丢掉会变成
/// "摘要说没置、断点文件里却置了"。
const KNOWN_FLAGS: u32 = FLAG_ADD_ONLY | FLAG_IGNORE_MTIME | FLAG_KEEP_STATE;

/// 字节 → 一位小数的 MiB 数字（不含单位；**E25①**）。
///
/// 算术全在整数上做（`bytes × 10 ÷ 1 MiB` 之后再拆个位），于是"截断而不是四舍五入"这一条
/// 与百分比那一格同源（**E25②**），也不依赖浮点表示。中间量取 `u128`：`bytes × 10` 在 `u64`
/// 上会在 1.6 EiB 那一档上溢（debug 下直接 panic），而下溢不可能发生 —— 用例表里 G 行就是那根针。
fn mib(bytes: u64) -> String {
    let tenths = (bytes as u128) * 10 / u128::from(MIB);
    format!("{}.{}", tenths / 10, tenths % 10)
}

/// 百分比：**向下**取整到 0.1（**E25②** 给的那个形状 `分子 × 1000 ÷ 分母`，这里用整数实现）。
///
/// `分母 == 0` 定死印 `100.0%`（**E25③**：`total_len == 0` 是**真·空文件**，`bytes */0` 那格有用例；
/// 覆盖集为空且全长为 0 时"这个文件已经下完了"就是 100%，与 `completion_verdict` 的
/// `on_disk == total_len == 0` 同口径）⇒ 这里既不许 `unwrap_or(1)` 造假分母（**E22②**），
/// 也不许让 `0 ÷ 0` 走到除法上。
///
/// ⚠ 那一支**只属于文件长度**：`dup` 那格的分母为 0 时的规则是"整格不印"（**E25④**），
/// 所以调用方必须先确认 `run_delivered > 0`（见 [`tick`] 里那句守卫）。
fn percent(numerator: u64, denominator: u64) -> String {
    let scaled = if denominator == 0 {
        1000
    } else {
        (numerator as u128) * 1000 / u128::from(denominator)
    };
    format!("{}.{}%", scaled / 10, scaled % 10)
}

/// 一拍进度行（spec §5 那一行，**E22①②⑥⑦ + E25①–⑤**）。
///
/// 格序：`<百分比>  <已下 MiB>/<全长 MiB>  <速率> MiB/s  x<在途>/<总连接>[  dup <重复%>][  slow-retry x<N>]`。
/// 三格缺席规则各归各的（**E25④⑤**）：`dup` 只在 `run_delivered == 0` 时不印、`slow-retry` 只在
/// `slow_retry_rounds == 0` 时不印，而 `x{active_workers}` **照字面印**、慢重试期合法地是 `x0`
/// —— 不许为了"看着还在动"把这三格绑成"要么全有要么全无"。
///
/// 那对 `MiB/MiB` 用 `covered_bytes / total_len`（**E22① / E21 附记 C5**：这一对说的是"文件下到哪儿了"，
/// 天然含预读与上一轮续传来的字节）；`run_delivered` / `run_bytes_duplicated` 只服务速率与 `dup`，
/// 是**本次运行**的流量（与 `Report` 那两本跨断点文件的账故意不同寿命，**E21③**）。
///
/// 交回的串**不带** `\r`：那一发属于"打到终端"那一层（`main.rs`），不属于渲染层 —— 带上它就没法
/// 拿字面量比这一行（**E25** 开头那句的立场）。
///
/// ## `x4/8` 而不是 spec §5 样例里的 `x4` —— 这一格是**刻意的偏离**，理由如下
/// §5 那一行样例只印了一个数，而 9A 已提交的那半份契约（`RunSnapshot::workers_total` 的字段文档）
/// 明写它是"进度行里那个 `x4` 的**分母**" ⇒ 样例印的是这一对的分子，分母本来就在契约里等着被印。
/// 印成 `x{active}/{total}` 换来两件按样例给不起的事：
/// ① 慢重试期 `active_workers` **合法地为 0**（**E21⑤**），光一个 `x0` 会被读成"这条任务只有 0 条连接"，
/// 而 `x0/8` 说的是"8 条里此刻一条都没在途" —— 那一格正是 §6.2 要用户能看出来的那种状态；
/// ② **E25⑤** 要求 `dup`/`slow-retry` 各自独立出现或不出现，读者要能判断"这一行少掉的是哪一格"，
/// 就得有一格恒在的长度参照。
/// 钉住它的位置：本文件 A 行期望的 `x4/8`，与
/// `tests/integration.rs::a_real_run_hands_the_renderer_cells_a_human_can_read` 里
/// 那句 `assert_eq!(workers, "4")`。
pub fn tick(snap: &RunSnapshot) -> String {
    let mut line = format!(
        "{}  {} MiB/{} MiB  {} MiB/s  x{}/{}",
        percent(snap.covered_bytes, snap.total_len),
        mib(snap.covered_bytes),
        mib(snap.total_len),
        mib(snap.instant_bytes_per_sec),
        snap.active_workers,
        snap.workers_total,
    );
    // E25④：分母为零时任何数字都是假数 ⇒ 整格不出现（隔壁 `x{active}` 那一格反过来，0 也照印）。
    if snap.run_delivered > 0 {
        line.push_str(&format!(
            "  dup {}",
            percent(snap.run_bytes_duplicated, snap.run_delivered)
        ));
    }
    // E22⑥/E25④：`rounds == 0` 的语义是"还没排下慢重试"，印 `slow-retry x0` 会被读成"进过 0 轮"。
    if snap.slow_retry_rounds > 0 {
        line.push_str(&format!("  slow-retry x{}", snap.slow_retry_rounds));
    }
    line
}

/// `--no-start` / `/a` 的任务摘要（**E20④/E23⑥/E24③**）：逐字段打，且**口令那一格整个不出现** ——
/// 连 `Task`/`Cli` 手写 `Debug` 里那个 `[redacted]` 也不要（那是日志防线，不是产品文案；
/// 摘要里"带了口令"这件事本身都不该被说出来，因为没有任何读者需要知道）。
///
/// ## 为什么不复用 `Debug`（**E23⑥** 的正面理由）
/// `{:?}` 的形状是"给维护者读的结构体转储"：它把 `flags` 交成一个数字（`flags: 5`），
/// 而用户要读的是"这个任务会怎么被登记"；它把口令交成 `[redacted]`，而摘要连这个话题都不该有。
/// 所以这里**手写每一格**，`Debug` 那道防线留在原处不动。
///
/// ## `flags` 那一格为什么是位名而不是数字
/// spec §5 的位义写在表上（bit0 只入队不下 / bit1 忽略 Last-Modified / bit2 完成后保留断点），
/// 数字要求读者去背位序 ⇒ 只有**置了的位**按名出现，没置的一个不出现（"哪些没置"在摘要里没有价值，
/// 用户没写就是没有）。本版不认识的位**不静默丢掉**（见 [`flags_cell`]）。
///
/// ## 交回的串不带行尾换行
/// 打印那一发属于 `main.rs`；渲染层交回可断言的字面量（**E25** 开头那句的立场，与 [`tick`] 同一规则）。
pub fn summary(task: &Task) -> String {
    let lines: Vec<String> = vec![
        // 第一格要说清"这一发只登记、不发起"，否则用户会以为摘要之后下载已经在跑了（E20④）。
        "任务摘要（--no-start：只登记，本次不发起任何请求）".to_string(),
        format!("URL：{}", task.url),
        format!("输出目录：{}", task.dir.display()),
        format!("输出文件名：{}", out_name_cell(task)),
        // 落点单独一格：它是 `dir` 与文件名**合起来**才是答案的那一格（`Task::target_path()`），
        // 只打前两个字段会逼读者自己拼，而拼错的那一发是直接写错文件。
        format!("落点：{}", task.target_path().display()),
        opt_cell("来源头（--referer）：", task.referer.as_deref()),
        opt_cell("Cookie（--cookie）：", task.cookies.as_deref()),
        match &task.data_file {
            Some(p) => format!("请求体文件（--data-file）：{}", p.display()),
            None => format!("请求体文件（--data-file）：{ABSENT}"),
        },
        opt_cell("用户（--user）：", task.user.as_deref()),
        // **口令那一格不在这里，也不在别处**（E23⑥/E24③）：`Task` 有 `password`，摘要没有这一行。
        flags_cell(task.flags),
    ];
    lines.join("\n")
}

/// "可选字符串字段"那一格：有值原样打，没值打 [`ABSENT`]。
fn opt_cell(label: &str, value: Option<&str>) -> String {
    match value {
        Some(v) => format!("{label}{v}"),
        None => format!("{label}{ABSENT}"),
    }
}

/// 输出文件名那一格。
///
/// 为什么不能只打 `task.name`：`name` 是 `Option`，而**落点永远有一个名字** ——
/// `Task::target_path()` 在没给 `--out-name` 时取 URL 末段（再兜到 `download`）。
/// 把 `None` 打成"未给"会让摘要说"这一格没定"，而它其实定了，只是不是用户定的；
/// 反过来把推导出来的名字当成用户给的和 `Some(name)` 混为一谈也不对 ⇒ 两支写法不同。
/// （这也是本文件唯一一处**允许**在 `ABSENT` 之外出现"未"字的格子：那两串不是同一个记号，
/// 用例 `the_summary_marks_absent_fields_and_no_flag_bits` 数的就是精确的 `ABSENT` 串。）
fn out_name_cell(task: &Task) -> String {
    match &task.name {
        Some(n) => n.clone(),
        None => match task.target_path().file_name() {
            Some(derived) => format!(
                "{}（未显式指定，取 URL 末段）",
                derived.to_string_lossy()
            ),
            // 构造上到不了（`target_path()` 永远 `join` 出一个非空末段），留 [`ABSENT`] 是
            // 不想让这一格在真到不了的时候编出一个名字。
            None => ABSENT.to_string(),
        },
    }
}

/// `flags` 那一格：按名字列出**置了的位**，一个都没置就明说"一个都没置"。
///
/// 数字一个都不打（**E23⑥** 的"别把结构体摆出来"）；本版不认识的位如果置了，那一格要说
/// "另有本版不认识的位"并**不显示其值** —— 静默丢掉会让摘要在"这个任务的登记形状"上说谎，
/// 而 `flags` 会随 `TaskView` 进断点文件、下一轮的比对读的是那一份。
fn flags_cell(flags: u32) -> String {
    let mut named: Vec<&str> = Vec::new();
    if flags & FLAG_ADD_ONLY != 0 {
        named.push("bit0 只入队不下");
    }
    if flags & FLAG_IGNORE_MTIME != 0 {
        named.push("bit1 忽略 Last-Modified");
    }
    if flags & FLAG_KEEP_STATE != 0 {
        named.push("bit2 完成后保留断点");
    }
    if flags & !KNOWN_FLAGS != 0 {
        named.push("另有本版不认识的位（不显示其值）");
    }
    if named.is_empty() {
        return "标志位（flags）：一个都没置".to_string();
    }
    format!("标志位（flags）：{}", named.join("、"))
}

/// 最终结果。**交回 `(文本, 该不该按成功退出)`**：
///
/// 判据只有 `rep.ok`（**E20⑦**：`message.is_some()` 只是"有一句要打印"，`ok = true` 时它也可以是
/// `Some` —— 收尾删断点删不掉那一支）。之所以由这里交回而不是让 `main` 自己读 `rep.ok`：
/// `ok = true` + `message = Some(..)` 那一组合**引擎侧今天造不出来**（**E20⑦** 末尾的具名欠账：
/// 要它得让 `fs::remove_file` 以一个非 `NotFound` 的错误失败，只有 Windows 共享冲突那类竞态形状），
/// 而"退出码读错方向"恰恰只在这一格才暴露 ⇒ 能对手搭 `Report` 断言的位置就是唯一可钉的位置。
///
/// `trace` 一个字都不打（**E20⑨**：它每事件一条、无上限）。措辞口径见 **E25⑥**：
/// `retries`/`duplicated` 说成"这个断点文件从建立起一共…"，`duplicated` **不是流量账单**（**E14③**），
/// 而 `--data-file` 那一支这两格（连同没有覆盖账的 `confirmed`）干脆不出现。
pub fn outcome(rep: &Report, task: &Task) -> (String, bool) {
    // **有没有那两本跨轮账**由任务形状决定，不是由数字决定：`Task::segmentable()` 取假的那一支
    // （带 `--data-file`）既不读也不写断点（**E13④**），于是 `retries`/`duplicated` 是硬编码的 0，
    // `confirmed` 也恒 0（`Report::confirmed` 的字段文档明写那一支"没有覆盖账"）。
    // ⇒ 那一路上这三格**整格不出现**，而不是印 0 冒充测过（**E25⑥** 末句）。
    let billed = task.segmentable();
    let verdict = if rep.ok { "下载已完成" } else { "下载未完成" };
    let mut lines: Vec<String> = vec![format!(
        "{verdict}  目标：{}  全长 {}，落盘 {}{}",
        // 落点取 `task.target_path()`（**E25⑦/E20⑨**）：`Report` 没有路径字段，
        // 而它与 `Store::final_path()` 同值是构造上的事实 ⇒ 不为此改 `Report` 的形状。
        task.target_path().display(),
        bytes(rep.bytes_total),
        bytes(rep.delivered),
        // 两本账分开报（**E7**）：`delivered` 是盘上真有多少字节，`confirmed` 是归属已定的那些，
        // 两者不等是常态（洞、在途窗口、上一轮残留），合成一格就报不出这个区别。
        if billed { format!("，归属已定 {}", bytes(rep.confirmed)) } else { String::new() },
    )];
    if billed {
        lines.push(format!(
            "这个断点文件从建立起一共重试 {} 次，归属之外可能被重下 {}",
            rep.retries,
            bytes(rep.duplicated)
        ));
    }
    // 耗时用 `ms` 一档到底（与 **E25①** 的"不做自适应单位"同一个立场）：
    // 换算成 `s`/`min` 就多一套要钉的字面量，而这一格的价值恰恰是"能和日志时间戳对上"。
    lines.push(format!("用时 {} ms", rep.elapsed.as_millis()));
    // `fresh_reason` 必须活着走到用户眼前（`resume.rs` 的承诺：中途重来的原因不许留在引擎里）。
    // 带 `--data-file` 那一支由构造给 `None`（`engine.rs` 的 `run_whole_once`），所以这里不需要
    // 第二条 `billed` 守卫：两支队伍在这一格上不会给出不同的读法。
    if let Some(reason) = &rep.fresh_reason {
        lines.push(format!("从头重来的原因：{reason}"));
    }
    // `message` **原样**打，不截断、不拼接（裁定 9：`FailKind::Transport` 的载荷在生产侧已是
    // 单行 ≤200 字符，两处截断会互相吃掉信息）。它的出现与否**不参与退出码**（**E20⑦**）。
    if let Some(msg) = &rep.message {
        lines.push(format!("说明：{msg}"));
    }
    // `rep.trace` 一个字都不交（**E20⑨**：每事件一条、无上限的调试载体，`--quiet` 之外也不许进 stdout）。
    // 退出档：判据**只有** `rep.ok`。
    (lines.join("\n"), rep.ok)
}

/// 最终行的字节写法：**精确字节数 + ` B`**，不做自适应单位。
///
/// 与 [`mib`] 的分工是刻意的：进度行要的是"一眼看出下到哪儿"（**E25①** 定死 MiB 一档、一位小数），
/// 最终行要的是"能对得上对端日志的那个数字" ⇒ 拿一位小数的 MiB 去报 50 MiB 会把末尾几 KB 抹掉，
/// 而那一格恰恰是用户要拿去校验的数。用例里 `全长 1024 B` 钉的就是这个形状。
fn bytes(value: u64) -> String {
    format!("{value} B")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::{FLAG_ADD_ONLY, FLAG_IGNORE_MTIME, FLAG_KEEP_STATE};
    use crate::engine::TraceStep;
    use crate::scheduler::Interval;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    const MIB_U64: u64 = 1_048_576;

    /// 手搭一台 `Report`。**`trace` 故意留一条**：三个入口都只该读那八个量，
    /// 断言里"渲染文本不含 `Seeded`"就是在钉这一条（**E20⑨**）。
    fn report_with(
        ok: bool,
        retries: u32,
        duplicated: u64,
        fresh: Option<&str>,
        msg: Option<&str>,
    ) -> Report {
        Report {
            bytes_total: 1024,
            delivered: 1024,
            confirmed: 1024,
            duplicated,
            retries,
            elapsed: Duration::from_millis(1500),
            ok,
            trace: vec![TraceStep::Seeded(vec![Interval { start: 0, end: 5 }])],
            fresh_reason: fresh.map(str::to_string),
            message: msg.map(str::to_string),
        }
    }

    fn sample_task(data_file: bool) -> Task {
        Task {
            url: "https://example.invalid/files/a.bin".to_string(),
            referer: Some("https://ref.example/page".to_string()),
            cookies: Some("a=1; b=2".to_string()),
            data_file: data_file.then(|| PathBuf::from("D:/body/post.bin")),
            user: Some("alice".to_string()),
            password: Some("hunter2".to_string()),
            dir: PathBuf::from("D:/downloads"),
            name: Some("renamed.bin".to_string()),
            flags: FLAG_ADD_ONLY | FLAG_KEEP_STATE,
        }
    }

    /// 路径那两格的期望由 `Path::display` 现拼（Windows 把 `/` 归一成 `\`，那个形状由
    /// `task.rs` 与 `store.rs` 的用例钉，本文件不复制一份），其余每一格都是**手算的字面量**。
    fn path_cell(p: &Path) -> String {
        format!("{}", p.display())
    }

    /// E 行写 `1 * MIB_U64` 而不是 `MIB_U64` 是**刻意的**：这张表的每一行都按 `n * MIB_U64` 排，
    /// 于是"这一格取几 MiB"与期望串里那个数一眼对得上（`64 * MIB_U64` ↔ `64.0 MiB`）。
    /// 单独约掉那一支的 `1 *` 会让表里出现两种形状，比多一个 lint 更贵。
    #[allow(clippy::identity_op)]
    #[test]
    fn the_tick_line_is_a_table_of_hand_computed_cells() {
        let cases: Vec<(RunSnapshot, &str)> = vec![
            // A 行＝计划 `:885` 那行样例的形状，只是把 `x4` 补上分母（`RunSnapshot::workers_total`
            // 的字段文档明写它是"进度行里那个 x4 的分母"）。逐格手算：
            //   12 MiB / 48 MiB ⇒ 12*1000/48 = 250 ⇒ 25.0%（不是 25.0 的四舍五入问题，整除）
            //   12 MiB ⇒ 12*1048576*10/1048576 = 120 ⇒ 12.0；48 MiB ⇒ 480 ⇒ 48.0
            //   3_300_000 B/s ⇒ 33_000_000/1_048_576 = 31.47 ⇒ 截断 31 ⇒ 3.1（3.2 MiB = 3_355_443 还没到）
            //   dup ⇒ 55_000*1000/1_000_000 = 55 ⇒ 5.5%（E22⑦：分母是 run_delivered）
            (
                RunSnapshot {
                    total_len: 48 * MIB_U64,
                    covered_bytes: 12 * MIB_U64,
                    run_delivered: 1_000_000,
                    run_bytes_duplicated: 55_000,
                    instant_bytes_per_sec: 3_300_000,
                    active_workers: 4,
                    workers_total: 8,
                    slow_retry_rounds: 0,
                },
                "25.0%  12.0 MiB/48.0 MiB  3.1 MiB/s  x4/8  dup 5.5%",
            ),
            // B 行＝三件事同时钉：百分比**截断**、单位不足 0.1 就印 0.0、`dup` 整格缺席。
            //   3 MiB / 13 MiB ⇒ 3000/13 = 230.77 ⇒ 截断 230 ⇒ 23.0%（四舍五入会报成 23.1%，
            //   而 23.1% 是一个**还没下到**的位置 ⇒ E25② 禁它）
            //   50_000 B/s ⇒ 500_000/1_048_576 = 0.477 ⇒ 0 ⇒ "0.0 MiB/s"（E25① 不做自适应单位）
            //   run_delivered = 0 ⇒ 分母为零，任何数字都是假数 ⇒ 整格不印（E25④）
            (
                RunSnapshot {
                    total_len: 13 * MIB_U64,
                    covered_bytes: 3 * MIB_U64,
                    run_delivered: 0,
                    run_bytes_duplicated: 0,
                    instant_bytes_per_sec: 50_000,
                    active_workers: 1,
                    workers_total: 2,
                    slow_retry_rounds: 0,
                },
                "23.0%  3.0 MiB/13.0 MiB  0.0 MiB/s  x1/2",
            ),
            // C 行＝`total_len == 0` 是**真·空文件**（E21②/E22②/E25③）：不许 panic、不许 `NaN`、
            // 不许拿 `unwrap_or(1)` 造假分母；定死印 `100.0%`（覆盖集为空且全长为 0 ⇒ 已经下完了）。
            (
                RunSnapshot {
                    total_len: 0,
                    covered_bytes: 0,
                    run_delivered: 0,
                    run_bytes_duplicated: 0,
                    instant_bytes_per_sec: 0,
                    active_workers: 0,
                    workers_total: 1,
                    slow_retry_rounds: 0,
                },
                "100.0%  0.0 MiB/0.0 MiB  0.0 MiB/s  x0/1",
            ),
            // D 行＝E25⑤ 那一支"诚实的组合"：全场都在 30 s 退避 ⇒ `x0` 照字面印，
            // `slow-retry x2` 照印，而 `dup 0.0%` 也照印（分母 `run_delivered > 0 ⇒ 有资格报数`）。
            // 三格各自独立出现/不出现 ⇒ 谁也不许替别人作保。
            (
                RunSnapshot {
                    total_len: 8 * MIB_U64,
                    covered_bytes: 4 * MIB_U64,
                    run_delivered: 1_000_000,
                    run_bytes_duplicated: 0,
                    instant_bytes_per_sec: 0,
                    active_workers: 0,
                    workers_total: 8,
                    slow_retry_rounds: 2,
                },
                "50.0%  4.0 MiB/8.0 MiB  0.0 MiB/s  x0/8  dup 0.0%  slow-retry x2",
            ),
            // E 行＝慢重试第 1 轮 + dup 的截断：123_456*1000/2_000_000 = 61.728 ⇒ 61 ⇒ 6.1%
            // （四舍五入会报 6.2%）。百分比：1 MiB / 64 MiB ⇒ 1000/64 = 15.625 ⇒ 15 ⇒ 1.5%。
            (
                RunSnapshot {
                    total_len: 64 * MIB_U64,
                    covered_bytes: 1 * MIB_U64,
                    run_delivered: 2_000_000,
                    run_bytes_duplicated: 123_456,
                    instant_bytes_per_sec: 4 * MIB_U64,
                    active_workers: 2,
                    workers_total: 8,
                    slow_retry_rounds: 1,
                },
                "1.5%  1.0 MiB/64.0 MiB  4.0 MiB/s  x2/8  dup 6.1%  slow-retry x1",
            ),
            // F 行＝小文件的真实形状（两侧都不足 1 MiB）：100_000*10/1_048_576 = 0.95 ⇒ 0 ⇒ 0.0 MiB，
            // 200_000*10/1_048_576 = 1.907 ⇒ 1 ⇒ 0.1 MiB。百分比整除：100_000*1000/200_000 = 500 ⇒ 50.0%。
            // 这一格存在的理由：自适应单位会把"0.0 MiB"读成"什么都没下"，而这里**确实**下到了半程。
            (
                RunSnapshot {
                    total_len: 200_000,
                    covered_bytes: 100_000,
                    run_delivered: 0,
                    run_bytes_duplicated: 0,
                    instant_bytes_per_sec: 0,
                    active_workers: 0,
                    workers_total: 1,
                    slow_retry_rounds: 0,
                },
                "50.0%  0.0 MiB/0.1 MiB  0.0 MiB/s  x0/1",
            ),
            // G 行＝不回绕。全部取 2 的幂，于是每个期望都能手算：
            //   (2^62)*10/2^20 = 10*2^42 = 43_980_465_111_040 ⇒ 4398046511104.0
            //   (2^63)*10/2^20 = 10*2^43 = 87_960_930_222_080 ⇒ 8796093022208.0
            //   (2^62)*1000/2^63 = 500 ⇒ 50.0%；dup 那一格同一个商 ⇒ 50.0%
            // 用 `u64` 中间量算 `*10` 与 `*1000` 在本行直接上溢（debug 下 panic）⇒ 这一格是溢出针。
            (
                RunSnapshot {
                    total_len: 1u64 << 63,
                    covered_bytes: 1u64 << 62,
                    run_delivered: 1u64 << 63,
                    run_bytes_duplicated: 1u64 << 62,
                    instant_bytes_per_sec: 1u64 << 63,
                    active_workers: 8,
                    workers_total: 8,
                    slow_retry_rounds: 0,
                },
                "50.0%  4398046511104.0 MiB/8796093022208.0 MiB  8796093022208.0 MiB/s  x8/8  dup 50.0%",
            ),
            // H 行＝分子超过分母那一支**不夹到 100%**：1500*1000/1000 = 1500 ⇒ 150.0%。
            // 夹起来会把"分子分母不同源/账对不上"（E22⑦ 点名的假小与 >100%）藏成一格好看的数。
            (
                RunSnapshot {
                    total_len: 4 * MIB_U64,
                    covered_bytes: 4 * MIB_U64,
                    run_delivered: 1000,
                    run_bytes_duplicated: 1500,
                    instant_bytes_per_sec: 0,
                    active_workers: 0,
                    workers_total: 1,
                    slow_retry_rounds: 0,
                },
                "100.0%  4.0 MiB/4.0 MiB  0.0 MiB/s  x0/1  dup 150.0%",
            ),
        ];
        for (n, (snap, want)) in cases.iter().enumerate() {
            let got = tick(snap);
            assert_eq!(got.as_str(), *want, "{n} 行的进度行与手算的期望不符：{got}");
            assert!(!got.contains("NaN") && !got.contains("inf"), "{n} 行不许出现 NaN/inf（E22②）：{got}");
            assert!(!got.contains("Seeded") && !got.contains("TraceStep"), "{n} 行不许带 trace 载体（E20⑨）：{got}");
        }
    }

    #[test]
    fn the_empty_file_row_and_the_zero_denominator_rows_omit_the_right_cells() {
        // E25④ 的两格缺席规则单独钉一遍（表里也钉着，这里是为了让"谁缺席"这句话有个独立落点）：
        // 0 分母那一格印出来的一切数字都是假数，而 `slow_retry_rounds == 0` 说的是"还没进慢重试"，
        // 印 `slow-retry x0` 会被读成"进过 0 轮"。
        let no_dup = tick(&RunSnapshot {
            total_len: 16 * MIB_U64,
            covered_bytes: 15 * MIB_U64,
            run_delivered: 0,
            run_bytes_duplicated: 700,
            instant_bytes_per_sec: 9 * MIB_U64,
            active_workers: 3,
            workers_total: 8,
            slow_retry_rounds: 0,
        });
        assert_eq!(no_dup, "93.7%  15.0 MiB/16.0 MiB  9.0 MiB/s  x3/8");
        assert!(!no_dup.contains("dup"), "run_delivered 为 0 时 dup 整格不许出现：{no_dup}");
        assert!(!no_dup.contains("slow-retry"), "还没进慢重试时那一格不许出现：{no_dup}");
        // 上面那一行的百分比手算：15*1000/16 = 937.5 ⇒ 截断 937 ⇒ 93.7%（四舍五入会报 93.8%）。
    }

    #[test]
    fn the_no_start_summary_prints_every_field_but_the_password_cell() {
        let t = sample_task(false);
        let got = summary(&t);
        let want = [
            "任务摘要（--no-start：只登记，本次不发起任何请求）",
            "URL：https://example.invalid/files/a.bin",
            &format!("输出目录：{}", path_cell(Path::new("D:/downloads"))),
            "输出文件名：renamed.bin",
            &format!(
                "落点：{}",
                path_cell(&Path::new("D:/downloads").join("renamed.bin"))
            ),
            "来源头（--referer）：https://ref.example/page",
            "Cookie（--cookie）：a=1; b=2",
            "请求体文件（--data-file）：（未给）",
            "用户（--user）：alice",
            "标志位（flags）：bit0 只入队不下、bit2 完成后保留断点",
        ]
        .join("\n");
        assert_eq!(got, want);
        // E23⑥/E24③：摘要里"口令"这个话题整个不出现 —— 值、掩码、字段名三者一个都不许有。
        assert!(!got.contains("hunter2"), "口令的值不许进摘要：{got}");
        assert!(!got.contains("[redacted]"), "连日志防线那个掩码也不许出现在摘要里：{got}");
        assert!(!got.to_lowercase().contains("password"), "摘要里不许有 password 这个字段名：{got}");
        assert!(!got.contains("口令"), "摘要里不许有口令这个话题：{got}");
        assert!(!got.contains("Seeded"), "摘要不是 `Debug`，不许把结构体摆出来：{got}");
    }

    #[test]
    fn the_summary_marks_absent_fields_and_no_flag_bits() {
        let t = Task {
            url: "http://example.invalid/x.bin".to_string(),
            referer: None,
            cookies: None,
            data_file: Some(PathBuf::from("D:/body/post.bin")),
            user: None,
            password: None,
            dir: PathBuf::from("."),
            name: None,
            flags: FLAG_IGNORE_MTIME,
        };
        let got = summary(&t);
        assert!(got.contains("标志位（flags）：bit1 忽略 Last-Modified"), "{got}");
        assert!(got.contains("请求体文件（--data-file）："), "{got}");
        assert!(got.contains("（未给）"), "缺字段要显式说\"未给\"，否则\"没有这一格\"与\"值为空\"读起来一样：{got}");
        assert_eq!(
            got.matches("（未给）").count(),
            3,
            "referer / cookie / user 三格未给，别的格各有值：{got}"
        );
        assert!(!got.contains("bit0 只入队不下"), "没置的位不许出现在摘要里：{got}");
    }

    /// 表里一行的形状（提出来只为过 `clippy::type_complexity`）：
    /// `(这一行在测什么, ok, message, fresh_reason, 走不走 --data-file, 期望首句, 该不该按成功退出)`。
    /// 表内每一行的**内容与位置一个字都没动**。
    type Row<'a> = (&'a str, bool, Option<&'a str>, Option<&'a str>, bool, &'a str, bool);

    #[test]
    fn the_final_line_bills_the_two_cross_run_counters_and_nothing_else() {
        // 表驱动：每行一个 `(ok, message, fresh_reason, --data-file, 期望全文, 期望该按成功还是失败退出)`。
        // 期望全文里路径那一格同样由 `Path::display` 现拼，其余逐字面。
        let target = path_cell(&Path::new("D:/downloads").join("renamed.bin"));
        let cases: Vec<Row<'_>> = vec![
            (
                "下完了，什么额外话都没有",
                true,
                None,
                None,
                false,
                "下载已完成",
                true,
            ),
            (
                "没下完：`ok = false` 一定带一句人话（Report::message 契约的第一半）",
                false,
                Some("worker 0 的重试到顶（已试 6 次，--max-retries=Some(5)）：连接被对端关闭"),
                None,
                false,
                "下载未完成",
                false,
            ),
            (
                // **E20⑦ 那一格的活体针**：`ok = true` 时 `message` 也可以有值（"下载已完成，但…"），
                // 判据仍然是 `ok`。引擎侧今天没有确定性夹具（要 `remove_file` 以非 NotFound 失败），
                // 所以这一格只能在这里钉住 —— 变异 m3 就是把第二个值改成读 `message`。
                "下完了但断点删不掉：成功档 + 一句说明",
                true,
                Some("下载已完成，但断点文件删不掉（D:/downloads/renamed.bin.dlm.json）：占用"),
                None,
                false,
                "下载已完成",
                true,
            ),
            (
                "中途重来 ⇒ 原因那句话必须走到用户眼前（resume.rs 的承诺）",
                true,
                None,
                Some("ETag 变了：上一轮是 \"old\"，这一轮服务器说 \"new\""),
                false,
                "下载已完成",
                true,
            ),
        ];
        for (label, ok, msg, fresh, data_file, verdict, want_success) in cases {
            let rep = report_with(ok, 2, 3, fresh, msg);
            let (text, success) = outcome(&rep, &sample_task(data_file));
            assert_eq!(success, want_success, "{label}：退出档读错了");
            assert!(text.starts_with(verdict), "{label}：首句该是 {verdict}：{text}");
            assert!(text.contains(&format!("目标：{target}")), "{label}：最终行要带落点（E25⑦）：{text}");
            assert!(text.contains("全长 1024 B，落盘 1024 B，归属已定 1024 B"), "{label}：两本账分开报（E7）：{text}");
            assert!(
                text.contains("这个断点文件从建立起一共重试 2 次，归属之外可能被重下 3 B"),
                "{label}：那两格是跨断点文件的账、且 `duplicated` 不是账单（E20⑧/E14③/E25⑥）：{text}"
            );
            assert!(text.contains("用时 1500 ms"), "{label}：耗时照 `elapsed` 报：{text}");
            if let Some(m) = msg {
                assert!(text.contains(m), "{label}：`message` 原样打，不截断不拼接（裁定 9）：{text}");
                assert_eq!(text.matches("说明：").count(), 1, "{label}：那句只许出现一次：{text}");
            } else {
                assert!(!text.contains("说明："), "{label}：没有话就别硬造一格：{text}");
            }
            if let Some(r) = fresh {
                assert!(text.contains(r), "{label}：`fresh_reason` 原样打：{text}");
            } else {
                assert!(!text.contains("从头重来的原因"), "{label}：不是从头重来就不许说这句话：{text}");
            }
            assert!(!text.contains("Seeded"), "{label}：`trace` 一个字都不许进 stdout（E20⑨）：{text}");
        }
    }

    #[test]
    fn the_data_file_route_prints_neither_of_the_two_counters_that_are_structurally_zero() {
        // E25⑥ 末句：`--data-file` 那一支既不读也不写断点 ⇒ `retries`/`duplicated` 恒 0，
        // **不许印 0 冒充测过**。同一理由还管着 `confirmed`（`Report::confirmed` 的字段文档写明
        // 那一支"没有覆盖账"）⇒ 那一格也整格不出现。
        let rep = report_with(true, 0, 0, None, None);
        let (text, success) = outcome(&rep, &sample_task(true));
        assert!(success);
        assert!(text.starts_with("下载已完成"), "{text}");
        assert!(text.contains("全长 1024 B，落盘 1024 B"), "{text}");
        for banned in ["重试", "归属之外", "归属已定", "从头重来"] {
            assert!(!text.contains(banned), "POST 那一支没有这本账，{banned} 那一格不许出现：{text}");
        }
        // 对照：同一份 `Report` 走可分段那一条就有那两格（缺席由形状决定，不是渲染器心情）。
        let (with_state, _) = outcome(&rep, &sample_task(false));
        assert!(with_state.contains("归属已定"), "{with_state}");
    }

    /// 我自己那三格的公共入参：`summary` 里"输出文件名"那一行的取法。
    fn name_line(task: &Task) -> String {
        summary(task)
            .lines()
            .find(|line| line.starts_with("输出文件名："))
            .expect("摘要必须有输出文件名那一行")
            .to_string()
    }

    #[test]
    fn the_output_name_cell_separates_the_given_name_from_the_derived_one() {
        // 给定那一支的字面量已由 `the_no_start_summary_...` 钉过；这里补的是**没给 `--out-name`** 那一支：
        // 落点**仍然有一个名字**（`Task::target_path()` 取 URL 末段），所以那一格不许打 [`ABSENT`] ——
        // 打了就等于摘要在说"这一格还没定"，而它已经定了，只是不是用户定的。
        let given = Task {
            name: Some("a.bin".to_string()),
            ..sample_task(false)
        };
        assert_eq!(name_line(&given), "输出文件名：a.bin");

        let derived = Task { name: None, ..sample_task(false) };
        let got = name_line(&derived);
        assert_eq!(got, "输出文件名：a.bin（未显式指定，取 URL 末段）");
        assert!(!got.contains(ABSENT), "推导出来的名字不是\"没给\"：{got}");
        // 两格不是两本账：这一格与 `落点` 那一行的末段必须是同一个名字。
        let target = summary(&derived)
            .lines()
            .find(|line| line.starts_with("落点："))
            .expect("摘要必须有落点那一行")
            .to_string();
        assert!(target.ends_with("a.bin"), "{target}");
    }

    #[test]
    fn the_flags_cell_names_the_set_bits_and_never_the_mask_value() {
        // **E23⑥**："别把 `flags` 的数字印出来"那一格的正面写法。位号（bit0/bit1/bit2）照字面留着
        // —— 那是 spec §5 那张表里的名字，不是掩码；掩码（`sample_task(false).flags == 5`）一个都不许出现。
        let base = sample_task(false);
        assert_eq!(base.flags, FLAG_ADD_ONLY | FLAG_KEEP_STATE);
        assert_eq!(
            flags_cell(base.flags),
            "标志位（flags）：bit0 只入队不下、bit2 完成后保留断点"
        );
        assert!(!flags_cell(base.flags).contains('5'), "掩码值 5 不许出现在那一格里");

        // 一个都没置：不许留空（空行读起来像渲染器坏了那一格），也不许说"未给"
        // （它不是"没给"，是"给了、值是零"）。
        let none = flags_cell(0);
        assert_eq!(none, "标志位（flags）：一个都没置");
        assert!(!none.contains(ABSENT), "{none}");

        // 本版不认识的位（spec §5 只到 bit2）：**不静默丢掉**（摘要会说"没置"而断点文件里置着，
        // 下一轮 `resume` 的 ①b 读的就是那一份），但也**不印它的值**。
        let unknown = flags_cell(FLAG_ADD_ONLY | 1 << 7);
        assert!(unknown.contains("bit0 只入队不下") && unknown.contains("本版不认识的位"), "{unknown}");
        assert!(!unknown.contains("128"), "未知那一位的值不许打出来：{unknown}");
    }

    #[test]
    fn a_zero_reading_still_bills_on_the_state_route() {
        // 与 `the_data_file_route_...` 的分工：那一发钉"恒 0 的那一支整格不出现"，这一发钉
        // "**真有**断点文件那一支读到 0 也要照打"。0 在这一支是**测出来的结论**（这一轮没重试过、
        // 归属之外没有可疑重复），省略它用户会以为渲染器掉了一格，而印出来才说得出"确实没有"。
        let rep = report_with(true, 0, 0, None, None);
        let (text, success) = outcome(&rep, &sample_task(false));
        assert!(success);
        assert!(
            text.contains("这个断点文件从建立起一共重试 0 次，归属之外可能被重下 0 B"),
            "零读数也要成账：{text}"
        );
        assert!(text.contains("归属已定 1024 B"), "{text}");
        assert!(!text.contains("说明："), "没有话就别硬造那一格：{text}");
    }
}
