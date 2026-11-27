//! 分段条：把断点文件里的覆盖集画成「哪几段已经下完了」。
//!
//! 和进度条的分工很清楚：
//! - 进度条回答**到哪了**（一个百分比）
//! - 分段条回答**是怎么走到这的**：中间有没有空洞、是不是几段并行在推
//!
//! 数据来自 `queue::segments`（断点文件里的已确认区间），最多每
//! `STATE_SAVE_MAX_INTERVAL_SECS`（5 秒）刷新一次。所以这一档是**覆盖集**，
//! 不是"此刻几条连接各自下到哪"—— 后者只在引擎内存里，界面上看不到。

use eframe::egui;

use super::super::snapshot::TaskView;
use super::super::theme;

/// 每一段在屏幕上至少占的宽度（点）。长文件上的窄段按比例会小到看不见，
/// 那样"这一段下完了"这个信息就丢了。
const MIN_SPAN: f32 = 1.5;

/// 把覆盖集归一化成轨道上的 `[(起, 止), ...]`（单位：点，`0..=width`）。
///
/// 纯几何，先测后画。规则：
/// - `total == 0` / `width <= 0` / 没有段 ⇒ 空
/// - 区间按**闭区间**理解（`(start, end)` 两端都算），越界夹到 `[0, total-1]`
/// - **相邻或重叠的先合并**：多段并行推进时覆盖集会碎成几十上百段，
///   不合并就成了一排毛刺，看不出"整体到哪了"
/// - 每段至少 `MIN_SPAN` 宽
pub fn spans(total: u64, segments: &[(u64, u64)], width: f32) -> Vec<(f32, f32)> {
    if total == 0 || width <= 0.0 {
        return Vec::new();
    }
    let mut rows: Vec<(u64, u64)> = segments
        .iter()
        .filter_map(|&(a, b)| {
            // 传进来的区间按 (start, end) 用，但写反了也不许 panic —— 数据来自文件。
            let (s, e) = if a <= b { (a, b) } else { (b, a) };
            if s >= total {
                return None;
            }
            let e = e.min(total - 1);
            (e >= s).then_some((s, e))
        })
        .collect();
    rows.sort_unstable();

    let mut merged: Vec<(u64, u64)> = Vec::with_capacity(rows.len());
    for (s, e) in rows {
        match merged.last_mut() {
            // `+1`：闭区间相邻（5..=9 与 10..=14）视觉上是连着的，要合并。
            Some(last) if s <= last.1.saturating_add(1) => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }

    let scale = f64::from(width) / total as f64;
    merged
        .into_iter()
        .map(|(s, e)| {
            let x0 = (s as f64 * scale) as f32;
            let mut x1 = ((e + 1) as f64 * scale) as f32;
            if x1 - x0 < MIN_SPAN {
                x1 = x0 + MIN_SPAN;
            }
            (x0, x1.min(width))
        })
        .collect()
}

/// 画到给定的轨道矩形里。没有分段信息时什么都不画（轨道本身由调用方画）。
/// 覆盖集有没有**空洞**（也就是"还有哪一段没下"）。
///
/// 列表里的进度条只有在有空洞时才画成分段 —— 否则一百多个相邻段会显示成一串碎块，
/// 看着像"进度条坏了"，其实它是连续的。用户 2026-11-27 说
/// 「列表进度条显示整体进度、侧边栏再显示更详细的分块」，这就是那条的判据。
pub fn has_holes(total: u64, segments: &[(u64, u64)]) -> bool {
    if total == 0 || segments.is_empty() {
        return false;
    }
    let mut rows: Vec<(u64, u64)> = segments
        .iter()
        .map(|&(a, b)| if a <= b { (a, b) } else { (b, a) })
        .filter(|(s, _)| *s < total)
        .map(|(s, e)| (s, e.min(total - 1)))
        .collect();
    if rows.is_empty() {
        return false;
    }
    rows.sort_unstable();
    // 闭区间口径，与 `spans` 一致：中间隔了一格才算空洞。
    if rows[0].0 > 0 {
        return true;
    }
    let mut reach = rows[0].1;
    for (s, e) in rows.into_iter().skip(1) {
        if s > reach.saturating_add(1) {
            return true;
        }
        reach = reach.max(e);
    }
    reach + 1 < total
}

pub fn draw(ui: &egui::Ui, track: egui::Rect, task: &TaskView) {
    let Some(total) = task.total_len else {
        return;
    };
    let rows = spans(total, &task.segments, track.width());
    if rows.is_empty() {
        return;
    }
    let color = if task.status == "done" {
        theme::ok()
    } else {
        theme::accent()
    };
    let painter = ui.painter();
    for (x0, x1) in rows {
        let rect = egui::Rect::from_min_size(
            egui::pos2(track.left() + x0, track.top()),
            egui::vec2((x1 - x0).max(0.0), track.height()),
        );
        painter.rect_filled(rect, 1.0, color);
    }
}

#[cfg(test)]
mod tests {

    /// 列表进度条只在**真有空洞**时才分段（用户 2026-11-27 的决定）。
    #[test]
    fn only_a_gapped_coverage_counts_as_holes() {
        use super::has_holes;
        let total = 100u64;
        assert!(!has_holes(total, &[]), "没有覆盖集就不分段");
        assert!(
            !has_holes(total, &[(0, 99)]),
            "整条覆盖完 ⇒ 一条实心条，不许画成碎块"
        );
        assert!(
            !has_holes(total, &[(0, 49), (50, 99)]),
            "两段正好相邻（50 接着 49）⇒ 仍然算连续"
        );
        assert!(!has_holes(total, &[(0, 49), (48, 99)]), "重叠也算连续");
        assert!(has_holes(total, &[(0, 49)]), "后一半没下 ⇒ 有空洞");
        assert!(has_holes(total, &[(10, 99)]), "开头没下 ⇒ 有空洞");
        assert!(
            has_holes(total, &[(0, 19), (40, 99)]),
            "中间断了一截 ⇒ 有空洞"
        );
        assert!(
            !has_holes(total, &[(0, 99), (200, 300)]),
            "越界的段被过滤掉，剩下 (0,99) 本身就是全覆盖 ⇒ 没有空洞"
        );
    }

    /// **文字里的块数必须与画出来的块数一致。**
    ///
    /// 这两处各有各的合并实现（`queue::merged_count` 报数、`spans` 画条），
    /// 口径一旦走样就会出现"文字说 353 段、图上像一整条"的自相矛盾 ——
    /// 2026-11-27 用户就是看着这个问「区间是不是分太多了」。
    #[test]
    fn the_displayed_block_count_matches_the_drawn_blocks() {
        let total = 1000u64;
        let cases: Vec<Vec<(u64, u64)>> = vec![
            vec![],
            vec![(0, 10)],
            vec![(0, 10), (11, 20)],    // 闭区间相邻：视觉上连着
            vec![(0, 10), (12, 20)],    // 中间断开
            vec![(500, 600), (0, 100)], // 乱序
            vec![(0, 100), (10, 20)],   // 被包含
            vec![(0, 999)],             // 整条
        ];
        for rows in cases {
            let drawn = spans(total, &rows, 600.0).len();
            let counted = crate::queue::merged_count(&rows);
            assert_eq!(
                drawn, counted,
                "画出来 {drawn} 块、文字却要报 {counted}：{rows:?}"
            );
        }
    }

    use super::spans;

    #[test]
    fn nothing_to_draw_when_there_is_no_total_or_no_width() {
        assert!(
            spans(0, &[(0, 5)], 100.0).is_empty(),
            "total 为 0 没法归一化"
        );
        assert!(spans(100, &[(0, 5)], 0.0).is_empty(), "没有宽度就没得画");
        assert!(spans(100, &[], 100.0).is_empty(), "没有段就是空");
    }

    #[test]
    fn a_full_cover_fills_the_whole_track() {
        let rows = spans(100, &[(0, 99)], 200.0);
        assert_eq!(rows.len(), 1);
        assert!((rows[0].0 - 0.0).abs() < 0.01);
        assert!((rows[0].1 - 200.0).abs() < 0.01, "{rows:?}");
    }

    #[test]
    fn two_finished_chunks_leave_a_visible_hole() {
        // 0..=24 与 50..=74 各占 1/4，中间空一半。
        let rows = spans(100, &[(0, 24), (50, 74)], 200.0);
        assert_eq!(rows.len(), 2, "两段之间必须留出空洞：{rows:?}");
        assert!((rows[0].0 - 0.0).abs() < 0.01);
        assert!((rows[0].1 - 50.0).abs() < 0.01, "{rows:?}");
        assert!((rows[1].0 - 100.0).abs() < 0.01, "{rows:?}");
        assert!((rows[1].1 - 150.0).abs() < 0.01, "{rows:?}");
    }

    /// 相邻（闭区间上首尾相接）与重叠的必须合并 —— 不然并行下载时覆盖集会碎成
    /// 上百段，画出来是一排毛刺。
    #[test]
    fn touching_and_overlapping_chunks_are_merged() {
        assert_eq!(
            spans(100, &[(0, 9), (10, 19)], 100.0).len(),
            1,
            "相邻要合并"
        );
        assert_eq!(
            spans(100, &[(0, 19), (5, 29)], 100.0).len(),
            1,
            "重叠要合并"
        );
        let rows = spans(100, &[(0, 19), (5, 29)], 100.0);
        assert!((rows[0].1 - 30.0).abs() < 0.01, "合并后取并集：{rows:?}");
    }

    #[test]
    fn out_of_range_and_reversed_chunks_do_not_panic() {
        // 越界夹到 total；写反的 (end, start) 按大小纠正；完全越界的丢掉。
        let rows = spans(100, &[(90, 500), (50, 40), (200, 300)], 100.0);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(
            (rows[0].1 - 51.0).abs() < 1.001,
            "40..=50 应到 51 附近：{rows:?}"
        );
        assert!(
            (rows[1].1 - 100.0).abs() < 0.01,
            "越界要夹到轨道末端：{rows:?}"
        );
    }

    /// 长文件上的窄段：按比例会小到一个像素以下，必须保底，否则信息丢了。
    #[test]
    fn a_tiny_chunk_still_occupies_a_sliver() {
        let rows = spans(1_000_000_000, &[(0, 0)], 300.0);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].1 - rows[0].0 >= super::MIN_SPAN - 0.001, "{rows:?}");
    }

    #[test]
    fn spans_never_exceed_the_track() {
        let rows = spans(1000, &[(0, 999)], 123.0);
        assert!(rows.iter().all(|(a, b)| *a >= 0.0 && *b <= 123.0 + 0.01));
    }
}
