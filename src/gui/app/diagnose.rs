//! 卡顿诊断（0.6.0 · 特色功能 4）：一条速度曲线 + 两行"为什么慢"的注脚。
//!
//! ## 只报能被证据支持的东西
//!
//! 界面上最容易撒的谎是"正在优化连接"这种安慰话。这里只有两样硬信息：
//!
//! - **重试 N 次**：来自 `Task.attempts`，runner 每派发一次就 +1（用户手动继续不算）；
//! - **卡住 M 秒**：来自采样层"最后一次有新字节到现在"，且必须**确实在跑**。
//!
//! 两个阈值都写在 `rate` 模块里（采样与判定同一处），这里只负责把数字变成人话。
//!
//! ## 曲线画什么
//!
//! 最近 `rate::HISTORY_CAP` 个**速率点**（已经是平滑过的值）。纵轴按这批点的最大值归一，
//! 所以曲线回答的是"它自己相对自己怎么变"，不是"和别的任务比谁快" —— 后者会让一条
//! 20 KiB/s 的小文件和一条 20 MiB/s 的大文件看起来一样高。

use eframe::egui;
use eframe::egui::Rect;

/// 诊断注脚里最多显示的行数（多了就不是"注脚"了）。
pub const MAX_NOTES: usize = 3;

/// 把「重试 / 卡住」变成给人看的话。
///
/// `stalled_secs` 由 `rates.stalled()` 给出（已经过了阈值才会是 `Some`）。
/// 返回空表就去掉整块诊断 —— 没有异常时不该占地方。
pub(crate) fn notes(attempts: u32, stalled_secs: Option<u64>) -> Vec<String> {
    let mut out = Vec::new();
    if attempts > 0 {
        out.push(format!("重试 {} 次", attempts));
    }
    if let Some(secs) = stalled_secs {
        out.push(format!("卡住 {secs}s（{STALL_HINT}）"));
    }
    out.truncate(MAX_NOTES);
    out
}

/// 卡住注脚里的括号说明。写在这里而不是拼在别处：同一句话只许有一份。
const STALL_HINT: &str = "15 秒没有新字节";

/// 曲线在 `rect` 里的折线点（`y` 向下，原点在左上，跟 egui 一致）。
///
/// - 少于两个点 ⇒ 空表（一个点画不出走势，硬画一条直线是在编）；
/// - 全 0 ⇒ 贴着底边画一条直线（"一直在跑但没速度"本身是有意义的信息）；
/// - 否则按这批点的最大值归一。
pub(crate) fn spark_points(history: &[f64], rect: Rect) -> Vec<egui::Pos2> {
    if history.len() < 2 {
        return Vec::new();
    }
    let max = history.iter().cloned().fold(0.0f64, f64::max);
    let step = rect.width() / (history.len() - 1) as f32;
    history
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let ratio = if max > 0.0 {
                (*v / max).clamp(0.0, 1.0) as f32
            } else {
                0.0
            };
            egui::pos2(
                rect.left() + step * i as f32,
                rect.bottom() - rect.height() * ratio,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{pos2, vec2};

    fn rect() -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 40.0))
    }

    #[test]
    fn a_healthy_task_has_no_notes() {
        assert!(notes(0, None).is_empty(), "没有异常就别占地方");
    }

    #[test]
    fn retries_and_stalls_both_show_up_in_words() {
        let n = notes(2, Some(18));
        assert_eq!(n[0], "重试 2 次");
        assert!(n[1].starts_with("卡住 18s"), "实际：{}", n[1]);
        assert_eq!(notes(0, Some(31)).len(), 1, "没重试就只报卡住");
        assert_eq!(notes(3, None).len(), 1, "不卡就只报重试");
    }

    #[test]
    fn one_point_draws_nothing_and_zero_speed_draws_the_baseline() {
        assert!(spark_points(&[], rect()).is_empty());
        assert!(
            spark_points(&[1024.0], rect()).is_empty(),
            "一个点画不出走势"
        );

        let flat = spark_points(&[0.0, 0.0, 0.0], rect());
        assert_eq!(flat.len(), 3);
        assert!(
            flat.iter().all(|p| (p.y - 40.0).abs() < 0.01),
            "全 0 贴着底边"
        );
    }

    /// 归一化：最大那个点顶到上边，其余按比例 —— 曲线比的是"它自己相对自己"。
    #[test]
    fn the_tallest_point_touches_the_top() {
        let pts = spark_points(&[10.0, 20.0, 40.0, 20.0], rect());
        assert_eq!(pts.len(), 4);
        assert!((pts[0].y - 30.0).abs() < 0.01, "10/40 ⇒ 从底边往上 1/4");
        assert!((pts[2].y - 0.0).abs() < 0.01, "最大值顶到上边");
        assert_eq!(pts[0].x, 0.0);
        assert!((pts[3].x - 100.0).abs() < 0.01, "最后一个点贴右边");
    }
}
