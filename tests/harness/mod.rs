//! 集成测试的**真对端**：起本机 node 探针服务器（同目录的 `probe-server.mjs`），
//! 并把它的 JSONL 日志读回来当证据。只用 std + 仓库里已有的 reqwest / serde_json / anyhow。
//!
//! ## 三条从 Task 0 实测继承下来的硬规矩（改本文件前先读完）
//!
//! 1. **不加 `Connection: close`，也不设 `pool_max_idle_per_host(0)`**（spec §10 勘误 **R5**、
//!    计划勘误 **E2**，两者都明写 Task 6/7 不得加）。Task 0 实测：416 卡死的根因是本 harness 推流
//!    循环漏了 `res.end()`（服务端把后续响应留在 socket 的 pending 队列里），不是 reqwest 的连接池；
//!    加 close 不致错，但会强制每发一次试探就新拆一条连接，把"**复用连接上打 416**"这条真实路径
//!    从测试覆盖里关掉 ⇒ 勘误 **E3** 要求的守卫用例根本写不出来。
//! 2. **超时显式配置**（R5 末段：别假设"不配 timeout 就等于永不等"，那组矩阵里没配 timeout 的
//!    客户端仍然在 30.005 s 死掉，时长来源未查清）。形状按简报 Step 2：`timeout 30 s` +
//!    `connect_timeout 3 s` + `tcp_nodelay`。
//! 3. **端口每次现挑、就绪靠轮询、`Drop` 真杀进程**：cargo 用多线程并行跑用例，写死端口一定互相抢
//!    （历史会话用过 8077/8100/8123/8161/8163）；固定 `sleep` 等就绪既慢又不稳。日志一律写在
//!    `target/` 下（`.gitignore` 里有 `target/`），文件名带端口 + pid + 轮次，绝不复用。

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

/// 试探服务器脚本（Task 0 起就固定在本仓库里，本轮**未改动**它的任何路由与收尾行为）。
const SCRIPT: &str = "tests/harness/probe-server.mjs";

/// 等就绪的兜底时限：本机 node 冷启动远快过这个数，超时就是真故障。
const SPAWN_TIMEOUT: Duration = Duration::from_secs(20);
/// 日志是 node 侧异步写的，客户端拿到响应时那一行**可能**还没落盘 ⇒ 读日志一律带时限轮询。
const LOG_TIMEOUT: Duration = Duration::from_secs(5);
/// 一次轮询的间隔；本文件的等待对象是 localhost + 一次文件写，20 ms 足够。
const POLL: Duration = Duration::from_millis(20);

/// 规矩 3 的实现：**挑一个真的没人占的端口**。
///
/// 做法：向 OS 要一个空闲端口（`bind` 端口 0），拿到之后**立刻放开**，再把号报给 node。
/// 存在一个理论上的空窗（放开到 node `listen` 之间被别人抢走），所以 `spawn()` 外面套了重试：
/// 就绪等待发现 node 自己退掉了就换下一个号再来。
fn free_port() -> Result<u16> {
    let listener =
        TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).context("向 OS 要空闲端口失败")?;
    let port = listener.local_addr().context("拿不到临时端口的本地地址")?.port();
    drop(listener); // 显式放开：不出作用域的话端口一直被我们占着，node 反而会 EADDRINUSE。
    Ok(port)
}

/// `server_start` 那行日志：**服务器自己宣布**的 blob 事实。
///
/// 存在的原因（派发第 7 条）：简报把 ETag 写成字面量 `"\"probe-f675558c108f46fc\""`，而 harness 的
/// ETag 是从 blob 的 sha256 现算的（`'"probe-' + SHA.slice(0, 16) + '"'`）⇒ 那个字面量是"上一轮会话
/// 抄下来的值"，blob 一改就漂。断言的对端必须是**这台正在服务的服务器报出来的值**，不是历史值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announced {
    pub port: u16,
    /// blob 总字节数（`/blob` 的 48 MiB）。
    pub size: u64,
    /// 整块 blob 的 sha256（十六进制小写）。
    pub sha256: String,
    /// 响应头 `ETag` 的原样值，**含**外层双引号。
    pub etag: String,
    /// 响应头 `Last-Modified` 的原样值。
    pub last_modified: String,
}

/// 一个活着的探针服务器进程 + 它的日志路径。`Drop` 时杀进程、等它退、删日志。
#[derive(Debug)]
pub struct Server {
    child: Child,
    port: u16,
    log: PathBuf,
}

impl Server {
    pub fn port(&self) -> u16 {
        self.port
    }

    /// `http://127.0.0.1:<port>`（末尾**不带**斜杠，调用方拼 `{base}/blob`）。
    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// 服务器自己宣布的事实（读 `server_start` 那行；落盘有延迟 ⇒ 轮询）。
    pub fn announced(&self) -> Result<Announced> {
        let evs = self
            .events_while(|evs| evs.iter().any(|e| e["ev"] == "server_start"))
            .context("等不到 server_start 那行日志（node 起来了却没写日志？）")?;
        let start = evs
            .iter()
            .find(|e| e["ev"] == "server_start")
            .ok_or_else(|| anyhow!("events_while 的期望没被满足却返回了（不可能到达）"))?;
        let text = |k: &str| {
            start[k]
                .as_str()
                .map(str::to_string)
                .with_context(|| format!("server_start 里 {k} 不是字符串"))
        };
        Ok(Announced {
            port: start["port"].as_u64().context("server_start.port 不是整数")? as u16,
            size: start["size"].as_u64().context("server_start.size 不是整数")?,
            sha256: text("sha256")?,
            etag: text("etag")?,
            last_modified: text("lastModified")?,
        })
    }

    /// 此刻已落盘的全部事件（不等待）。
    pub fn events(&self) -> Result<Vec<Value>> {
        read_jsonl(&self.log)
    }

    /// 轮询读日志直到 `want` 满意（或 `LOG_TIMEOUT` 到）。
    pub fn events_while(&self, want: impl Fn(&[Value]) -> bool) -> Result<Vec<Value>> {
        let deadline = Instant::now() + LOG_TIMEOUT;
        loop {
            let evs = self.events()?;
            if want(&evs) {
                return Ok(evs);
            }
            if Instant::now() >= deadline {
                bail!(
                    "{LOG_TIMEOUT:?} 内日志没满足期望（{} 行，末尾三条：{:?}；日志 {}）",
                    evs.len(),
                    evs.iter().rev().take(3).collect::<Vec<_>>(),
                    self.log.display()
                );
            }
            thread::sleep(POLL);
        }
    }

    /// 服务端看到的**请求**行（`ev == "request"`），按到达顺序。
    ///
    /// 这是"我们的客户端到底**发出了**什么头"的唯一权威证据 —— 响应侧的断言只能证明收到什么，
    /// 证明不了发出什么（`Range` 有没有发、发的是不是限量那一条，都只能在这里看）。
    pub fn requests(&self) -> Result<Vec<Value>> {
        Ok(self.events()?.into_iter().filter(|e| e["ev"] == "request").collect())
    }

    /// 等"服务端至少看到了 `n` 条请求"，返回请求行（不含 `server_start` 之类）。
    pub fn requests_at_least(&self, n: usize) -> Result<Vec<Value>> {
        let evs =
            self.events_while(|evs| evs.iter().filter(|e| e["ev"] == "request").count() >= n)?;
        Ok(evs.into_iter().filter(|e| e["ev"] == "request").collect())
    }

    /// 在 `wait` 这段时间里**守着**日志：一旦出现请求就立刻把请求行交回来；一直没有就给空表。
    ///
    /// "不该发出任何请求"这类断言不能用 `requests().is_empty()` 直接下 —— 日志行的落盘晚于
    /// 客户端拿到响应，那样的断言会在真故障上碰巧为真（本项目已经踩过三次"恒绿夹具"）。
    pub fn requests_within(&self, wait: Duration) -> Result<Vec<Value>> {
        let deadline = Instant::now() + wait;
        loop {
            let got = self.requests()?;
            if !got.is_empty() || Instant::now() >= deadline {
                return Ok(got);
            }
            thread::sleep(POLL);
        }
    }

    /// 杀掉 node 进程（`Drop` 调它）。
    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
        // 日志留在 `target/` 下会长，用例跑完就删；删不掉不是故障（进程刚放开盘子时可能还没放）。
        let _ = std::fs::remove_file(&self.log);
    }
}

/// 起一台探针服务器，返回 `(守护, base_url)`。见模块文档第 3 条。
///
/// 重试的必要性：`free_port()` 放开端口到 node `listen` 之间有空窗，端口被别人抢先占掉时
/// node 会以 `EADDRINUSE` 退出 —— 那种失败必须换号重来，而不是把超时摊给用例。
pub fn spawn() -> Result<(Server, String)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let script = root.join(SCRIPT);
    if !script.exists() {
        bail!("找不到探针服务器脚本：{}", script.display());
    }
    let dir = log_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("建日志目录失败：{}", dir.display()))?;

    let mut problems = vec![];
    for attempt in 0..5u32 {
        let port = free_port()?;
        let log = dir.join(format!("probe-{}-{}-{attempt}.jsonl", port, std::process::id()));
        // node 以 **append** 打开日志，同名残留会污染断言 ⇒ 先确保没有。
        let _ = std::fs::remove_file(&log);
        let mut child = start_node(&script, port, &log)?;
        match wait_until_listening(&mut child, port, attempt) {
            Ok(()) => {
                let base = format!("http://127.0.0.1:{port}");
                return Ok((Server { child, port, log }, base));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                problems.push(format!("第 {attempt} 次（端口 {port}）：{e:#}"));
            }
        }
    }
    Err(anyhow!("试了 5 次都没能把探针服务器起起来：\n{}", problems.join("\n")))
}

fn start_node(script: &Path, port: u16, log: &Path) -> Result<Child> {
    // 三个 stdio 全挂 null：node 起来会 `console.log` 一行横幅，留着管道没人读的话
    // 缓冲区写满会把它堵住（我们的断言全在日志文件里，不需要 stdout）。
    let spawned = Command::new("node")
        .arg(script)
        .arg(port.to_string())
        .arg(log)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match spawned {
        Ok(child) => Ok(child),
        // Windows 上 PATH 里通常是 `node.exe`，`Command::new("node")` 本来就能找到；
        // 这一支只兜"只有 node.exe 这个可执行名被注册"的怪形状，错误里带上两次尝试。
        Err(first) => Command::new("node.exe")
            .arg(script)
            .arg(port.to_string())
            .arg(log)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("本机没有可用的 node（先试 node 报：{first}）")),
    }
}

/// 轮询端口直到能连；node 半路自己退出就立刻失败（不空等到超时）。
fn wait_until_listening(child: &mut Child, port: u16, attempt: u32) -> Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let deadline = Instant::now() + SPAWN_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().context("查 node 进程状态失败")? {
            bail!(
                "node 自己退出了（{status}）—— 大概率是端口 {port} 在空窗里被人抢了（第 {attempt} 次尝试）"
            );
        }
        // `connect_timeout` 而不是 `connect`：后者对"没人听的端口"在本机上也可能停一会儿，
        // 那会把 SPAWN_TIMEOUT 摊薄成"每次轮询都睡几秒"。
        if TcpStream::connect_timeout(&addr, POLL).is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("{SPAWN_TIMEOUT:?} 内端口 {port} 连不上（node 没起来或没 listen 到 127.0.0.1）");
        }
        thread::sleep(POLL);
    }
}

/// 读 JSONL；文件还不存在时给空表（node 可能在 `listen` 之后才创建它）。
fn read_jsonl(path: &Path) -> Result<Vec<Value>> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e).with_context(|| format!("读探针日志失败：{}", path.display())),
    };
    let mut out = vec![];
    for (i, line) in raw.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue; // 纯空行；半截行由下面那一步当场报错，不静默跳过
        }
        let v: Value = serde_json::from_str(line)
            .with_context(|| format!("探针日志第 {} 行不是合法 JSON：{line}", i + 1))?;
        out.push(v);
    }
    Ok(out)
}

/// `<仓库>/target/harness`（`target/` 已在 `.gitignore` 里）。
fn log_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("harness")
}

// ---------------------------------------------------------------- 客户端与任务夹具

/// 简报 Step 2 要求的形状（见模块文档第 2 条）：**没有** `Connection: close`、**没有**
/// `pool_max_idle_per_host(0)`。
pub fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(3))
        .tcp_nodelay(true)
        .build()
        .context("建 reqwest blocking 客户端失败")
}

/// 简报用例里 `..harness::blank_task()` 的那个空任务：除 `url`（和个别用例的 `dir`）外全是默认值。
///
/// `dir` 指向 `target/harness-scratch`（建好）：`probe` 自己不碰磁盘，但 Task 7/8 的用例拿同一个夹具
/// 就要落盘，落点必须 gitignored。
pub fn blank_task() -> dlm::task::Task {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("harness-scratch");
    std::fs::create_dir_all(&dir).expect("建 harness-scratch 目录");
    dlm::task::Task {
        url: String::new(),
        referer: None,
        cookies: None,
        data_file: None,
        user: None,
        password: None,
        dir,
        name: None,
        flags: 0,
    }
}

// ---------------------------------------------------------------- 线上头的独立取证

/// **不经 reqwest** 手写一次 HTTP/1.1 请求，只取回响应头那一段（含状态行）。
///
/// 为什么需要它：`ProbeFacts` 是"我们对响应头的解读"，而解读不能拿自己的输出去证明 ——
/// 简报要断言 `Accept-Ranges` 在 `/noranges` 上**不存在**，那就必须有一个独立于解析代码的观察者。
/// 本函数返回原始文本，用例里 `eprintln!` 出来就是报告引用的证据。
///
/// 只读到头结束（`\r\n\r\n`）就松手，body 留在连接里 ⇒ 这条连接之后由 node 那侧收尾；
/// 它**不是** probe 走的路径（probe 用 `client()`），所以它的连接状态不进任何断言。
pub fn raw_response_head(port: u16, target: &str, extra: &[(&str, &str)]) -> Result<String> {
    let mut sock = TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], port)))
        .with_context(|| format!("连不上 127.0.0.1:{port}"))?;
    let mut req = format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n");
    for (k, v) in extra {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    sock.write_all(req.as_bytes()).context("手写请求没发出去")?;
    sock.flush().context("手写请求没 flush")?;
    let mut got = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = sock.read(&mut buf).context("读手写响应失败")?;
        if n == 0 {
            break; // 对端先关了
        }
        got.extend_from_slice(&buf[..n]);
        if let Some(pos) = find_head_end(&got) {
            // 只留头段：同一次读里已经带进来的 body 字节不当成"响应头"交出去
            // （否则用例的 `eprintln!` 会往测试输出里倒几 KiB 的 blob，取证反被噪声埋掉）。
            got.truncate(pos + 4);
            break;
        }
        if got.len() > 64 * 1024 {
            break; // 一直没等到头结束 = 对端不对劲
        }
    }
    Ok(String::from_utf8_lossy(&got).into_owned())
}

/// `\r\n\r\n` 的起始下标。
fn find_head_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|w| w == b"\r\n\r\n")
}

/// 状态行里的三位数字（`HTTP/1.1 206 ...` → `206`）。
pub fn head_status(head: &str) -> Result<u16> {
    let line = head.lines().next().context("响应头一段是空的")?;
    let code = line
        .split_whitespace()
        .nth(1)
        .with_context(|| format!("状态行看不出状态码：{line}"))?;
    code.parse::<u16>().with_context(|| format!("状态码不是数字：{line}"))
}

/// 某个响应头的原样值（同名多行取第一条；名字大小写不敏感）。
pub fn head_header(head: &str, name: &str) -> Option<String> {
    let name = name.to_ascii_lowercase();
    head.lines().skip(1).find_map(|line| {
        let (k, v) = line.split_once(':')?;
        (k.trim().to_ascii_lowercase() == name).then(|| v.trim().to_string())
    })
}

/// 某个响应头**是否存在**。`/noranges` 的断言靠的就是"不存在"这一侧。
pub fn head_has(head: &str, name: &str) -> bool {
    head_header(head, name).is_some()
}

/// blob 的**内容**取证：harness 的 blob 是"每 4 KiB 块的块号（小端 u32）写在块首"
/// （`BLOB.writeUInt32LE(i / 4096, i)`）。块与块之间是 `Buffer.allocUnsafe` 留下的未初始化字节
/// —— 本机实测为大页分配、通常是 0，但那是分配器的副产品，不是契约 ⇒ **只断言块号那 4 字节**。
///
/// 为什么值得断：`prelude.len()` 只证明"读了这么多字节"，不证明"读的是开头那批字节、而且没错位"。
/// 块号按位置递增，少读一段、或者从偏移 4096 开始读，都会在这里当场红。
pub fn assert_blob_block_numbers(bytes: &[u8], start_offset: u64) -> Result<()> {
    const BLOCK: usize = 4096;
    let first_block = usize::try_from(start_offset / BLOCK as u64).context("偏移太大")?;
    for (i, chunk) in bytes.chunks(BLOCK).enumerate() {
        if chunk.len() < 4 {
            continue; // 末尾不足 4 字节的一截取不出块号
        }
        let want = u32::try_from(first_block + i).context("块号溢出 u32")?;
        let got = u32::from_le_bytes(chunk[..4].try_into().expect("上面已判过长度 >= 4"));
        assert_eq!(
            got,
            want,
            "第 {} 块（文件偏移 {}）开头的块号不对：拿到 {got}，blob 该是 {want} \
             —— 预读的字节不是从 {start_offset} 起，或者中间掉过一段",
            first_block + i,
            start_offset + (i * BLOCK) as u64,
        );
    }
    Ok(())
}

// ---------------------------------------------------------------- 手写的窄对端

/// 一个 **std 手写**的窄对端：每条请求由调用方给的 `responder` 决定回什么原始字节，
/// 同时记下自己收到过的请求目标。
///
/// 为什么不改 `probe-server.mjs`：那个 harness **没有**重定向路由，也没有"既不报 Content-Length
/// 也不报 Content-Range"的路由，而给它加路由不在本轮授权里（派发只允许我对 `src/task.rs` 做
/// 可见性级别的改动）。这两条形状又都是本轮必须真跑一遍的：
/// - **落地 URL 带凭据**（派发第 9 条 / Task 5 评审 C3）：只有服务器真的 `Location` 一发，
///   `ProbeFacts.final_url` 才是"服务器控制的值"。在测试里手搓一个 `Url` 塞给被测函数证明不了
///   "落地 URL 来自响应"这一环，也就挡不住"忘了调那道闸"这种伪装修法。
/// - **没有长度可取**：`total_len` 不许猜。
///
/// 线程模型：监听线程最多 `accept` `MAX_CONNECTIONS` 条连接、每条最多处理 `PER_CONNECTION` 个请求；
/// 客户端不来时它阻塞在 `accept` 上 —— 那是**测试进程内的一个线程**，测试二进制退出即随进程消失，
/// 不构成"残留监听"（`Server` 那种真 node 进程才需要 `Drop` 里 kill）。
#[derive(Debug)]
pub struct Stub {
    port: u16,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Stub {
    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// 服务端实际收到过的请求目标（`GET "<target>" HTTP/1.1` 里的那个 target）。
    pub fn seen(&self) -> Vec<String> {
        self.seen.lock().expect("对端的记录锁没竞争者").clone()
    }

    /// 收到过某个前缀的请求吗（"确实跟随了重定向"这类断言用它，别拿 `seen()` 自己拼）。
    pub fn saw(&self, prefix: &str) -> bool {
        self.seen().iter().any(|t| t.starts_with(prefix))
    }
}

/// 起一个通用窄对端。`responder(target)` 返回**整段原始响应字节**（含状态行与结尾的空行）。
fn spawn_stub(responder: impl Fn(&str) -> String + Send + 'static) -> Result<Stub> {
    const MAX_CONNECTIONS: usize = 8;
    const PER_CONNECTION: usize = 4;
    let listener =
        TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).context("窄对端 bind 失败")?;
    let port = listener.local_addr().context("拿不到窄对端的端口")?.port();
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let thread_seen = Arc::clone(&seen);

    thread::Builder::new()
        .name("dlm-narrow-stub".into())
        .spawn(move || {
            for stream in listener.incoming().take(MAX_CONNECTIONS) {
                let Ok(mut stream) = stream else { break };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                for _ in 0..PER_CONNECTION {
                    let Some(target) = read_request_target(&mut stream) else { break };
                    let resp = responder(&target);
                    thread_seen.lock().expect("线程侧记录锁").push(target);
                    if stream.write_all(resp.as_bytes()).is_err() || stream.flush().is_err() {
                        break;
                    }
                }
            }
        })
        .context("起窄对端线程失败")?;

    Ok(Stub { port, seen })
}

/// 重定向对端：`GET /start` ⇒ `302` + `Location: http://<authority>/landed`，落地那条 ⇒ 诚实的 200。
///
/// `authority_template` 里的 `{port}` 会被换成本对端**实际拿到**的端口（端口要 bind 之后才知道，
/// 所以只能传模板）。例如 `alice:s3cret@127.0.0.1:{port}` ⇒
/// `Location: http://alice:s3cret@127.0.0.1:53127/landed`。
///
/// 落地那条 200 的形状：`Content-Length: 0`、`Accept-Ranges: bytes`、`ETag: "stub"`（带协议要求的引号）。
pub fn spawn_redirect_stub(authority_template: &str) -> Result<(Stub, String)> {
    // 端口在 `spawn_stub` 里才知道 ⇒ 先 bind 一次拿端口、把 Location 拼死在闭包里，
    // 再把同一个 listener 交给对端：所以这里自己 bind、自己起线程，不复用 `spawn_stub`。
    let listener =
        TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).context("重定向对端 bind 失败")?;
    let port = listener.local_addr().context("拿不到重定向对端的端口")?.port();
    let land_url = format!(
        "http://{}/landed",
        authority_template.replace("{port}", &port.to_string())
    );
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let (thread_seen, thread_land) = (Arc::clone(&seen), land_url.clone());
    const MAX_CONNECTIONS: usize = 8;

    thread::Builder::new()
        .name("dlm-redirect-stub".into())
        .spawn(move || {
            for stream in listener.incoming().take(MAX_CONNECTIONS) {
                let Ok(mut stream) = stream else { break };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                // 一条连接上最多两条：`/start`（回 302）与落地那条（回 200）。
                for served in 0..2usize {
                    let Some(target) = read_request_target(&mut stream) else { break };
                    thread_seen.lock().expect("线程侧记录锁").push(target.clone());
                    let resp = if target.starts_with("/start") {
                        format!("HTTP/1.1 302 Found\r\nLocation: {thread_land}\r\nContent-Length: 0\r\n\r\n")
                    } else {
                        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                         Content-Length: 0\r\nAccept-Ranges: bytes\r\nETag: \"stub\"\r\n\r\n"
                            .to_string()
                    };
                    if stream.write_all(resp.as_bytes()).is_err() || stream.flush().is_err() {
                        break;
                    }
                    if served == 1 {
                        break; // 重定向只有一次，答完落地那条就收连接
                    }
                }
            }
        })
        .context("起重定向对端线程失败")?;

    Ok((Stub { port, seen }, land_url))
}

/// "说不出长度"的对端：`/no-length` 回 `200` + `Transfer-Encoding: chunked`，
/// **既没有** `Content-Length` **也没有** `Content-Range`，也不发 `Accept-Ranges`。
pub fn spawn_lengthless_stub() -> Result<Stub> {
    spawn_stub(|target| {
        if target.starts_with("/no-length") {
            // chunked：4 字节一块 + 终止块。头里刻意没有 Content-Length。
            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
             Transfer-Encoding: chunked\r\n\r\n4\r\nabcd\r\n0\r\n\r\n"
                .to_string()
        } else {
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_string()
        }
    })
}

/// 永远回 **416** 的对端：`Content-Range: bytes */<N>`，`N` 写在请求目标里
/// （`/total/0` ⇒ `bytes */0`，`/total/4194304` ⇒ `bytes */4194304`）。
///
/// 为什么需要它（评审 **F3**）：`probe` 里那一支空文件分支的成立条件是**三条并列**
/// （收到 416 + `Content-Range` 的分母是 0 + 我们**确实**发了 Range），而本仓库的 node 服务器
/// 只在"文件真是 0 字节"时发 `bytes */0`（`/sz/0`），**拿不到**另外两支：
/// - `*/N`（N>0）那一支要有对端：越界 Range 在这台服务器上只会得到 `bytes */<sendLen>`，
///   而 `sendLen` 是路由算出来的，我们那条 `bytes=0-…` 打不出"N>0 的 416"（起点 0 时 `start >=
///   sendLen` 只在 `sendLen == 0` 成立）。这一支钉的是"416 不许一律当成空文件"。
/// - "我们没发 Range 却收到 416"那一支要有对端：`prelude_max == 0` 时 probe 确实不发 Range，
///   但 `/sz/0` 对无 Range 的请求回的是诚实的 `200`（自己算得出来），拿不到"不对着 Range 答 416"
///   的形状。这一支钉的是"三条并列不是两条"。
///
/// 头形状照 RFC 9110 §14.21 的最小回答：只有 `Content-Range` 与 `Content-Length: 0`，**不发**
/// `Accept-Ranges`（与本仓库 node 服务器的 416 分支同一形状 —— `a_zero_length_file_...` 那条
/// 用例把它的线上原文打了出来）。
pub fn spawn_416_stub() -> Result<Stub> {
    spawn_stub(|target| match target.strip_prefix("/total/") {
        Some(total) => format!(
            "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total}\r\n\
             Content-Length: 0\r\n\r\n"
        ),
        // 416 但**没有** `Content-Range`（Task 7 裁定 2 要的那一支：分母取不到 ⇒ 不能当成"这活儿干完了"）。
        None if target.starts_with("/no-content-range") => {
            "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\n\r\n".to_string()
        }
        // 416 且分母不是数字（`bytes */nine`）：与"缺失"同一支读法，但形状不同，分开钉着。
        None if target.starts_with("/garbage-total") => {
            "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */nine\r\n\
             Content-Length: 0\r\n\r\n"
                .to_string()
        }
        None => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_string(),
    })
}

/// 对我们的 `Range` 回 **200**、却照样发 `Accept-Ranges: bytes` 的对端（评审 **I1**）。
///
/// 为什么需要它（而不是只改那条重定向对端的期望）：本仓库的 node 服务器是**诚实**的退化对端
/// —— `/noranges` 无视 `Range` 时**同时**不发 `Accept-Ranges`，两条证据一起为假，于是它挡不住
/// "两条证据取其一"这个错写法（旧写法 `accepts_bytes || range_honored` 在那条路由上照样给 false）。
/// 真正要钉的是**只坏一条**的形状：头说认 Range，行为上把 `bytes=0-…` 当没有。线上这种对端不少
/// （反代/CDN 把 Range 吞掉但把源站的头透出来），而 spec §2.2 第二条把它判成**退化**：
/// "无 `Accept-Ranges` **或**首请求返回 `200` 而非预期的 `206`" ⇒ 两条都是触发退化的条件，
/// 不是两条可以互相顶替的证据。
///
/// 形状：`/claims-bytes-answers-200` ⇒ `200` + `Accept-Ranges: bytes` + `ETag` +
/// `Content-Length: <body_len>` + 正好 `<body_len>` 字节 body。**body 与 `Content-Length` 严格一致**
/// （它只在"认不认 Range"这一件事上骗人，别处诚实），所以 `read_bounded` 读得满、
/// 退化读法的 `total_len` 就是 `body_len`。
pub fn spawn_range_ignoring_stub(body_len: u64) -> Result<Stub> {
    spawn_stub(move |target| {
        if !target.starts_with("/claims-bytes-answers-200") {
            return "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_string();
        }
        let len = usize::try_from(body_len).unwrap_or(0);
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
             Content-Length: {body_len}\r\nAccept-Ranges: bytes\r\nETag: \"stub-ignores-range\"\r\n\
             \r\n{}",
            "r".repeat(len)
        )
    })
}

/// 回 **206** 但**说不出文件总长**的对端（评审 **I2**），外加一条诚实的对照组。
///
/// 为什么需要它：本仓库的 node 服务器在 206 上**永远**发得出可用分母
/// （`Content-Range: bytes <s>-<e>/<sendLen>`，`sendLen` 是它自己算出来的整数），所以
/// `bytes 0-…/*`（RFC 9110 §14.3 允许"总长未知"写成 `*`）、分母是垃圾、206 干脆不发
/// `Content-Range` 这三种形状它都产不出。而这三形状下"退到 `Content-Length`"读回来的都是
/// **本次窗口**的字节数 —— 窗口又正好等于预读量，于是那份谎**永远自洽**：`accept_ranges` 靠 206
/// 读成 true、预读长度对上假全长、spec §6 第 7 条那句"已写字节数 == total_len"照样通过，
/// 236 KiB 的截断文件被报成 48 MiB 下完了，之后每一次续传都同意这个谎。
///
/// `window` 是本次窗口的字节数（调用方传 `PROBE_READ_BYTES`）：三条拒绝形状都把它写成
/// `Content-Length`，body 也正好是那么多字节 —— 这正是"看起来完全成功"的那一份谎。
///
/// 路由：
/// - `/unknown-total` ⇒ `Content-Range: bytes 0-<window-1>/*`（总长未知）
/// - `/garbage-total` ⇒ `Content-Range: bytes 0-<window-1>/nine`（分母不是数字）
/// - `/no-content-range` ⇒ 206 **不带** `Content-Range`
/// - `/known-total`（**对照组**）⇒ `Content-Range: bytes 0-<window-1>/50331648` ⇒ 必须照常成功，
///   且读出的全长是 48 MiB 而不是窗口。没有这一支，"把所有 206 都拒掉"也算修好了。
pub fn spawn_unknown_total_stub(window: u64) -> Result<Stub> {
    const FULL_LEN: u64 = 48 * 1024 * 1024;
    let last = window.saturating_sub(1);
    let body = "w".repeat(usize::try_from(window).unwrap_or(0));
    spawn_stub(move |target| {
        // 状态行下面那一条 `Content-Range` 由路由决定；`/no-content-range` 刻意一条都不带。
        let head = match target {
            "/known-total" => Some(format!("Content-Range: bytes 0-{last}/{FULL_LEN}\r\n")),
            "/garbage-total" => Some(format!("Content-Range: bytes 0-{last}/nine\r\n")),
            "/no-content-range" => Some(String::new()),
            "/unknown-total" => Some(format!("Content-Range: bytes 0-{last}/*\r\n")),
            // 认不出的路由给 404 而不是随便挑一种 206 形状：拼错 URL 时用例要当场红，
            // 不能悄悄落到某条真路由上恒绿。
            _ => None,
        };
        let Some(head) = head else {
            return "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_string();
        };
        format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Type: application/octet-stream\r\n\
             Content-Length: {window}\r\n{head}Accept-Ranges: bytes\r\nETag: \"stub-window\"\r\n\
             \r\n{body}"
        )
    })
}

/// 从一条 HTTP/1.1 连接上取走一条请求的**请求目标**（并把请求头读干；本对端从不收 body）。
fn read_request_target(stream: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return None,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 64 * 1024 {
                    break;
                }
            }
            Err(_) => return None,
        }
    }
    // `GET /start HTTP/1.1`
    let first = String::from_utf8_lossy(&buf).lines().next()?.to_string();
    let mut it = first.split_whitespace();
    let (_method, target) = (it.next()?, it.next()?);
    Some(target.to_string())
}

// ---------------------------------------------------------------- Task 7 的落盘夹具

/// 建一个真的 `Store`（落在 gitignored 的 `target/harness-scratch/`），返回 `(Store, 路径)`。
///
/// 名字带 `pid`：`cargo test` 用多线程并行跑用例，同机并发跑多个测试进程时不能互相踩同一个文件。
/// `truncate = true`：本夹具服务的每一条用例都从"空文件"起步，`store.len()` 才只由这条用例的写入
/// 决定（Task 3 的自然延长语义）；残留文件会让长度断言读到上一轮的字节的。
/// **调用方负责删**（返回的是裸 `PathBuf`，不是 guard）：留在 `target/` 下不进版本库，
/// 但用例里会显式 `remove_file`，跑完整轮 `target/harness-scratch` 是干净的。
pub fn store(name: &str) -> Result<(dlm::store::Store, PathBuf)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("harness-scratch");
    std::fs::create_dir_all(&dir).with_context(|| format!("建 harness-scratch 目录失败：{}", dir.display()))?;
    let path = dir.join(format!("{}-{name}", std::process::id()));
    let store = dlm::store::Store::open(&path, true)
        .with_context(|| format!("开测试目标文件失败：{}", path.display()))?;
    Ok((store, path))
}

/// 按**绝对偏移**从文件读 `len` 字节 —— 独立于被测代码的读者（`Store` 只有写入口，
/// Task 3 的 deferred 条目明说"要读回就加 `read_at`，别放宽那句柄可见性"，所以取证走这条路）。
/// 文件比 `offset + len` 短 ⇒ 当场报错，不静默给短切片（短切片会让"内容对不对"变成"读了几个字节"）。
pub fn bytes_at(path: &Path, offset: u64, len: usize) -> Result<Vec<u8>> {
    let mut f = File::open(path).with_context(|| format!("打开取证文件失败：{}", path.display()))?;
    let have = f.metadata().context("取取证文件长度失败")?.len();
    let end = offset.checked_add(len as u64).context("取证区间溢出")?;
    if end > have {
        bail!("取证要读 [{offset}, {end})，文件只有 {have} 字节 —— 该写的字节没写够");
    }
    f.seek(SeekFrom::Start(offset)).with_context(|| format!("定位到 {offset} 失败"))?;
    let mut buf = vec![0u8; len];
    f.read_exact(&mut buf).context("读取证字节失败")?;
    Ok(buf)
}

/// blob 的**内容**取证（文件侧）：读回 `[start, start+len)` 并要求每 4 KiB 块首的块号对得上。
///
/// 为什么要求 `start % 4096 == 0`（简报第 1 条用例给的是 `1_000_000`，那个数**不是**对齐的）：
/// harness 的 blob 只在 4096 的整数倍处写了块号，块内其余字节是 `allocUnsafe` 的未初始化填充
/// ⇒ `assert_blob_block_numbers` 的"切片第 0 字节起就是块号"这个读法，只有切片起点落在块首才成立。
/// 拿一个不对齐的起点调它，断言会在**正确实现**上读出一堆填充字节（或在错误实现上碰巧不红）。
/// 这里显式 `ensure!` 而不是悄悄容忍：宁可当场红，也不要一条瞎掉的 oracle（本项目已踩过三次）。
pub fn require_block_alignment(path: &Path, start: u64, len: u64) -> Result<()> {
    const BLOCK: u64 = 4096;
    anyhow::ensure!(start.is_multiple_of(BLOCK), "块号取证要求 start 是 {BLOCK} 的整数倍（给了 {start}）");
    anyhow::ensure!(len > 0, "块号取证的长度不能是 0");
    let len = usize::try_from(len).context("取证长度超出 usize")?;
    let bytes = bytes_at(path, start, len)?;
    assert_blob_block_numbers(&bytes, start)
}

/// **位置可辨**的 body：与 harness 的 blob 同一约定（每 4096 块首写块号，小端 u32，其余留零）。
///
/// 存在的理由：`spawn_range_ignoring_stub` 的 body 是清一色 `'r'` —— 那种内容**分不清偏移**，
/// 拿它证"降级 200 时前 `iv.start` 个字节被丢掉了、剩下的落在该落的位置"会是一条**瞎测试**
/// （不丢的写法在那种 body 上写出的一模一样）。派发点名用那条对端，这里保留它的语义
/// （`Accept-Ranges: bytes` + 回 `200`），只把 body 换成能定位的那一份。
///
/// `pub` 是修复轮的可见性改动：F1 的那几条用例要交一份"位置可辨的**响应**体"，
/// 好把"落盘的字节确实是响应体"与"落盘的字节确实是请求体"这两件事分辨开（两者内容必须不同）。
pub fn positioned_body(len: usize) -> Vec<u8> {
    let mut body = vec![0u8; len];
    for i in (0..len).step_by(4096) {
        if i + 4 <= len {
            body[i..i + 4].copy_from_slice(&u32::try_from(i / 4096).expect("块号超出 u32").to_le_bytes());
        }
    }
    body
}

// ---------------------------------------------------------------- Task 7 的两种真形状对端

/// 收了请求**永不回答**的对端：连上、读走请求行、然后挂着不动。
///
/// 为什么需要它（裁定 3(c) 的落点）：本仓库的 node 服务器每一枪都必然收尾（`res.end()`），
/// `/sz/<n>s` 也只是**慢**、从不"停在中间不送"。而 `fetch` 要报 `Stalled` 的那一支是
/// "预算之内一个字节都没等到"——那个形状在本 harness 的 node 侧**不存在**（E5 记的就是这件事）。
/// 进程内线程对端：测试二进制一退它就没了，不构成残留监听。
pub fn spawn_silent_stub() -> Result<Stub> {
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .context("静默对端 bind 失败")?;
    let port = listener.local_addr().context("拿不到静默对端的端口")?.port();
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let thread_seen = Arc::clone(&seen);
    thread::Builder::new()
        .name("dlm-silent-stub".into())
        .spawn(move || {
            for stream in listener.incoming().take(4) {
                let Ok(mut stream) = stream else { break };
                if let Some(target) = read_request_target(&mut stream) {
                    thread_seen.lock().expect("静默对端的记录锁没竞争者").push(target);
                }
                // 永不回答：读着把连接吊住，直到客户端超时后自己断。
                let mut sink = [0u8; 1024];
                loop {
                    match stream.read(&mut sink) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
            }
        })
        .context("起静默对端线程失败")?;
    Ok(Stub { port, seen })
}

/// 把**响应头交出去、body 一个字节都不发**的对端（`200` + `Content-Length: <n>`，然后一直挂着）。
///
/// 与 `spawn_silent_stub` 的分工：那边测的是"连响应头都没有" ⇒ 超时发生在 `execute`，错误是
/// `reqwest::Error`（`is_timeout()` 直接可用）；这一边超时发生在 `Response::read`，而 reqwest
/// blocking 把读超时包成 `io::Error`（`src/blocking/response.rs` 的 `impl Read for Response`：
/// `Waited::TimedOut ⇒ error::decode(e).into_io()`，`into_io` = `io::Error::new(Other, self)`）
/// ⇒ 只按 `reqwest::Error::is_timeout()` 分类的代码在这一支会漏，必须把 payload 再 downcast 回去。
/// 这一支形状 node 服务器也造不出来（每一条都必然 `res.end()` 收尾）。
pub fn spawn_head_only_stub(content_length: u64) -> Result<Stub> {
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .context("只回头的对端 bind 失败")?;
    let port = listener.local_addr().context("拿不到只回头的对端的端口")?.port();
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let (thread_seen, thread_len) = (Arc::clone(&seen), content_length);
    thread::Builder::new()
        .name("dlm-head-only-stub".into())
        .spawn(move || {
            for stream in listener.incoming().take(4) {
                let Ok(mut stream) = stream else { break };
                let Some(target) = read_request_target(&mut stream) else { continue };
                thread_seen.lock().expect("只回头的对端的记录锁没竞争者").push(target);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                     Content-Length: {thread_len}\r\nAccept-Ranges: bytes\r\nETag: \"stub-head-only\"\r\n\r\n"
                );
                if stream.write_all(head.as_bytes()).is_err() {
                    continue;
                }
                let _ = stream.flush();
                // 头之后一个 body 字节都不发，把连接吊着（客户端的窗口预算就是在这里到点）。
                let mut sink = [0u8; 1024];
                loop {
                    match stream.read(&mut sink) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
            }
        })
        .context("起只回头的对端线程失败")?;
    Ok(Stub { port, seen })
}

/// 一个**按节拍**写 body 的降级对端（`200` + `Accept-Ranges: bytes` + 位置可辨的 body），
/// 并记下"客户端挂断前我写出了多少字节"。
#[derive(Debug)]
pub struct PacedPeer {
    port: u16,
    seen: Arc<Mutex<Vec<String>>>,
    written: Arc<AtomicU64>,
    /// 每条请求带来的 `Range` 原样值（没有那条头 ⇒ 空串）。修复轮加这一味是因为
    /// `spawn_paced_partial_stub` 要按它决定交哪一段，取证也就得能看到它。
    ranges: Arc<Mutex<Vec<String>>>,
}

impl PacedPeer {
    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
    pub fn seen(&self) -> Vec<String> {
        self.seen.lock().expect("节拍对端的记录锁没竞争者").clone()
    }
    pub fn saw(&self, prefix: &str) -> bool {
        self.seen().iter().any(|t| t.starts_with(prefix))
    }
    /// 收到过的 `Range`（按到达顺序；没有那条头的那一枪记成空串）。
    pub fn ranges(&self) -> Vec<String> {
        self.ranges.lock().expect("节拍对端的 Range 记录锁没竞争者").clone()
    }
    /// 已经交给 socket 的 body 字节数（含内核缓冲里还没被读走的那一段）。
    pub fn written(&self) -> u64 {
        self.written.load(Ordering::SeqCst)
    }
}

/// 起节拍降级对端。路由 `/ignores-range`（认不出的 target 直接断链，好让拼错 URL 的用例当场红）。
///
/// 它同时钉两件事：
/// 1. **内容落对了偏移**：body 是 `positioned_body`，所以"没丢前 `iv.start` 字节"的写法会把
///    块号 0 那份内容写到 `iv.start` 上去 ⇒ `require_block_alignment` 当场红。
/// 2. **丢字节是流式的、不是把整个 body 读进内存**：`written()` 给的是"对端在客户端挂断前
///    写到了哪儿"。正确实现读完 `iv.start + 本次要的字节` 就收手（远小于 `body_len`），
///    而"先把整个 body 吸干净再切片"的写法必然把 `body_len` 都吸完 ⇒ 断言红。
///    节拍（每块之间 `pacing`）就是为了让这个差别在字节数上看得出来，而不是被内核缓冲糊掉。
pub fn spawn_paced_range_ignoring_stub(body_len: u64, chunk: usize, pacing: Duration) -> Result<PacedPeer> {
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .context("节拍降级对端 bind 失败")?;
    let port = listener.local_addr().context("拿不到节拍降级对端的端口")?.port();
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let written: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
    let ranges: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let (thread_seen, thread_written, thread_ranges) =
        (Arc::clone(&seen), Arc::clone(&written), Arc::clone(&ranges));
    let body = positioned_body(usize::try_from(body_len).context("body 长度超出 usize")?);

    thread::Builder::new()
        .name("dlm-paced-degraded".into())
        .spawn(move || {
            for stream in listener.incoming().take(4) {
                let Ok(mut stream) = stream else { break };
                let Some(target) = read_request_target(&mut stream) else { continue };
                thread_seen.lock().expect("线程侧记录锁").push(target);
                thread_ranges.lock().expect("线程侧 Range 记录锁").push(String::new());
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                     Content-Length: {body_len}\r\nAccept-Ranges: bytes\r\n\
                     ETag: \"stub-paced-ignores-range\"\r\n\r\n"
                );
                if stream.write_all(head.as_bytes()).is_err() {
                    break;
                }
                let _ = stream.flush();
                for (n, piece) in body.chunks(chunk).enumerate() {
                    if n > 0 && !pacing.is_zero() {
                        thread::sleep(pacing);
                    }
                    if stream.write_all(piece).is_err() {
                        break; // 客户端已经收手：本对端跟着停，`written` 就停在它读到的地方附近
                    }
                    let _ = stream.flush();
                    thread_written.fetch_add(piece.len() as u64, Ordering::SeqCst);
                }
            }
        })
        .context("起节拍降级对端线程失败")?;

    Ok(PacedPeer { port, seen, written, ranges })
}

/// 按**节拍**交出**诚实 206** 的对端（`run_window` 读侧超时那条命名用例要它）。
///
/// 为什么 node 那台 200 ms 的限速对端不够用（E4）：窗口预算的下界是
/// `WINDOW_MIN_TIMEOUT_SECS = 15 s`，而 64 KiB / 200 ms 的节拍下**读满一个窗口**
/// （4 MiB = 64 块）只要 ~12.8 s ⇒ 两种 `.timeout()` 语义（"整请求期限" vs
/// "每次 `read` 重新起算的静默期限"）在那种节拍下给出**同一个结论**，断言是瞎的。
/// 把节拍调到 260 ms：一个窗口要 ~16.6 s > 15 s 预算 ⇒
///
/// - 整请求期限：15 s 到点，只交回 ~3.7 MiB；
/// - 静默期限：连续静默从没超过 260 ms ⇒ 根本不响，交回的是**正好一个窗口** 4 MiB。
///
/// 用例 `the_window_budget_is_a_whole_request_deadline_on_the_read_side_too` 断的就是这两个数之分
/// （本轮实测落在这两条的第一种上，于是它同时推翻了上一轮模块文档里的那句"静默期限"）。
///
/// 响应是**诚实**的：按收到的 `Range` 截段、带 `Content-Range: bytes s-e/<total_len>`、
/// `Content-Length` 与本次要写的字节数严格一致、`Accept-Ranges: bytes` ⇒ 走的是 `run_window`
/// 正常分段那一支（206 且起点可核对），不是降级 200 那一支。`total_len` 是本对端承认的文件长度，
/// 起点越过它按 Range 语义给 416。
pub fn spawn_paced_partial_stub(total_len: u64, chunk: usize, pacing: Duration) -> Result<PacedPeer> {
    let listener =
        TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).context("节拍 206 对端 bind 失败")?;
    let port = listener.local_addr().context("拿不到节拍 206 对端的端口")?.port();
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let written: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
    let ranges: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let (thread_seen, thread_written, thread_ranges) =
        (Arc::clone(&seen), Arc::clone(&written), Arc::clone(&ranges));
    let blob = positioned_body(usize::try_from(total_len).context("body 长度超出 usize")?);

    thread::Builder::new()
        .name("dlm-paced-partial".into())
        .spawn(move || {
            for stream in listener.incoming().take(4) {
                let Ok(mut stream) = stream else { break };
                let Some(request) = read_request_with_body(&mut stream) else { break };
                let range = request.header("range").unwrap_or("").to_string();
                let (start, asked_end) = parse_range(&range).unwrap_or((0, None));
                thread_seen.lock().expect("线程侧记录锁").push(request.target);
                thread_ranges.lock().expect("线程侧 Range 记录锁").push(range);
                let last = asked_end.unwrap_or(total_len - 1).min(total_len - 1);
                if start >= total_len || start > last {
                    let resp = format!(
                        "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total_len}\r\n\
                         Content-Length: 0\r\n\r\n"
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                    continue;
                }
                let from = usize::try_from(start).expect("start 落在 usize 内");
                let to = usize::try_from(last + 1).expect("end 落在 usize 内");
                let body = blob[from..to].to_vec();
                let head = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Type: application/octet-stream\r\n\
                     Content-Length: {}\r\nContent-Range: bytes {start}-{last}/{total_len}\r\n\
                     Accept-Ranges: bytes\r\nETag: \"stub-paced-partial\"\r\n\r\n",
                    body.len()
                );
                if stream.write_all(head.as_bytes()).is_err() {
                    break;
                }
                let _ = stream.flush();
                for (n, piece) in body.chunks(chunk).enumerate() {
                    if n > 0 && !pacing.is_zero() {
                        thread::sleep(pacing);
                    }
                    if stream.write_all(piece).is_err() {
                        break; // 客户端收手了：跟着停，`written` 就停在它读到的地方附近
                    }
                    let _ = stream.flush();
                    thread_written.fetch_add(piece.len() as u64, Ordering::SeqCst);
                }
            }
        })
        .context("起节拍 206 对端线程失败")?;

    Ok(PacedPeer { port, seen, written, ranges })
}

/// `bytes=1048576-1114111` / `bytes=1048576-` ⇒ `(start, end)`（开放终点 ⇒ `end` 为 `None`）。
/// 看不懂的形状 ⇒ `None`（调用方按"没发 Range"处理，与 node 服务器同一读法）。
fn parse_range(value: &str) -> Option<(u64, Option<u64>)> {
    let spec = value.trim().strip_prefix("bytes=")?;
    let (start, end) = spec.split_once('-')?;
    let start = start.trim().parse::<u64>().ok()?;
    let end = match end.trim() {
        "" => None,
        other => Some(other.parse::<u64>().ok()?),
    };
    Some((start, end))
}

#[test]
fn the_harness_range_parser_reads_both_ending_shapes() {
    // 夹具自己的解析件要在它自己这一层钉住：它错了，节拍 206 那条用例就会挑错 416/206 分支，
    // 而红出来的原因看着像被测代码的问题（取证工具不许在被测代码之后红）。
    assert_eq!(parse_range("bytes=0-16383"), Some((0, Some(16383))));
    assert_eq!(parse_range("bytes=1048576-"), Some((1048576, None)));
    assert_eq!(parse_range("bytes=-1024"), None, "后缀式 Range 本 harness 不发，也不许被读成起点 0");
    assert_eq!(parse_range(""), None);
    assert_eq!(parse_range("bytes=abc-"), None);
}

// ---------------------------------------------------------------- 修复轮：收得下**请求体**的对端

/// 一条被对端**完整收下**的请求：请求行的两段 + 全部请求头（名字已小写）+ body 原文。
///
/// 存在的理由（F1）：`fetch` 里"发出去的方法是什么、体是什么"这件事**不能由我们自己拼的字符串自证**。
/// 既有的两条取证通道都看不见 body：node 服务器只把头记进 JSONL（`probe-server.mjs` 不在本轮授权里，
/// 且它从不读请求体），`spawn_stub` 那族的 `read_request_target` 只读请求行与头。
/// 于是本夹具把 body 一起收下并交回来 —— 那才是"POST 确实带着那份体上线"的**对端侧**证据。
#[derive(Debug, Clone)]
pub struct ReceivedRequest {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl ReceivedRequest {
    /// 某个请求头的值（名字大小写不敏感，与本 harness 所有头读取同一立场）。
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers.iter().find(|(k, _)| *k == name).map(|(_, v)| v.as_str())
    }

    /// 某个请求头**是否存在**（"不许出现 Range"这一侧的断言用它，别拿 `header().is_none()` 绕）。
    pub fn has(&self, name: &str) -> bool {
        self.header(name).is_some()
    }

    /// body 按 lossy UTF-8 读出来，只为把哨兵/凭据那种 ASCII 片段打进断言消息里。
    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// 收下**请求体**、并对每一枪回一份调用方指定的 body 的对端。
#[derive(Debug)]
pub struct PostPeer {
    port: u16,
    requests: Arc<Mutex<Vec<ReceivedRequest>>>,
}

impl PostPeer {
    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// 服务过的全部请求（按到达顺序）。
    pub fn requests(&self) -> Vec<ReceivedRequest> {
        self.requests.lock().expect("POST 对端的记录锁没竞争者").clone()
    }

    /// 第一条请求；一条都没有就当场报错（"根本没打到对端"与"打到了但方法不对"是两种故障）。
    pub fn first(&self) -> Result<ReceivedRequest> {
        self.requests()
            .into_iter()
            .next()
            .context("POST 对端一条请求都没收到（请求根本没上线？）")
    }

    pub fn saw(&self, prefix: &str) -> bool {
        self.requests().iter().any(|r| r.target.starts_with(prefix))
    }
}

/// 起一个收体的对端。路由 `/submit`（认不出的 target 回 404，好让拼错 URL 的用例当场红）。
///
/// `accept_ranges`：响应里 `Accept-Ranges` 那一条头的值，`None` 表示**压根不发**这个头。
/// 之所以要它：`run_whole` 收割 `ProbeFacts.accept_ranges` 走的是与 `probe` **同一个读法**
/// （`asked_for_range = false` ⇒ 只看头），要钉住那一味就必须能让对端说 `bytes` / 说 `none` / 闭嘴。
///
/// `response_body` 是每一枪的响应体（调用方给什么就回什么，`Content-Length` 与它严格一致 ⇒
/// 读得满、body 干净收尾）。它与**请求体**刻意由调用方分成两份不同的字节，
/// 这样"落盘的是响应体"才不是拿同一份内容自证。
pub fn spawn_post_stub(accept_ranges: Option<&str>, response_body: Vec<u8>) -> Result<PostPeer> {
    let listener =
        TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).context("收体对端 bind 失败")?;
    let port = listener.local_addr().context("拿不到收体对端的端口")?.port();
    let requests: Arc<Mutex<Vec<ReceivedRequest>>> = Arc::new(Mutex::new(vec![]));
    let thread_requests = Arc::clone(&requests);
    let ranges = accept_ranges.map(str::to_string);

    thread::Builder::new()
        .name("dlm-post-stub".into())
        .spawn(move || {
            for stream in listener.incoming().take(4) {
                let Ok(mut stream) = stream else { break };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let Some(req) = read_request_with_body(&mut stream) else { break };
                let recognized = req.target.starts_with("/submit");
                thread_requests.lock().expect("线程侧记录锁").push(req);
                let written = if recognized {
                    let mut head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                         Content-Length: {}\r\nETag: \"stub-post-peer\"\r\n",
                        response_body.len()
                    );
                    if let Some(value) = &ranges {
                        head.push_str(&format!("Accept-Ranges: {value}\r\n"));
                    }
                    head.push_str("\r\n");
                    stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(&response_body))
                } else {
                    stream
                        .write_all("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".as_bytes())
                };
                if written.is_err() || stream.flush().is_err() {
                    break;
                }
            }
        })
        .context("起收体对端线程失败")?;

    Ok(PostPeer { port, requests })
}

/// 从一条连接上收下**一整条请求**（请求行 + 头 + body）。收不齐（或对端拆线）给 `None`。
///
/// body 只认 `Content-Length`：reqwest 的阻塞层对 `Vec<u8>` 与 `File` 两种体都会写出那一条头
/// （`blocking/request.rs` 的 `into_async`：`if let Some(len) = len { insert(CONTENT_LENGTH, ..) }`，
/// 而 `Body::from(File)` 的 `len` 取自 `metadata().len()`）。真出现 `Transfer-Encoding: chunked`
/// 的请求体时本夹具**故意**不收（返回 `None` 并拆线）：那样用例会在"对端一条请求都没收到"上当场红，
/// 比悄悄少读一截 body、把"体确实是那份字节"变成一条瞎断言好。
fn read_request_with_body(stream: &mut TcpStream) -> Option<ReceivedRequest> {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(req) = take_complete_request(&buf) {
            return Some(req);
        }
        if buf.len() > 512 * 1024 {
            return None; // 头 + body 超过这个数还拼不成一条请求 = 对端不对劲
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return None,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
}

/// 试着把缓冲区解释成**一条完整请求**；头没读全或 body 没读全 ⇒ `None`（继续读）。
fn take_complete_request(buf: &[u8]) -> Option<ReceivedRequest> {
    let head_end = find_head_end(buf)?;
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.lines();
    let mut request_line = lines.next()?.split_whitespace();
    let method = request_line.next()?.to_string();
    let target = request_line.next()?.to_string();
    let mut headers: Vec<(String, String)> = vec![];
    for line in lines {
        let (name, value) = line.split_once(':')?;
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }
    let value_of = |name: &str| {
        let name = name.to_ascii_lowercase();
        headers
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.as_str().to_string())
    };
    // chunked 请求体：本夹具不收（理由见 `read_request_with_body`）。
    if value_of("transfer-encoding").is_some() {
        return None;
    }
    let body_area = &buf[head_end + 4..];
    let want: usize = value_of("content-length")
        .unwrap_or_else(|| "0".into())
        .trim()
        .parse()
        .ok()?;
    if body_area.len() < want {
        return None; // body 还没收齐
    }
    Some(ReceivedRequest {
        method,
        target,
        headers,
        body: body_area[..want].to_vec(),
    })
}

// ---------------------------------------------------------------- 修复轮：`--data-file` 的体文件

/// 在 gitignored 的 `target/harness-scratch/` 里写一份**真的**请求体文件，返回路径。
///
/// 为什么要真文件：F1 那几条用例断的是"上线的 body 确实等于那份文件的字节"，
/// 以及"错误/日志里一个字都不抄那份体"。拿一个不存在的路径断后者是**恒真**的
/// （谁都抄不出没读过的东西）—— 与 `probe.rs` 里 `scratch_body` 同一条理由，只是落在集成侧。
/// 名字带 `pid`（并行跑与同机多进程不互踩），**调用方负责删**。
pub fn scratch_body(name: &str, content: &[u8]) -> Result<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("harness-scratch");
    std::fs::create_dir_all(&dir).with_context(|| format!("建 harness-scratch 目录失败：{}", dir.display()))?;
    let path = dir.join(format!("{}-{name}", std::process::id()));
    std::fs::write(&path, content).with_context(|| format!("写请求体文件失败：{}", path.display()))?;
    Ok(path)
}
