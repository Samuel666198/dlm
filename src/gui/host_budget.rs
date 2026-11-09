//! 按 host 的连接预算（勘误 E4）。
//!
//! ## 为什么在队列层而不是调度器
//!
//! 每个任务 spawn 自己的 `dlm` 进程，各有各的 `scheduler`。改 `scheduler.rs`
//! 只能限「一个任务内」的连接数；而默认 `connections=8` 且 `per_host=8` 时
//! 那是恒等操作 —— 同一 host 上跑两个任务照样 2×8 条连接打过去。
//!
//! 真正会被服务器拉黑的是**跨任务的总量**，所以预算必须放在能看见所有任务的
//! 地方：runner 的派发点。
//!
//! ## 预算怎么算
//!
//! 每个在跑的任务按「它这一单会开几条连接」计数（`connections`，来自设置）。
//! 新任务要启动时，看它目标 host 上已经用掉多少；加上它自己要用的，
//! 超过 `per_host_connections` 就**这一轮不派**（留在队列，下一轮再试）。
//!
//! `per_host_connections == 0` = 不限。

use std::collections::HashMap;

/// 从 URL 里取 host（含端口）。取不到就返回空串 —— 空串单独算一组，
/// 不会跟真实 host 混在一起。
pub fn host_of(url: &str) -> String {
    let rest = match url.split_once("://") {
        Some((_, rest)) => rest,
        None => return String::new(),
    };
    // 去掉 userinfo：`user:pass@host` 里的 host 从 @ 之后开始。
    let rest = match rest.rsplit_once('@') {
        Some((_, after)) => after,
        None => rest,
    };
    rest.split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// 每个 host 已经用掉的连接数。
pub type HostUsage = HashMap<String, usize>;

/// 这个任务能不能现在启动。
///
/// - `per_host == 0`：不限，永远可以
/// - `want == 0`：任务不要连接（不该发生），当作可以
/// - 否则：`used + want <= per_host`
pub fn can_start(used: usize, want: usize, per_host: usize) -> bool {
    if per_host == 0 {
        return true;
    }
    if want == 0 {
        return true;
    }
    used.saturating_add(want) <= per_host
}

/// 把一个任务计入用量。启动时调一次，任务结束时调 [`release`]。
pub fn acquire(usage: &mut HostUsage, host: &str, want: usize) {
    *usage.entry(host.to_string()).or_insert(0) += want;
}

/// 任务结束，把它的用量还回去。用量到 0 就把这一格删掉，避免表无限长。
pub fn release(usage: &mut HostUsage, host: &str, want: usize) {
    if let Some(slot) = usage.get_mut(host) {
        *slot = slot.saturating_sub(want);
        if *slot == 0 {
            usage.remove(host);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_is_extracted_from_every_url_shape() {
        assert_eq!(host_of("https://example.com/a/b.zip"), "example.com");
        assert_eq!(host_of("http://example.com"), "example.com");
        assert_eq!(host_of("https://example.com:8443/a"), "example.com:8443");
        assert_eq!(host_of("https://EXAMPLE.COM/A"), "example.com", "统一小写");
        assert_eq!(
            host_of("https://user:pass@example.com/a"),
            "example.com",
            "userinfo 不算 host"
        );
        assert_eq!(host_of("https://example.com/a?x=1#f"), "example.com");
        assert_eq!(host_of("not a url"), "");
    }

    #[test]
    fn zero_per_host_means_unlimited() {
        assert!(can_start(999, 8, 0));
    }

    #[test]
    fn a_second_task_is_blocked_when_the_host_budget_is_full() {
        // 8 条预算，第一个任务占了 8，第二个同 host 的任务必须等。
        assert!(can_start(0, 8, 8), "空的时候可以起");
        assert!(
            !can_start(8, 8, 8),
            "已经占满 8 条，同 host 的第二个任务不能再起"
        );
        assert!(can_start(8, 8, 16), "预算放宽到 16 就可以");
    }

    #[test]
    fn partial_usage_still_fits_when_the_budget_allows() {
        assert!(can_start(4, 4, 8), "4+4=8 正好装下");
        assert!(!can_start(4, 5, 8), "4+5=9 超了");
    }

    #[test]
    fn acquire_and_release_track_usage_per_host() {
        let mut usage = HostUsage::new();
        acquire(&mut usage, "a.com", 8);
        acquire(&mut usage, "a.com", 4);
        acquire(&mut usage, "b.com", 2);
        assert_eq!(usage["a.com"], 12);
        assert_eq!(usage["b.com"], 2);

        release(&mut usage, "a.com", 8);
        assert_eq!(usage["a.com"], 4);
        release(&mut usage, "a.com", 4);
        assert!(
            !usage.contains_key("a.com"),
            "归零的 host 要从表里删掉，避免长跑后表无限长"
        );
        assert_eq!(usage["b.com"], 2, "别的 host 不受影响");
    }

    #[test]
    fn release_of_an_unknown_host_does_not_panic() {
        let mut usage = HostUsage::new();
        release(&mut usage, "never-seen.com", 8);
        assert!(usage.is_empty());
    }

    /// 这条是 E4 的核心：两个任务打同一 host，各自 8 条，预算 8 ⇒ 只能起一个。
    #[test]
    fn two_tasks_on_one_host_cannot_both_use_the_full_budget() {
        let mut usage = HostUsage::new();
        let per_host = 8;
        let want = 8;

        assert!(can_start(
            usage.get("x.com").copied().unwrap_or(0),
            want,
            per_host
        ));
        acquire(&mut usage, "x.com", want);

        assert!(
            !can_start(usage.get("x.com").copied().unwrap_or(0), want, per_host),
            "第二个同 host 任务必须被挡住 —— 这正是改 scheduler.rs 做不到的"
        );
    }
}
