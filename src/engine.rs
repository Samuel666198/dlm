//! 主循环：把七个模块接成真线程、真定时器、真文件的下载引擎。
//!
//! 依据：spec §2（模块表与 §2.1/§2.2）、§3（常量）、§4（断点文件与"写者唯一"）、§6（重试与完成判定）、
//! §7（测试策略），以及计划勘误 **E7**（两本账）、**E8**（`flush` 只在收尾调一次、不许在 writer 活着时调）、
//! **E11①**/**E12⑧**/**R7**（`--data-file` 那一枪只发一次、该形状既不读也不写断点）、
//! **E13**（本模块要改的三处断接口与配套裁定）。

use std::time::Duration;

use crate::consts::{FAST_RETRY_MAX, SLOW_RETRY_INTERVAL_SECS};

/// 一次重试要等多久（spec §6.1 的快速退避 + §6.2 的慢速无限重试 + `--max-retries` 封顶）。
///
/// 纯函数、零睡眠 —— 表可以在单元层钉死，真睡眠只在集成层跑一条（**E13④**："退避不许造假时钟"，
/// 也不许为此引入 `Sleep` trait 或可注入的调度器：v1 保持真实时间，这是裁定不是疏漏）。
///
/// `attempt` 是**这一条链上已经重试过几次**（0 ⇒ 马上要等的是第一次重试）。
/// 三条规则按 spec 的原文排：
/// 1. §6.1 快速退避 `1, 2, 4, 8, 16` 秒（`FAST_RETRY_MAX = 5` 次）。
/// 2. §6.2 超过 5 次**不判整任务失败**，转入 `SLOW_RETRY_INTERVAL_SECS` 的无限慢重试
///    （对齐 IDM 队列行为 A15）。
/// 3. `--max-retries` 封顶的是**整条链**（快速 + 慢速一起数），所以它排在最前面：
///    `Some(n)` 且已经重试满 `n` 次 ⇒ `None` = "别再重试了"，调用方据此让整任务失败。
///    `None`（没给 `--max-retries`）⇒ 无限慢重试。
pub fn next_retry_delay(attempt: u32, max_retries: Option<u32>) -> Option<Duration> {
    if max_retries.is_some_and(|cap| attempt >= cap) {
        return None;
    }
    Some(Duration::from_secs(if attempt < FAST_RETRY_MAX {
        // `attempt < FAST_RETRY_MAX = 5` ⇒ 位移最多 4，不会溢出 u64 的秒数。
        1u64 << attempt
    } else {
        SLOW_RETRY_INTERVAL_SECS
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::WINDOW_MAX_TIMEOUT_SECS;

    /// **E13④ 要求的表驱动用例**：退避表逐格钉死，零睡眠。
    ///
    /// 三列各自的来源：`1/2/4/8/16` 是 spec §6.1 的原文；第 6 次起 `30 s` 是 §6.2（"超过 5 次不判
    /// 整任务失败，转入 30 s 无限慢重试"）；`--max-retries` 封顶整条链是 §6.2 末句。
    /// `attempt` 一律是"这条链上已经重试过几次"，所以第一行 `(0, None, 1s)` 说的是"第一次重试等 1 秒"。
    #[test]
    fn retry_clock_follows_the_spec_table() {
        let table: &[(u32, Option<u32>, Option<u64>)] = &[
            // —— 没给 --max-retries：快速五次，之后无限慢重试 ——
            (0, None, Some(1)),
            (1, None, Some(2)),
            (2, None, Some(4)),
            (3, None, Some(8)),
            (4, None, Some(16)),
            (5, None, Some(30)),   // §6.2：第 6 次起转 30 s，而不是判整任务失败
            (6, None, Some(30)),
            (9, None, Some(30)),
            (u32::MAX, None, Some(30)),   // 无限慢重试不许因为计数溢出而崩或停
            // —— 给了 --max-retries：封顶的是整条链（快速 + 慢速一起数）——
            (0, Some(0), None),    // 封顶 0 ⇒ 一次都不重试
            (0, Some(1), Some(1)),
            (1, Some(1), None),    // 已经重试满 1 次 ⇒ 第 2 次不给
            (4, Some(5), Some(16)),
            (5, Some(5), None),    // 快速用满 ⇒ 慢速一档也不给留
            (5, Some(6), Some(30)),
            (6, Some(7), Some(30)),
            (7, Some(7), None),    // 慢速重试同样计入上限
            (8, Some(7), None),
        ];
        for (attempt, cap, want) in table {
            let got = next_retry_delay(*attempt, *cap);
            let want_dur = want.map(Duration::from_secs);
            assert_eq!(got, want_dur, "attempt={attempt} max_retries={cap:?}");
        }
    }

    /// 上限的**边界方向**单独钉：`Some(n)` 放行的必须正好是 `n` 档，`None` 必须一档都不封。
    /// 上面那张表读得出趋势，但"差一档"这种缺陷要一格具体的格子才拦得住
    /// （`attempt >= cap` 写成 `attempt > cap` 时，本用例的第一条立刻红，而表格其余 17 格全绿）。
    #[test]
    fn the_cap_admits_exactly_that_many_retries_and_no_more() {
        for cap in 0u32..12 {
            let allowed: Vec<Option<Duration>> =
                (0..cap + 3).map(|a| next_retry_delay(a, Some(cap))).collect();
            let given = allowed.iter().filter(|d| d.is_some()).count();
            assert_eq!(given, cap as usize, "封顶 {cap} 次却放行了 {given} 档: {allowed:?}");
            assert!(allowed[cap as usize].is_none(), "第 {} 档该收手（封顶 {cap}）", cap + 1);
        }
        // 没封顶：再往后也永远有下一档（§6.2 的"无限"）。
        for attempt in [5u32, 500, 100_000, u32::MAX] {
            assert_eq!(next_retry_delay(attempt, None), Some(Duration::from_secs(30)), "attempt={attempt}");
        }
    }

    /// 退避的**量级**必须落在可用范围内，且每一档都不许短到"等于没退避"：
    /// 1 秒是最短的一档，而窗口预算的下界是 15 秒 —— 重试比一次请求还短的话，
    /// 主循环里"先等到 deadline 再收事件"的那一步就白等了（这条是给引擎的等待逻辑定标的）。
    #[test]
    fn every_permitted_delay_is_a_real_pause_below_the_longest_window_budget() {
        for cap in [None, Some(1), Some(3), Some(FAST_RETRY_MAX), Some(64)] {
            for attempt in 0u32..64 {
                if let Some(d) = next_retry_delay(attempt, cap) {
                    assert!(d.as_secs() >= 1, "退避不许是零秒（那等于立刻重打同一枪）: {d:?}");
                    assert!(
                        d.as_secs() <= WINDOW_MAX_TIMEOUT_SECS,
                        "退避不该长过最宽的一次窗口预算，否则主循环的等待没有上界: {d:?}"
                    );
                }
            }
        }
    }
}
