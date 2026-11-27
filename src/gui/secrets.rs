//! 队列任务的凭证（HTTP Cookie）：DPAPI 加密后落盘，**明文只在内存里活一帧**。
//!
//! ## 为什么要这个模块
//!
//! 浏览器发来的下载几乎都带 Cookie（`background-*.js` 每次都把该 URL 的 cookie 拼上）。
//! 2026-11-27 之前这类下载被 [`crate::host`] 分流到「直连」—— 因为队列的任务模型里
//! 压根没有凭证字段。直连是起一个独立的 `dlm.exe`，桌面端窗口看不见它，
//! 于是用户看到的是「扩展里有进度、软件里什么都没有」。
//!
//! 要让它进队列，就必须存凭证。而 [`crate::state`] 立过一条规矩：
//! **「口令不进这个文件，靠结构里没有这个字段保证」**。那条规矩不能破，
//! 否则 `.dlm.json` 就变成明文凭证库了。
//!
//! 这里的折中：**密文进文件，明文永不**。用 Windows DPAPI
//! （`CryptProtectData` / `CryptUnprotectData`）以**当前用户**为作用域加密 ——
//! 别的用户、别的机器都解不开。浏览器存 cookie 用的是同一类机制。
//!
//! ## 为什么是裸 FFI 而不是 crate
//!
//! 与 [`crate::gui::clipboard`] 同一个取舍：系统能力用 `#[link]` 直接调，
//! 不为两三个函数引一棵依赖树。代价是 Windows 之外的平台必须显式走到
//! 「不支持」那条分支，不能假装能存。
//!
//! ## 生命周期
//!
//! 建任务时写入 → 每次派发子进程时读出（喂 `--cookie-stdin`）→ 任务进入终态
//! （完成 / 最终失败 / 被删除）时删掉。**暂停与重试不删**：那两种情况下用户
//! 还要接着下，凭证必须还在。

use std::io;
use std::path::{Path, PathBuf};

/// 凭证目录名，落在**队列文件的同级目录**下。
pub const DIR_NAME: &str = "secrets";

/// 由队列文件路径推出凭证目录：`<data>/queue.json` → `<data>/secrets`。
pub fn dir_for(queue_file: &Path) -> PathBuf {
    queue_file
        .parent()
        .map(|parent| parent.join(DIR_NAME))
        .unwrap_or_else(|| PathBuf::from(DIR_NAME))
}

/// 某条任务的凭证文件路径。
///
/// `task_id` 由队列生成（形如 `1795763981725-1`），但这里仍然做一次白名单过滤：
/// 路径拼接是最容易变成目录穿越的地方，而 `task_id` 将来可能来自别处。
pub fn path(dir: &Path, task_id: &str) -> Option<PathBuf> {
    let safe = !task_id.is_empty()
        && task_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    safe.then(|| dir.join(format!("{task_id}.bin")))
}

/// 把 cookie 加密写盘。目录不存在就建。
pub fn put(dir: &Path, task_id: &str, cookie: &str) -> io::Result<()> {
    let file = path(dir, task_id)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad task id"))?;
    let blob = protect(cookie.as_bytes())?;
    std::fs::create_dir_all(dir)?;
    std::fs::write(file, blob)
}

/// 读出并解密。文件不存在、解不开（换了用户/换了机器/被改过）一律返回 `None` ——
/// 调用方要把它当成「凭证没了，请重新发送」，**不许**静默当成没有 cookie 继续下。
pub fn take(dir: &Path, task_id: &str) -> Option<String> {
    let file = path(dir, task_id)?;
    let blob = std::fs::read(file).ok()?;
    let plain = unprotect(&blob).ok()?;
    String::from_utf8(plain).ok()
}

/// 删掉某条任务的凭证。文件本来就不在也算成功。
pub fn remove(dir: &Path, task_id: &str) {
    if let Some(file) = path(dir, task_id) {
        let _ = std::fs::remove_file(file);
    }
}

// ---------------------------------------------------------------------------
// Windows DPAPI
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod platform {
    use super::*;

    /// `DATA_BLOB`。`pb_data` 由 DPAPI 分配，必须用 `LocalFree` 还。
    #[repr(C)]
    struct DataBlob {
        cb_data: u32,
        pb_data: *mut u8,
    }

    // SAFETY: 只在这个模块内部构造，`pb_data` 要么指向我们自己借出去的切片，
    // 要么指向 DPAPI 分配、随后 `LocalFree` 的内存。
    #[link(name = "crypt32")]
    unsafe extern "system" {
        fn CryptProtectData(
            data_in: *const DataBlob,
            data_descr: *const u16,
            entropy: *const DataBlob,
            reserved: *mut std::ffi::c_void,
            prompt: *mut std::ffi::c_void,
            flags: u32,
            data_out: *mut DataBlob,
        ) -> i32;
        fn CryptUnprotectData(
            data_in: *const DataBlob,
            data_descr: *mut *mut u16,
            entropy: *const DataBlob,
            reserved: *mut std::ffi::c_void,
            prompt: *mut std::ffi::c_void,
            flags: u32,
            data_out: *mut DataBlob,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LocalFree(mem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    }

    /// 不许弹 UI：这个进程没有前台窗口，弹窗会卡住后台线程直到超时。
    const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x0000_0001;

    fn last_error(what: &str) -> io::Error {
        io::Error::other(format!(
            "{what} 失败（GetLastError={}）",
            io::Error::last_os_error()
        ))
    }

    /// 把 DPAPI 交回来的 blob 拷出来并释放。
    ///
    /// # Safety
    /// `out.pb_data` 必须是 DPAPI 分配、尚未释放的指针。
    unsafe fn take_blob(out: DataBlob) -> Vec<u8> {
        let mut bytes = vec![0u8; out.cb_data as usize];
        // SAFETY: DPAPI 保证 (pb_data, cb_data) 是一段有效可读内存。
        unsafe {
            std::ptr::copy_nonoverlapping(out.pb_data, bytes.as_mut_ptr(), out.cb_data as usize);
            LocalFree(out.pb_data as *mut std::ffi::c_void);
        }
        bytes
    }

    pub fn protect(plain: &[u8]) -> io::Result<Vec<u8>> {
        if plain.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty secret"));
        }
        let input = DataBlob {
            cb_data: plain.len() as u32,
            // DPAPI 只读这段内存，不会改；转换掉 mut 是安全的。
            pb_data: plain.as_ptr() as *mut u8,
        };
        let mut out = DataBlob {
            cb_data: 0,
            pb_data: std::ptr::null_mut(),
        };
        // SAFETY: 入参都是本栈上的有效结构；entropy/reserved/prompt 按文档传空。
        let ok = unsafe {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 {
            return Err(last_error("CryptProtectData"));
        }
        // SAFETY: 成功时 out 由 DPAPI 分配。
        Ok(unsafe { take_blob(out) })
    }

    pub fn unprotect(blob: &[u8]) -> io::Result<Vec<u8>> {
        if blob.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty blob"));
        }
        let input = DataBlob {
            cb_data: blob.len() as u32,
            pb_data: blob.as_ptr() as *mut u8,
        };
        let mut out = DataBlob {
            cb_data: 0,
            pb_data: std::ptr::null_mut(),
        };
        // SAFETY: 同上；`data_descr` 传空指针，表示不需要描述串。
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 {
            return Err(last_error("CryptUnprotectData"));
        }
        // SAFETY: 成功时 out 由 DPAPI 分配。
        Ok(unsafe { take_blob(out) })
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;

    pub fn protect(_plain: &[u8]) -> io::Result<Vec<u8>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "凭证加密只有 Windows（DPAPI）实现",
        ))
    }

    pub fn unprotect(_blob: &[u8]) -> io::Result<Vec<u8>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "凭证加密只有 Windows（DPAPI）实现",
        ))
    }
}

pub use platform::{protect, unprotect};

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dlm-secrets-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_secret_round_trips_through_the_filesystem() {
        let dir = scratch("roundtrip");
        let cookie = "sessionid=abc123; csrftoken=zzz";
        put(&dir, "1700000000000-1", cookie).unwrap();
        assert_eq!(take(&dir, "1700000000000-1").as_deref(), Some(cookie));
        remove(&dir, "1700000000000-1");
        assert_eq!(take(&dir, "1700000000000-1"), None, "删掉之后就取不到了");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **这条是本模块的存在理由**：磁盘上不许出现明文。
    ///
    /// 只断言「能解密回来」是不够的 —— 明文直接写盘的实现同样能通过那种用例。
    #[test]
    fn the_file_on_disk_never_contains_the_plaintext() {
        let dir = scratch("plaintext");
        let cookie = "sessionid=SUPER-SECRET-VALUE-42";
        put(&dir, "t-1", cookie).unwrap();
        let blob = std::fs::read(path(&dir, "t-1").unwrap()).unwrap();
        let needle = b"SUPER-SECRET-VALUE-42";
        let leaked = blob.windows(needle.len()).any(|w| w == needle);
        assert!(!leaked, "密文里出现了 cookie 明文 —— 加密没生效");
        assert!(
            !blob.starts_with(b"sessionid="),
            "文件开头就是明文，说明根本没加密"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 换了用户 / 换了机器 / 文件被改过 ⇒ 必须明确「取不到」，
    /// 由调用方转成「凭证已过期，请重新发送」，而不是当成没有 cookie 继续下。
    #[test]
    fn a_damaged_secret_is_none_not_a_panic() {
        let dir = scratch("damaged");
        put(&dir, "t-1", "sessionid=x").unwrap();
        let file = path(&dir, "t-1").unwrap();
        std::fs::write(&file, b"not a dpapi blob at all").unwrap();
        assert_eq!(take(&dir, "t-1"), None, "解不开要返回 None，不许 panic");
        std::fs::write(&file, b"").unwrap();
        assert_eq!(take(&dir, "t-1"), None, "空文件同样是 None");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_task_id_cannot_escape_the_secrets_directory() {
        let dir = Path::new("C:/data/secrets-root");
        assert!(path(dir, "../../evil").is_none(), "路径分隔符与 . 必须被拒");
        assert!(path(dir, "..").is_none());
        assert!(path(dir, "").is_none());
        assert!(path(dir, "a/b").is_none());
        assert!(path(dir, "1700000000000-1").is_some(), "正常 id 要能过");
    }

    #[test]
    fn the_secrets_dir_sits_next_to_the_queue_file() {
        assert_eq!(
            dir_for(Path::new("C:/data/queue.json")),
            PathBuf::from("C:/data/secrets"),
            "凭证目录必须与队列同级，不能散到别处"
        );
    }
}
