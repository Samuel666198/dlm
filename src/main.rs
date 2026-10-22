//! 二进制那一层：把 `cli` → `report` → `engine` 接起来（Task 9B-2 的第二半，落 **E24** 的六条义务）。
//!
//! 这一层**没有任何算法**：渲染全部在 [`dlm::report`]（纯函数），下载全部在 [`dlm::engine::run`]。
//! 它唯一的实质工作是"**什么时候不上网**"与"**退出码从哪儿来**"，所以四条落点按勘误编号写在这里，
//! 每一条都能在 `tests/integration.rs` 里那些起真二进制的用例里找到归宿（**数条数没写**：这一族
//! 注释只要带上"几发"就必然漂，本轮已经漂过一次 ⇒ 只写去处，不写数量）：
//!
//! - **E24②**（`--help` / `--version` ⇒ 0）：`cli::parse_from` 把 `clap::Error` 折进
//!   `anyhow::Error` 时**没丢类型** ⇒ 这里 `downcast_ref::<clap::Error>()` 拿回来再 `exit()`，
//!   由 clap 自己按 kind 决定"打到哪条流、给哪个码"。为什么不能一律 `Err ⇒ 1`：
//!   **E20⑦** 那"两档"说的是**下载结果**，参数层不在里面；一刀切会让 `dlm --help` 以 1 退出。
//! - **E24③ / E20④**（`--no-start` 在 `engine::run` **之前**短路）：引擎明写不看 bit0，
//!   而"进了 `run` 就一定会上网取字节" ⇒ 这一位在 CLI 侧消费，摘要打完直接 0 退出。
//! - **E24① / E20③**（`--quiet` ⇒ **一次都不挂 sink**）：不是"挂了但里面不打印"，见 [`dispatch`]。
//! - **E20⑦ / E25⑥⑦**（`exit 0 iff ok`）：判据**只有** `!ok`，绝不看 `message.is_some()`
//!   —— 引擎在成功那一支也会写 `message`（断点文件删不掉那一支）。`run` 的 `Err` 同样非零。
//!
//! 渲染层不做 IO，所以 `\r` 那一发住在这一层（**E25** 开头那句的分工）。

use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Result;

use dlm::cli;
use dlm::engine::{self, OnTick, Report, RunSnapshot};
use dlm::report;
use dlm::task::Task;

/// 下载没完成的退出档（**E20⑦**：v1 只有 0/1 两档，`!ok` 与 `Err` 都落在非零那一档）。
const DOWNLOAD_FAILED: u8 = 1;
/// 参数层的码。**取 2 是为了与 clap 那一支同值**（实测：`clap::Error::exit_code()` 给用法错误 2）：
/// 脚本读到的"话没说对"只有一种码，不分"clap 发现的"与"我们发现的"。
/// 这一格**两边都有判据**：进程级由 `tests/integration.rs` 的
/// `the_binary_holds_a_malformed_argument_at_the_parameter_tier` 钉（真二进制跑 `--sha256 xyz`
/// 与 `--connections 0`，实测变异"把参数层交回 1"会红在那里），本文件的
/// `the_error_layer_does_not_reuse_the_download_tiers` 钉"2 撞不撞下载档"。
const BAD_ARGS: u8 = 2;

fn main() -> ExitCode {
    ExitCode::from(match dispatch() {
        Ok(code) => code,
        Err(err) => {
            // 走到这里只有一种来源：`engine::run` 的 `Err` ＝**根本跑不起来**
            // （断点损坏、建目录失败、`resume` 判 `Abort`、落盘/写断点失败…）。
            // "跑完但没下完"是另一支（`Ok(Report{ok:false})`），它的话由 `report::outcome` 打在 stdout。
            // 措辞里那三个字是分档的依据之一，也是用例钉住的措辞（`ok = false` 那一支不会有它）。
            // 走 stderr：`--quiet` 关的是进度行，不关错误（**E24①**）。
            eprintln!("dlm 跑不起来：{err:#}");
            DOWNLOAD_FAILED
        }
    })
}

/// 装配全过程。交回退出码；`Err` 只可能来自 `engine::run`（见 `main` 那段注释）。
fn dispatch() -> Result<u8> {
    let cli = match cli::parse_from(std::env::args_os()) {
        Ok(cli) => cli,
        Err(err) => return Ok(parameter_error(&err)),
    };

    // `quiet()` / `no_start()` 先读：`into_task()` 会**消费** `Cli`（**E23⑦** 交回两件东西的形状）。
    let quiet = cli.quiet();
    let no_start = cli.no_start();
    // R19：完成后动作也是"消费 Cli 之前"取走的一格；它不进 Task/Config，也不参与续传比对。
    let action = cli.action().cloned();
    let mut cfg = cli.into_config();
    let task = match cli.into_task() {
        Ok(task) => task,
        Err(err) => return Ok(parameter_error(&err)),
    };

    if no_start {
        // **E20④**：全树生产代码没有任何地方读 bit0，引擎文档明写"本函数不看这一位"，
        // 所以"一个字节都不许多打"只能靠**不进 `run`** 来保证 —— 那发用例断的正是
        // "对端一条请求都没看到、目标文件没被创建"。摘要里口令这个话题整个不出现
        // （**E23⑥/E24③**），所以这里打的必须是 `report::summary`，不是 `{:?}`。
        println!("{}", report::summary(&task));
        return Ok(0);
    }

    if !quiet {
        // **E24①**：静默那一支**根本不挂** sink。落点选在"装配"而不是"闭包里判断"，
        // 因为"这一趟要不要有进度源"本来就是装配期的决定；挂在闭包里会留一条永远走不到、
        // 因而红不了的分支。顺带照住 **E21⑧**：`--data-file` 那一条路引擎一发 tick 都不交，
        // 挂了 sink 也不会多出一个进度源 —— 那条路的权威始终是 `Report`。
        cfg.on_tick = Some(tick_sink());
    }

    let rep = engine::run(&task, cfg)?;
    let code = finish(&rep, &task, quiet);
    if let Some(action) = action.as_ref() {
        if rep.ok {
            if let dlm::actions::CompletionAction::Shutdown { delay_secs } = action {
                eprintln!("警告：将在 {delay_secs} 秒后关机；取消请运行 shutdown /a");
            }
            // R19：动作失败只警告，不改下载结果/退出码（"动作失败不许掩盖下载结果"）。
            if let Err(err) = dlm::actions::execute(&dlm::actions::plan(action, &task.target_path())) {
                eprintln!("警告：完成后动作失败：{err:#}");
            }
        } else {
            eprintln!("完成后动作未执行：下载未完成");
        }
    }
    Ok(code)
}

/// 结果行 + 退出码。**这一层是把"有没有下完"折成码的唯一位置**（**E20⑦**：非零档的判据只有 `!ok`，
/// `message` 已经作为"说明"那一行打出去了，它有没有值**不参与**这里的决定）。
///
/// 为什么要把这一小段从 [`dispatch`] 里提出来单独成函数（不是为了好看）：**实测变异 m1b** 把
/// `dispatch` 里那一行改成"自己读 `rep.message`"之后，`cargo test --lib` 200 全绿、
/// 那三发起真二进制的用例也全绿 —— 因为**一次真的成功下载今天不带 `message`**
/// （**E20⑦** 末尾那笔具名欠账：要造那一格得让 `fs::remove_file` 以非 `NotFound` 失败）。
/// `report::outcome` 的用例钉的是"它交回的布尔等于 `rep.ok`"，而**装配层有没有去读那个布尔**
/// 落在判据之外。把这一格做成"输入是 `Report` 而不是布尔"的函数，手搭 `Report` 就又进得来了。
fn finish(rep: &Report, task: &Task, quiet: bool) -> u8 {
    let (line, ok) = report::outcome(rep, task);
    // 收尾让出那一行：进度行是 `\r` 原地覆盖的，不换行就会把结果行盖在它上面。
    //
    // **但只有"这一趟会有进度行"时才让**（**E28①** 实测到的外观缺陷）：带 `--data-file` 的那一支由
    // `engine::run` 走 `run_whole_once`，全程**一发 tick 都不交**（**E21⑧**），也就没有原地覆盖的
    // 那一行要让位 ⇒ 无条件换行会给用户一个凭空的**前导空行**。
    // 判据取**任务形状** `Task::segmentable()` —— 那正是 `engine::run` 在 `run_whole_once` 与
    // 多 worker 那条路之间分流用的同一位（它读 `data_file.is_some()`，而 `segmentable()` 就是它的
    // 否定），也与 `report::outcome` 决定"跨轮那三格出不出现"用的是同一位 ⇒ 三处同一个判据。
    // ⚠ **不取"这一趟实际收到过几发 tick"**：**E22③** 禁止 CLI 把"收到过 tick"当证据/依赖，
    // 而数发数要给 `tick_sink` 加内部可变性 —— 那条路 **E26/C6** 已经明否。
    // 退出码**不看这一位**：唯一的输入还是 `outcome` 交回的布尔（**E20⑦**）。
    if !quiet && task.segmentable() {
        println!();
    }
    println!("{line}");
    if ok { 0 } else { DOWNLOAD_FAILED }
}

/// 参数层那一支的统一收尾（**E24②**："两档"不覆盖这一层）。
///
/// `clap::Error::exit()` 是 `-> !`，帮助/版本自己给 0、用法错误自己给 2 ⇒ 我们**不替它猜码**；
/// `downcast_ref` 拿不回类型就说明那不是 clap，而是 `check_boundaries`（**E23⑤**）或
/// `Task::validate`（`http://` + 口令那一支，**E3**）写的那句话 ⇒ 原样打到 stderr，
/// 码取与 clap 同值的 2（两个来源不给两种码）。
fn parameter_error(err: &anyhow::Error) -> u8 {
    if let Some(clap_err) = err.downcast_ref::<clap::Error>() {
        clap_err.exit();
    }
    eprintln!("{err:#}");
    BAD_ARGS
}

/// 挂到 `Config::on_tick` 的那一发：在**引擎线程里同步**打印一行
/// （**E21** 判据⑨点名的预期用法：回调慢一拍，`recv_timeout` 的期限、退避到点、保存节奏全跟着慢
/// ⇒ 除了打印什么都不做，绝不在这里做长阻塞 I/O）。
///
/// `\r` 放在**最前**：先回行首再写这一行，于是同一行被原地覆盖，屏幕上永远只有一行进度。
/// 不做"补空格擦到行尾"：上一发比这一发长时留下的残尾会在更长的下一发被盖掉，
/// 而收尾那一发之后另有 [`finish`] 的那一发改行把整行让出来（而且只在**会有进度行的那一路**让，
/// 见那里的 **E28①** 判据）—— 为这一格引入宽度状态
/// 就要让闭包变成有内部可变性的对象，代价比收益大，且没有任何用例读它。
///
/// 写失败**不当失败处理**：进度行是给人看的附加物，管道断了（`dlm … | head`）不该让
/// 一次已经下完的下载变成非零退出；真正的失败另有出口（`Report::ok` 与 `engine::run` 的 `Err`）。
fn tick_sink() -> OnTick {
    Arc::new(|snap: &RunSnapshot| {
        let mut out = std::io::stdout().lock();
        // `report::tick` 交回的串**不带** `\r`（渲染层要能被字面量断言，**E25** 开头那句）。
        let _ = write!(out, "\r{}", report::tick(snap));
        let _ = out.flush();
    })
}

/// 二进制自己的单元测试。**这一族只在 `cargo test --bins`（以及 `cargo test` 全量）里跑**：
/// `main.rs` 不属于库的编译单元，`cargo test --lib` 那道闸看不见它 —— 而这里钉的恰恰是
/// "装配层有没有去读 `outcome` 交回的那个布尔"，是库面判据唯一够不着的一格（见 [`finish`] 那段）。
#[cfg(test)]
mod tests {
    use super::*;
    // 只在测试里用得到：手搭 `Report` 要给 `elapsed` 一个数。
    use std::time::Duration;

    /// 手搭一台 `Report`：只填 [`finish`] 会读到的那几格，其余取构造上合法的零值。
    /// `trace` 留空 —— 它**不该被读**（**E20⑨**），留一条也会被断言"不许出现在结果行"的位置是
    /// `report.rs` 那两发的职责，不在这里重复。
    fn report(ok: bool, message: Option<&str>) -> Report {
        Report {
            bytes_total: 8,
            delivered: 8,
            confirmed: 8,
            duplicated: 0,
            retries: 0,
            elapsed: Duration::from_millis(1),
            ok,
            trace: vec![],
            fresh_reason: None,
            message: message.map(str::to_string),
        }
    }

    fn task() -> Task {
        Task {
            url: "https://example.invalid/a.bin".to_string(),
            referer: None,
            cookies: None,
            data_file: None,
            user: None,
            password: None,
            dir: ".".into(),
            name: Some("a.bin".to_string()),
            flags: 0,
        }
    }

    #[test]
    fn the_exit_tier_is_the_bool_from_outcome_and_not_a_second_read_of_the_report() {
        // **E20⑦** 在装配层的那一半。四行按"判据读错方向"的四种写法排：
        // ① `ok = true` + `message = Some(..)` ⇒ **0**：这一格引擎今天造不出来（**E20⑦** 末尾那笔
        //    具名欠账），所以真二进制那三发**看不见**这个变异 —— 实测（变异 m1b：把 `finish` 里
        //    那一行改成读 `rep.message.is_none()`）`--lib` 200 全绿、三发二进制用例也全绿，
        //    只有这一发行①会红。这就是它存在的理由。
        assert_eq!(
            finish(&report(true, Some("下载已完成，但断点文件删不掉")), &task(), true),
            0,
            "成功档 + 一句说明 ⇒ 还是 0（`message.is_some()` 不是失败信号）"
        );
        // ② 成功且什么话都没有 ⇒ 0。
        assert_eq!(finish(&report(true, None), &task(), true), 0);
        // ③ 没下完且带一句人话（`Report::message` 契约的第一半）⇒ 1。
        assert_eq!(
            finish(&report(false, Some("worker 0 的重试到顶")), &task(), true),
            DOWNLOAD_FAILED
        );
        // ④ 反方向：没下完**而且没话**也不许被读成 0（挡住"`message` 空就当成功"那种写法）。
        assert_eq!(finish(&report(false, None), &task(), true), DOWNLOAD_FAILED);
    }

    #[test]
    fn quiet_moves_the_newline_only_and_never_the_exit_tier() {
        // **E24①**：`--quiet` 关的是每一拍那一发进度行（以及收尾那一发换行），
        // 结果行与退出码**照旧**。这一格是"静默 ⇒ 什么都不说"那种误读的反针。
        assert_eq!(
            finish(&report(true, None), &task(), false),
            finish(&report(true, None), &task(), true),
            "同一份成功结论，静默与否不许改退出档"
        );
        assert_eq!(
            finish(&report(false, Some("校验不符")), &task(), false),
            finish(&report(false, Some("校验不符")), &task(), true),
            "同一份失败结论，静默与否不许改退出档"
        );
    }

    #[test]
    fn the_error_layer_does_not_reuse_the_download_tiers() {
        // **E24②**：参数层不在 E20⑦ 那两档里。这里能钉的只有"我们自己写的那句话"那一支
        // （clap 那一支由 `clap::Error::exit()` 自己收尾，它是 `-> !`，进程内测不了）。
        let boundary = anyhow::anyhow!("参数写错了：--connections 至少 1（给了 0）");
        assert_eq!(parameter_error(&boundary), BAD_ARGS);
        assert_ne!(BAD_ARGS, DOWNLOAD_FAILED, "两档不许撞在同一个数上");
        assert_ne!(parameter_error(&boundary), 0, "参数层的失败绝不能是 0");
    }
}
