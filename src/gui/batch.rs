//! 夜间批次（0.6.0 · 特色功能 1）：把「限速窗口 + 队列编排 + 跑完关机」串成**一个开关**。
//!
//! ## 为什么合成一个开关
//!
//! 这三件事单独看都不值得设：限速计划要写一行 `22:00-07:00=512`，队列编排要人守着看，
//! 「跑完关机」得记着自己开。但它们**总是一起用**：睡前把活排上、夜里慢慢下、早上起来
//! 机器已经关了。所以这里给一个开关，把三段串成一条。
//!
//! ## 语义（都写在这里，界面不再解释第二遍）
//!
//! - **窗口内**才派发新任务：窗口外排队等着。已经在跑的**不会**被掐断 ——
//!   半夜到点了把用户下了 90% 的任务停掉，那不是"编排"，是事故。
//! - **窗口内**的限速用批次自己那一档，**不再叠加限速计划**：两个都生效会得出一个
//!   用户解释不清的数（"我明明设了 2 MiB/s，怎么才 512"）。
//! - **跑完关机**：这次批处理至少派发过一个任务、且此刻队列里没有在跑/排队的 ⇒ 关机。
//!   从没干过活就不关 —— 否则一打开开关、队列恰好空着，机器就关了。
//!
//! ## 跨午夜
//!
//! `start > end` 表示跨午夜（22 → 7）。`start == end` 按**全天**处理，不是 0 长窗口：
//! 用户把两个数设成一样时想的是"一直"，不是"从不"。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NightBatch {
    pub on: bool,
    /// 窗口开始小时（0-23）。
    pub start_hour: u8,
    /// 窗口结束小时（0-23）。跨午夜用 `start > end` 表达。
    pub end_hour: u8,
    /// 窗口内的限速（KiB/s）。0 = 不限。
    pub speed_kib: u64,
    /// 跑完关机（队列空了才关）。
    pub shutdown_when_done: bool,
}

impl Default for NightBatch {
    fn default() -> Self {
        Self {
            on: false,
            // 默认 22:00 → 07:00：多数人的"夜里"。
            start_hour: 22,
            end_hour: 7,
            speed_kib: 0,
            shutdown_when_done: false,
        }
    }
}

impl NightBatch {
    /// 此刻是不是在窗口里。`minute` 是本地时间的"一天第几分钟"。
    pub fn in_window(&self, minute: u16) -> bool {
        let start = (self.start_hour.min(23) as u16) * 60;
        let end = (self.end_hour.min(23) as u16) * 60;
        if start == end {
            // 两个数一样 ⇒ 全天（不是"从不"）。
            return true;
        }
        if start < end {
            (start..end).contains(&minute)
        } else {
            // 跨午夜：22:00-24:00 与 00:00-07:00 两段都算。
            minute >= start || minute < end
        }
    }

    /// 该不该派发新任务。
    pub fn may_dispatch(&self, minute: u16) -> bool {
        !self.on || self.in_window(minute)
    }

    /// 此刻的限速（KiB/s）。窗口内用批次那一档，否则回全局。
    pub fn speed_at(&self, minute: u16, global_kib: u64) -> u64 {
        if self.on && self.in_window(minute) {
            self.speed_kib
        } else {
            global_kib
        }
    }

    /// 该不该关机：开着 + 勾了跑完关机 + **这次至少干过活** + 队列已经空了。
    ///
    /// `had_work` 是关键的一条：只看"队列空"会在用户刚打开开关时就把机器关掉。
    pub fn should_shutdown(&self, had_work: bool, busy: bool) -> bool {
        self.on && self.shutdown_when_done && had_work && !busy
    }

    /// 给人看的一句话（状态栏/设置页共用：同一个值不许两处两种说法）。
    pub fn summary(&self) -> String {
        if !self.on {
            return "夜间批次：关".to_string();
        }
        let speed = if self.speed_kib == 0 {
            "不限速".to_string()
        } else {
            format!("{} KiB/s", self.speed_kib)
        };
        format!(
            "夜间批次 {start:02}:00-{end:02}:00 · {speed}{shut}",
            start = self.start_hour.min(23),
            end = self.end_hour.min(23),
            shut = if self.shutdown_when_done {
                " · 跑完关机"
            } else {
                ""
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(start: u8, end: u8) -> NightBatch {
        NightBatch {
            on: true,
            start_hour: start,
            end_hour: end,
            ..NightBatch::default()
        }
    }

    #[test]
    fn a_window_that_crosses_midnight_covers_both_ends() {
        let b = batch(22, 7);
        assert!(b.in_window(23 * 60), "23:00 在窗口里");
        assert!(b.in_window(3 * 60), "03:00 在窗口里");
        assert!(b.in_window(6 * 60 + 59), "06:59 在窗口里");
        assert!(!b.in_window(7 * 60), "07:00 已出窗口（左闭右开）");
        assert!(!b.in_window(12 * 60), "中午不在");
        assert!(!b.in_window(21 * 60 + 59), "21:59 还没进");
    }

    #[test]
    fn a_same_day_window_is_half_open() {
        let b = batch(1, 5);
        assert!(!b.in_window(0));
        assert!(b.in_window(60));
        assert!(!b.in_window(5 * 60));
    }

    /// 两个数设成一样时想的是"一直开着"，不是"从不"。
    #[test]
    fn an_empty_looking_window_means_all_day_not_never() {
        let b = batch(9, 9);
        assert!(b.in_window(0));
        assert!(b.in_window(12 * 60));
    }

    #[test]
    fn dispatch_is_gated_only_while_the_switch_is_on() {
        let off = NightBatch::default();
        assert!(off.may_dispatch(12 * 60), "关着的时候白天也能下");

        let on = batch(22, 7);
        assert!(!on.may_dispatch(12 * 60), "开着就只夜里派发");
        assert!(on.may_dispatch(2 * 60));
    }

    #[test]
    fn the_batch_speed_replaces_the_global_one_inside_the_window() {
        let b = NightBatch {
            speed_kib: 512,
            ..batch(22, 7)
        };
        assert_eq!(b.speed_at(2 * 60, 4096), 512, "窗口内用批次那一档");
        assert_eq!(b.speed_at(12 * 60, 4096), 4096, "窗口外回全局");
        // 关掉之后哪一档都不该生效。
        let off = NightBatch {
            on: false,
            speed_kib: 512,
            ..NightBatch::default()
        };
        assert_eq!(off.speed_at(2 * 60, 4096), 4096);
    }

    /// 关机要三个条件同时成立 —— 尤其是"这次真的干过活"。
    /// 少了它，用户刚打开开关、队列恰好空着，机器就关了。
    #[test]
    fn shutdown_needs_the_switch_work_and_an_idle_queue() {
        let mut b = batch(22, 7);
        b.shutdown_when_done = true;

        assert!(!b.should_shutdown(false, false), "没干过活不许关机");
        assert!(!b.should_shutdown(true, true), "还有活在跑不许关机");
        assert!(b.should_shutdown(true, false), "干过活且队列空了 ⇒ 关机");

        b.shutdown_when_done = false;
        assert!(!b.should_shutdown(true, false), "没勾跑完关机就不关");

        b.on = false;
        b.shutdown_when_done = true;
        assert!(!b.should_shutdown(true, false), "总开关关着就不关");
    }

    #[test]
    fn the_summary_says_what_is_actually_on() {
        let mut b = batch(22, 7);
        assert_eq!(b.summary(), "夜间批次 22:00-07:00 · 不限速");
        b.speed_kib = 512;
        b.shutdown_when_done = true;
        assert_eq!(b.summary(), "夜间批次 22:00-07:00 · 512 KiB/s · 跑完关机");
        assert_eq!(NightBatch::default().summary(), "夜间批次：关");
    }
}
