//! 限速计划（0.5.0 · 1）：按时间段自动切速度。
//!
//! 只有一条规则：**先命中先用**。窗口按用户排的顺序匹配，重叠时第一个赢 ——
//! 这样"重叠"不会变成"取最小值"这种事后猜的语义，用户看界面顺序就知道结果。
//!
//! 跨零点必须支持（`22:00–06:00` 是最常见的写法），这是本模块唯一容易写错的地方，
//! 所以 `in_window` 单独抽出来测边界。

use serde::{Deserialize, Serialize};

/// 一条限速窗口。时间用"当天第几分钟"（0..1440），避免再引入时间库。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeedWindow {
    pub start_minute: u16,
    pub end_minute: u16,
    /// `0` = 不限速。
    pub kibps: u32,
}

impl SpeedWindow {
    /// `22:00-06:00=512` → 一条窗口。
    ///
    /// 只认这一种形状：冒号时间、`-`、`=`、整数。写错就报错而不是猜 ——
    /// 猜错的后果是"半夜突然不限速"，比报错难发现得多。
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let (span, kibps) = text
            .split_once('=')
            .ok_or_else(|| format!("缺少 `=速度`：{text}"))?;
        let (start, end) = span
            .split_once('-')
            .ok_or_else(|| format!("缺少 `-`：{text}"))?;
        let kibps: u32 = kibps
            .trim()
            .parse()
            .map_err(|_| format!("速度不是整数：{}", kibps.trim()))?;
        Ok(Self {
            start_minute: parse_hhmm(start)?,
            end_minute: parse_hhmm(end)?,
            kibps,
        })
    }

    pub fn to_text(&self) -> String {
        format!(
            "{}-{}={}",
            hhmm(self.start_minute),
            hhmm(self.end_minute),
            self.kibps
        )
    }

    /// 这一分钟落在这条窗口里吗。
    ///
    /// - `start < end`：普通区间 `[start, end)`
    /// - `start > end`：跨零点，`[start, 1440) ∪ [0, end)`
    /// - `start == end`：视为**全天**（用户写 `08:00-08:00` 想表达的只能是这个）
    pub fn contains(&self, minute: u16) -> bool {
        let m = minute % 1440;
        if self.start_minute == self.end_minute {
            return true;
        }
        if self.start_minute < self.end_minute {
            self.start_minute <= m && m < self.end_minute
        } else {
            m >= self.start_minute || m < self.end_minute
        }
    }
}

fn parse_hhmm(text: &str) -> Result<u16, String> {
    let text = text.trim();
    let (h, m) = text
        .split_once(':')
        .ok_or_else(|| format!("时间不是 HH:MM：{text}"))?;
    let h: u16 = h.parse().map_err(|_| format!("小时不是整数：{h}"))?;
    let m: u16 = m.parse().map_err(|_| format!("分钟不是整数：{m}"))?;
    if h > 23 || m > 59 {
        return Err(format!("时间超出范围：{text}"));
    }
    Ok(h * 60 + m)
}

fn hhmm(minute: u16) -> String {
    format!("{:02}:{:02}", minute / 60, minute % 60)
}

/// 当前该用多少 KiB/s。
///
/// `fallback` 是设置里的全局上限（0 = 不限）。窗口都不命中就用它。
pub fn rate_at(minute: u16, windows: &[SpeedWindow], fallback: u32) -> u32 {
    windows
        .iter()
        .find(|w| w.contains(minute))
        .map_or(fallback, |w| w.kibps)
}

/// 本机"当天第几分钟"。Windows 走 `GetLocalTime`：用户写 22:00 指的是本地时间，
/// 用 UTC 会整体偏 8 小时 —— 这种错用户只会觉得"限速没生效"。
#[cfg(windows)]
pub fn local_minute_of_day() -> u16 {
    #[repr(C)]
    struct SysTime {
        year: u16,
        month: u16,
        dow: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        ms: u16,
    }
    unsafe extern "system" {
        fn GetLocalTime(out: *mut SysTime);
    }
    let mut t = SysTime {
        year: 0,
        month: 0,
        dow: 0,
        day: 0,
        hour: 0,
        minute: 0,
        second: 0,
        ms: 0,
    };
    unsafe { GetLocalTime(&mut t) };
    (t.hour % 24) * 60 + (t.minute % 60)
}

#[cfg(not(windows))]
pub fn local_minute_of_day() -> u16 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(start: &str, end: &str, kibps: u32) -> SpeedWindow {
        SpeedWindow::parse(&format!("{start}-{end}={kibps}")).expect("窗口应可解析")
    }

    #[test]
    fn parses_and_roundtrips_the_documented_shape() {
        let win = w("22:00", "06:00", 512);
        assert_eq!(win.start_minute, 22 * 60);
        assert_eq!(win.end_minute, 6 * 60);
        assert_eq!(win.to_text(), "22:00-06:00=512");
    }

    #[test]
    fn refuses_shapes_it_cannot_read_instead_of_guessing() {
        for bad in [
            "22:00-06:00",
            "22:00=512",
            "25:00-06:00=1",
            "22:70-06:00=1",
            "x-y=1",
        ] {
            assert!(SpeedWindow::parse(bad).is_err(), "{bad} 不该被接受");
        }
    }

    #[test]
    fn a_same_day_window_uses_half_open_bounds() {
        let win = w("09:00", "18:00", 100);
        assert!(!win.contains(8 * 60 + 59), "开始前一分钟不在窗口里");
        assert!(win.contains(9 * 60), "整点包含");
        assert!(win.contains(17 * 60 + 59), "结束前一分钟包含");
        assert!(!win.contains(18 * 60), "结束整点不包含");
    }

    #[test]
    fn a_window_across_midnight_covers_both_ends() {
        let win = w("22:00", "06:00", 512);
        assert!(win.contains(22 * 60), "22:00 属于");
        assert!(win.contains(23 * 60 + 59), "23:59 属于");
        assert!(win.contains(0), "00:00 属于");
        assert!(win.contains(5 * 60 + 59), "05:59 属于");
        assert!(!win.contains(6 * 60), "06:00 不属于");
        assert!(!win.contains(12 * 60), "中午不属于");
    }

    #[test]
    fn identical_bounds_mean_the_whole_day() {
        let win = w("08:00", "08:00", 64);
        for m in [0, 8 * 60, 12 * 60, 23 * 60] {
            assert!(win.contains(m), "{m} 应属于全天窗口");
        }
    }

    #[test]
    fn first_matching_window_wins_and_empty_falls_back() {
        let windows = vec![w("22:00", "06:00", 512), w("00:00", "23:59", 100)];
        assert_eq!(rate_at(23 * 60, &windows, 999), 512, "重叠时第一条赢");
        assert_eq!(rate_at(12 * 60, &windows, 999), 100);
        assert_eq!(rate_at(12 * 60, &[], 999), 999, "没有窗口就用全局值");
    }
}
