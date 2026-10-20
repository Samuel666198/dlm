//! 断点文件 `<目标>.dlm.json`：一次运行**关于远端文件已验证到什么** + **哪些字节已确认** + 几个计数。
//!
//! 依据：spec §4（第 69–90 行的形状与安全边界）、plan 第 18–19 行「断点文件绝不写入口令」「写文件一律
//! tmp + rename 原子替换」、以及 plan 文末勘误 **E6**（模块声明进 `src/lib.rs`）/**E7** 最后一条
//! （`snapshot_intervals()` 是"已确认字节的极小覆盖集"）/**T2-D**（覆盖集 ≠ 落盘量，分开算是 Task 8 的活）。
//!
//! ## 本模块的形状决定（都有对应用例钉着，不是注释里说说）
//!
//! 1. **口令不进这个文件，靠"结构里没有这个字段"保证。** `TaskView` 是 `Task` 九个字段去掉 `password`
//!    的那个视图：字段少一个 ⇒ 序列化、`Debug`、将来任何人新加的写盘路径都不可能把它带出去。
//!    `#[serde(skip)]` 同样能写出正确结果，但它要求"每个后来加字段的人都不忘 skip"，
//!    而漏掉的后果是把用户口令明文落到用户磁盘上 —— 取前者。`needs_secret` 是这条硬规约的补偿：
//!    文件里没口令，续传时就必须先问出口令，判据归 Task 5（spec §4 末段 + plan 第 705 行）。
//!    **"少一个字段"挡不住别人写进自由文本里的凭据**（勘误 **E9**）：`--url https://alice:s3cret@x/a.bin`
//!    会把口令原样带进 `task.url` 与 `validator.final_url`，而本模块删不掉 URL ⇒ 这一支由
//!    `Task::validate()` 在唯一的校验闸口拒绝（规则只住在那一处，本模块不复述），`from_task` 已调它。
//!    **这句话还有另一半**：`validator.final_url` 除了用户自己写的那条 URL，也可能来自**服务器重定向
//!    交回的落地串**，而 `validator.etag` / `last_modified` 整个由服务器给 —— 这三样都不经 `validate()`
//!    ⇒ 由 `probe::landed_url_is_refused`（落地串）与 `probe` 的验证子闸（`facts_from_response` 与
//!    416 那一支 `empty_file_facts`，T99-H2 §3）管。**判据本体住在那一处，本模块不复述。**
//!    ⚠ **"永远没有口令"的确切面积**（T99-H2 之后按实测改准，别再读成"什么都看得见"，也别读成"已防住"）：
//!    - 堵的是**三个字段**（`final_url`／`etag`／`last_modified`）里出现**本任务口令的两组形状**
//!      （第一组＝编我们那份：原样、百分号解码后撞中的字面形、`std`/`url-safe` × 带/不带 padding 的四形；
//!      第二组＝解对面那份：待判串里的 base64 段解出来带着 `:<口令>`，管的是线上真发出去的那份
//!      `base64("<user>:<口令>")`）。⚠ 第一组里**带 padding 的两形永远不会是唯一决定者**（它是去
//!      padding 那一形加尾点）⇒ 六个写法不等于六维面积（T99-H2 M-1 的口径，README 同步）。
//!      看得见的前提是"对面（或我们自己的 HTTP 客户端在序列化时）按**标准**编码/书写我们这份凭据"，
//!      不是对面任意变形；比对是**大小写敏感的字节比对**。
//!    - 口令**短于 4 字节**时两组都不比内容（误杀代价的边界，理由与形状集合都写在 `probe::PASSWORD_CONTENT_FLOOR`
//!      与 `carries_our_password`），那种任务只剩 userinfo／形状两支在管；
//!    - **判据只比口令、不比用户名**（有意为之）：用户名本来就按下面第 2 条落盘 ⇒ 回显它不增加新暴露；
//!      `--user` 单独给不产生 `Authorization`；换用户名由 `resume` 的 ①b 管。三条支撑写在 README
//!      「凭证与边界」那一段（复审 Q3-5 要求把理由写出来，旧文档只写了口令的面积）。
//!    - **覆盖用户自己写进 `--url` 的那份**（旧那句"不覆盖/不加判据"在 HEAD 是**假的**，T99-J1 §3 改准）：
//!      落地闸读的 `final_url` 在**没有发生重定向时就是 `--url` 本身** ⇒ 同一个口令写在自己的 path/query 里
//!      会让这一枪被**整条拒**（实测退 1）。方向是多拒（安全侧），不是放行；真正**不覆盖**的仍然只有
//!      `task.url` 这个字段本身（它与由它派生的落点、摘要一律原样落盘，没人改写它）。
//!    - **读侧也过同一份判据**（T99-J1 §2）：文件是**历史写的**，写侧闸守不住旧值 ——
//!      `resume::decide` 在打 ②③ 那两句原因之前，对断点里那三份值过**同一个**判据，命中就换成一句
//!      两个值都不回显的原因（结论仍是"从头重来"）。判据本体仍然只住在 `probe` 一处，本模块不复述。
//! 2. **`user`（用户名）照 spec §4 的示例落盘** —— v1 硬规约点名的是口令，不是用户名，示例里也写着
//!    `"user": null`。`referer`/`cookies` 同理照抄 spec 的形状（cookie 也是凭据，这条风险登记在 Task 1
//!    的遗留清单里、等 Task 9 议；本模块不擅自给它加规则）。用例 `username_is_persisted_by_ruling`
//!    把这个读法钉住，改它必须是有意的编辑。
//! 3. **`SegView` 只有 `start`/`end`**，即 E7 那条覆盖集的一行；spec 示例里的 `written`/`state` 不搬，
//!    理由见 `SegView` 的文档（它们在本模型里恒等于 `end - start + 1` 与"done"，是死字段，
//!    而 `written` 更糟：它对 416 封存的行会说谎）。
//! 4. **`load` 只在"文件不存在"时给 `Ok(None)`。** 存在但读不懂 ⇒ `Err`：把损坏当成没有，等于把
//!    已经验证过的字节重新算成待下载（还会顺带骗过 `--no-resume` 的判据），所以损坏必须是显式失败。
//! 5. **原子写 = `<目标>.dlm.json.tmp` → `rename`。** Windows 语义见 `save` 的文档。

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::scheduler::Interval;
use crate::task::Task;

/// 本模块认识的文件格式版本（spec §4 示例里的 `"v": 1`）。
/// 与本文件写的 JSON 形状**有任何不兼容改动**（改键名、改语义、删字段）都要抬这个数，
/// 并且 `load` 会拒读不是它的文件 —— 见 `load` 的版本闸。
pub const STATE_VERSION: u8 = 1;

/// 断点文件名的追加后缀（`path_for`）：`big.zip` → `big.zip.dlm.json`。
const STATE_SUFFIX: &str = ".dlm.json";
/// 原子写的临时文件后缀（`save`）：`<状态文件>.tmp`。**不是** `with_extension("tmp")` ——
/// 那会把 `a.bin.dlm.json` 变成 `a.bin.dlm.tmp`：一个谁也认不出、下一轮也不会被消耗掉的兄弟文件，
/// 于是"用同一命名的临时文件"这条约定本身就断了（brief 给的那条 `.tmp` 断言会因此变成假断言）。
const TEMP_SUFFIX: &str = ".tmp";

/// 整个断点文件。七个键与 spec §4 的示例 JSON 一字不差（用例 `json_keys_match_the_spec_example_shape` 钉着）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    /// 格式版本，见 `STATE_VERSION`。
    pub v: u8,
    /// 任务的可持久部分：**没有口令字段**。
    pub task: TaskView,
    /// 我们关于远端文件已验证到什么（续传的判据，Task 5 消费）。
    pub validator: Validator,
    /// 已确认字节的极小覆盖集，语义见 `SegView`。
    pub segments: Vec<SegView>,
    /// 计数；本模块只负责存，累加归 Task 8。
    pub stats: Stats,
    /// 这个任务的口令**没有**被记在这里：文件里只留"当初带了口令"这个事实。
    /// 续传时必须重新给口令，判据归 Task 5（spec §4 末段、plan 第 705 行的 ①）。
    pub needs_secret: bool,
    /// UTC 墙钟 `YYYY-MM-DDThh:mm:ssZ`，只给人看，不参与任何判定（不参与校验的是 `validator` 里的
    /// `etag`/`last_modified`）。落盘前刷新它请调 `utc_now()`（本模块导出的那口钟，`save` 拿的是
    /// `&Snapshot`，自己盖不了时间戳）。见 `format_utc`。
    pub saved_at: String,
}

/// `Task` 的 serde 视图 —— 九个字段里**去掉了 `password`**，其余按 spec §4 的键名原样保留。
///
/// 为什么是"少一个字段"而不是 `Option<String>` + `#[serde(skip)]`：两种写法今天都对，
/// 但后者把安全性寄托在"每个后来动这个结构的人都不忘写 skip"上，而漏掉的后果是把用户口令明文
/// 落到用户磁盘上。少一个字段则**没有可以写错的地方**：口令在类型层面到不了序列化边界，
/// 也到不了 `{:?}`（用例 `debug_output_carries_no_secret_either` 钉的是后者那条通道）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskView {
    pub url: String,
    pub referer: Option<String>,
    pub cookies: Option<String>,
    pub data_file: Option<PathBuf>,
    /// 用户名**照 spec §4 落盘**（v1 硬规约点名的是口令，同节示例 JSON 里写着 `"user": null`）。
    /// 读法由用例 `username_is_persisted_by_ruling` 钉住。
    pub user: Option<String>,
    pub dir: PathBuf,
    pub name: Option<String>,
    pub flags: u32,
}

impl From<&Task> for TaskView {
    fn from(t: &Task) -> TaskView {
        TaskView {
            url: t.url.clone(),
            referer: t.referer.clone(),
            cookies: t.cookies.clone(),
            data_file: t.data_file.clone(),
            user: t.user.clone(),
            dir: t.dir.clone(),
            name: t.name.clone(),
            flags: t.flags,
        }
    }
}

/// "远端文件还是不是同一个东西"的凭据集合（spec §4 / §6.3、Task 5 的判定输入）。
/// `total_len` 是覆盖集的边界，`segments` 的合法性按它来查（见 `check_segments`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Validator {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub total_len: u64,
    pub final_url: String,
    /// 本次运行用的"忽略 Last-Modified"判据。**本模块不代读 `task.flags`**（谁调用谁负责写入真实值，
    /// 静默覆写更容易出错）；Task 8 请传 `task.ignore_mtime()`。判定本身归 Task 5。
    pub ignore_mtime: bool,
}

/// **已确认字节覆盖集的一行**：闭区间 `[start, end]`，两端都是绝对字节偏移。
///
/// 措辞按 **E7**：这不是 IDM 那种"分段边界列表"。供数是 `scheduler::snapshot_intervals()` ⇒
/// 升序、相邻/重叠已合并、两两不重叠。于是：
/// - 一段被劈过再交回 ⇒ 这里可能只有**合并后的一条**；
/// - 整份文件连续下完 ⇒ 这里就**只有** `[0, total_len-1]` 一条；
/// - 一行**不**代表"曾经有一个 worker 被派过这一段"。
///
/// **它说的是"归属已定、不再下载"的字节，不是"磁盘上真有多少字节"**：`Event::SegmentComplete`
/// （spec §6.4 的 416）封存整段时不看 `written`，那种行里可能含从未落盘的字节（**E7**/**T2-D**）。
/// 落盘量取自 `store.len()`，两个量分开算是 Task 8 的义务，本模块不做调和、也不加 `sealed`/`dirty`
/// 之类的标记位（那正是"在这里调和两个量"的形状）。
///
/// 也正因为这样，spec §4 示例里的 `written` 与 `state` 两个键**没有搬进来**：在本模型里每行都是
/// 整段确认，`written` 恒等于 `end - start + 1`、`state` 恒为 `"done"`（两个恒等字段是死重量），
/// 而 `written` 更糟 —— 它对 416 封存的行会说谎，读的人以为那是盘上的字节数。
/// 剩下的两个偏移量就是这行需要且仅有的信息。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SegView {
    pub start: u64,
    /// 闭区间终点（含）。`total_len = 10` 的整份确认记作 `end = 9`。
    pub end: u64,
}

impl SegView {
    /// 这一行覆盖多少字节；`end < start` 视为空区间返回 0（与 `Interval::len` 同规则）。
    pub fn len(&self) -> u64 {
        if self.is_empty() { 0 } else { self.end - self.start + 1 }
    }
    pub fn is_empty(&self) -> bool {
        self.end < self.start
    }
    /// 回到 `scheduler::Interval` 形状（Task 8 若要把断点喂回调度器）。
    pub fn interval(&self) -> Interval {
        Interval { start: self.start, end: self.end }
    }
}

impl From<Interval> for SegView {
    /// E7 的那条数据通路：`snapshot_intervals()` 的每个区间原样变成一行。
    fn from(iv: Interval) -> SegView {
        SegView { start: iv.start, end: iv.end }
    }
}

/// 进度计数。本模块只负责存与取；累加规则（哪些字节算 duplicated、什么时候算一次 retry）归 Task 8
/// （plan 第 843 行）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stats {
    /// 已交出去落盘的字节（含后来被覆盖/丢弃的）。
    pub bytes_delivered: u64,
    /// **落在归属范围之外、可能被别人重下的字节**（勘误 **E14**③ 的口径裁定：这一格回答的只是
    /// "归属之外**可能**由别人重下的字节"这一个问题，**不是一张线上流量账单** —— E14③ 当时给的第二个选项
    /// "如实标注成上界"也被 **E14 附记**的复量否掉了，理由见下面"阶段5A 之后它反过来低报"那一段，
    /// 字段名里的 duplicated 说的是这件事）。来源只有 `engine::translate`
    /// 算的那一笔，别无其它：
    /// 1. 一枪写下去的字节里，落在调度器**此刻**认给这个 worker 的区间之外的部分（`Retighten` 之后
    ///    在途那一枪还读过去的那一截 ⇒ 这些字节真在盘上、进不了覆盖集 ⇒ **可能**被别人再下一次；
    ///    措辞按 E14③ 批准的那一句，不许写成"必然"）。
    ///    **E14**① 的终点帽把这一项压到"帽落下之后最多一块（64 KiB）"，但压不到零：调度器只知道
    ///    上一枪**结束时**交回的进度，所以它可能把一刀落在 incumbent 早已写过的偏移之下。
    ///
    /// **已知的少报（三处，逐条写明量级怎么来的）**。理由**不共用**（原先这一句写成"这三批字节都
    /// 不在这两个量里"，对它正要介绍的 ① 是假的 —— 修复轮 8F-3a 的 F15）：E17 撤掉"拿 `store.len()`
    /// 减覆盖集"那笔倒推之后，引擎手里只剩"覆盖集 + `store.len()`"两个量，而
    /// - **① 在这两个量里**（就在 `store.len() − 覆盖集` 那个差值里）—— 但它和洞**分不开**，
    ///   所以照样记不成账（下面 ① 那段自己就是这么论证的）；
    /// - **②③ 根本不在这两个量里** —— 那两批字节只上过网线、一个字节都没落到盘上，而覆盖集与
    ///   `store.len()` 都只看盘。
    ///
    /// 唯一的诚实线上量具是对端日志（计划勘误 **E14 附记**：`Σ conn_close_side.sent`），引擎运行时拿不到。
    /// 宁可少报，不记假账（**E14③**），也不许为它们开第三本账（**E7**）。
    ///
    /// ① **被杀时的在途半段**。任务在**任意时刻**被杀时，上一轮那些在途枪**已经落盘、却没来得及进段表**
    /// 的字节会被下一轮重下，而它们**不在**这个字段里。洞同理造出来：`Store::write_at` 一延长文件，
    /// 中间没写的洞读起来是零、照样算进 `len()`；多 worker 的中点劈分（`scheduler.rs` 的 `split_tail`）
    /// 天生造洞 —— 把那个差值记成重下就是 E14③ 禁止的第二本账。
    /// **算术上界按 `fetch::plan_shot` 的两条形状分别推**（**E16** 改掉了"每枪至多一个窗口"那条形状，
    /// 所以下面这一句在 8F-1 那一版里是**不完整的**，照新形状重推）：
    /// - 认 `Range` 那一支（`BodyShape::Windowed` ⇒ `stop = min(iv.end + 1, iv.start + WINDOW_BYTES)`）：
    ///   一条在途的枪最多留下 `WINDOW_BYTES` 个未确认字节，同时在途的最多 `workers` 条 ⇒ **`WINDOW_BYTES × workers`**。
    /// - 退化那一支（`BodyShape::ToEof` ⇒ `stop = iv.end + 1`，一枪下完剩余整条 body）：一条在途的枪被掐断时
    ///   最多留下**整条剩余 body**那么多个未确认字节；而调度器在退化模式下**一次只派一段**
    ///   （`scheduler.rs` 的 `dispatch` 里 `no_ranges` 那一支：只要还有人在途就一律 `Idle`）⇒ 同时在途的只有一条 ⇒
    ///   **`≤ total_len − 已确认字节`**。
    ///
    /// 两支在一次运行里**互斥**（形状由 `ProbeFacts.accept_ranges` 定，整轮不变），所以这一笔的上界取
    /// **两者的较大**：`max(WINDOW_BYTES × workers, 剩余 body)`。
    /// ⚠ **这是从代码形状推出来的算术结论，不是实测数字**（本项目已五次把算术/记忆数写成"实测"被打回）。
    ///
    /// ② **续传那一支丢掉的预读字节**。`probe` 每一轮都会把**至多** `min(prelude_max, total_len)`
    /// 个字节（`prelude_max = consts::PROBE_READ_BYTES`，引擎直接取常数）从线上取回来 —— "至多"是因为
    /// 短 body / 中途掐断时取回更少（与 `engine.rs` 那条"每一轮 `Resume` 至多 …"同一口径，修复轮
    /// 8F-3a 的 F16⑤）；engine 只在 `Decision::Fresh`
    /// 那一支把它写在偏移 0，`Resume` 那一支**不写、也不落任何账**（落盘就变成给覆盖集里已有的字节
    /// 记一笔重复账）。这批字节是**真线上代价**，反复续传按轮累积（评审 **B-I-2②** = 8F-2 的 F10）。
    ///
    /// ③ **退化那一枪丢掉的前缀**。`BodyShape::ToEof` 的 body 仍从文件偏移 0 开始，所以 `iv.start`
    /// 之前那一段要**流式丢掉**（`fetch.rs` 的 `skip`，E12⑤）才落盘。那批字节真的上过我们的网线，
    /// 却**一个字节都没落到盘上** ⇒ 既不在 `bytes_delivered`（只记落盘量）也不在本字段（只记"落在归属之外
    /// 且**写过**的字节"）。E16③ 允许的两支里这里走的是"**写明没进账**"那一支：要把它记成账就有线上
    /// 证据的要求，而引擎没有 —— 唯一的改进方向是让 `Outcome` 多带一个"读走但丢弃的字节数"（**契约变更**，
    /// 牵动 `Outcome`/`translate`/调度器事件，登记给控制员，不在本发做）。
    /// 修好 E16 之前这一笔是**平方级**的（每个窗口重付一遍"从头到那里"）；之后每一**枪**只付一次
    /// —— 不是"每一轮只付一次"：退化模式下每一次掐断都会重派一条**起点更靠后**的区间
    /// （`scheduler.rs` 的 `dispatch` 里 `no_ranges` 那一支按 `first_pending` 派），
    /// **每条区间各付自己那一份 `skip`**（正确写法已在 `fetch.rs` 的 `BodyShape::ToEof` 文档里）。
    /// 一枪之内的量级 `≤ iv.start`（推出来的，非实测）。
    ///
    /// 它**不是**"线上多打了多少字节"：对端塞进 socket 而我们没收的字节、以及我们读走但没落盘的那一点点
    /// 都不在这本账里，而引擎**没有**独立证据去数它们（唯一记着线上 Range 的是对端日志）。
    /// 反过来用覆盖集倒推"被第二次请求的字节"就是 **E7** 明令禁止的第二本账。
    /// 阶段4 之前这一格读起来像账单（名字与注释都那么写）。**E14 附记**（2026-10-17 控制员复量）给出的
    /// 数字，同一形状（8 MiB × 4 worker），线上口径只认 **`Σ conn_close_side.sent`** 这一把尺子
    /// （`response_done` 漏掉半路被丢弃的响应，而 `done + conn_close_side` 会把两条都记的正常响应**重复计数**）：
    /// 阶段4 这一格报 8,334,284，线上多打 8,465,356 ⇒ 差 1.6%，那一轮它基本就是一张真账单；
    /// **阶段5A 之后它反过来低报**（2,341,183 对线上的 6,221,236）—— 乙那条终点帽把浪费从"我们读走又丢掉"
    /// 挪到了"**对端推上来而我们没接**"，后半截根本不在这本账的可见范围里。所以它既不是账单的下界也不是
    /// 上界，只是"归属之外**可能**由别人重下的字节"这一个问题的答案；线上流量要问对端日志，不许问这个字段。
    ///
    /// 这一格**真实被钉到的强度**（修复轮 8F-3a 重新逐条 `grep -n duplicated` 过全树，照实写；
    /// 原先这一节漏了两处断言、还把三处数量数错了。⚠ **8F-4a 补上那把量具自己的洞**：按字段名
    /// `grep duplicated` **看不见整条结构体的相等断言** —— `assert_eq!(plan.stats, Stats::default())`
    /// 这种写法里字段名根本不出现，于是上一轮把两条"其实钉住了 0"的用例读成了"填了数却没断"。
    /// 下一轮扫这本账时必须连着 `Stats::default()` / 整结构体比较一起扫，不能只按字段名 grep）：
    /// - 单元层钉**口径**，用字面值：
    ///   `engine::tests::the_duplicate_book_counts_only_what_fell_outside_the_owned_range`
    ///   （**五个断言块**：读满自己那个窗口 ⇒ 恰好 **0**；越帽那一笔 ⇒ 恰好 **65,536 = 一个
    ///   `CHUNK_BYTES`**；整枪落在归属之外 ⇒ 全算；`Complete` 那一支同样零；不许为负）
    ///   与 `duplicate_bytes_count_only_what_fell_outside_the_owned_interval`（表驱动 **六** 行）。
    /// - 单元层还钉**零进度那一支**：`one_fetch_outcome_maps_to_exactly_the_documented_events`
    ///   对 6 个 `Outcome` 都断 `duplicated == 0`（"没被收紧过的区间不产生重复账"）。
    /// - 单元层钉**别的模块不许动这本账**：`resume::tests::resume_hands_the_snapshot_back_untouched`
    ///   断 `Resume` 交回的 `stats.bytes_duplicated == 0`。
    /// - 落盘 schema：`state::tests::json_keys_match_the_spec_example_shape` 断 `stats` 里就是那三个键。
    /// - 跨轮 **+ 起零**：`state::tests::stats_start_at_zero_and_survive_a_roundtrip` 钉的是两件事，
    ///   原先这里只写了后一件 —— ① `assert_eq!(snap.stats, Stats::default())`：**新建快照的整条
    ///   `Stats` 从 0 起**，这一格因此也被钉成 0；② "存多少读回多少"（400 进 400 出）。
    ///   **注意②里那个数会随断点文件跨续传累积**（`engine::run` 从 `snap.stats` 读回来接着加），
    ///   所以它不是"这一轮"的量。
    /// - 集成层只有**两条**，都在 `the_tightening_bill_on_a_paced_peer_is_priced_by_chunk_not_by_window`：
    ///   `duplicated > 0`（这笔账不许折成 0，E14③ 末句）与一侧界 `duplicated ≤ excess + workers × CHUNK_BYTES`
    ///   （引擎的账不许比线上账单更凶；那个加数**故意放宽**）。
    /// - `engine::tests::fresh_resume_and_abort_route_to_their_own_plan` **两格都有份**（原先这一节把
    ///   它整条列进"填了数却没断"，那是错的）：`Fresh` 那一支的 `assert_eq!(plan.stats, Stats::default())`
    ///   把这一格钉成 0；而 `Resume` 那一支往夹具里塞的 `bytes_duplicated: 6` **确实仍然没断**
    ///   （那一支只断 `retries` 与 `bytes_delivered`）⇒ "6 原样带过去"这件事没有判据。
    /// - **填了数却没断**的只剩一处：`tests/integration.rs` 铺断点文件的那处夹具塞的是 0。
    ///
    /// 一句话总结强度：上面这些断言钉的全是"**口径怎么算**"（给定归属与落盘量 ⇒ 期望值）与
    /// "**不许折成 0**"；**没有任何**断言把这一格与"真被重下的字节数"钉在一起。
    ///
    /// 也**没有**任何断言把这一格与线上多打的字节数钉到 KiB 量级上 —— 原先这里写的"实测差 ≤ 7 KiB"
    /// 没有任何断言支撑（T8 评审 A3），已按实际断言强度改写。
    pub bytes_duplicated: u64,
    /// 快速/慢速重试的总次数（spec §6.1/§6.2）。
    pub retries: u32,
}

impl Snapshot {
    /// 由任务 + 已验证事实 + 覆盖集组一份快照：`v = STATE_VERSION`、`stats` 归零、
    /// `needs_secret = task.password.is_some()`、`saved_at` = 此刻的 UTC 墙钟。
    ///
    /// 两道闸（都只挡本模块自己的契约，不改 `Task`/`Scheduler` 的任何语义）：
    /// 1. `task.validate()` —— 落盘点是 spec §4 凭据硬规约的**第二个**强制点：`validate()` 现在只由
    ///    引擎在跑之前调，但谁都可能直接调 `from_task`，凭据规则不该只在"跑起来那一刻"生效。
    ///    规则本身（明文 HTTP 带口令、以及 **E9** 的 userinfo URL）**只住在 `Task::validate()` 里**，
    ///    本模块不复述实现，只保证从这里出去的任务都过过它。
    ///    错误原样上抛（`Task::validate` 的文案已经说明是口令/userinfo），不加 `context` —— anyhow 的
    ///    `to_string()` 只显示最外层，包一层会把"口令"三个字从消息里挤掉。
    /// 2. `check_segments` —— 供数必须是 E7 那个极小覆盖集，写坏的文件比不写更糟（下一轮照着它跳段）。
    pub fn from_task(task: &Task, validator: Validator, segments: Vec<SegView>) -> anyhow::Result<Snapshot> {
        task.validate()?;
        check_segments(&segments, validator.total_len)?;
        Ok(Snapshot {
            v: STATE_VERSION,
            task: TaskView::from(task),
            validator,
            segments,
            stats: Stats::default(),
            needs_secret: task.password.is_some(),
            saved_at: utc_now(),
        })
    }
}

#[cfg(test)]
impl Snapshot {
    /// 计划第 682 行 Task 5 的表驱动用例用的构造器：不经 `Task`（它带口令）造一份最小快照。
    /// 只在 `cfg(test)` 下编译，不进发布 API；`task` 里的字段取"与 validator 自洽"的占位值。
    ///
    /// **段表走与 `save` 同一条闸（`check_segments`），违约直接 panic。** 少这道闸的失败模式是隐形的：
    /// 夹具能造出 `[(0,9),(10,19)]` 这种相邻两行（Task 5 表驱动最容易写错的一支），而真实的 `save`
    /// 永远拒写它 —— 于是续传断言是在对一个"盘上不可能存在"的形状成立，要拖到 Task 8 才看得见。
    /// 取 panic 不取 `Result`：这是测试脚手架里的程序员错误（写夹具的人当场就该知道），
    /// 返回 `Result` 只会让每个调用点为一个不该存在的失败模式多写一层 unwrap。
    pub fn stub_for_test(validator: Validator, segments: Vec<SegView>) -> Snapshot {
        if let Err(e) = check_segments(&segments, validator.total_len) {
            panic!("stub_for_test 收到了 `save` 会拒绝的段表，拒绝构造这份夹具：{e}");
        }
        let task = TaskView {
            url: validator.final_url.clone(),
            referer: None,
            cookies: None,
            data_file: None,
            user: None,
            dir: PathBuf::from("."),
            name: None,
            flags: 0,
        };
        Snapshot {
            v: STATE_VERSION,
            task,
            validator,
            segments,
            stats: Stats::default(),
            needs_secret: false,
            saved_at: format_utc(0),
        }
    }
}

/// `<目标>.dlm.json`：与目标同目录、在**目标文件名之后追加**后缀（spec §4）。
///
/// 两个刻意的写法：
/// - 不用 `with_extension`：那会把 `big.zip` 变成 `big.dlm.json`（吞掉目标自己的扩展名），
///   而 spec 要的是 `big.zip.dlm.json`。用例 `state_path_is_appended_and_never_replaces_the_extension` 钉住。
/// - 用 `OsString::push` 而不是 `format!("{name}.dlm.json", name = ...display())`：
///   文件名的原始字节原样保留，非 UTF-8 的文件名不被 lossy 改写（Windows 上代理对也一样保得住）。
pub fn path_for(target: &Path) -> PathBuf {
    match target.file_name() {
        Some(name) => {
            let mut s = name.to_os_string();
            s.push(STATE_SUFFIX);
            target.with_file_name(s)
        }
        // `""` / `"/"` / `"C:\` 这类没有文件名的路径不是合法目标（`Task::target_path` 造不出它），
        // 但这里也不该 panic 或返回 `None`：退化成"整个路径 + 后缀"，让后面真正的失败在开文件时发生。
        None => PathBuf::from(format!("{}{STATE_SUFFIX}", target.display())),
    }
}

/// 原子写：整份 JSON → `<状态文件>.tmp` → `rename` 覆盖到状态文件。
///
/// **Windows 上 `std::fs::rename` 的语义**（std 文档 + 本机用例实测，二者都对得上）：
/// - 目标**已存在**时整份替换（std："replaces the original file if `to` already exists"；
///   Windows 10 1607+ 且文件系统支持 `FileRenameInfoEx` 时行为与 Unix 一致 —— 本机 `target/` 在 NTFS 上）。
///   用例 `save_replaces_an_existing_state_file_on_windows` 走的就是这一支，替换失败会当场红。
/// - 同卷 rename 在 NTFS 里是一次目录项元数据事务 ⇒ 观察者任何时刻只看得到"旧的那份完整文件"或
///   "新的那份完整文件"，看不到半截、也看不到"文件不见了"。用例
///   `a_concurrent_reader_never_sees_a_half_written_record` 用另一个线程持续 `load` 观察这一条 ——
///   它是**回归网**（挡住"以后有人改成直接 `fs::write(目标)`"），不是内核级原子性的证明。
/// - 局限要说清：std 现在的文档**没有**承诺 rename 与"别人正在读这个文件"的并发关系（本机核过
///   `std::fs::rename` 页面，只写了替换语义与 `FileRenameInfoEx` 那条平台分支）。所以本模块对
///   "另一个进程正开着这个断点文件"这种情形**不写断言、也不做兜底**：不做"先删后写"（那会在删除与
///   写入之间留一个真的空窗，比 rename 失败更糟），rename 失败一律原样上抛给调用方
///   （spec §4 的"写者唯一"本来就不该出现两个写者）。
///   能实测的那一支实测了：**同进程自己开着目标文件**时 rename 照样成功
///   （std 开文件一律带 `FILE_SHARE_DELETE`，Task 3 的 `len_follows_the_handle_not_the_path`
///   正是靠这点改名了一个正被打开的文件）⇒ 见 `save_succeeds_while_the_destination_is_open_by_this_process`。
/// - **不做 fsync。** v1 的中断模型是"进程被杀、机器还在"（与 `src/store.rs` 同一立场），那种情形下
///   `WriteFile` 交回的字节对后来的读者本来就已可见；断电级保证不在需求里，也不许从这里衍生。
///   `FlushFileBuffers` 的实测代价（Task 3：2.4–3.2 ms/次起）正好是"别顺手加"的量化理由。
///
/// 不建父目录（与 `Store::open` 一致，`task.dir` 由 cli/engine 保证存在），失败信息带上路径。
pub fn save(path: &Path, snap: &Snapshot) -> anyhow::Result<()> {
    check_segments(&snap.segments, snap.validator.total_len)
        .with_context(|| format!("断点文件段表不合法，拒绝写盘：{}", path.display()))?;
    let text = serde_json::to_string_pretty(snap)
        .with_context(|| format!("序列化断点文件失败：{}", path.display()))?;
    let tmp = temp_path(path);
    std::fs::write(&tmp, text)
        .with_context(|| format!("写断点文件临时文件失败（父目录不存在或不可写？）：{}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("断点文件原子替换失败：{} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

/// 读断点文件。**`Ok(None)` 只表示"文件不存在"**：
/// 存在而读不懂（截断、0 字节、非 JSON、版本不符、未知键、段表违约）一律 `Err` ——
/// 把损坏当成"没有断点"会让下一轮从 0 重下，还会把"这个 etag 下哪些字节已经算数了"这条
/// 唯一记录抹掉，所以损坏必须显式失败（决策记在模块文档第 4 条）。
///
/// 父目录不存在同样给 `Ok(None)`：续传起步时目录可能还没建，那是"没有断点"而不是故障。
pub fn load(path: &Path) -> anyhow::Result<Option<Snapshot>> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(anyhow::Error::new(e).context(format!("读断点文件失败：{}", path.display()))),
    };
    let snap: Snapshot = serde_json::from_str(&raw).with_context(|| {
        format!(
            "断点文件读不懂，已拒绝当作「没有断点」：{} —— 当成没有会把已验证过的字节重新算成待下载；\
             确认要重来请删掉这个文件",
            path.display()
        )
    })?;
    if snap.v != STATE_VERSION {
        anyhow::bail!(
            "断点文件版本不兼容：{} 里写的是 v={}，本程序只认 v={STATE_VERSION} —— 确认要重来请删掉这个文件",
            path.display(),
            snap.v
        );
    }
    check_segments(&snap.segments, snap.validator.total_len)
        .with_context(|| format!("断点文件段表不合法：{}", path.display()))?;
    Ok(Some(snap))
}

/// 此刻的 UTC 墙钟，`Snapshot::saved_at` 的取值。**公开给落盘方刷新时间戳用**：
/// `save(path, &snap)` 拿的是不可变引用（plan 第 627 行的签名），自己盖不了时间戳，
/// 所以 Task 8 每次保存前 `snap.saved_at = state::utc_now();`。不参与任何判定（见 `Snapshot::saved_at`）。
pub fn utc_now() -> String {
    format_utc(epoch_secs_now())
}

/// 覆盖集里的字节数 = "归属已定、不再下载"的字节数。
/// **不是磁盘上的落盘量**（E7/T2-D）：416 封存的行含从未写入的字节；落盘量问 `store::Store::len()`。
pub fn confirmed_bytes(snap: &Snapshot) -> u64 {
    snap.segments.iter().map(SegView::len).sum()
}

/// 段表必须是 E7 那个**极小覆盖集**：升序、两两不重叠、**也不相邻**（相邻行必然已被
/// `scheduler::push_done` 合并过 ⇒ 出现两行说明供数没走 `snapshot_intervals()`）、
/// 每行都是 `start <= end < total_len`。
///
/// 写入侧（`from_task`/`save`）与读取侧（`load`）都调它：前者挡 Task 8 传错东西，
/// 后者挡被外部改过的文件。本函数**不做合并、不做排序** —— 修数据不是这里的活，
/// 静默修好只会让真正的供数缺陷查不出来。
fn check_segments(segments: &[SegView], total_len: u64) -> anyhow::Result<()> {
    let mut prev_end: Option<u64> = None;
    for (i, s) in segments.iter().enumerate() {
        anyhow::ensure!(!s.is_empty(), "段表第 {i} 行起点晚于终点：[{}, {}]", s.start, s.end);
        if let Some(p) = prev_end {
            anyhow::ensure!(
                s.start > p + 1,
                "段表第 {i} 行与上一行重叠或相邻（本行起点 {} ≤ 上一行终点 {p} + 1）：\
                 进文件的必须是 snapshot_intervals() 那个已合并的极小覆盖集",
                s.start
            );
        }
        // `total_len = 0` 时这条对任何行都成立 ⇒ 空文件不许有段表；u64 比较，不会下溢。
        anyhow::ensure!(
            s.end < total_len,
            "段表第 {i} 行的终点 {} 超出已验证的文件长度 {total_len}",
            s.end
        );
        prev_end = Some(s.end);
    }
    Ok(())
}

/// `<状态文件>.tmp`（追加后缀，见 `TEMP_SUFFIX` 的说明）。
fn temp_path(state: &Path) -> PathBuf {
    let mut s = state.as_os_str().to_os_string();
    s.push(TEMP_SUFFIX);
    PathBuf::from(s)
}

/// 现在的 epoch 秒（可能为负：时钟被拨到 1970 之前时 `duration_since` 给 `Err`）。
fn epoch_secs_now() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        // 纪元左右被当成 1970 会是个更难查的谎，所以按负秒照实算（`saved_at` 只给人看，不值得为此报错）。
        Err(e) => -i64::try_from(e.duration().as_secs()).unwrap_or(i64::MAX),
    }
}

/// epoch 秒 → `YYYY-MM-DDThh:mm:ssZ`（**UTC**，不带本地时区、不带秒小数）。
/// 手算：不引 `chrono`（plan 的依赖白名单里没有它）。对照表见用例 `saved_at_is_utc_wall_clock`
/// （含 2000 闰 / 2100 不闰的世纪规则、跨日、纪元之前的负秒）。
fn format_utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", tod / 3_600, (tod % 3_600) / 60, tod % 60)
}

/// Howard Hinnant 的 `days_from_civil` 逆运算（`[1600, 9999]` 内精确，本用例覆盖 1900–9999）。
/// 天数以 0000-03-01 为纪元起点重新编号（`era`/`doe`/`yoe`/`doy`/`mp` 全落在非负区间，
/// 所以可以用 `u64` 跑，顺带保证 debug 构建里不会减法下溢）。
fn civil_from_days(days: i64) -> (i64, u64, u64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::Interval;
    use crate::task::Task;
    use std::path::{Path, PathBuf};

    const SECRET: &str = "s3cret";

    /// 每条用例一个**私有目录** `<仓库>/target/t4-scratch/<case>/`（`target/` 已在 `.gitignore` 里，
    /// `*.dlm.json` 也单独忽略）。cargo 用多个线程并行跑测试 ⇒ 目录按用例分开，才敢扫
    /// "这个目录里除了预期文件不该有别的断点文件"；入口先把上一轮崩在半路的残留清空。
    fn case_dir(case: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("t4-scratch")
            .join(case);
        std::fs::create_dir_all(&dir).expect("建 scratch 目录");
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        dir
    }

    /// 该用例的目标文件路径（断点文件由 `path_for` 挂在它旁边）。
    fn target_of(case: &str, file: &str) -> PathBuf {
        case_dir(case).join(file)
    }

    /// 目标文件落在 `target` 状态的最小任务；`user`/`password` 由用例自己给。
    fn task_at(target: &Path, user: Option<&str>, password: Option<&str>) -> Task {
        Task {
            url: "https://x/a.bin".into(),
            referer: None,
            cookies: None,
            data_file: None,
            user: user.map(str::to_string),
            password: password.map(str::to_string),
            dir: target.parent().expect("用例的目标路径必须有父目录").to_path_buf(),
            name: Some(
                target
                    .file_name()
                    .expect("用例的目标路径必须有文件名")
                    .to_string_lossy()
                    .into_owned(),
            ),
            flags: 0,
        }
    }

    fn validator(total_len: u64) -> Validator {
        Validator {
            etag: Some("E".into()),
            last_modified: Some("L".into()),
            total_len,
            final_url: "https://x/a.bin".into(),
            ignore_mtime: false,
        }
    }

    /// 覆盖集的行**只能来自 `scheduler::Interval`** —— 这条数据通路（E7）由测试自己走一遍。
    fn rows(spec: &[(u64, u64)]) -> Vec<SegView> {
        spec.iter()
            .map(|&(start, end)| SegView::from(Interval { start, end }))
            .collect()
    }

    /// `save` 用的临时文件名（`<状态文件>.tmp`）。测试自己拼一遍，为的是把命名约定钉住：
    /// 实现若改成 `path.with_extension("tmp")`，`big.zip.dlm.json` 会变成 `big.zip.dlm.tmp`（只换最后一段
    /// 扩展名）—— 那是**另一个**文件名，brief 给的 `assert!(!path.with_extension("tmp").exists())`
    /// 查的也正是这个名字，于是它永远为真（假断言，见 `save_is_atomic_and_roundtrips` 里那条注释）。
    fn tmp_of(state: &Path) -> PathBuf {
        let mut s = state.as_os_str().to_os_string();
        s.push(".tmp");
        PathBuf::from(s)
    }

    /// 递归收集 JSON 里的**全部**字符串（键名与字符串值都算）：转义过的、嵌套的、以后新加字段里的
    /// 口令都躲不过这一步。`raw.contains(口令)` 只对"不需要转义的口令"有效。
    fn all_strings(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::Object(map) => {
                for (k, val) in map {
                    out.push(k.clone());
                    all_strings(val, out);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    all_strings(item, out);
                }
            }
            serde_json::Value::String(s) => out.push(s.clone()),
            _ => {}
        }
    }

    fn collect_keys(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::Object(map) => {
                for (k, val) in map {
                    out.push(k.clone());
                    collect_keys(val, out);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    collect_keys(item, out);
                }
            }
            _ => {}
        }
    }

    /// 目录里属于断点文件的兄弟文件（`.dlm.json` 与其 `.tmp`）必须只有 `allowed` 这几个。
    fn assert_no_stray_state_files(dir: &Path, allowed: &[&Path]) {
        for entry in std::fs::read_dir(dir).expect("列 scratch 目录") {
            let p = entry.expect("读目录项").path();
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if !(name.ends_with(".dlm.json") || name.ends_with(".dlm.json.tmp")) {
                continue; // 目标文件本身不归本条管
            }
            assert!(
                allowed.contains(&p.as_path()),
                "断点文件写到了预期之外的位置：{}（预期只有 {:?}）",
                p.display(),
                allowed.iter().map(|a| a.display()).collect::<Vec<_>>()
            );
        }
    }

    // ---- brief Step 1 的两条用例（brief 的形状逐字保留，`scratch` 落点换成受控目录）----

    #[test]
    fn password_never_written_to_state() -> anyhow::Result<()> {
        let dir = std::env::temp_dir().join("dlm-state-probe");
        std::fs::create_dir_all(&dir)?;
        let mut t = Task {
            url: "https://x/a".into(),
            referer: None,
            cookies: None,
            data_file: None,
            user: Some("u".into()),
            password: Some("s3cret".into()),
            dir: dir.clone(),
            name: None,
            flags: 0,
        };
        let p = path_for(&t.target_path());
        let snap = Snapshot::from_task(
            &t,
            Validator {
                etag: None,
                last_modified: None,
                total_len: 10,
                final_url: t.url.clone(),
                ignore_mtime: false,
            },
            vec![],
        )?;
        assert!(snap.needs_secret);
        save(&p, &snap)?;
        let raw = std::fs::read_to_string(&p)?;
        assert!(!raw.contains("s3cret"), "断点文件里出现了口令");
        assert!(raw.contains("\"needs_secret\": true"));
        std::fs::remove_file(&p)?;
        t.password = None;
        let snap2 = Snapshot::from_task(&t, snap.validator.clone(), vec![])?;
        assert!(!snap2.needs_secret);
        Ok(())
    }

    #[test]
    fn save_is_atomic_and_roundtrips() -> anyhow::Result<()> {
        // brief 要求把这条写成**真实断言**（save→load 相等、无 .tmp 残留），不许留 `unimplemented!`。
        let target = target_of("t4_roundtrip", "a.bin");
        let t = task_at(&target, Some("u"), Some(SECRET));
        let p = path_for(&target);
        let snap = Snapshot::from_task(&t, validator(1_000), rows(&[(0, 499), (700, 999)]))?;
        save(&p, &snap)?;

        let loaded = load(&p)?;
        assert_eq!(loaded.as_ref(), Some(&snap), "save→load 必须一模一样（含 needs_secret / v / saved_at）");
        assert!(!tmp_of(&p).exists(), "rename 之后不该留下 .tmp");
        // brief 逐字的那条：它查的是 `p.with_extension("tmp")` ⇒ `a.bin.dlm.tmp`，与本模块真正用的
        // `<p>.tmp`（= `a.bin.dlm.json.tmp`）不是同一个文件，所以它单独存在时恒为真（假断言，见 `tmp_of`
        // 的文档）。保留它，真闸是下面两条。
        assert!(!p.with_extension("tmp").exists());
        assert_no_stray_state_files(target.parent().unwrap(), &[&p]);

        // 再存一次（改点计数）：覆盖已存在的状态文件，照样一字不差地读回来。
        let mut again = snap.clone();
        again.stats.bytes_delivered = 1_000;
        again.stats.retries = 3;
        save(&p, &again)?;
        assert_eq!(load(&p)?.as_ref(), Some(&again), "第二次 save 必须整份替换，不残留上一版");
        assert!(!tmp_of(&p).exists());
        std::fs::remove_file(&p)?;
        assert_eq!(load(&p)?, None, "删掉状态文件之后 load 给 Ok(None)");
        Ok(())
    }

    // ---- 落点命名：path_for ----

    #[test]
    fn state_path_is_appended_and_never_replaces_the_extension() -> anyhow::Result<()> {
        let dir = case_dir("t4_naming");
        let cases = [
            ("big.zip", "big.zip.dlm.json"), // 追加，不是换扩展名（`with_extension` 的写法错在这里）
            ("a", "a.dlm.json"),
            ("archive.tar.gz", "archive.tar.gz.dlm.json"),
            ("中文 文件.zip", "中文 文件.zip.dlm.json"), // 非 ASCII 文件名不得被 lossy 转换
        ];
        for (name, expected) in cases {
            let target = dir.join(name);
            let got = path_for(&target);
            assert_eq!(
                got.file_name().map(|n| n.to_string_lossy().into_owned()).as_deref(),
                Some(expected),
                "目标 {}",
                target.display()
            );
            assert_eq!(got.parent(), Some(target.parent().unwrap()), "断点文件与目标同目录（spec §4）");
        }
        Ok(())
    }

    // ---- 口令不外泄的其它通道 ----

    #[test]
    fn no_key_anywhere_in_the_record_is_named_like_a_credential() -> anyhow::Result<()> {
        // spec §4 只放行 `needs_secret` 这一个"看起来像凭据"的键名，其余一律不许出现。
        let target = target_of("t4_keys", "a.bin");
        let t = task_at(&target, Some("someone"), Some(SECRET));
        let snap = Snapshot::from_task(&t, validator(10), rows(&[(0, 9)]))?;
        let value = serde_json::to_value(&snap).expect("快照可序列化");
        let mut strings = vec![];
        all_strings(&value, &mut strings);
        let mut keys = vec![];
        collect_keys(&value, &mut keys);
        let forbidden = ["password", "passwd", "pass", "pwd", "credential", "token", "encpassword"];
        for key in &keys {
            let low = key.to_ascii_lowercase();
            assert!(
                forbidden.iter().all(|f| !low.contains(f)) || low == "needs_secret",
                "键名 {key} 像是凭据字段：断点文件里只允许 needs_secret 这一处擦边（{keys:?}）"
            );
        }
        assert!(keys.contains(&"needs_secret".to_string()));
        assert!(strings.iter().all(|s| !s.contains(SECRET)), "序列化结果里任何一处都不得出现口令");
        Ok(())
    }

    #[test]
    fn debug_output_carries_no_secret_either() -> anyhow::Result<()> {
        // 另一个泄漏通道是日志：Task 8/9 会打 `{:?}`/`{e:?}`。TaskView 没这个字段 ⇒ 打不出来。
        // （`Task` 本尊那条通道由 `src/task.rs` 的手写 `Debug` 掩码，用例
        // `debug_prints_the_password_field_masked` 钉着；本模块从来不格式化 `Task`。）
        let target = target_of("t4_debug", "a.bin");
        let t = task_at(&target, Some("someone"), Some(SECRET));
        let snap = Snapshot::from_task(&t, validator(10), rows(&[(0, 9)]))?;
        assert!(snap.needs_secret);
        assert!(!format!("{snap:?}").contains(SECRET), "Debug 里也不能有口令");
        assert!(!format!("{:?}", snap.task).contains(SECRET));
        Ok(())
    }

    #[test]
    fn a_userinfo_url_never_reaches_the_disk() -> anyhow::Result<()> {
        // 勘误 E9：口令写在 URL 的 userinfo 里时，"`TaskView` 少一个字段"那条防线挡不住（它是自由文本），
        // 而本模块又删不掉 URL（续传要用）。规则住在 `Task::validate()`（`src/task.rs`）那一处，
        // 本模块只保证一件事：**落盘口经过它**（`from_task` 调 `task.validate()`）。
        let target = target_of("t4_userinfo", "a.bin");
        let mut t = task_at(&target, Some("alice"), Some(SECRET));
        t.url = format!("https://alice:{SECRET}@x/a.bin");
        let err = Snapshot::from_task(&t, validator(10), rows(&[(0, 9)]))
            .expect_err("authority 里带凭据的任务不能被写进断点文件");
        let msg = err.to_string();
        assert!(msg.contains("--user") && msg.contains("--password"), "错误要指使用哪两个参数：{msg}");
        assert!(!msg.contains(SECRET), "错误消息自己不该把口令抄一遍：{msg}");
        assert!(!path_for(&target).exists(), "拒了就不该顺手写盘");
        // 把凭据改写成两个参数（我们期望用户走的形状）后，同一个任务写得动、读得回，且盘上没有口令。
        t.url = "https://x/a.bin".into();
        let snap = Snapshot::from_task(&t, validator(10), rows(&[(0, 9)]))?;
        let p = path_for(&target);
        save(&p, &snap)?;
        let raw = std::fs::read_to_string(&p)?;
        assert!(!raw.contains(SECRET), "断点文件里没有口令：{raw}");
        assert_eq!(load(&p)?.as_ref(), Some(&snap));
        std::fs::remove_file(&p)?;
        Ok(())
    }

    #[test]
    fn a_secret_that_needs_json_escaping_still_never_leaks() -> anyhow::Result<()> {
        // 带引号/反斜杠/中文的口令在原文里是**转义后**的形态，`raw.contains(口令)` 会因为形态不同而漏判；
        // 所以这条既查原文（含一个不依赖转义的片段），也查解码后递归收集到的每个字符串。
        let secret = "p\"ss\\词 1";
        let target = target_of("t4_escapes", "a.bin");
        let mut t = task_at(&target, Some("someone"), Some(secret));
        let snap = Snapshot::from_task(&t, validator(10), rows(&[(0, 9)]))?;
        let p = path_for(&target);
        save(&p, &snap)?;
        let raw = std::fs::read_to_string(&p)?;
        let value: serde_json::Value = serde_json::from_str(&raw)?;
        let mut strings = vec![];
        all_strings(&value, &mut strings);
        assert!(strings.iter().all(|s| !s.contains(secret)), "解码后的某个字符串含口令：{strings:?}");
        assert!(!raw.contains(secret), "原文里不该有口令本尊");
        assert!(!raw.contains("词"), "原文里连口令里那段不受转义影响的片段都不该有");
        assert!(snap.needs_secret, "口令照样得被记成\"需要口令\"");
        t.password = None;
        assert!(
            !Snapshot::from_task(&t, validator(10), rows(&[(0, 9)]))?.needs_secret,
            "没给口令 ⇒ needs_secret 为假"
        );
        std::fs::remove_file(&p)?;
        Ok(())
    }

    #[test]
    fn username_is_persisted_by_ruling() -> anyhow::Result<()> {
        // 裁定：**用户名落盘**（spec §4 的硬规约点名的是口令；同节示例 JSON 里就写着 "user": null）。
        // 同一条理由同样覆盖 `referer` 与 `cookies` —— 示例里也写了 `"cookies": null`，本模块照抄 spec 的形状。
        // cookie 也是凭据，这一点已知并登记在 Task 1 的遗留清单里（"本轮不自作规则，留给 T9"）：
        // 想改成"cookie 也不落盘"要动的是 spec §4 的示例，不是这里的实现，所以让它在这里红一次。
        let target = target_of("t4_user", "a.bin");
        let mut t = task_at(&target, Some("alice"), Some(SECRET));
        t.cookies = Some("sid=42".into());
        let snap = Snapshot::from_task(&t, validator(10), rows(&[(0, 9)]))?;
        let p = path_for(&target);
        save(&p, &snap)?;
        let raw = std::fs::read_to_string(&p)?;
        assert!(raw.contains("\"user\": \"alice\""), "用户名按 spec 形状落盘：{raw}");
        assert!(raw.contains("\"cookies\": \"sid=42\""), "cookie 同样照 spec 形状落盘（已知风险，见注释）");
        assert!(!raw.contains(SECRET), "同一份文件里唯独没有口令");
        assert_eq!(load(&p)?.unwrap().task.user.as_deref(), Some("alice"));
        std::fs::remove_file(&p)?;
        Ok(())
    }

    // ---- 原子写 ----

    #[test]
    fn save_replaces_an_existing_state_file_on_windows() -> anyhow::Result<()> {
        // `std::fs::rename` 在 Windows 上是否替换已存在的目标，是本机必须实测而不是假设的事
        // （std 文档："replaces the original file if `to` already exists"；Win10 1607+ 且文件系统
        //  支持 FileRenameInfoEx 时行为与 Unix 相同 —— NTFS 支持）。这条走的就是**状态文件已存在**那一支。
        let target = target_of("t4_overwrite", "a.bin");
        let p = path_for(&target);
        let first = Snapshot::from_task(&task_at(&target, None, None), validator(10), rows(&[(0, 9)]))?;
        save(&p, &first)?;
        let second = Snapshot::from_task(&task_at(&target, None, None), validator(20), rows(&[]))?;
        save(&p, &second).expect("第二次 save 撞上已存在的状态文件必须成功替换，不能报\"目标已存在\"");
        assert_eq!(load(&p)?.as_ref(), Some(&second), "读回来必须已是新版");
        let raw = std::fs::read_to_string(&p)?;
        assert!(!raw.contains("\"total_len\": 10"), "旧版内容不得残留：{raw}");
        assert!(!tmp_of(&p).exists(), "替换之后 .tmp 不该还在");
        assert_no_stray_state_files(target.parent().unwrap(), &[&p]);
        std::fs::remove_file(&p)?;
        Ok(())
    }

    #[test]
    fn save_succeeds_while_the_destination_is_open_by_this_process() -> anyhow::Result<()> {
        // Windows 的 rename 要求**已存在的目标**把 DELETE 共享让出来；std 开文件默认带
        // `FILE_SHARE_DELETE`（Task 3 的 `len_follows_the_handle_not_the_path` 正是靠这点改名了一个
        // 正被打开的文件）。这条把同一个事实搬到本模块的路径上实测：读者正握着状态文件，写者照样换得掉。
        // 反过来说，若哪天"另一个进程以不让 DELETE 共享的方式开着它"（记事本 / 杀毒扫描），
        // `std::fs::rename` 会返回 Err —— 本模块照实上抛、不做"先删后写"的兜底（理由见 `save` 文档），
        // 那种对端用 std 造不出来（它不给"去掉 FILE_SHARE_DELETE"的选项），所以这里**不写断言**。
        let target = target_of("t4_open_dst", "a.bin");
        let p = path_for(&target);
        let first = Snapshot::from_task(&task_at(&target, None, None), validator(10), rows(&[(0, 9)]))?;
        save(&p, &first)?;
        let held = std::fs::File::open(&p).with_context(|| format!("读者句柄开不了：{}", p.display()))?;
        let second = Snapshot::from_task(&task_at(&target, None, None), validator(30), rows(&[(0, 29)]))?;
        save(&p, &second)
            .unwrap_or_else(|e| panic!("目标被本进程开着时 rename 竟然失败（那 tmp+rename 就站不住了）：{e}"));
        assert_eq!(load(&p)?.as_ref(), Some(&second), "换掉的必须是新版");
        drop(held);
        assert!(!tmp_of(&p).exists(), ".tmp 不得残留");
        std::fs::remove_file(&p)?;
        Ok(())
    }

    #[test]
    fn a_stale_tmp_is_overwritten_and_never_read_back() -> anyhow::Result<()> {
        // 上一轮被杀在 write 与 rename 之间 ⇒ `.tmp` 里留着一份半截/过期的 JSON。
        // 下一轮必须：照样写得动、读回来的是本轮内容、`.tmp` 被 rename 消耗掉。
        let target = target_of("t4_stale_tmp", "a.bin");
        let p = path_for(&target);
        std::fs::write(tmp_of(&p), b"{ \"v\": 1, \"task\": {\"url\"")?; // 故意留一份截断的残留
        let snap = Snapshot::from_task(&task_at(&target, None, None), validator(10), rows(&[(0, 9)]))?;
        save(&p, &snap)?;
        assert_eq!(
            load(&p)?.as_ref(),
            Some(&snap),
            "读回来的必须是本轮那份，而不是残留 .tmp 的内容"
        );
        assert!(!tmp_of(&p).exists(), ".tmp 应当已被 rename 消耗");
        std::fs::remove_file(&p)?;
        Ok(())
    }

    #[test]
    fn a_concurrent_reader_never_sees_a_half_written_record() -> anyhow::Result<()> {
        // "原子替换"可被另一个观察者看到的**唯一**性质：任何时刻读都只得到完整记录
        // （既不是 ENOENT，也不是截断 JSON）。这正是 tmp+rename 相对 `fs::write(path)` 的区别。
        // 定位：回归网。真原子性的依据是 NTFS 同卷 rename 是一个元数据事务（见 `save` 文档），
        // 这条挡的是"以后有人图省事改成直接覆盖写"这一类退化。
        let target = target_of("t4_reader_race", "a.bin");
        let p = path_for(&target);
        let t = task_at(&target, None, None);
        let mut snap = Snapshot::from_task(&t, validator(100_000), rows(&[(0, 99_999)]))?;
        save(&p, &snap)?; // 先让文件存在，读者才有东西可看

        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let watch = p.clone();
        let watching = std::sync::Arc::clone(&done);
        let reader = std::thread::spawn(move || {
            let mut seen = 0usize;
            while !watching.load(std::sync::atomic::Ordering::Acquire) {
                match load(&watch) {
                    Ok(None) => {
                        let _ = tx.send(format!("第 {seen} 次读看到 Ok(None)：记录凭空消失过"));
                        break;
                    }
                    Ok(Some(s)) => {
                        seen += 1;
                        if s.segments.len() != 1 || s.validator.total_len == 0 {
                            let _ = tx.send(format!("读到结构不完整的记录：{s:?}"));
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(format!("第 {seen} 次读遇到错误：{e}"));
                        break;
                    }
                }
            }
            seen
        });
        for i in 0..400 {
            snap.stats.bytes_delivered = i * 500;
            snap.segments = rows(&[(0, 99_999 - i * 100)]);
            save(&p, &snap)?;
        }
        done.store(true, std::sync::atomic::Ordering::Release);
        let reads = reader.join().expect("读者线程不该 panic");
        eprintln!("读者线程在 {reads} 次连续 load 中始终读到完整记录（写者做了 400 次 save）");
        let anomalies: Vec<String> = rx.try_iter().collect();
        assert!(anomalies.is_empty(), "并发读者发现了非原子写：{anomalies:?}");
        assert!(reads > 0, "读者一次都没读到过 ⇒ 这条用例没跑起来（并发失效也要能发现）");
        assert_eq!(load(&p)?.as_ref(), Some(&snap), "最后一次的写必须就是最终态");
        assert!(!tmp_of(&p).exists());
        std::fs::remove_file(&p)?;
        Ok(())
    }

    #[test]
    fn save_reports_the_destination_path_and_never_creates_parent_dirs() -> anyhow::Result<()> {
        let dir = case_dir("t4_no_parent");
        let parent = dir.join("missing-sub");
        let p = parent.join("a.bin.dlm.json");
        let snap = Snapshot::from_task(&task_at(&p, None, None), validator(1), vec![])?;
        let err = save(&p, &snap)
            .expect_err("父目录不存在时 save 必须返回 Err，不能 panic、也不能自愈");
        assert!(err.to_string().contains("a.bin.dlm.json"), "错误里要带上路径：{err}");
        assert!(!parent.exists(), "本模块不建父目录（那是 cli/engine 的活，与 store.rs 一致）");
        Ok(())
    }

    // ---- load 的几种失败与一种"没有" ----

    #[test]
    fn load_of_a_missing_file_is_ok_none() -> anyhow::Result<()> {
        let dir = case_dir("t4_absent");
        let p = dir.join("nope.bin.dlm.json");
        assert_eq!(load(&p)?, None, "没有断点文件 = 全新任务，不是错误");
        // 连父目录都不存在也照给 Ok(None)：续传起步时目标目录可能还没建，那是"没有断点"而不是故障。
        let deeper = dir.join("no-such-dir").join("nope.bin.dlm.json");
        assert_eq!(load(&deeper)?, None);
        assert!(!deeper.parent().unwrap().exists());
        Ok(())
    }

    #[test]
    fn load_rejects_truncated_or_empty_json_instead_of_no_state() -> anyhow::Result<()> {
        let target = target_of("t4_truncated", "a.bin");
        let p = path_for(&target);
        let snap = Snapshot::from_task(&task_at(&target, None, None), validator(10), rows(&[(0, 9)]))?;
        save(&p, &snap)?;
        let mut raw = std::fs::read_to_string(&p)?;
        let cut = raw.len() / 2;
        raw.truncate(cut);
        std::fs::write(&p, &raw)?;
        let err = load(&p)
            .expect_err("截断的断点文件必须是 Err，不许当成\"没有断点\"");
        assert!(err.to_string().contains("t4_truncated"), "错误里要带路径：{err}");

        std::fs::write(&p, "")?;
        assert!(load(&p).is_err(), "0 字节的断点文件同样必须 Err");
        std::fs::write(&p, "not json at all")?;
        assert!(load(&p).is_err());
        std::fs::remove_file(&p)?;
        Ok(())
    }

    #[test]
    fn load_rejects_an_unknown_version() -> anyhow::Result<()> {
        let target = target_of("t4_version", "a.bin");
        let p = path_for(&target);
        let mut snap = Snapshot::from_task(&task_at(&target, None, None), validator(10), rows(&[(0, 9)]))?;
        snap.v = STATE_VERSION + 1;
        std::fs::write(&p, serde_json::to_string_pretty(&snap)?)?;
        let err = load(&p)
            .expect_err("未来版本的断点文件必须 Err，不能按旧格式猜着读");
        assert!(err.to_string().contains("版本"), "错误要说明是版本不符：{err}");
        snap.v = 0;
        std::fs::write(&p, serde_json::to_string_pretty(&snap)?)?;
        assert!(load(&p).is_err(), "v=0（含别家工具的伪文件）同样拒读");
        assert_eq!(STATE_VERSION, 1, "spec §4 示例的 \"v\": 1");
        snap.v = STATE_VERSION;
        std::fs::write(&p, serde_json::to_string_pretty(&snap)?)?;
        assert_eq!(load(&p)?.as_ref(), Some(&snap), "本版本读写自如");
        std::fs::remove_file(&p)?;
        Ok(())
    }

    #[test]
    fn load_rejects_a_field_we_do_not_write() -> anyhow::Result<()> {
        // `deny_unknown_fields` 的三个方向，每个都对应一种真实的坏文件：
        let target = target_of("t4_extra_key", "a.bin");
        let p = path_for(&target);
        let snap = Snapshot::from_task(&task_at(&target, None, None), validator(10), rows(&[(0, 9)]))?;
        save(&p, &snap)?;
        let good = std::fs::read_to_string(&p)?;

        // ① 键名拼错 ⇒ 该字段变成"缺失"（`needs_secret: bool` 没有 default）⇒ Err，不能静默默认成 false。
        std::fs::write(&p, good.replace("\"needs_secret\":", "\"neds_secret\":"))?;
        assert!(load(&p).is_err(), "拼错的键不能让字段悄悄落到默认值");

        // ② 顶层多一个我们不写的键 ⇒ Err。少了 `deny_unknown_fields` 这就是"无声丢掉半份信息"：
        //    文件里明明有东西，我们读的时候当它不存在。
        std::fs::write(&p, good.replace("\"needs_secret\": false", "\"needs_secret\": false, \"bytes_pending\": 42"))?;
        assert!(load(&p).is_err(), "顶层未知键必须拒读（deny_unknown_fields）");

        // ③ 别的工具（或某个忘了 skip 的自己人）写出一份带 `password` 的同后缀文件 ⇒ 必须**拒读**，
        //    而不是"忽略那个键、其余照用"。口令已经明文躺在别人写的那个文件里了，我们参与不进去。
        std::fs::write(&p, good.replace("\"user\": null", &format!("\"user\": null, \"password\": \"{SECRET}\"")))?;
        let err = load(&p).expect_err("task 视图里多出 password 键必须拒读，不能当作可忽略的噪音");
        assert!(err.to_string().contains("t4_extra_key"), "错误里要带路径，用户才知道该删哪个文件：{err}");

        std::fs::write(&p, &good)?;
        assert_eq!(load(&p)?.as_ref(), Some(&snap), "原样那份照样读得回来（上面三条不是恒假断言）");
        std::fs::remove_file(&p)?;
        Ok(())
    }

    #[test]
    fn a_segment_table_must_be_the_merged_cover_set() -> anyhow::Result<()> {
        // E7：进文件的是"已确认字节的极小覆盖集"。乱序/相邻/重叠/被包含/越界/空区间六种都违约 ⇒
        // 两侧都要拦：写入侧（from_task）挡住 Task 8 传错东西，读取侧（load）挡住被改过的文件。
        // "相邻"也算违规：覆盖集里相邻的两行必然已被 scheduler 合并（`push_done`），
        // 出现 `[0,9] [10,19]` 说明供数没走 `snapshot_intervals()`。
        let target = target_of("t4_segments", "a.bin");
        let t = task_at(&target, None, None);
        let bad: Vec<(Vec<SegView>, &str)> = vec![
            (rows(&[(10, 19), (0, 9)]), "乱序"),
            (rows(&[(0, 9), (10, 19)]), "相邻未合并"),
            (rows(&[(0, 19), (10, 29)]), "重叠"),
            (rows(&[(0, 19), (5, 6)]), "被包含"),
            (rows(&[(0, 10)]), "终点越界（total_len=10 ⇒ 合法终点最大是 9）"),
            (rows(&[(3, 1)]), "起点晚于终点"),
        ];
        for (segments, why) in bad {
            assert!(
                Snapshot::from_task(&t, validator(10), segments.clone()).is_err(),
                "{why} 的段表必须被 from_task 拒掉"
            );
        }
        // 合法形状：升序、互不相邻、都在 total_len 之内；整份下完就是**一条**（E7）。
        let ok = Snapshot::from_task(&t, validator(10), rows(&[(0, 3), (5, 5), (7, 9)]))?;
        assert_eq!(ok.segments.len(), 3);
        let p = path_for(&target);
        save(&p, &ok)?;
        assert_eq!(load(&p)?.as_ref(), Some(&ok));

        // 写入侧的第二道闸：绕开 `from_task`、直接改公开字段（Task 8 就是这么更新快照的）⇒ `save` 自己
        // 必须复查段表。少了这一道，"save 只信调用方"的实现同样能过上面那六条 from_task 断言。
        let mut mutated = ok.clone();
        mutated.segments = rows(&[(0, 3), (4, 5)]); // 相邻未合并
        assert!(save(&p, &mutated).is_err(), "save 必须自己复查段表，不能只信 from_task");
        assert_eq!(load(&p)?.as_ref(), Some(&ok), "被拒的那次 save 不得动到盘上的旧记录");

        // 读取侧：手写一份相邻两行的 JSON（其余字段都对）⇒ 仍然 Err。
        let tampered = format!(
            r#"{{
  "v": 1,
  "task": {{ "url": "https://x/a.bin", "referer": null, "cookies": null, "data_file": null,
            "user": null, "dir": "{}", "name": "a.bin", "flags": 0 }},
  "validator": {{ "etag": "E", "last_modified": "L", "total_len": 100, "final_url": "https://x/a.bin", "ignore_mtime": false }},
  "segments": [ {{ "start": 0, "end": 9 }}, {{ "start": 10, "end": 19 }} ],
  "stats": {{ "bytes_delivered": 0, "bytes_duplicated": 0, "retries": 0 }},
  "needs_secret": false,
  "saved_at": "2026-10-16T00:00:00Z"
}}"#,
            target.parent().unwrap().display().to_string().replace('\\', "\\\\")
        );
        std::fs::write(&p, &tampered)?;
        assert!(load(&p).is_err(), "被改坏的段表在读取侧同样要挡住");
        // 而 spec §4 示例那种"整份文件一段确认"必须读得进来（100 字节 ⇒ 合法终点 99）。
        std::fs::write(
            &p,
            tampered.replace(
                r#"[ { "start": 0, "end": 9 }, { "start": 10, "end": 19 } ]"#,
                r#"[ { "start": 0, "end": 99 } ]"#,
            ),
        )?;
        let single = load(&p)?.expect("单行全覆盖是合法形状（E7：整份下完就是一条）");
        assert_eq!(single.segments.len(), 1);
        assert_eq!(confirmed_bytes(&single), 100);
        std::fs::remove_file(&p)?;
        Ok(())
    }

    // ---- 形状与内容 ----

    #[test]
    fn json_keys_match_the_spec_example_shape() -> anyhow::Result<()> {
        // spec §4 的示例 JSON 就是文件格式的规范：顶层七个键、task 八个键（**没有 password**）、
        // validator 五个、segments 每行两个、stats 三个。这里的断言 = 本文件的对外契约。
        let target = target_of("t4_shape", "a.bin");
        let t = task_at(&target, Some("alice"), Some(SECRET));
        let snap = Snapshot::from_task(&t, validator(10), rows(&[(0, 9)]))?;
        let p = path_for(&target);
        save(&p, &snap)?;
        let raw = std::fs::read_to_string(&p)?;
        let v: serde_json::Value = serde_json::from_str(&raw)?;
        let top = v.as_object().expect("顶层是对象");
        let mut keys: Vec<&String> = top.keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec!["needs_secret", "saved_at", "segments", "stats", "task", "v", "validator"],
            "顶层键必须与 spec §4 一致"
        );
        assert_eq!(v["v"].as_u64(), Some(1));
        assert_eq!(v["needs_secret"].as_bool(), Some(true));
        let mut task_keys: Vec<&String> = top["task"].as_object().expect("task 是对象").keys().collect();
        task_keys.sort();
        assert_eq!(
            task_keys,
            vec!["cookies", "data_file", "dir", "flags", "name", "referer", "url", "user"],
            "task 视图 = Task 的九个字段去掉 password"
        );
        let mut val_keys: Vec<&String> = top["validator"]
            .as_object()
            .expect("validator 是对象")
            .keys()
            .collect();
        val_keys.sort();
        assert_eq!(val_keys, vec!["etag", "final_url", "ignore_mtime", "last_modified", "total_len"]);
        let mut stat_keys: Vec<&String> = top["stats"].as_object().expect("stats 是对象").keys().collect();
        stat_keys.sort();
        assert_eq!(stat_keys, vec!["bytes_delivered", "bytes_duplicated", "retries"]);
        let seg0 = top["segments"][0].as_object().expect("段行是对象");
        let mut seg_keys: Vec<&String> = seg0.keys().collect();
        seg_keys.sort();
        assert_eq!(seg_keys, vec!["end", "start"], "E7 的覆盖集一行只有起止两个偏移（闭区间，含端点）");
        assert_eq!(seg0["start"].as_u64(), Some(0));
        assert_eq!(seg0["end"].as_u64(), Some(9), "闭区间：total_len=10 的整份确认记到 9，不是 10");
        std::fs::remove_file(&p)?;
        Ok(())
    }

    #[test]
    fn stats_start_at_zero_and_survive_a_roundtrip() -> anyhow::Result<()> {
        // Task 8 靠这三个计数（plan 第 843 行写作 `stats.duplicated`，按勘误 **E9** 读作
        // `bytes_duplicated` —— 落盘名以本文件为准）。
        let target = target_of("t4_stats", "a.bin");
        let mut snap = Snapshot::from_task(&task_at(&target, None, None), validator(1_000), rows(&[(0, 499)]))?;
        assert_eq!(snap.stats, Stats::default(), "新建快照的计数从 0 起");
        snap.stats.bytes_delivered = 900;
        snap.stats.bytes_duplicated = 400; // 这一格存什么读回什么；口径（含 E17 撤账后的少报）见字段文档
        snap.stats.retries = 7;
        let p = path_for(&target);
        save(&p, &snap)?;
        let loaded = load(&p)?.expect("刚写的文件读得回来");
        assert_eq!(loaded.stats.bytes_delivered, 900);
        assert_eq!(loaded.stats.bytes_duplicated, 400);
        assert_eq!(loaded.stats.retries, 7);
        std::fs::remove_file(&p)?;
        Ok(())
    }

    #[test]
    fn segments_are_the_inclusive_cover_set() -> anyhow::Result<()> {
        let target = target_of("t4_cover", "a.bin");
        let snap = Snapshot::from_task(&task_at(&target, None, None), validator(1_000), rows(&[(0, 249), (500, 999)]))?;
        assert_eq!(snap.segments[0].len(), 250, "闭区间 [0,249] 是 250 字节");
        assert_eq!(snap.segments[1].len(), 500);
        assert_eq!(snap.segments[0].interval(), Interval { start: 0, end: 249 }, "行能回到 scheduler 的区间形状");
        assert_eq!(confirmed_bytes(&snap), 750);
        let p = path_for(&target);
        save(&p, &snap)?;
        assert_eq!(confirmed_bytes(&load(&p)?.unwrap()), 750);
        std::fs::remove_file(&p)?;
        Ok(())
    }

    // ---- Task 5 用的测试夹具：`stub_for_test` 必须与真实写盘口同一条闸 ----

    /// 跑一段应当 panic 的夹具构造，把 panic 消息收回来（没 panic 就返回 `None`）。
    /// `AssertUnwindSafe` 是因为闭包只读 `spec`（`(u64, u64)` 切片），没有任何"半途可变状态"要看。
    fn stub_panic_message(total_len: u64, spec: &[(u64, u64)]) -> Option<String> {
        let got = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Snapshot::stub_for_test(validator(total_len), rows(spec));
        }));
        match got {
            Ok(()) => None,
            Err(payload) => Some(
                payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "(panic 载荷不是字符串)".to_string()),
            ),
        }
    }

    #[test]
    fn the_test_stub_panics_on_a_table_that_save_would_refuse() {
        // 评审 F2：`save` 拒写的段表不许被夹具喂给 Task 5 的续传判定 —— 那种断言对着一个"盘上永远
        // 不可能存在"的形状成立，要等到 Task 8 才看得见。夹具是测试脚手架，所以是**大声的程序员
        // panic**（不是 Result：那会让每个调用点为不存在的失败模式多一层 unwrap）。
        // 第一支形状取自计划第 700 行 Task 5 的 branch 7（`snap_multi(val, 2)`，`total_len: 100`）
        // 最容易写错的那一行：相邻两行。
        // (段表内容, 违约原因, `check_segments` 消息里应当出现的词)
        type BadCase = (Vec<(u64, u64)>, &'static str, &'static str);
        let bad: [BadCase; 6] = [
            (vec![(0, 9), (10, 19)], "相邻未合并（Task 5 表里最容易写错的那一支）", "相邻"),
            (vec![(10, 19), (0, 9)], "乱序", "相邻"),
            (vec![(0, 19), (10, 29)], "重叠", "相邻"),
            (vec![(0, 19), (5, 6)], "被包含", "相邻"),
            (vec![(0, 100)], "终点越界", "超出"),
            (vec![(3, 1)], "起点晚于终点", "起点晚于终点"),
        ];
        for (spec, why, keyword) in bad {
            let Some(msg) = stub_panic_message(100, &spec) else {
                panic!("{why}：{spec:?} 必须让 stub_for_test panic —— 它与 save 共用同一条闸");
            };
            assert!(
                msg.contains("段表") && msg.contains(keyword),
                "{why} 的 panic 要说清违约的是哪一行：{msg}"
            );
        }
        // 反面对照（同一条用例里，证明上面六条不是恒真）：Task 5 要的合法形状照样构造得出来。
        assert_eq!(stub_panic_message(100, &[(0, 9), (20, 49)]), None, "合法多行夹具不该 panic");
        assert_eq!(stub_panic_message(100, &[(0, 99)]), None, "整份一条不该 panic");
        assert_eq!(stub_panic_message(100, &[]), None, "空段表不该 panic");
    }

    #[test]
    fn the_stub_shape_is_a_file_save_can_actually_write() -> anyhow::Result<()> {
        // F2 的另一半：过了闸的夹具必须真能落成断点文件（Task 5 的续传断言这才对着一个存在的形状）。
        let target = target_of("t4_stub", "a.bin");
        let p = path_for(&target);
        let snap = Snapshot::stub_for_test(validator(100), rows(&[(0, 9), (20, 49)]));
        save(&p, &snap).expect("夹具过了段表闸就必须写得出去");
        assert_eq!(load(&p)?.as_ref(), Some(&snap), "夹具形状要能原样 round-trip");
        assert_eq!(confirmed_bytes(&snap), 40, "夹具的覆盖集算法与真实快照同一条");
        assert_eq!(snap.task.url, snap.validator.final_url, "夹具的 task/validator 自洽（url 取自 final_url）");
        std::fs::remove_file(&p)?;
        Ok(())
    }

    // ---- saved_at ----

    #[test]
    fn saved_at_is_utc_wall_clock() {
        // 不引 chrono（plan 的依赖白名单没有它），所以 UTC 墙钟是手算的 —— 手算就得拿已知对照钉住：
        // 纪元、跨日、**世纪闰年规则**（2000 闰、2100 不闰）、纪元之前（负秒，考 div_euclid 而不是 /）。
        let cases: [(i64, &str); 12] = [
            (0, "1970-01-01T00:00:00Z"),
            (1, "1970-01-01T00:00:01Z"),
            (-1, "1969-12-31T23:59:59Z"),
            (-86_400, "1969-12-31T00:00:00Z"),
            (86_399, "1970-01-01T23:59:59Z"),
            (946_684_799, "1999-12-31T23:59:59Z"),
            (946_684_800, "2000-01-01T00:00:00Z"),
            (951_782_400, "2000-02-29T00:00:00Z"),
            (1_700_000_000, "2023-11-14T22:13:20Z"),
            (1_709_251_199, "2024-02-29T23:59:59Z"),
            (4_107_542_399, "2100-02-28T23:59:59Z"),
            (4_107_542_400, "2100-03-01T00:00:00Z"),
        ];
        for (secs, want) in cases {
            assert_eq!(format_utc(secs), want, "epoch 秒 {secs}");
        }
    }

    #[test]
    fn saved_at_of_a_new_snapshot_has_the_rfc3339_shape() -> anyhow::Result<()> {
        let target = target_of("t4_saved_at", "a.bin");
        let snap = Snapshot::from_task(&task_at(&target, None, None), validator(10), vec![])?;
        let s = &snap.saved_at;
        let b = s.as_bytes();
        assert_eq!(b.len(), 20, "形如 2026-10-16T13:05:44Z：{s}");
        assert_eq!(b[4], b'-');
        assert_eq!(b[7], b'-');
        assert_eq!(b[10], b'T');
        assert_eq!(b[13], b':');
        assert_eq!(b[16], b':');
        assert_eq!(b[19], b'Z');
        assert!(b[..4].iter().all(u8::is_ascii_digit) && s.starts_with("20"), "年份看着不对：{s}");
        let digit_positions: &[usize] = &[5, 6, 8, 9, 11, 12, 14, 15, 17, 18];
        assert!(digit_positions.iter().all(|&i| b[i].is_ascii_digit()), "月/日/时/分/秒的位置上该是数字：{s}");
        // `utc_now()` 就是 `from_task` 用的那口钟，格式一致 ⇒ 这个字符串的字典序 == 时间序
        // （Task 8 每次落盘前用它刷 `saved_at`，见 `Snapshot::saved_at` 的文档）。
        let later = utc_now();
        assert_eq!(later.len(), 20, "同一个格式：{later}");
        assert!(later.as_str() >= s.as_str(), "同一口钟不该倒退：{s} -> {later}");
        Ok(())
    }

    #[test]
    fn from_task_refuses_a_task_the_engine_would_refuse_to_run() -> anyhow::Result<()> {
        // `Task::validate` 的第二条是 spec §4 的硬规约（明文 HTTP 不送口令）。落盘点是它之外的第二个
        // 强制点：口令规则不该只在"跑起来那一刻"生效 —— 谁都可能直接调 from_task。
        let target = target_of("t4_validate_gate", "a.bin");
        let mut t = task_at(&target, Some("alice"), Some(SECRET));
        t.url = "http://x/a.bin".into();
        let err = Snapshot::from_task(&t, validator(10), vec![])
            .expect_err("明文 HTTP + 口令的任务不得能被写进断点文件");
        assert!(!path_for(&target).exists(), "拒了就不该顺手写盘");
        assert!(err.to_string().contains("口令"), "错误要说明是口令规则：{err}");
        Ok(())
    }
}
