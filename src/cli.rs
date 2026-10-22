//! 参数解析层（Task 9B-1）：POSIX 长选项为主，IDM 的斜杠别名**在进 clap 之前**就地展开。
//!
//! 依据：spec §5（usage 列出的那串长选项 + `flags` 位义 + 那句"别名在同一层展开，目的是让现成调用
//! IDM 的脚本不改就能接上"）、勘误 **E20②③④⑤⑨**、**E23①–⑦**（别名表的取证权威是
//! `projects/tool/idm-recon/docs/50-idm-capability-spec.md` 的 **C3** 行）。
//!
//! ## 这一层的边界（它只管"参数形状"）
//! - **不上网、不碰磁盘、不读 `Report`**：`parse_from` → [`Cli`]，`into_task` → `task::Task`，
//!   `into_config` → `engine::Config`。进度行渲染与退出码住在 `report.rs`/`main.rs`（9B-2）。
//! - **边界形状校验归这一层**（E23⑤）：`--sha256` 的 64 位十六进制形状、`--connections` 的下限、
//!   `http://` + 口令 ⇒ 都在这里失败，而不是走到引擎里换一句"校验不符"（E20⑨：引擎侧
//!   `hash_matches` 只做大小写不敏感的等值比较，给错长度只会得到"不符"，那是对用户撒谎）。
//!   ⚠ 复量补一句实情：`engine::run` **今天确实有** `cfg.workers >= 1` 那一发 `ensure!`
//!   （`src/engine.rs` 里 `run` 的第二行），所以 E23⑤ 那句"引擎侧对 workers 无下限闸"在这一点上
//!   已经过时 ⇒ 这一格的 `--connections 0` 拦截**不是**唯一防线，它是**边界**防线：报错要发生在
//!   参数区、措辞是"参数写错了"，而不是等一次试探之后才说"跑不起来"。
//! - **`min_seg` 没有 CLI 入口**（E20⑥/E23⑤：spec §5 那一串长选项里没有一个能改它），
//!   `prelude_max` 同理 ⇒ 本层不许自己发明 `--min-seg` / `--probe-bytes`。
//! - **`Config` 的两个数只从 `Config` 取**（E20⑤）：`Task` 上那一族常数访问器（`limits()`）已整条
//!   删除，照它填会把 `--connections` 静默吞成默认值。本文件的写法是**从 `Config::default()` 起步、
//!   只覆盖四格**，于是 `min_seg` 与 `on_tick` 保持默认这件事是构造上的事实，不是注释里的承诺。
//! - **`--quiet` 不住 `Task.flags`**（E20③，见 [`Cli::quiet`]）；`--no-start` 除了置 bit0 还要留一个
//!   访问器（E20④：引擎明确不看 bit0，短路只能由调用方在进入 `run` 之前做）。
//! - **`Cli` 的 `Debug` 是手写的**，立场与 **E9** 给 `Task` 的那一份相同：错误路径上的 `{:?}`、
//!   `{e:?}` 不该把口令交给日志。刻意只动 `Debug`，`PartialEq`/`Eq` 的语义一个字没改。

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use clap::Parser;

use crate::actions::CompletionAction;
use crate::consts::{FLAG_ADD_ONLY, FLAG_IGNORE_MTIME, FLAG_KEEP_STATE};
use crate::engine::Config;
use crate::task::Task;

/// 斜杠别名 → 长选项。取证原文（recon `docs/50` 的 **C3** 行）：
/// `/d URL` `/p 路径` `/f 文件名` `/n 静默` `/q 下完退出` `/a 只入队` `/s 启动计划` `/onboot`。
///
/// 这张表有三种读法，逐条都有出处：
/// - **映射**（值 `Some(长名)`）五条：`/d→--url`、`/p→--out-dir`、`/f→--out-name`、`/n→--quiet`、
///   `/a→--no-start`。计划正文 Task 9 Step 3 那张别名表把 `/q` 记成 `--quiet`、`/a` 记成 `--add-only` 是**反的**
///   而且是**没取证的长名** —— `--no-start` 这个名字的依据是 spec §5 的 flags 表自己写的
///   "bit0 = 只入队不下（等价 `--no-start`）"（**E23①②**）。`the_alias_table_is_the_c3_attestation…`
///   那一格就是这个改口的活体针（mu1 打在这里）。
/// - **接受并忽略**（值 `None`）三条：`/q`（下完退出）、`/s`（启动计划）、`/onboot`（开机自启）。
///   它们在 C3 里有据，但语义属于"下完把进程退掉 / 按计划或开机启动"这两个 **v1 内核里没有的
///   子系统**（spec §8 明确不做守护进程、不做完成后动作链）。**报错**会让现成的 IDM 脚本整条失败
///   （那就背离了这一层存在的理由），**发明语义**就是假功能 ⇒ 两害取轻：收下、什么都不做，
///   并留一条"它不映射到任何东西"的用例（**E23③**）。
/// - **表里没有的一律报错**（**E23④**）：`/h` 也落在这一支 —— C3 那行根本没有 `/h`，帮助走 clap
///   自带的 `--help`。未知别名若被静默吞掉，用户的脚本就变成"参数神秘消失"，比报错坏得多。
const ALIASES: &[(&str, Option<&str>)] = &[
    ("/d", Some("--url")),
    ("/p", Some("--out-dir")),
    ("/f", Some("--out-name")),
    ("/n", Some("--quiet")),
    ("/a", Some("--no-start")),
    // 有据、但 v1 没有对应子系统 ⇒ 接受并忽略。
    ("/q", None),
    ("/s", None),
    ("/onboot", None),
];

/// `ALIASES` 那张表的**用户面**版本：只写"哪种写法换成哪条长选项、哪几种写法什么都不做"，
/// 一行一个事实，取证编号与取舍理由一个字都不带（判据住在 `tests/integration.rs` 的
/// `the_binary_help_prints_no_internal_review_prose`，射程把这一段也算在内）。
///
/// 为什么这一族话**只有在这里出现一次**：`/q` 这类写法今天唯一的去处是 `expand_idm_aliases`
/// 那句报错——那要求用户先写错一次才知道有这回事。只会 IDM 写法的人没有任何地方能查出 `/q`
/// 是什么，所以别名表必须进 `--help`，而不只进错误信息。
///
/// ⚠ 排版：本仓库的 clap 没开 `wrap_help`，这一段**逐字**打印（不折行、不改缩进），
/// 所以行首的两格缩进会原样出现在帮助里；期望值按渲染现场写死在结构针里。
const ALIAS_EPILOG: &str = "\
斜杠写法（IDM 风格，与长选项等价）：
  /d → --url
  /p → --out-dir
  /f → --out-name
  /n → --quiet
  /a → --no-start
  /q、/s、/onboot 被接受但不做任何事（v1 没有对应的子系统）
表以外的斜杠写法会直接报错";

/// 需要**再吃一个参数**的长选项（别名展开的"值位"探测用）。这份清单必须与下面 `Cli` 的字段一一对应：
/// 少一格就会把用户的**值**当成别名位（`--out-dir /d` 里那个 `/d` 在 POSIX 上是一条绝对路径，
/// 展开成 `--url` 就把参数吃掉了）；多一格就会把旗标当值吃掉。开关型的那五个
/// （`--ignore-mtime` `--no-resume` `--keep-state` `--quiet` `--no-start`）不在表里。
const VALUE_TAKING_LONGS: &[&str] = &[
    "--url",
    "--out-dir",
    "--out-name",
    "--connections",
    "--referer",
    "--cookie",
    "--data-file",
    "--user",
    "--password",
    "--sha256",
    "--max-retries",
];

/// 这个 token 是不是斜杠别名；`Some(Some(长名))` 展开成它，`Some(None)` 接受并忽略，`None` 不是别名。
fn alias_target(arg: &OsStr) -> Option<Option<&'static str>> {
    ALIASES
        .iter()
        .find(|(token, _)| OsStr::new(token) == arg)
        .map(|(_, target)| *target)
}

/// token 是否以某个 ASCII 前缀开头。走 `as_encoded_bytes` 而不是 `to_str()`：非 UTF-8 的 token
/// （POSIX 上完全合法）照样要能认出形状；而 Windows 那侧的编码里 `<0x80` 的字节不会当延续字节
/// 出现，所以 ASCII 前缀逐字节比较是可靠的。`std::ffi::OsStr` 本身**没有** `starts_with`。
fn starts_with_ascii(arg: &OsStr, prefix: &[u8]) -> bool {
    arg.as_encoded_bytes().starts_with(prefix)
}

/// 把斜杠别名换成 `--长名`（或直接丢掉），其余 token 原样通过；未知斜杠 token 在这里报错。
///
/// 为什么展开必须在 clap **之前**（spec §5："别名在同一层展开"）：clap 的 `prefix`/`alias` 机制
/// 只认 `-` 开头的写法，`/d` 这种形状它当成"未知的短旗标串"；而且我们要的语义是
/// **"就地替换成同一条长选项"**（于是 `--url` 的必填性、值类型、报错文案两种写法完全一致），
/// 这不是 clap 的 alias 能表达的。
fn expand_idm_aliases<I: IntoIterator<Item = OsString>>(args: I) -> anyhow::Result<Vec<OsString>> {
    let mut out: Vec<OsString> = Vec::new();
    let mut iter = args.into_iter();
    // `argv[0]` 是程序名（clap 自己也这么认）⇒ 别名只在参数区展开。
    if let Some(program) = iter.next() {
        out.push(program);
    }
    let mut after_ddash = false;
    let mut value_slot = false;
    for arg in iter {
        // `--` 之后全是位置参数，不再有任何旗标语义（本 CLI 没有位置参数，那一支由 clap 负责报错）。
        if after_ddash {
            out.push(arg);
            continue;
        }
        if arg == OsStr::new("--") {
            after_ddash = true;
            out.push(arg);
            continue;
        }
        if value_slot {
            value_slot = false;
            out.push(arg);
            continue;
        }
        if let Some(long) = alias_target(&arg) {
            // 命中的别名：`Some(长名)` 换成那条长选项写法；`None`（`/q` `/s` `/onboot`）连 token 一起丢掉。
            // **E28④**：展开出来的长名同样要占值位。下面那条值位探测只认**输入里**的 `--长名`，
            // 这里不置上的话，用户的**值**（`/f /n` 里那个 `/n`）会被再当别名位展开 ⇒ 等价性自己拆了。
            // 展开成旗标（`/n`→`--quiet`、`/a`→`--no-start`）或被忽略（`None`）的那两支**不许**置值位：
            // 多置一格就是把下一个真参数吞成旗标的值（`VALUE_TAKING_LONGS` 文档写的另一半 hazard）。
            value_slot = long.is_some_and(|name| VALUE_TAKING_LONGS.contains(&name));
            out.extend(long.map(OsString::from));
            continue;
        }
        anyhow::ensure!(
            !starts_with_ascii(&arg, b"/"),
            "未知别名 {}：dlm 认的斜杠写法只有 /d /p /f /n /a（映射到长选项）与 /q /s /onboot\
             （接受但不做任何事），其余斜杠写法一律不认；完整的长选项面请看 --help",
            arg.display()
        );
        // 值位探测：`--opt=值` 那种写法**不吃**下一个参数，只有裸 `--opt` 才可能进这张表。
        value_slot = starts_with_ascii(&arg, b"--")
            && !arg.as_encoded_bytes().contains(&b'=')
            && VALUE_TAKING_LONGS.iter().any(|name| OsStr::new(name) == arg);
        out.push(arg);
    }
    Ok(out)
}

/// 边界形状校验（**E23⑤**）。措辞一律"参数写错了"，与引擎那句校验失败**不同源**：
/// 让形状错走到引擎会换回一句"校验不符"（E20⑨），那是把"你话没说对"报成"东西下坏了"。
fn check_boundaries(cli: &Cli) -> anyhow::Result<()> {
    if let Some(hex) = cli.sha256.as_deref() {
        anyhow::ensure!(
            hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()),
            "参数写错了：--sha256 要的是 64 个十六进制字符（sha256 摘要本体的长度），本轮拿到 {} 个；\
             现在拦下来是为了不白跑一整趟下载再告诉你结论",
            hex.chars().count()
        );
    }
    if let Some(n) = cli.connections {
        anyhow::ensure!(
            n != 0,
            "参数写错了：--connections 至少 1（给了 0 就没有 worker，一个字节都下不动）"
        );
    }
    Ok(())
}

/// 解析一条命令线（含 `argv[0]`；生产调用方交 `std::env::args_os()`）。
///
/// `--help` / `--version` 走 clap 自己（**E23④**：`/h` 没有取证支撑，不发明它）。这两条会从这里
/// 返回一个 `Err`，但它的**底层类型是 `clap::Error`** —— 9B-2 想按"帮助⇒退出码 0"分档的话，
/// `err.downcast_ref::<clap::Error>()` 拿得回来（`clap::Error::exit()` 自己会打帮助并给 0），
/// 所以这里不提前把它折成字符串。
pub fn parse_from<I: IntoIterator<Item = OsString>>(args: I) -> anyhow::Result<Cli> {
    let cli = Cli::try_parse_from(expand_idm_aliases(args)?)?;
    check_boundaries(&cli)?;
    Ok(cli)
}

// 这段是 `Cli` 的实现依据，**必须留在源码、绝不能到用户表面**：clap derive 会把结构体的 `///`
// 当 `long_about`，`dlm --help` 就原样打印它（E28③ 实测漏出去过一整段内部勘误论证）。
// 判据是 `tests/integration.rs::the_binary_help_prints_no_internal_review_prose`（带对照臂的
// 禁串/禁形状扫描）；按 `c6ac42e` 的纪律，这里不写选项计数，只写去处。
// - 长选项面照 spec §5 的 usage 清单，另补 **E23②** 那条 `--no-start`：usage 代码块漏了它，
//   而同节的 flags 表已经写了"bit0 等价 `--no-start`" ⇒ 不补就没有任何入口能置起 bit0，
//   那句"打印任务摘要后退出"就落不了地。
// - 字段一律私有：外面的面只有 `into_task` / `into_config` / `quiet` / `no_start` 四格，
//   少一条"顺手读一下 cli.url"的暗道。
// - **T99-G5b 的分工（写成三条才不会下一次又混起来）**：
//   ① 上面这种 `//` ＝ 实现纪律，永不进用户表面；
//   ② 每个字段那串 `help = "…"` ＝ 该行选项的一行说明，**只写实现真给得起的事实**
//      （默认值、取值范围、哪一路不读写断点，都能在下面几行的实现处对号）；
//   ③ `ALIAS_EPILOG` ＝ 别名表的用户面。
//   ⚠ 字段上**不要写 `///`**：derive 会把字段文档同时喂给 `help` 与 `long_help`，
//   于是"顺手在字段上写一段论证"就变成"论证印进 --help"（E28③ 那一族的字段版）。
//   这里刻意用 `help = ` 属性而不是 `///`：derive 生成的调用顺序是文档注释在前、属性在后，
//   所以属性写的用户文案**压得住**将来误加的字段文档，而不是被它顶掉。
#[derive(Parser, PartialEq, Eq)]
#[command(
    name = "dlm",
    version,
    about = "多连接下载内核（POSIX 长选项 + IDM 斜杠别名）",
    after_help = ALIAS_EPILOG
)]
pub struct Cli {
    #[arg(long, value_name = "URL", help = "下载 URL（必填；仅接受 http://、https:// 规范写法）")]
    url: String,
    #[arg(long, value_name = "DIR", help = "输出目录（默认当前目录）")]
    out_dir: Option<PathBuf>,
    #[arg(long, value_name = "NAME", help = "输出文件名（不给取 URL 末段；都不许走出 --out-dir）")]
    out_name: Option<String>,
    #[arg(long, value_name = "N", help = "并发连接数（默认 8，至少 1）")]
    connections: Option<usize>,
    #[arg(long, value_name = "BYTES_PER_SECOND", default_value_t = 0, help = "所有连接合计的速率上限（字节/秒；0 = 不限）")]
    max_speed: u64,
    #[arg(long, value_name = "URL", help = "原样发送的 Referer 头")]
    referer: Option<String>,
    #[arg(long, value_name = "COOKIE", help = "原样发送 Cookie 头（不分 http/https，写入断点文件）")]
    cookie: Option<String>,
    #[arg(long, value_name = "FILE", help = "从这个文件读请求体（不试探、不读写断点、不可续传）")]
    data_file: Option<PathBuf>,
    #[arg(long, value_name = "USER", help = "Basic 认证的用户名（只在 https 上发送）")]
    user: Option<String>,
    #[arg(long, value_name = "PASSWORD", help = "Basic 口令（仅 https 发送；配 http:// 会直接失败）")]
    password: Option<String>,
    #[arg(long, help = "仅 Last-Modified 变化时可续传（ETag 变则从头重来）")]
    ignore_mtime: bool,
    #[arg(long, help = "无视断点文件，从头下载")]
    no_resume: bool,
    #[arg(long, value_name = "HEX", help = "完成后校验（需 64 位十六进制；不符保留文件并报错、不自动删）")]
    sha256: Option<String>,
    #[arg(long, value_name = "N", help = "给无限慢重试封顶的重试次数")]
    max_retries: Option<u32>,
    #[arg(long, help = "完成后保留断点文件")]
    keep_state: bool,
    #[arg(long, help = "关掉逐拍进度行（最终摘要与错误照样打印）")]
    quiet: bool,
    #[arg(long, help = "只打印任务摘要后退出，不发送任何请求")]
    no_start: bool,
    #[arg(long, value_name = "ACTION", help = "完成后动作：open 或 run:<exe>（程序收到落点路径作为第一个参数）")]
    on_complete: Option<CompletionAction>,
}

/// 手写 `Debug`（**E9** 给 `Task` 的那份立场在这里同样成立）：`Cli` 是 `parse_from` 的返回值，
/// 而 `parse_from` 的错误路径会把它交给 `{,e:?}` —— 派生的话口令就进日志了。
/// 只掩 `password` 一格，别的字段照派生的形状打印（掩码不该顺手把调试价值一起抹掉）；
/// 口令与 `Task` 那边用同一个 `[redacted]`，两处日志读起来是同一个记号。
impl std::fmt::Debug for Cli {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cli")
            .field("url", &self.url)
            .field("out_dir", &self.out_dir)
            .field("out_name", &self.out_name)
            .field("connections", &self.connections)
            .field("max_speed", &self.max_speed)
            .field("referer", &self.referer)
            .field("cookie", &self.cookie)
            .field("data_file", &self.data_file)
            .field("user", &self.user)
            // 掩码而不是省略：读日志的人要能看出"这条命令带了口令"。
            .field("password", &self.password.as_ref().map(|_| "[redacted]"))
            .field("ignore_mtime", &self.ignore_mtime)
            .field("no_resume", &self.no_resume)
            .field("sha256", &self.sha256)
            .field("max_retries", &self.max_retries)
            .field("keep_state", &self.keep_state)
            .field("quiet", &self.quiet)
            .field("no_start", &self.no_start)
            .field("on_complete", &self.on_complete)
            .finish()
    }
}

impl Cli {
    /// `--quiet` / `/n`。**住在 `Cli` 而不是 `Task.flags`**（**E20③**）：spec §5 的位义只定义了
    /// bit0/bit1/bit2，把静默塞进 flags 会改变断点文件里那份 `flags` 的**比对语义** ——
    /// "只是不想看进度行"就变成"请求变了，从头下"。
    pub fn quiet(&self) -> bool {
        self.quiet
    }

    /// `--no-start` / `/a`。**bit0 没有任何引擎侧的读取点**（`engine::run` 的文档明写"本函数不看
    /// 这一位"，**E20④**）⇒ 这一格是给 9B-2 在进入 `run` **之前**短路用的；进了 `run` 就一定上网。
    pub fn no_start(&self) -> bool {
        self.no_start
    }

    /// `--on-complete`：完成后动作（R19）。`main` 在下载成功后消费它。
    pub fn action(&self) -> Option<&CompletionAction> {
        self.on_complete.as_ref()
    }

    /// 交回 `Task`（C2 那九项契约 + `flags`）。
    ///
    /// 为什么在这里调 `task.validate()`：那条"`http://` + 口令直接报错、不降级发送"的硬规定
    /// （spec §4 / E3）**只住在 `Task::validate()` 一处** —— `task.rs` 里 userinfo 那条规则立的
    /// 就是同一个先例（"规则只住一处，否则两份判据迟早读成两样"）。这一层重抄一遍是复刻判据。
    /// 失败点仍然是"解析期"：计划 Step 1 那格用例的形状就是
    /// `parse_from(..).and_then(|c| c.into_task())`，那条链上没有网络也没有磁盘。
    pub fn into_task(self) -> anyhow::Result<Task> {
        // spec §5 的位义：bit0 只入队不下、bit1 忽略 Last-Modified、bit2 完成后保留断点。
        // **只有这三位**（E20③）：静默不住这里，`--no-resume` 也不住这里（它折进 `Config::force_fresh`）。
        let mut flags = 0u32;
        if self.no_start {
            flags |= FLAG_ADD_ONLY;
        }
        if self.ignore_mtime {
            flags |= FLAG_IGNORE_MTIME;
        }
        if self.keep_state {
            flags |= FLAG_KEEP_STATE;
        }
        let task = Task {
            url: self.url,
            referer: self.referer,
            cookies: self.cookie,
            data_file: self.data_file,
            user: self.user,
            password: self.password,
            // spec §5 没给 `--out-dir` 的默认值 ⇒ 当前目录（`target_path()` 是 `dir.join(name)`，
            // 给 "." 就是"下到用户站着的地方"，与 IDM 默认行为同形）。
            dir: self.out_dir.unwrap_or_else(|| PathBuf::from(".")),
            name: self.out_name,
            flags,
        };
        task.validate()?;
        Ok(task)
    }

    /// 交回 `Config`：`--connections`→`workers`、`--max-speed`→`max_speed`、
    /// `--max-retries`→`max_retries`、`--sha256`→`verify_sha256`、`--no-resume`→`force_fresh`
    /// 五格；`min_seg` 保持默认（§5 没有它的入口，E20⑥），`on_tick` 留 `None`
    /// （进度行的 sink 由 9B-2 挂，E22 第 4 条）。
    ///
    /// 写法是"从 `Config::default()` 起步、只覆盖四格"，为的是让**没被覆盖的两格保持默认**这件事
    /// 不依赖任何注释（**E20⑤** 的教训：`Task` 上那族已删除的常数访问器一旦被人照文档填回 `Config`，
    /// `--connections` 会被静默吞成默认值）。所以这里既不读 `Task`，也不读 `Config` 的字段清单。
    ///
    /// 命名带 `into_` 却借 `&self`：这是 9B-2 的契约（它要先看 `quiet()`/`no_start()` 再消费 `Cli`），
    /// 而 `Config` 里那四格都是可以从引用克隆出来的 ⇒ 不改名、不改签名。
    pub fn into_config(&self) -> Config {
        let mut cfg = Config::default();
        if let Some(connections) = self.connections {
            cfg.workers = connections;
        }
        cfg.max_speed = self.max_speed;
        cfg.max_retries = self.max_retries;
        cfg.verify_sha256 = self.sha256.clone();
        cfg.force_fresh = self.no_resume;
        cfg
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::{DEFAULT_CONNECTIONS, MIN_SEGMENT_BYTES};

    /// `parse_from` 的入参形状是 `IntoIterator<Item = OsString>`（真调用给的是
    /// `std::env::args_os()`）；计划正文那三格用例写的是 `&str` 数组，照字面编不过 ⇒ 这里加一发
    /// 只做类型转换的辅助函数，**不放宽签名**。
    fn argv(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }
    fn parse(items: &[&str]) -> anyhow::Result<Cli> {
        parse_from(argv(items))
    }
    fn task(items: &[&str]) -> anyhow::Result<Task> {
        parse(items)?.into_task()
    }

    const HEX64: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn idm_style_slash_aliases_expand_to_same_task() -> anyhow::Result<()> {
        let posix = task(&[
            "dlm",
            "--url",
            "https://x/a.bin",
            "--out-dir",
            "D:/d",
            "--out-name",
            "b.bin",
        ])?;
        let idm = task(&["dlm", "/d", "https://x/a.bin", "/p", "D:/d", "/f", "b.bin"])?;
        assert_eq!(posix, idm);
        assert_eq!(posix.target_path(), PathBuf::from("D:/d").join("b.bin"));
        Ok(())
    }

    #[test]
    fn flag_bits_map_to_task_flags() -> anyhow::Result<()> {
        let c = parse(&["dlm", "/d", "https://x/a", "/n", "/a", "--keep-state"])?;
        assert!(c.quiet(), "/n 才是静默别名（E23①：plan 正文把 /q 记成静默是反的）");
        let t = c.into_task()?;
        assert_eq!(t.flags & FLAG_ADD_ONLY, FLAG_ADD_ONLY);
        assert_eq!(t.flags & FLAG_KEEP_STATE, FLAG_KEEP_STATE);
        assert_eq!(t.flags, FLAG_ADD_ONLY | FLAG_KEEP_STATE, "flags 里只有 bit0/bit1/bit2 三格，静默不许占一格（E20③）：{:b}", t.flags);
        Ok(())
    }

    #[test]
    fn plain_http_with_password_fails_at_parse_time() {
        for url in ["http://x/a", "HTTP://x/a", "HtTp://x/a"] {
            let r = parse(&["dlm", "--url", url, "--password", "p"]).and_then(|c| c.into_task());
            assert!(r.is_err(), "{url} 带口令必须在解析期失败，不许降级成明文发送（E3 / spec §4）");
        }
        let ok = parse(&["dlm", "--url", "https://x/a", "--password", "p"]).and_then(|c| c.into_task());
        assert!(ok.is_ok(), "https 带口令是本工具支持的形状：{}", ok.unwrap_err());
    }

    #[test]
    fn quiet_lives_on_cli_not_in_task_flags() -> anyhow::Result<()> {
        for spelling in ["--quiet", "/n"] {
            let c = parse(&["dlm", "--url", "https://x/a", spelling])?;
            assert!(c.quiet(), "{spelling} 该置静默位");
            assert_eq!(c.into_task()?.flags, 0, "{spelling} 不许往 flags 里塞任何位（E20③）");
        }
        assert!(!parse(&["dlm", "--url", "https://x/a"])?.quiet(), "不给就是非静默");
        Ok(())
    }

    #[test]
    fn no_start_and_slash_a_are_two_spellings_of_bit0() -> anyhow::Result<()> {
        let a = parse(&["dlm", "--url", "https://x/a", "/a"])?;
        let b = parse(&["dlm", "--url", "https://x/a", "--no-start"])?;
        assert_eq!(a, b, "/a 与 --no-start 同义（E23②）");
        assert!(a.no_start() && b.no_start(), "9B-2 要用这个访问器在 engine::run 之前短路（E20④）");
        assert_eq!(a.into_task()?.flags, FLAG_ADD_ONLY, "bit0 是唯一落点，别的位一格都不许多置");
        assert_eq!(b.into_task()?.flags, FLAG_ADD_ONLY);
        let none = parse(&["dlm", "--url", "https://x/a"])?;
        assert!(!none.no_start(), "不给 --no-start 就不许默认入队");
        assert_eq!(none.into_task()?.flags, 0);
        Ok(())
    }

    #[test]
    fn ignore_mtime_and_keep_state_map_to_bit1_and_bit2() -> anyhow::Result<()> {
        assert_eq!(task(&["dlm", "--url", "https://x/a", "--ignore-mtime"])?.flags, FLAG_IGNORE_MTIME);
        assert_eq!(task(&["dlm", "--url", "https://x/a", "--keep-state"])?.flags, FLAG_KEEP_STATE);
        let both = task(&["dlm", "--url", "https://x/a", "--ignore-mtime", "--keep-state"])?;
        assert_eq!(both.flags, FLAG_IGNORE_MTIME | FLAG_KEEP_STATE);
        assert_eq!(task(&["dlm", "--url", "https://x/a", "--no-resume"])?.flags, 0, "--no-resume 走 Config::force_fresh，不占 flags 位");
        Ok(())
    }

    #[test]
    fn attested_but_unsupported_aliases_are_accepted_and_set_nothing() -> anyhow::Result<()> {
        for ignored in ["/q", "/s", "/onboot"] {
            let plain = parse(&["dlm", "/d", "https://x/a.bin"])?;
            let with = parse(&["dlm", ignored, "/d", "https://x/a.bin", ignored])?;
            assert_eq!(plain, with, "{ignored} 必须像不存在一样（逐字段相等）");
            assert!(!with.quiet(), "{ignored} 不是静默（E23①：静默是 /n）");
            assert_eq!(with.into_task()?.flags, 0, "{ignored} 不许置任何 flag 位（E23③：接受并忽略 ≠ 发明语义）");
        }
        let c = parse(&["dlm", "/d", "https://x/a.bin", "/n", "/q", "/s", "/onboot", "/a"])?;
        assert!(c.quiet() && c.no_start(), "现成 IDM 脚本会 /n /q /a 一起用，忽略项不许把别的别名带倒");
        assert_eq!(c.into_task()?.flags, FLAG_ADD_ONLY);
        Ok(())
    }

    #[test]
    fn an_unknown_slash_alias_errors_instead_of_being_swallowed() {
        for bad in ["/z", "/h", "/D", "/q=1", "/on", "/"] {
            let Err(err) = parse(&["dlm", "/d", "https://x/a", bad]) else {
                panic!("{bad} 不在 C3 取证表里，必须报错：拼错一个字母不能变成\"参数被吃掉了\"（E23④）");
            };
            let msg = err.to_string();
            assert!(msg.contains("未知别名"), "{bad} 的错误要说清是未知别名：{msg}");
        }
    }

    #[test]
    fn sha256_shape_is_a_parameter_error_not_a_verification_failure() -> anyhow::Result<()> {
        for bad in ["deadbeef".repeat(4), "g".repeat(64), String::new(), "f".repeat(65)] {
            let Err(err) = parse(&["dlm", "--url", "https://x/a", "--sha256", bad.as_str()]) else {
                panic!("--sha256 给的是 {} 个字符（{}…），形状不合法必须在解析期报错（E23⑤）", bad.chars().count(), &bad[..bad.len().min(8)]);
            };
            let msg = err.to_string();
            assert!(msg.contains("参数写错了") && msg.contains("--sha256"), "这是参数形状问题，措辞要照参数错说：{msg}");
            assert!(!msg.contains("校验不符"), "不许把形状错路由成引擎那句\"校验不符\"（E20⑨）：{msg}");
        }
        for good in [HEX64.to_string(), "A".repeat(64)] {
            let c = parse(&["dlm", "--url", "https://x/a", "--sha256", good.as_str()])?;
            assert_eq!(c.into_config().verify_sha256.as_deref(), Some(good.as_str()), "64 位十六进制原样交给引擎（大小写由 hash_matches 负责）");
        }
        Ok(())
    }

    /// 从**帮助文本**里把一个数读出来（**T99-H3 第 3 节：交叉针的期望值唯一的来源**）。
    ///
    /// 取法：`Cli::command()` → 找到 `get_long() == long` 那条参数 → 取它的 `get_help()` →
    /// 在里头找 `marker`（如"至少"、"需"），跳过空格后把紧跟的那一串数字读成 `usize`。
    /// ⚠ 五件事任何一件不成立都**当场 panic**，一条都不许静默跳过（那就是把交叉针做成恒真）：
    /// 选项在 `Command` 里找不到／那条没有 help／help 里没有 `marker`／`marker` 后面没有数字／
    /// 那串数字 `parse` 不出来。这一族的"期望值从对面读"与 `src/probe.rs` 的 base64 期望表
    /// "没有一个期望来自调用被测编码器"同一条立场：**问帮助，不问实现**。
    fn number_from_help(long: &str, marker: &str) -> usize {
        use clap::CommandFactory;
        let cmd = Cli::command();
        let help = cmd
            .get_arguments()
            .find(|a| a.get_long() == Some(long))
            .unwrap_or_else(|| panic!("交叉针读不到 --{long}：这条选项已从帮助面上消失"))
            .get_help()
            .unwrap_or_else(|| panic!("--{long} 在帮助里没有说明（T99-G5b 那根针管的就是这一格）"))
            .to_string();
        let after = help
            .split_once(marker)
            .map(|(_, rest)| rest)
            .unwrap_or_else(|| panic!("--{long} 的帮助里找不到 {marker:?}，数没处读：{help}"));
        let digits: String = after
            .chars()
            .skip_while(|c| *c == ' ')
            .take_while(|c| c.is_ascii_digit())
            .collect();
        digits
            .parse::<usize>()
            .unwrap_or_else(|err| panic!("--{long} 的帮助里 {marker:?} 之后读不出一个数（拿到 {digits:?}）：{err}；原文：{help}"))
    }

    /// **T99-H3 第 3 节第 1 条**：`--connections` 的**帮助写的数**与**实现拒的数**必须是同一个数。
    ///
    /// 扩的是既有这一发（简报明令"别重复钉、别新开平行用例"）：旧写法把 0 与 1 两个数**写死**在
    /// 测试里 ⇒ 帮助改成"至少 2"、或实现改成拒 `n < 2`，四闸照绿（控制员复量：`grep -rn "至少 1" src/ tests/`
    /// 三处同源文本、零根针）。现在数从 `number_from_help("connections", "至少")` 读，
    /// 于是 `N-1` 必须被拒、`N` 必须放行且真的进到 `Config::workers`。
    /// 两臂各管一侧：只断 `N-1` 被拒的话，"把实现改成什么都拒"也能绿；只断 `N` 放行的话，
    /// "把实现改成什么都放"也能绿（E28⑧-1 那条纪律在边界针上的样子）。
    #[test]
    fn connections_lower_bound_is_a_parameter_error() -> anyhow::Result<()> {
        let n = number_from_help("connections", "至少");
        let below = n
            .checked_sub(1)
            .unwrap_or_else(|| panic!("帮助里的下限是 {n}，那 `N-1` 这一臂没有输入可用"));
        let below_text = below.to_string();
        let n_text = n.to_string();

        let Err(err) = parse(&["dlm", "--url", "https://x/a", "--connections", &below_text]) else {
            panic!("--connections {below} 必须在边界就被拒（这个数是**帮助**里那个下限减一，E23⑤）");
        };
        let msg = err.to_string();
        assert!(msg.contains("参数写错了") && msg.contains("--connections"), "{msg}");

        let c = parse(&["dlm", "--url", "https://x/a", "--connections", &n_text])?;
        assert_eq!(
            c.into_config().workers, n,
            "帮助说至少 {n}，那 {n} 就是最小合法值，必须原样进 Config::workers（两侧不同源＝这一发的全部意义）"
        );
        Ok(())
    }

    /// **T99-H3 第 3 节第 2 条**：`--sha256` 同一形状 —— 帮助里"需 **N** 位十六进制"那个数与
    /// `check_boundaries` 的长度判据绑在一起（`N-1` 位拒、`N` 位放）。
    ///
    /// ⚠ 期望值**不许问产品自己要**：这里不调用被测的长度判据去算"该给几位"，输入由测试自己
    /// 按**读来的那个数**构造（`"0".repeat(n - 1)` 与 `"0".repeat(n)`，全用合法十六进制字符 ⇒
    /// 红的时候只能是**长度**这一维撞的，不可能是字母表撞的）。既有那圈硬编码用例（32 位／64 个
    /// `g`／空串／65 位）一字没动，这一格只是把"边界到底在哪一格"从写死改成从帮助读。
    #[test]
    fn sha256_length_bound_is_read_from_the_help_not_hardcoded() -> anyhow::Result<()> {
        let n = number_from_help("sha256", "需");
        let short = "0".repeat(n - 1);
        let exact = "0".repeat(n);

        let Err(err) = parse(&["dlm", "--url", "https://x/a", "--sha256", &short]) else {
            panic!("--sha256 给 {n} 减一（{} 个）位必须被拒（帮助写的就是这个数）", n - 1);
        };
        let msg = err.to_string();
        assert!(
            msg.contains("参数写错了") && msg.contains("--sha256"),
            "越界要照参数错说，不许路由成引擎那句\"校验不符\"（E20⑨）：{msg}"
        );

        let c = parse(&["dlm", "--url", "https://x/a", "--sha256", &exact])?;
        assert_eq!(
            c.into_config().verify_sha256.as_deref(),
            Some(exact.as_str()),
            "帮助说需 {n} 位，那 {n} 个十六进制字符就是合法值，必须原样交给引擎"
        );
        Ok(())
    }

    #[test]
    fn every_long_option_lands_on_the_task() -> anyhow::Result<()> {
        let t = task(&[
            "dlm",
            "--url",
            "https://x/dir/a.bin",
            "--out-dir",
            "D:/downloads",
            "--out-name",
            "renamed.bin",
            "--referer",
            "https://ref.example/page",
            "--cookie",
            "a=1; b=2",
            "--data-file",
            "D:/body/post.bin",
            "--user",
            "alice",
            "--password",
            "hunter2",
            "--ignore-mtime",
            "--keep-state",
        ])?;
        assert_eq!(t.url, "https://x/dir/a.bin");
        assert_eq!(t.referer.as_deref(), Some("https://ref.example/page"));
        assert_eq!(t.cookies.as_deref(), Some("a=1; b=2"));
        assert_eq!(t.data_file, Some(PathBuf::from("D:/body/post.bin")));
        assert_eq!(t.user.as_deref(), Some("alice"));
        assert_eq!(t.password.as_deref(), Some("hunter2"), "口令的值必须原样走到 Task（掩码只属于 Debug）");
        assert_eq!(t.dir, PathBuf::from("D:/downloads"));
        assert_eq!(t.name.as_deref(), Some("renamed.bin"));
        assert_eq!(t.flags, FLAG_IGNORE_MTIME | FLAG_KEEP_STATE);
        Ok(())
    }

    #[test]
    fn url_is_required_and_out_dir_defaults_to_the_cwd() -> anyhow::Result<()> {
        let Err(err) = parse(&["dlm", "--quiet"]) else {
            panic!("没有 URL 就没有可下的东西，--url 必须是必填项（spec §5 的 usage）");
        };
        assert!(err.to_string().contains("--url"), "错误要指出缺的是 --url：{err}");
        assert_eq!(task(&["dlm", "--url", "https://x/a.bin"])?.dir, PathBuf::from("."), "spec §5 没给 --out-dir 的默认值 ⇒ 落在当前目录");
        Ok(())
    }

    /// `Config` 的四格默认值与显式值各归其位。
    ///
    /// **T99-J2 第 3 节（Q5-1）在这里加了一格交叉针**：`--connections` 帮助里"默认 **8**"那个数与
    /// `src/consts.rs` 的 `DEFAULT_CONNECTIONS` 必须相等。这**不**违反"期望值不许问被测产品自己要"
    /// —— 这里正是拿一份对面（帮助文本）问另一份（实现常数），与 `connections_lower_bound_is_a_parameter_error`
    /// 同一立场；此前两侧同源于实现常数 ⇒ 把帮助里的 8 改成 9（纯文案漂移）四闸照绿。
    /// ⚠ `number_from_help` 只认「marker ＋ 空格 ＋ 十进制」这一种排版，帮助哪天写成 `默认：8`／`≥ 8`
    /// 它会**当场 panic**（宁可红不静默，与 M-4② 同一立场）。
    #[test]
    fn into_config_maps_every_documented_field() -> anyhow::Result<()> {
        let c = parse(&[
            "dlm",
            "--url",
            "https://x/a",
            "--connections",
            "3",
            "--max-speed",
            "1048576",
            "--max-retries",
            "7",
            "--sha256",
            HEX64,
            "--no-resume",
        ])?;
        let cfg = c.into_config();
        assert_eq!(cfg.workers, 3, "E20⑤：--connections 必须是 Config::workers 的唯一来源，不许被 Task 上任何数吞掉");
        assert_eq!(cfg.max_speed, 1_048_576, "--max-speed 必须原样进 Config::max_speed");
        assert_eq!(cfg.max_retries, Some(7));
        assert_eq!(cfg.verify_sha256.as_deref(), Some(HEX64));
        assert!(cfg.force_fresh, "--no-resume ⇒ Config::force_fresh（spec §4）");
        assert_eq!(cfg.min_seg, MIN_SEGMENT_BYTES, "spec §5 的长选项里没有一个能改 min_seg ⇒ 它只能保持默认");
        assert!(cfg.on_tick.is_none(), "进度行的 sink 由 9B-2 挂（E22 第 4 条），这一层不碰");
        // `into_config` 借 `&self` ⇒ 之后还能 `into_task`（9B-2 要先看 quiet/no_start 再消费）。
        assert_eq!(c.into_task()?.flags, 0);
        let d = parse(&["dlm", "--url", "https://x/a"])?.into_config();
        assert_eq!(
            (d.workers, d.max_speed, d.max_retries, d.verify_sha256, d.force_fresh),
            (DEFAULT_CONNECTIONS, 0, None, None, false),
            "什么都不给时五格各归其位（max_speed 默认 0 = 不限）"
        );
        // **T99-J2 第 3 节（Q5-1）**：帮助里那个"默认 N"与实现常数交叉一次（期望值来自**实现常数**、
        // 输入来自**帮助文本**，两份对面互问；这不是"问被测产品自己要"，与 `…_lower_bound_…` 同一立场）。
        // 不并进来时的实测缺口：把本文件选项表里 `--connections` 那条 `#[arg(help = …)]` 的"默认 8"
        // 改成"默认 9"而实现一字不动，四闸全绿。
        assert_eq!(
            number_from_help("connections", "默认"),
            DEFAULT_CONNECTIONS,
            "`--connections` 的帮助写的默认值与 `consts::DEFAULT_CONNECTIONS` 必须是同一个数（T99-J2 第 3 节）"
        );
        Ok(())
    }

    #[test]
    fn long_option_equal_form_does_not_swallow_the_next_token() -> anyhow::Result<()> {
        let c = parse(&["dlm", "--url=https://x/a.bin", "--connections=4", "/n", "/f", "b.bin"])?;
        assert!(c.quiet(), "--url=… 这种写法不吃下一个参数，所以后面的 /n 仍是别名");
        assert_eq!(c.into_task()?.name.as_deref(), Some("b.bin"));
        Ok(())
    }

    /// 值位上长得像别名的 token 不许被展开成旗标（E28④ 的骨架）。
    /// **T99-G5 第 1 条的裁定（控制员）**：新立的 `--out-name` 越目录闸住在 `Task::validate()`，
    /// 于是这一发原来那两臂（都拿 `--out-name` 的值走 `into_task()`）会被新闸当场拒 ⇒
    /// 处置＝**降到 parse 层钉性质 ＋ 另起一小臂钉新闸**：本臂真正测的"值位上的 `/f` 不被展开"
    /// 与"这个名字合不合规"无关，所以期望值从 `Task.name` 改读 `Cli.out_name`（同一件事的同一方向，
    /// 不是放宽、不是删）；越目录那一格现在由 `into_task()` 拒，那是第 1 条的牙，不是别名层的牙。
    #[test]
    fn a_slash_token_in_value_position_stays_a_value() -> anyhow::Result<()> {
        // 臂 1（parse 层）：`/f` 在值位上是合法的**值**。
        let c1 = parse(&["dlm", "--url", "https://x/a.bin", "--out-name", "/f"])?;
        assert_eq!(c1.out_name.as_deref(), Some("/f"), "`/f` 在值位上**不被展开成别名**（它合不合规是落点闸那道另一码事）；把它展开成别名会把用户的参数吃掉");
        // 臂 2（parse 层）：`--url`/`--out-name` 都是值位旗标，值位上的 `/d` 既不当静默旗标、也不吃左值。
        let c2 = parse(&["dlm", "--url", "https://x/n", "--out-name", "/d"])?;
        assert!(!c2.quiet(), "`--url`/`--out-name` 都是值位旗标：值位上长得像别名的 /xxx 串不被展开成静默旗标");
        assert_eq!(c2.url, "https://x/n", "值位的 /d 没有把 --url 的值吃掉");
        // 臂 3（新闸，T99-G5 第 1 条）：越目录的名字由 `Task::validate()` 当场拒，不在 parse 层。
        assert!(c2.into_task().is_err(), "/d 这种绝对形状的 --out-name 现在必须在 into_task() 就被拒");
        Ok(())
    }

    /// **E28④**：值位探测今天只认**输入里**的 `--长名`；别名命中那一支吐出长名后 `continue`，
    /// 不置值位 ⇒ 别名展开出来的 `--out-name` 后面那个 token（用户的**值**）又被当别名位展开。
    /// 实测：`dlm /d URL /f /n` 退 2（"a value is required for '--out-name <NAME>'"），
    /// 而长选项写法 `dlm --url URL --out-name /n` 退 0 —— 别名层的等价性自己拆了。
    /// 每一行是（别名写法，长选项写法），两写法必须**逐字段相等**（`Cli` 派生 `PartialEq`，
    /// 手写 `Debug` 掩口令 ⇒ 两格的 `assert_eq!` 都是本文件的既有形状）。
    /// 表里 (b) 组是牙：值本身长得像别名 token；(c) 组守另一半 hazard —— 展开成**旗标**或
    /// **什么都不是**的别名（`/n` `/a` `/q`）绝不许多置值位，那会把下一个真参数吞掉。
    #[test]
    fn an_expanded_alias_still_occupies_its_value_slot() {
        const URL: &str = "https://x/a.bin";
        let cases: &[(&[&str], &[&str])] = &[
            // (a) 普通值：三条吃值别名各一发（今天已是绿的，留作表的骨架）。
            (&["dlm", "/d", URL], &["dlm", "--url", URL]),
            (&["dlm", "/d", URL, "/p", "D:/d"], &["dlm", "--url", URL, "--out-dir", "D:/d"]),
            (&["dlm", "/d", URL, "/f", "b.bin"], &["dlm", "--url", URL, "--out-name", "b.bin"]),
            // (b) 值本身长得像别名 token —— E28④ 的实测缺陷就咬在这一组。
            (&["dlm", "/d", URL, "/f", "/n"], &["dlm", "--url", URL, "--out-name", "/n"]),
            (&["dlm", "/d", URL, "/f", "/q"], &["dlm", "--url", URL, "--out-name", "/q"]),
            (&["dlm", "/d", URL, "/p", "/onboot"], &["dlm", "--url", URL, "--out-dir", "/onboot"]),
            (&["dlm", "/d", "/n", "/f", "/q"], &["dlm", "--url", "/n", "--out-name", "/q"]),
            // (c) 展开成旗标/被忽略项的别名不吃下一 token：旗标别名后头跟的仍是待展开的别名。
            (&["dlm", "/d", URL, "/n", "/f", "b.bin"], &["dlm", "--url", URL, "--quiet", "--out-name", "b.bin"]),
            (&["dlm", "/d", URL, "/a", "/f", "b.bin"], &["dlm", "--url", URL, "--no-start", "--out-name", "b.bin"]),
            (&["dlm", "/d", URL, "/q", "/f", "b.bin"], &["dlm", "--url", URL, "--out-name", "b.bin"]),
        ];
        for (idm, posix) in cases {
            let a = parse(idm);
            let b = parse(posix);
            match (a, b) {
                (Ok(x), Ok(y)) => assert_eq!(x, y, "别名写法 {idm:?} 必须与长选项写法 {posix:?} 逐字段相等（E28④ 的等价性）"),
                (Err(x), Err(y)) => panic!("两种写法都失败了：{idm:?} ⇒ {x}；{posix:?} ⇒ {y}"),
                (Ok(_), Err(y)) => panic!("别名写法 {idm:?} 成了、长选项写法 {posix:?} 却失败：{y}"),
                (Err(x), Ok(_)) => panic!("长选项写法 {posix:?} 成了、别名写法 {idm:?} 却失败：{x}（E28④：展开出来的长名没占值位）"),
            }
        }
    }

    /// **E28⑧-2 的反向针**：`--min-seg` / `--probe-bytes` 这两个旋钮**不存在**
    /// （E20⑥/E23⑤ 裁过"不许为 `min_seg`/`prelude_max` 发明 CLI 入口"，spec §10 R8 如实登记）。
    /// 今天它们缺席只由"`Cli` 里没有这个字段"保证——那种保证红不了，也就不会在下一轮真把字段
    /// 加上时报警。这一发不测新行为，是两行保险：谁给 `Cli` 加了这两条长选项，这里先红。
    #[test]
    fn min_seg_and_probe_bytes_have_no_cli_entry() {
        for (flag, value) in [("--min-seg", "1"), ("--probe-bytes", "4096")] {
            let Err(err) = parse(&["dlm", "--url", "https://x/a", flag, value]) else {
                panic!("{flag} 被接受了 —— dlm 没有这个旋钮，E20⑥/E23⑤ 禁止为它发明 CLI 入口");
            };
            let msg = err.to_string();
            assert!(msg.contains(flag), "拒绝要原样点名 {flag}，用户才知道哪格写错了：{msg}");
        }
    }

    #[test]
    fn cli_debug_masks_the_password_field() -> anyhow::Result<()> {
        let c = parse(&[
            "dlm",
            "--url",
            "https://x/a",
            "--user",
            "alice",
            "--password",
            "hunter2",
            "--quiet",
            "--no-start",
            "--connections",
            "2",
            "--sha256",
            HEX64,
        ])?;
        let d = format!("{c:?}");
        assert!(!d.contains("hunter2"), "Debug 里不能有口令本尊：错误路径上的 Debug 打印会把口令交进日志（E9 给 Task 的那份立场）：{d}");
        assert!(d.contains("password: Some(\"[redacted]\")"), "口令要显式渲染成掩码而不是消失：{d}");
        for want in [
            "url: \"https://x/a\"",
            "user: Some(\"alice\")",
            "quiet: true",
            "no_start: true",
            "connections: Some(2)",
            HEX64,
        ] {
            assert!(d.contains(want), "Debug 里 {want} 该还在（掩码只针对 password）：{d}");
        }
        let clean = format!("{:?}", parse(&["dlm", "--url", "https://x/a"])?);
        assert!(clean.contains("password: None"), "没口令时该是 password: None：{clean}");
        assert!(!clean.contains("[redacted]"), "没口令就不该留空掩码：{clean}");
        Ok(())
    }

    /// **重复别名那一支（E23 附记② / E24④）**：别名层的唯一判据是"与长选项写法等价"，
    /// 所以 `dlm /d A /d B` 与 `dlm --url A --url B` 必须给出**同一个**结果，不许一种报错一种不报。
    ///
    /// ⚠ 派发那句"`--url A --url B` ⇒ `url == \"B\"`，不报错"在本仓库今天的 clap 4 上**不成立**：
    /// `Cli` 的 `url: String` 走派生出来的 `ArgAction::Set`，而 clap 4 对重复出现的单值参数默认就报
    /// `the argument '--url <URL>' cannot be used multiple times`（实测见这条用例的 `eprintln!`）。
    /// 要让那半句成真得改 `Cli` 的 `#[arg(...)]` 属性面，而那不在 9B-2 的允许改动清单里
    /// ⇒ 这一格把**裁定里可钉的那一半**（两写法逐字段等价、且不许静默吞掉第二次出现）钉死，
    /// 另一-half 等控制员裁（报告关切 1）。等价性一旦自己拆了（例如别名层给重复别名一句自己的报错），
    /// 下面那条 mixed 分支就是活体针。
    #[test]
    fn a_repeated_alias_behaves_exactly_like_a_repeated_long_option() {
        let alias = parse(&["dlm", "/d", "https://x/a", "/d", "https://x/b"]);
        let long = parse(&["dlm", "--url", "https://x/a", "--url", "https://x/b"]);
        let text = |r: &anyhow::Result<Cli>| r.as_ref().map(|c| format!("{c:?}")).unwrap_or_else(|e| e.to_string());
        eprintln!("重复别名 ⇒ {}\n重复长选项 ⇒ {}", text(&alias), text(&long));
        match (alias, long) {
            (Ok(a), Ok(b)) => {
                assert_eq!(a, b, "重复给同一条别名必须与重复给长选项逐字段相等（E23 附记②）");
                assert_eq!(a.into_task().unwrap().url, "https://x/b", "后者覆盖前者，且不许因此报错");
            }
            (Err(a), Err(b)) => {
                let (sa, sb) = (a.to_string(), b.to_string());
                assert_eq!(sa, sb, "两种写法给的错误必须逐字相同（别名层只是另一种拼法）");
                assert!(sa.contains("--url"), "重复那一支不许被静默吞掉，报错要点名长选项：{sa}");
            }
            _ => panic!("一种写法报错、另一种不报错 ⇒ 别名层的等价性自己拆了（E23 附记② 的立论）"),
        }
        // 重复的是**吃值的别名**，值位探测也得跟着对：`/p` 重复时两写法仍然同形。
        let both = parse(&["dlm", "/d", "https://x/a", "/p", "D:/one", "/p", "D:/two"]);
        let both_long = parse(&["dlm", "/d", "https://x/a", "--out-dir", "D:/one", "--out-dir", "D:/two"]);
        assert_eq!(both.is_err(), both_long.is_err(), "重复 `/p` 与重复 `--out-dir` 必须同一个结论");
        match (both, both_long) {
            (Ok(x), Ok(y)) => {
                assert_eq!(x, y);
                assert_eq!(y.into_task().unwrap().dir, PathBuf::from("D:/two"), "以后者为准");
            }
            (Err(x), Err(y)) => assert_eq!(x.to_string(), y.to_string(), "同上：错误也要逐字相同"),
            _ => unreachable!("上面已经断过 is_err 同形"),
        }
        // 重复的**开关型**别名（`/n`）与重复的 `--quiet` 也必须在同一个结论上对齐：
        // 实测两写法都报 `the argument '--quiet' cannot be used multiple times`（clap 4 对重复出现的
        // 单值参数与 `SetTrue` 旗标都不给"后者覆盖"）。这一格的意义不在"报错对不对"（那归控制员裁，
        // 报告关切 1），而在**别名层没给自己单开一条结论**。
        let flag_alias = parse(&["dlm", "/d", "https://x/a", "/n", "/n"]);
        let flag_long = parse(&["dlm", "--url", "https://x/a", "--quiet", "--quiet"]);
        assert_eq!(flag_alias.is_err(), flag_long.is_err(), "重复 `/n` 与重复 `--quiet` 必须同一个结论");
        match (flag_alias, flag_long) {
            (Ok(a), Ok(b)) => {
                assert_eq!(a, b, "两种写法逐字段相等");
                assert!(b.quiet(), "重复的静默旗标仍然只 meant 一件事");
                assert_eq!(a.into_task().unwrap().flags, 0, "重复 `/n` 不许往 flags 里塞任何位（E20③）");
            }
            (Err(a), Err(b)) => assert_eq!(a.to_string(), b.to_string(), "错误也要逐字相同：{a}"),
            _ => unreachable!("上面已经断过 is_err 同形"),
        }
    }

    #[test]
    fn the_alias_table_is_the_c3_attestation_and_nothing_more() {
        // 这一格钉的是**表本身**（E23①②③④）：映射错了就整体读回头像，而"多一个别名"就是发明功能。
        let mapped: Vec<&str> = ALIASES.iter().filter(|(_, t)| t.is_some()).map(|(a, _)| *a).collect();
        let ignored: Vec<&str> = ALIASES.iter().filter(|(_, t)| t.is_none()).map(|(a, _)| *a).collect();
        assert_eq!(mapped, vec!["/d", "/p", "/f", "/n", "/a"], "长选项映射逐条照 recon C3");
        assert_eq!(ignored, vec!["/q", "/s", "/onboot"], "接受并忽略的那三条也只许是 C3 里有据的三条");
        assert!(!ALIASES.iter().any(|(a, _)| *a == "/h"), "/h 在 C3 里没有那一行 ⇒ v1 不实现（E23④）");
        assert_eq!(
            ALIASES.iter().find(|(a, _)| *a == "/d").and_then(|(_, t)| *t),
            Some("--url"),
            "mu1 的靶子：/q 才是被计划写反的那条，/n 才映射 --quiet"
        );
        assert_eq!(
            ALIASES.iter().find(|(a, _)| *a == "/n").and_then(|(_, t)| *t),
            Some("--quiet")
        );
        assert_eq!(
            ALIASES.iter().find(|(a, _)| *a == "/a").and_then(|(_, t)| *t),
            Some("--no-start")
        );
    }

    /// **T99-G5b 1c-1：结构针 —— 帮助面每一格都得有说明，别名表真要印出来。**
    ///
    /// 这一族的第二课（G5 之后才看得清，也是本发存在的理由）：**名字出现在帮助里 ≠ 帮助教了东西**。
    /// 既有的进程针 `tests/integration.rs::the_binary_help_prints_no_internal_review_prose` 断的是
    /// "六条长名在场 ＋ 没有内部措辞"，那种判据对 `c3a3cfd` 那一版帮助（16 条选项全部列出、
    /// 说明一格都没有）**照样绿** —— 四闸全绿的那一发就是它的活体现场。所以这里断 `Command` 的结构，
    /// 两个方向都要有牙：
    /// ① 任何**带长名的参数**（含 clap 自带的 help/version）`get_help()` 必须是 `Some` 且文本非空
    ///    ⇒ 谁给 `Cli` 加了新选项却没写说明，这里先红；
    /// ② 下面 `DOCUMENTED_LONGS` 那串长名**一条都不许缺**
    ///    ⇒ 反过来，把选项整条删掉或藏起来让①只剩空面，也红。
    /// ③ 别名表那五行、"接受并忽略"那一行、"表以外报错"那一行都必须在**渲染出来的**帮助文本里
    ///    （m2 删掉 `ALIAS_EPILOG` 里 `/d` 那一行，红的就是这一格）。
    /// 期望值按渲染现场写死，不复用 `ALIAS_EPILOG`（拿被测对象自己当期望值就是恒真）。
    #[test]
    fn every_help_entry_carries_an_explanation_and_the_alias_table_is_printed() {
        use clap::CommandFactory;
        const DOCUMENTED_LONGS: &[&str] = &[
            "url",
            "out-dir",
            "out-name",
            "connections",
            "max-speed",
            "referer",
            "cookie",
            "data-file",
            "user",
            "password",
            "ignore-mtime",
            "no-resume",
            "sha256",
            "max-retries",
            "keep-state",
            "quiet",
            "no-start",
            "on-complete",
        ];
        const PRINTED_ALIAS_LINES: &[&str] = &[
            "/d → --url",
            "/p → --out-dir",
            "/f → --out-name",
            "/n → --quiet",
            "/a → --no-start",
            "/q、/s、/onboot 被接受但不做任何事（v1 没有对应的子系统）",
            "表以外的斜杠写法会直接报错",
        ];
        let mut cmd = Cli::command();
        // 先渲染：`render_long_help` 会 build，于是 clap 自带的那两条自动参数也进了 `get_arguments()`。
        let rendered = cmd.render_long_help().to_string();
        let mut longs: Vec<&str> = Vec::new();
        for arg in cmd.get_arguments() {
            let Some(long) = arg.get_long() else { continue };
            let text = arg.get_help().map(|h| h.to_string()).unwrap_or_default();
            assert!(
                !text.trim().is_empty(),
                "--{long} 在帮助里只列了名字、没有说明（名字出现在帮助里 ≠ 帮助教了东西，T99-G5b）：\n{rendered}"
            );
            longs.push(long);
        }
        for name in DOCUMENTED_LONGS {
            assert!(
                longs.contains(name),
                "帮助面少了 --{name} 这条选项 ⇒ 上面那圈\"每条都有说明\"扫的已经不是完整的面了"
            );
        }
        for line in PRINTED_ALIAS_LINES {
            assert!(
                rendered.contains(line),
                "帮助里少了别名那一句 {line:?}（斜杠写法唯一的去处不该只剩报错信息，T99-G5b 1b）：\n{rendered}"
            );
        }
    }

    #[test]
    fn on_complete_parses_open_run_and_shutdown_and_rejects_bad_values() -> anyhow::Result<()> {
        assert_eq!(
            parse(&["dlm", "--url", "https://x/a", "--on-complete", "open"])?.action(),
            Some(&CompletionAction::Open)
        );
        let run = parse(&["dlm", "--url", "https://x/a", "--on-complete", "run:C:\\tools\\scan.exe"])?;
        assert_eq!(run.action(), Some(&CompletionAction::Run("C:\\tools\\scan.exe".into())));
        let shutdown = parse(&["dlm", "--url", "https://x/a", "--on-complete", "shutdown:60"])?;
        assert_eq!(shutdown.action(), Some(&CompletionAction::Shutdown { delay_secs: 60 }));

        let err = parse(&["dlm", "--url", "https://x/a", "--on-complete", "wat"]).unwrap_err();
        assert!(format!("{err:#}").contains("open"), "错误要指可用值：{err:#}");
        let bare = parse(&["dlm", "--url", "https://x/a", "--on-complete", "shutdown"]).unwrap_err();
        assert!(format!("{bare:#}").contains("延迟"));
        let too_soon = parse(&["dlm", "--url", "https://x/a", "--on-complete", "shutdown:59"]).unwrap_err();
        assert!(format!("{too_soon:#}").contains("60"));
        Ok(())
    }
}
