//! 按偏移落盘：**全工程唯一**持有目标文件句柄的模块。
//!
//! 依据：spec §2 模块表（`store` 行「按偏移落盘；唯一持有文件句柄」）、spec §3「明确不继承 IDM 的
//! 两点」第 ① 条（不预分配全长，靠写入自然延长）、plan 第 572–619 行与其勘误 **E7**。
//!
//! ## 三条形状决定
//!
//! 1. **不预分配、不建 per-segment 副文件、不碰稀疏。** 文件长度只由"写到的最高偏移"决定：
//!    `write_at(1_000_000, 4 字节)` ⇒ 长度 1_000_004，中间 999_990 个字节没人写过。
//!    单测钉住的只有两件事：**长度随偏移自然延长**、**从未写过的区域读回来是零**。
//!    这两条都不是"本文件是稀疏文件"的证据、也不依赖它是：NTFS 对从未分配过的簇本来就返回零。
//!    本机唯一可用的稀疏判据是 `fsutil sparse queryflag`，Task 3 实测它对这种写法建出来的文件报
//!    `This file is NOT set as sparse`。所以 v1 的诚实说法是：**本模块既不创建稀疏文件，也不要求
//!    它是稀疏的**；簇分配不在本模块的观察范围，也不在它的契约里。
//! 2. **一个句柄 + 一把 `Mutex`，不是每段一条句柄。** `std::fs` 在 Windows 上没有定位写
//!    （POSIX `pwrite` 那种"一次调用带着偏移写完"），能用的只有"先 `SeekFrom::Start` 再 `write`"
//!    两步。两步之间让出锁，另一个线程的 seek 就会把本线程的落点搬走 ⇒ A 段的字节写进 B 段的偏移。
//!    所以整条 seek+write 必须在**同一个临界区**里，锁必须粗到包住 `write_all` —— 这是这把锁的
//!    全部职责。同一路径开两条句柄在 Windows 上是允许的（可共享读写），但那样每条句柄各有自己的
//!    落点问题、还要再裁定"谁的位置说了算"；plan 第 576 行选的就是单句柄 + 锁这个形状。
//! 3. **`len()` 报的是真文件长度**：每次调用都用 `File::metadata()` 现问那条已打开的句柄。
//!    本机实测两件事各有用例钉着 —— 它**不按路径重新解析**（`len_follows_the_handle_not_the_path`：
//!    文件改名之后长度照样报对），也**不是内存里缓存的高水位**
//!    （`len_counts_bytes_written_outside_this_module`：另一条句柄写进来的字节立刻算数）。
//!    **E7** 把这条变成 Task 8 的承重：「磁盘上真有多少字节」必须问 `store.len()`（配合 `Progress`
//!    交回的字节），不许拿 `scheduler::snapshot_intervals()` 的覆盖集代替 —— 那是"归属已定"的字节，
//!    可能含 416 封存、从没落盘的段。为什么不缓存：`write_all` 失败前可能已经落进一部分字节，
//!    缓存会**多报**，而句柄查询问的就是文件系统本身、不会。成本：一次系统调用，实测均值在
//!    个位数 µs 量级（`len_is_cheap_enough_to_call_per_progress_line` 钉住上限，进度行一秒调几次
//!    完全够）；它要短暂拿锁，但临界区里没有网络、也没有内存拷贝。

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use anyhow::Context;

/// 目标文件的唯一写者。跨线程共享用 `Arc<Store>`（`Store` 本身不可复制，也不该复制）；
/// `Send + Sync` 由下面那条编译期断言钉住 —— Task 7/8 的每个 worker 线程都只拿 `&Store`。
pub struct Store {
    /// 句柄藏在锁里，理由见模块文档第 2 条。`pub(crate)` 只为本模块的单测能直读回去核对；
    /// 模块外（Task 7 的 `run_window`、`tests/` 集成测试）只能经 `write_at` / `len` / `final_path`。
    pub(crate) file: Mutex<File>,
    path: PathBuf,
}

const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Store>();
};

impl Store {
    /// 打开（必要时创建）目标文件。
    ///
    /// `truncate` 就是 **fresh / resume 的开关**：
    /// - `true`（重来）：已存在的残留半截文件被清成 0 字节。上一次运行留下的字节不能继续算数，
    ///   否则重下后的内容与校验和、与"覆盖集"三方对不上；
    /// - `false`（续传）：已有字节原样留着，长度不变，之后的 `write_at` 只在写得更远时才延长。
    ///
    /// 文件不存在时两种取值都创建它 —— 续传碰上"断点文件说有进度、目标却已被用户删掉"要能空手
    /// 起步（长度 0），而不是报错。父目录**不**由本模块创建（那是 `cli`/`engine` 的活），
    /// 打不开就带上路径报错。
    pub fn open(path: &Path, truncate: bool) -> anyhow::Result<Store> {
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create(true);
        if truncate {
            opts.truncate(true);
        }
        let file = opts
            .open(path)
            .with_context(|| format!("打开目标文件失败：{}", path.display()))?;
        Ok(Store {
            file: Mutex::new(file),
            path: path.to_owned(),
        })
    }

    /// 把 `bytes` 整块写到绝对偏移 `offset`，返回写入的字节数。
    ///
    /// - 成功时返回值恒等于 `bytes.len()`（`write_all` 不会"部分成功还报 Ok"）。
    /// - 并发：seek 与 write 在同一个临界区内完成，多个 worker 同时调只会互相排队，
    ///   不会把彼此的落点搅乱（模块文档第 2 条）。
    /// - 长度：`offset + bytes.len()` 超过当前长度时文件自然延长，中间空洞读作零；
    ///   零长度写不延长文件（Task 7 body 读空时交来空块是安全的）。
    pub fn write_at(&self, offset: u64, bytes: &[u8]) -> anyhow::Result<usize> {
        let mut file = self.handle();
        file.seek(SeekFrom::Start(offset))
            .with_context(|| format!("定位到偏移 {offset} 失败：{}", self.path.display()))?;
        file.write_all(bytes).with_context(|| {
            format!(
                "在偏移 {offset} 写 {} 字节失败：{}",
                bytes.len(),
                self.path.display()
            )
        })?;
        Ok(bytes.len())
    }

    /// 把这条句柄已写出的字节真正同步到磁盘：`File::sync_all`（**T3-A**）。
    ///
    /// 原来的实现是转 `File::flush`，而 std 源码（本机 `rustc -V` = 1.95.0，按该 tag 读上游；
    /// 本机没装 `rust-src`）对 `impl Write for &File` 的 `flush` 写得很明白："Since a `File`
    /// structure doesn't contain any buffers, this function is currently a **no-op** on Unix and
    /// Windows"。一个名字承诺落盘、方法体什么都不做的 API 会骗到调用方，所以按裁定换掉。
    /// `sync_all` 在 Windows 上走 `c::FlushFileBuffers`（`library/std/src/sys/fs/windows.rs` 的
    /// `fsync`），顺带一提：那里 `datasync` 直接转 `fsync`，**Windows 上 `sync_data` 与 `sync_all`
    /// 是同一个调用**，没有"换个便宜点的"这个选项，也就不必纠结选哪个。
    ///
    /// ## 只在任务收尾调用一次，别进热路径
    ///
    /// 调用点是"下载完成、可以给校验和 / 改状态"的那一刻（Task 8）。**不要放进 `run_window` 的
    /// 每块循环**：`FlushFileBuffers` 要等这条句柄的全部在途写穿过系统缓存抵达设备。本机
    /// （`target/` 所在卷，dev profile，64 块 × 64 KiB = 4 MiB，6 轮实测）：
    /// - 每写一个 64 KiB 块就 flush：**2.3–7.3 ms/块**；
    /// - 同一批写不打断、只在收尾 flush 一次：写本身 **15–38 µs/块**，收尾那次 **3.4–17.7 ms**。
    ///
    /// 也就是说热路径上调它是 **每块 60–480 倍**的白送开销（两端相除），而中途那份"持久性"没人消费：
    /// `write_at` 交回的字节对同机后来的读者（Task 10 的校验读、`len()`）本来就已经可见。
    ///
    /// ## 保证到哪一步为止
    ///
    /// 在"进程被杀 / 机器还在"（v1 的中断模型，续传靠 `len()` + 自然延长接上）之上，再多保证一层
    /// "OS 内部缓存的内容与元数据都进了设备"（std 文档：sync_all "sync all OS-internal file content
    /// and metadata to disk"；元数据里就包括靠延长得到的长度）。它**不是断电级保证** —— 磁盘自己的
    /// 易失写缓存不归 std 管，需要卷级/设备级手段。所以本模块没有任何"断电后字节还在"的断言或测试，
    /// 也不许调用方这么写。
    ///
    /// 拿锁是必须的（`sync_all` 问的就是那条装在 `Mutex` 里的句柄，模块文档第 2 条）。方法名与
    /// 签名仍与 plan 第 576 行的 `Store::flush()` 一致，Task 8 的调用形状不变。
    pub fn flush(&self) -> anyhow::Result<()> {
        let file = self.handle();
        file.sync_all()
            .with_context(|| format!("同步落盘失败：{}", self.path.display()))?;
        Ok(())
    }

    /// 磁盘上的真实文件长度（字节）—— 语义与取法见模块文档第 3 条。
    ///
    /// 它是"落盘量"，不是"归属已定的字节数"（后者是 `snapshot_intervals()`，两者在 416 封存之后
    /// 会分道扬镳，**E7**）。重写已有字节、或往低偏移写短块，都不会让它变小。
    ///
    /// 不带 `is_empty`：clippy 的 `len_without_is_empty` 讲的是集合（长度为 0 是特殊状态、值得单独问），
    /// 而这里 `len()` 是一次可失败的系统调用、返回的是字节数，"目标文件还是空的"在下载中途是常态
    /// 而不是边界情形（刚 fresh 打开、或断点在偏移 0）。名字与签名由 plan 第 576 行钉死（Task 8 按它取
    /// 落盘量），所以压掉这条 lint 而不是加一个用不上的方法。
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> anyhow::Result<u64> {
        let file = self.handle();
        let len = file
            .metadata()
            .with_context(|| format!("取文件长度失败：{}", self.path.display()))?
            .len();
        Ok(len)
    }

    /// `open` 时传进来的路径：完成后打印它、按它删残留、校验和也按它读。
    /// 它是 `open` 那一刻的快照 —— 本模块不在运行中改名/移动目标文件（v1 没有这种流程），
    /// 所以真出现"路径与句柄不是同一个名字"只在单测里（`len_follows_the_handle_not_the_path`）。
    pub fn final_path(&self) -> &Path {
        &self.path
    }

    /// 取句柄。**锁中毒时继续用**，不跟着 panic：`File` 没有跨调用的不变量（每次 `write_at`
    /// 都自己 seek，`len`/`flush` 不看位置），某个 worker 在临界区里 panic 之后句柄仍然完好。
    /// 在这里 panic 只会把"哪个 worker 崩了"换成一条更难懂的 poisoning 消息、还崩到主线程上；
    /// 真正的病因由那个线程自己的 `Result`/panic 上报。
    fn handle(&self) -> MutexGuard<'_, File> {
        self.file
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn write_at_extends_and_zero_fills() -> anyhow::Result<()> {
        let p = std::env::temp_dir().join("dlm-store-probe.bin");
        let _ = std::fs::remove_file(&p);
        let s = Store::open(&p, true)?;
        s.write_at(0, &[9u8; 10])?;
        let n = s.write_at(1_000_000, &[7u8; 4])?;
        assert_eq!(n, 4);
        assert_eq!(s.len()?, 1_000_004, "长度应随写入偏移自然延长");
        let mut b = vec![0u8; 8];
        {
            use std::io::{Read, Seek, SeekFrom};
            let f = &mut *s.file.lock().unwrap();
            f.seek(SeekFrom::Start(500_000))?;
            f.read_exact(&mut b)?;
        }
        assert!(b.iter().all(|x| *x == 0), "未写区域读作零");
        s.flush()?;
        std::fs::remove_file(&p)?;
        Ok(())
    }
    #[test]
    fn open_existing_keeps_content() -> anyhow::Result<()> {
        let p = std::env::temp_dir().join("dlm-store-probe2.bin");
        {
            let s = Store::open(&p, true)?;
            s.write_at(0, b"abc")?;
        }
        let s = Store::open(&p, false)?;
        assert_eq!(s.len()?, 3);
        std::fs::remove_file(&p)?;
        Ok(())
    }

    /// 本模块自建用例的 scratch 落点：`<仓库>/target/t3-scratch/<name>`（`target/` 已在
    /// `.gitignore` 里），一条一个名字、用例结尾删除。brief 逐字给的那两条保留它自己的
    /// `std::env::temp_dir()` 写法（系统临时目录，同样不落进仓库），原样不动。
    fn scratch(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("t3-scratch");
        std::fs::create_dir_all(&dir).expect("建 scratch 目录");
        let p = dir.join(name);
        let _ = std::fs::remove_file(&p); // 上一轮崩在半路的残留，不带进这一轮
        p
    }

    /// `truncate` 的**另一个方向**：fresh 必须把上一次运行留下的半截文件清成空。
    /// （`open_existing_keeps_content` 只证了 resume 那一侧。）
    #[test]
    fn fresh_open_truncates_a_leftover_partial_target() -> anyhow::Result<()> {
        let p = scratch("t3_fresh_truncates.bin");
        std::fs::write(&p, vec![0xABu8; 100_000])?; // 模拟上次中断留下的部分文件
        {
            let s = Store::open(&p, false)?;
            assert_eq!(s.len()?, 100_000, "truncate=false 必须留住已有字节");
        }
        let s = Store::open(&p, true)?;
        assert_eq!(s.len()?, 0, "truncate=true 必须把残留清干净");
        s.write_at(0, b"xyz")?;
        drop(s);
        assert_eq!(
            std::fs::read(&p)?,
            b"xyz",
            "清干净之后新写的字节就是全部内容"
        );
        std::fs::remove_file(&p)?;
        Ok(())
    }

    /// 续传碰上目标文件不见了（用户删了、或断点文件是别处拷来的）：`truncate=false` 也得成功，
    /// 开出一个 0 字节的文件让 engine 从头写，而不是在 `open` 就报错。
    #[test]
    fn resume_open_of_a_missing_file_creates_it() -> anyhow::Result<()> {
        let p = scratch("t3_resume_missing.bin");
        assert!(!p.exists());
        let s = Store::open(&p, false)?;
        assert_eq!(s.len()?, 0);
        s.write_at(0, b"abc")?;
        assert_eq!(s.len()?, 3);
        std::fs::remove_file(&p)?;
        Ok(())
    }

    /// Task 7 的 `run_window` 会在 body 读完后交出零字节块（或干脆不调）——这里钉死"空写不延长"。
    #[test]
    fn zero_length_write_does_not_extend_the_file() -> anyhow::Result<()> {
        let p = scratch("t3_zero_write.bin");
        let s = Store::open(&p, true)?;
        assert_eq!(s.write_at(5_000, &[])?, 0, "空写的返回值是 0");
        assert_eq!(s.len()?, 0, "零字节写不该把文件顶出一个假洞");
        s.write_at(0, b"abc")?;
        s.write_at(1_000, &[])?;
        assert_eq!(s.len()?, 3, "已写长度不受空写影响");
        assert_eq!(std::fs::read(&p)?, b"abc");
        std::fs::remove_file(&p)?;
        Ok(())
    }

    /// `len()` 是文件长度，不是句柄当前位置 —— 这条区分对 resume 之后的第一次低偏移重写才显现，
    /// 也正是 Task 8 算"落盘字节"时唯一有意义的量。
    #[test]
    fn len_is_the_file_length_not_the_handle_position() -> anyhow::Result<()> {
        let p = scratch("t3_len_not_position.bin");
        {
            let s = Store::open(&p, true)?;
            s.write_at(0, b"abcdef")?;
            assert_eq!(s.len()?, 6);
        }
        let s = Store::open(&p, false)?; // 续传：已有 6 字节
        s.write_at(0, b"x")?; // 句柄位置现在在 1，文件仍是 6 字节
        assert_eq!(s.len()?, 6, "len() 必须是文件长度，不是句柄当前位置");
        s.write_at(6, b"gh")?;
        assert_eq!(s.len()?, 8, "写到更远处才延长");
        assert_eq!(
            std::fs::read(&p)?,
            b"xbcdefgh",
            "重写只覆盖那一段，其余字节不动"
        );
        std::fs::remove_file(&p)?;
        Ok(())
    }

    #[test]
    fn final_path_echoes_the_opened_path() -> anyhow::Result<()> {
        let p = scratch("t3_final_path.bin");
        let s = Store::open(&p, true)?;
        assert_eq!(s.final_path(), p.as_path(), "Task 8/9 按它报最终文件");
        std::fs::remove_file(&p)?;
        Ok(())
    }

    /// T3-A 之后 `flush()` 走 `File::sync_all`（原来的 `File::flush` 对文件是 std 文档写明的空操作）。
    /// 这条只断言**在这台机器上真能断言的两件事**：开着的 store 上调用成功（含一个字节都没写过的
    /// fresh store、含连调两次），以及 flush 之前写进去的字节 —— 包括高偏移写留下的空洞 ——
    /// flush 之后按路径用另一条句柄读回来全在。
    ///
    /// **故意不写**"断电后字节还在"那种用例：它需要掉电注入，单测里做不到，写一条假装证明它的
    /// 断言比不写更糟。也别说本条能区分 `sync_all` 与 no-op —— 同机读者本来就看得到 `WriteFile`
    /// 交出的字节。方法体真的换成了 `sync_all` 的证据是成本量级变了（方法文档里的实测数字，
    /// 原始 no-op 版实测中位 0 µs/次 → 现在 2.4–3.2 ms/次），不是这条断言。
    #[test]
    fn flush_succeeds_and_earlier_bytes_are_readable_afterwards() -> anyhow::Result<()> {
        let p = scratch("t3_flush_then_read.bin");
        let s = Store::open(&p, true)?;
        s.flush()?; // 空 store 也刷得动：fresh 之后、第一块到达之前就收尾是合法路径
        s.write_at(0, b"head")?;
        s.write_at(1_000, b"tail")?;
        s.flush()?;
        s.flush()?; // 可重复调用，Task 8 收尾 + drop 前再刷一次不该报错
        assert_eq!(s.len()?, 1_004, "flush 不改变长度");
        let got =
            std::fs::read(&p).with_context(|| format!("flush 之后重开读不回：{}", p.display()))?;
        assert_eq!(got.len(), 1_004);
        assert_eq!(&got[0..4], b"head", "flush 前写的低偏移块");
        assert_eq!(&got[1_000..1_004], b"tail", "flush 前写的高偏移块");
        assert!(
            got[4..1_000].iter().all(|&b| b == 0),
            "空洞读回来仍是零 —— sync_all 不该往没写过的地方塞东西"
        );
        drop(s);
        assert_eq!(std::fs::read(&p)?, got, "句柄关掉后内容一字不变");
        std::fs::remove_file(&p)?;
        Ok(())
    }

    /// **E7** 的语义底：`len()` 是"磁盘上真有多少字节"，不是本模块内部的计数。
    /// 证据：本模块之外的一条句柄把文件写大，下一次 `len()` 立刻如实报出。
    #[test]
    fn len_counts_bytes_written_outside_this_module() -> anyhow::Result<()> {
        let p = scratch("t3_len_external_writer.bin");
        let s = Store::open(&p, true)?;
        s.write_at(0, b"abc")?;
        assert_eq!(s.len()?, 3);
        {
            // 模拟"别的写者"：Task 10 的校验读、或用户对同一文件动过手。
            use std::io::{Seek, SeekFrom, Write};
            let mut other = std::fs::OpenOptions::new()
                .write(true)
                .open(&p)
                .with_context(|| format!("第二条句柄打不开：{}", p.display()))?;
            other.seek(SeekFrom::Start(50))?;
            other.write_all(b"def")?;
        }
        assert_eq!(
            s.len()?,
            53,
            "外部写入的字节也算落盘量 —— 缓存计数会在这里说谎"
        );
        std::fs::remove_file(&p)?;
        Ok(())
    }

    /// 再进一步：`len()` 问的是**这条已打开的句柄**，不是路径。改名之后（std 默认给句柄带了
    /// DELETE 共享权限，所以 Windows 允许改名一个正被打开的文件）长度照样报对 —— 若它是按
    /// `self.path` 再查一次，这一步会直接失败。
    #[test]
    fn len_follows_the_handle_not_the_path() -> anyhow::Result<()> {
        let p = scratch("t3_len_rename.bin");
        let renamed = scratch("t3_len_renamed.bin");
        let s = Store::open(&p, true)?;
        s.write_at(1_000, b"xyz")?;
        std::fs::rename(&p, &renamed).with_context(|| {
            format!(
                "改名失败，这条用例的前提不成立：{} -> {}",
                p.display(),
                renamed.display()
            )
        })?;
        assert_eq!(s.len()?, 1_003, "路径已经不是当初那个了，长度仍来自句柄");
        s.write_at(2_000, b"abc")?;
        assert_eq!(s.len()?, 2_003);
        let got = std::fs::read(&renamed)?;
        assert_eq!(&got[1_000..1_003], b"xyz");
        assert_eq!(&got[2_000..2_003], b"abc");
        assert!(
            got[1_003..2_000].iter().all(|&b| b == 0),
            "两段之间的空洞读作零"
        );
        std::fs::remove_file(&renamed)?;
        Ok(())
    }

    /// 打不开就带路径报错，且**不**自作主张建目录。
    #[test]
    fn open_fails_with_the_path_and_never_creates_parent_dirs() -> anyhow::Result<()> {
        let p = scratch("t3-missing-dir").join("x.bin");
        let err = Store::open(&p, true)
            .err()
            .expect("父目录不存在时必须返回 Err，不能 panic、也不能自愈");
        assert!(
            err.to_string().contains("x.bin"),
            "错误里要带上目标路径，用户才知道是哪个文件开不了：{err}"
        );
        assert!(
            !p.parent().unwrap().exists(),
            "本模块不建父目录：{} 不该被顺手创建出来",
            p.parent().unwrap().display()
        );
        Ok(())
    }

    /// Task 7 起，`write_at` 是**多个 worker 线程同时调**的：每条连接每 64 KiB 调一次。
    /// 这条钉"并发写不串位"—— 每个 worker 写自己那一片，写完逐字节回读校验。
    #[test]
    fn concurrent_write_at_lands_at_each_workers_own_offset() -> anyhow::Result<()> {
        const CHUNK: usize = 64 * 1024; // 与 Task 7 每次交给 store 的块大小一致
        const WORKERS: usize = 4;
        const CHUNKS_PER_WORKER: usize = 8;
        let p = scratch("t3_concurrent_offsets.bin");
        let store = Arc::new(Store::open(&p, true)?);
        let handles: Vec<_> = (0..WORKERS)
            .map(|w| {
                let store = Arc::clone(&store);
                std::thread::spawn(move || -> anyhow::Result<()> {
                    let payload = vec![(w + 1) as u8; CHUNK];
                    for i in 0..CHUNKS_PER_WORKER {
                        let offset = (w * CHUNKS_PER_WORKER + i) * CHUNK;
                        store.write_at(offset as u64, &payload)?;
                    }
                    Ok(())
                })
            })
            .collect();
        for h in handles {
            h.join().expect("worker 线程不该 panic")?;
        }
        store.flush()?;
        let total = WORKERS * CHUNKS_PER_WORKER * CHUNK;
        assert_eq!(store.len()?, total as u64, "长度 = 最高写到的偏移");
        let got = std::fs::read(&p)?;
        assert_eq!(got.len(), total);
        for w in 0..WORKERS {
            for i in 0..CHUNKS_PER_WORKER {
                let lo = (w * CHUNKS_PER_WORKER + i) * CHUNK;
                assert!(
                    got[lo..lo + CHUNK].iter().all(|&b| b == (w + 1) as u8),
                    "worker {w} 的第 {i} 块（偏移 {lo}）串位了或被别的 worker 改过"
                );
            }
        }
        std::fs::remove_file(&p)?;
        Ok(())
    }

    /// 单句柄 + 一把锁的**全部意义**在这条：`seek` 与 `write` 是同一个临界区，所以同一个块
    /// 要么整个是 A 线程写的、要么整个是 B 线程写的，不可能一半一半。
    /// 若有人把锁拆成"seek 一次、write 再一次"（或改成每线程一条句柄），这条就会红。
    #[test]
    fn concurrent_writes_to_the_same_blocks_never_tear() -> anyhow::Result<()> {
        const BLOCK: usize = 64;
        const BLOCKS: usize = 16;
        const WRITERS: usize = 4;
        const ROUNDS: usize = 400;
        let p = scratch("t3_concurrent_torn.bin");
        let store = Arc::new(Store::open(&p, true)?);
        store.write_at(0, &[0u8; BLOCKS * BLOCK])?; // 先铺满，之后长度不再变
        let handles: Vec<_> = (0..WRITERS)
            .map(|t| {
                let store = Arc::clone(&store);
                std::thread::spawn(move || -> anyhow::Result<()> {
                    let tag = (t + 1) as u8;
                    for _ in 0..ROUNDS {
                        for b in 0..BLOCKS {
                            store.write_at((b * BLOCK) as u64, &[tag; BLOCK])?;
                        }
                    }
                    Ok(())
                })
            })
            .collect();
        for h in handles {
            h.join().expect("worker 线程不该 panic")?;
        }
        assert_eq!(store.len()?, (BLOCKS * BLOCK) as u64);
        let got = std::fs::read(&p)?;
        for b in 0..BLOCKS {
            let blk = &got[b * BLOCK..(b + 1) * BLOCK];
            assert!(
                blk.windows(2).all(|w| w[0] == w[1]),
                "第 {b} 块被写花了（seek 与 write 之间被别的线程插队）：{blk:?}"
            );
        }
        std::fs::remove_file(&p)?;
        Ok(())
    }

    /// **E7** 让 `len()` 变成进度行的数据源（一秒可能调几次）。这里钉量级：本机实测均值
    /// **3 µs/次**（50_000 次一轮，空载连跑 20 轮全在 3–5，另在 8 路 CPU 负载下跑到过 6），
    /// 上限 **500 µs/次** —— 距实测最差还有 80 倍以上余量，挡得住忙时的抖动；但"退化成每次重开
    /// 句柄 / 走路径解析 / 顺带同步磁盘"那一类一定撞线（后一种实测中位就有 2.4–3.2 ms/次，
    /// 见 `flush` 的方法文档）。
    ///
    /// 上限从 5 000 µs 收到 500 µs 是为了修掉 `.superpowers/sdd/2026-10-15-dlm-kernel/task-3-report.md`
    /// §4 与本文件的不一致：报告写的"< 500 µs"才是当初的意图，代码里却留了 `5_000`。按实测数字
    /// 裁定**收常数**（更严的一侧），文档与代码从此是同一个数。
    #[test]
    fn len_is_cheap_enough_to_call_per_progress_line() -> anyhow::Result<()> {
        const CALLS: u64 = 50_000;
        const MAX_AVG_MICROS: u64 = 500;
        let p = scratch("t3_len_cost.bin");
        let s = Store::open(&p, true)?;
        s.write_at(1_000_000, b"abc")?;
        let started = std::time::Instant::now();
        for _ in 0..CALLS {
            assert_eq!(s.len()?, 1_000_003);
        }
        let elapsed = started.elapsed();
        let avg_micros = elapsed.as_micros() as u64 / CALLS;
        eprintln!(
            "len() {CALLS} 次共 {elapsed:?}，均值 {avg_micros} µs/次，上限 {MAX_AVG_MICROS} µs/次（{} 字节文件）",
            s.len()?
        );
        assert!(
            avg_micros < MAX_AVG_MICROS,
            "len() 平均 {avg_micros} µs/次，超过 {MAX_AVG_MICROS} µs 上限 —— 慢到不该拿去做进度行"
        );
        std::fs::remove_file(&p)?;
        Ok(())
    }
}
