//! 单任务共享速率上限（R18）：一只漏桶，所有网络读取合计按它排队。
//!
//! 形状：`reserve(now, bytes)` 把这一块排到共享的 `next` 时刻，返回调用方还要等多久；`consume`
//! 是生产入口（用 `Instant::now()` 预约后睡眠）。第一块立刻可用；之后每块按 `bytes / rate` 间隔。
//! 空闲不累计突发：`next` 落后于 `now` 时从 `now` 重新起算。
//!
//! 为什么把 `now` 交给调用方：单测要能拿注入时刻把"下一个时隙"算成确定值，而不是拿墙钟碰运气。
//! 生产侧只有 `consume` 一处读 `Instant::now()`。

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 单任务的共享限速器。`bytes_per_second == 0` ⇒ 不限（`reserve` 恒返回零）。
pub struct RateLimiter {
    bytes_per_second: u64,
    /// 下一次可以立刻读取的时刻（共享、单调推进；只由 `reserve` 改写）。
    next: Mutex<Instant>,
}

const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<RateLimiter>();
};

/// Windows 的 `thread::sleep` 按约 15.6 ms 的定时器粒度取整：小块（例如 4 KiB）的 3.9 ms 等待
/// 会被取整成 15.6 ms，实测把 1 MiB/s 拖成约 190 KiB/s。低于这个阈值的等待不睡，债务留在 `next` 里，
/// 下一次的等待会更大 ⇒ 长期平均仍由 `next` 守住，只放过不超过一个定时器粒度的突发。
const MIN_SLEEP: Duration = Duration::from_millis(16);

impl RateLimiter {
    pub fn new(bytes_per_second: u64) -> Self {
        Self {
            bytes_per_second,
            next: Mutex::new(Instant::now()),
        }
    }

    /// 预约 `bytes` 个字节的读取时隙；返回调用方还需要等多久。
    ///
    /// 第一块在 `next <= now` 时立刻可用；之后每块把 `next` 往后推 `bytes / rate`。
    /// `bytes == 0` 或 `rate == 0` 不改变任何状态。
    pub fn reserve(&self, now: Instant, bytes: u64) -> Duration {
        if self.bytes_per_second == 0 || bytes == 0 {
            return Duration::ZERO;
        }
        let nanos = ((bytes as u128) * 1_000_000_000u128 / self.bytes_per_second as u128)
            .min(u64::MAX as u128) as u64;
        let spacing = Duration::from_nanos(nanos);
        let mut next = self.next.lock().expect("rate limiter 的锁没有别的持有者");
        let ready = (*next).max(now);
        *next = ready
            .checked_add(spacing)
            .unwrap_or(ready + Duration::from_secs(3600));
        ready.saturating_duration_since(now)
    }

    /// 生产入口：按当前时刻预约，再睡眠补足。小于一个定时器粒度的等待不睡（债务留在 `next`）。
    pub fn consume(&self, bytes: u64) {
        let wait = self.reserve(Instant::now(), bytes);
        if wait >= MIN_SLEEP {
            std::thread::sleep(wait);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_never_waits() {
        let l = RateLimiter::new(0);
        let now = Instant::now();
        assert_eq!(l.reserve(now, 1_000_000), Duration::ZERO);
        assert_eq!(l.reserve(now, 1), Duration::ZERO);
    }

    #[test]
    fn first_chunk_is_free_and_the_next_ones_are_spaced() {
        let l = RateLimiter::new(1_048_576); // 1 MiB/s
        let now = Instant::now();
        assert_eq!(l.reserve(now, 65_536), Duration::ZERO);
        assert_eq!(l.reserve(now, 65_536), Duration::from_nanos(62_500_000));
        assert_eq!(
            l.reserve(now + Duration::from_nanos(125_000_000), 65_536),
            Duration::ZERO
        );
    }

    #[test]
    fn idle_time_is_not_banked() {
        let l = RateLimiter::new(1_048_576);
        let now = Instant::now();
        assert_eq!(l.reserve(now, 65_536), Duration::ZERO);
        assert_eq!(
            l.reserve(now + Duration::from_secs(600), 65_536),
            Duration::ZERO
        );
    }

    #[test]
    fn zero_bytes_never_advances_the_clock() {
        let l = RateLimiter::new(1_000);
        let now = Instant::now();
        assert_eq!(l.reserve(now, 0), Duration::ZERO);
        assert_eq!(
            l.reserve(now, 1),
            Duration::ZERO,
            "零字节预约不许把时隙往后推"
        );
    }

    #[test]
    fn one_byte_per_second_is_exact() {
        let l = RateLimiter::new(1);
        let now = Instant::now();
        assert_eq!(l.reserve(now, 1), Duration::ZERO);
        assert_eq!(l.reserve(now, 1), Duration::from_secs(1));
    }

    #[test]
    fn concurrent_reservations_are_a_permutation_of_the_expected_slots() {
        let l = std::sync::Arc::new(RateLimiter::new(1_048_576));
        let now = Instant::now();
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let l = std::sync::Arc::clone(&l);
                std::thread::spawn(move || l.reserve(now, 65_536))
            })
            .collect();
        let mut waits: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        waits.sort();
        assert_eq!(
            waits,
            vec![
                Duration::ZERO,
                Duration::from_nanos(62_500_000),
                Duration::from_nanos(125_000_000),
                Duration::from_nanos(187_500_000),
            ]
        );
    }
}
