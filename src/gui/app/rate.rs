//! 速率采样：列表的「速度」列与状态栏的「总速度」都从这里来。
//!
//! ## 为什么在 GUI 侧算，而不是让引擎报
//!
//! 引擎每 500ms 交一份快照，里面有 `delivered`（已交付字节）与状态，但没有「速率」。
//! 要引擎再报一个速率，就得改 `dlm.exe` 的输出协议、再加一层解析 —— 而**两次快照的差**
//! 本来就是速率，算一下就有。副作用是可测：喂两个时刻进去断言就行，不用跑真网络。
//!
//! ## 几条纪律（都是踩出来的，不是洁癖）
//!
//! - **第一次采样永远没有速率**：只有一个点算不出斜率，此时必须显示「—」而不是编一个数。
//! - **暂停 / 完成 / 失败立刻清掉速率**：否则暂停后那一列还挂着一个旧速度，界面在说谎。
//! - **两次采样太密就不算**：UI 是 100ms 一帧，帧间隔抖动会把噪声放大成几倍的速度。
//! - **重试或回退（`delivered` 变小）重新起算**：不能拿「变小」当负速度。
//! - 新样本只占 30%：水龙头式的速率（一块一块到达）不平滑的话数字会来回跳。
//!
//! ## 0.6.0 阶段 4-4：曲线与卡顿
//!
//! 这里同时留**最近若干个速率点**（抽屉里那条曲线）与**最近一次有进展的时刻**
//! （判断"卡住多久"）。两样都放在采样层，而不是各处自己记：速率的定义只该有一份，
//! 否则曲线、数字、卡顿判定会各说各话。

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// 两次采样至少要隔这么久才算一个有效窗口。
pub const MIN_WINDOW: Duration = Duration::from_millis(250);

/// 每个任务留多少个速率点。60 个点 ≈ 按 UI 采样节奏画出最近一分钟的走势。
pub const HISTORY_CAP: usize = 60;

/// 多久没有新字节就算"卡住"。15 秒比 HTTP 超时短得多：卡片式的站点经常
/// 二三十秒才吐一块，15 秒既不会刷屏误报，也足够早。
pub const STALL_AFTER: Duration = Duration::from_secs(15);

/// 新样本在平滑里的权重。
const SMOOTHING: f64 = 0.3;

#[derive(Debug, Clone, Copy)]
struct Sample {
    at: Instant,
    delivered: u64,
}

#[derive(Debug, Default)]
pub struct Rates {
    last: HashMap<String, Sample>,
    speed: HashMap<String, f64>,
    /// 最近若干个速率点（老的在左），画曲线用。
    history: HashMap<String, Vec<f64>>,
    /// 最近一次 `delivered` **变大**的时刻；判断卡顿用。
    progress_at: HashMap<String, Instant>,
}

impl Rates {
    /// 记一次观测。
    ///
    /// `running == false` 时把这个任务的速率**清掉**（不是置零）：暂停的任务那一列该是「—」，
    /// 不是「0 B/s」——后者看着像还在跑只是没数据。
    pub fn observe(&mut self, id: &str, delivered: u64, running: bool, now: Instant) {
        if !running {
            self.last.remove(id);
            self.speed.remove(id);
            // 停了就不再画曲线、也不再判卡顿：那两样都是"正在跑"的信息。
            self.history.remove(id);
            self.progress_at.remove(id);
            return;
        }
        let Some(prev) = self.last.get(id).copied() else {
            // 第一个点：只记下来，不产生速率。
            self.last
                .insert(id.to_string(), Sample { at: now, delivered });
            self.progress_at.insert(id.to_string(), now);
            return;
        };
        if delivered > prev.delivered {
            // 有新字节进来 ⇒ 进度是活的（卡顿计时的起点）。
            self.progress_at.insert(id.to_string(), now);
        }
        if delivered < prev.delivered {
            // 重试/回退：重新起算，别把负增长当速度。
            self.last
                .insert(id.to_string(), Sample { at: now, delivered });
            self.speed.remove(id);
            return;
        }
        let dt = now.saturating_duration_since(prev.at);
        if dt < MIN_WINDOW {
            // 采样太密：保留上一次的速率，也不推进基准点（否则永远攒不满一个窗口）。
            return;
        }
        let instant = delivered.saturating_sub(prev.delivered) as f64 / dt.as_secs_f64();
        let smoothed = match self.speed.get(id) {
            Some(&old) => old * (1.0 - SMOOTHING) + instant * SMOOTHING,
            None => instant,
        };
        let smoothed = smoothed.max(0.0);
        self.speed.insert(id.to_string(), smoothed);
        // 用 `Vec` 而不是 `VecDeque`：`VecDeque::as_slices()` 在环形回绕之后会给出**两段**，
        // 只取 `.0` 就会少画一截（实测 60 个点只画出来 59 个）。60 个元素的 `remove(0)`
        // 是 O(60)，这里无所谓。
        let series = self.history.entry(id.to_string()).or_default();
        series.push(smoothed);
        while series.len() > HISTORY_CAP {
            series.remove(0);
        }
        self.last
            .insert(id.to_string(), Sample { at: now, delivered });
    }

    /// 最近若干个速率点（老的在左）。没在跑或还没采到样的是空切片。
    pub fn history(&self, id: &str) -> &[f64] {
        self.history
            .get(id)
            .map(|series| series.as_slice())
            .unwrap_or(&[])
    }

    /// 已经多久没有新字节了（只在"在跑但没进展"时给 `Some`）。
    ///
    /// 判据只看**有没有新字节**，不看速率大小：限速到 1 KiB/s 的任务速率很小，
    /// 但它确实在动 —— 那种不该被标成卡住。
    pub fn stalled(&self, id: &str, now: Instant) -> Option<Duration> {
        let since = self.progress_at.get(id)?;
        let idle = now.saturating_duration_since(*since);
        (idle >= STALL_AFTER).then_some(idle)
    }

    /// 这个任务当前速率（字节/秒）。`None` = 还不知道（第一次采样）或没在跑。
    pub fn speed(&self, id: &str) -> Option<f64> {
        self.speed.get(id).copied()
    }

    /// 在跑任务的总速率。没有任务在跑时是 0。
    ///
    /// 只累加「已知速率」的任务：刚启动那一瞬间还没采到样的任务贡献 0，而不是被当成未知跳过整条。
    pub fn total(&self) -> f64 {
        self.speed.values().sum()
    }

    /// 清掉已经不在快照里的任务（被删掉的那些），避免表越攒越大。
    pub fn retain(&mut self, alive: impl IntoIterator<Item = String>) {
        let keep: std::collections::HashSet<String> = alive.into_iter().collect();
        self.last.retain(|id, _| keep.contains(id));
        self.speed.retain(|id, _| keep.contains(id));
        self.history.retain(|id, _| keep.contains(id));
        self.progress_at.retain(|id, _| keep.contains(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: u64) -> Instant {
        // 从一个固定基准往前推，测试里不依赖真实时钟。
        static BASE: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        let base = *BASE.get_or_init(Instant::now);
        base + Duration::from_millis(ms)
    }

    #[test]
    fn the_first_sample_has_no_speed() {
        let mut r = Rates::default();
        r.observe("a", 1000, true, at(0));
        assert_eq!(r.speed("a"), None, "一个点算不出斜率，只能是未知");
        assert_eq!(r.total(), 0.0);
    }

    #[test]
    fn two_samples_a_second_apart_give_bytes_per_second() {
        let mut r = Rates::default();
        r.observe("a", 0, true, at(0));
        r.observe("a", 1024, true, at(1000));
        // 第一个有效窗口没有旧值可平滑，直接就是这一段的真值
        let first = r.speed("a").expect("有速率");
        assert!((first - 1024.0).abs() < 1.0, "实际 {first}");
        // 再喂一口同样速率：平滑之后仍应贴着 1024
        r.observe("a", 2048, true, at(2000));
        let again = r.speed("a").expect("有速率");
        assert!((again - 1024.0).abs() < 1.0, "实际 {again}");
        // 速率翻倍：平滑只走 30%，所以既不是 1024 也不是 2048
        r.observe("a", 4096, true, at(3000));
        let up = r.speed("a").expect("有速率");
        assert!(up > 1024.0 && up < 2048.0, "平滑没起作用：{up}");
    }

    #[test]
    fn samples_closer_than_the_minimum_window_do_not_produce_a_rate() {
        let mut r = Rates::default();
        r.observe("a", 0, true, at(0));
        r.observe("a", 999, true, at(100)); // 100ms < 250ms
        assert_eq!(r.speed("a"), None, "太密的采样是噪声，不许出数");
    }

    #[test]
    fn a_paused_task_loses_its_rate_instead_of_showing_zero() {
        let mut r = Rates::default();
        r.observe("a", 0, true, at(0));
        r.observe("a", 10_000, true, at(1000));
        assert!(r.speed("a").is_some());
        r.observe("a", 10_000, false, at(1500)); // 暂停
        assert_eq!(r.speed("a"), None, "暂停后那一列是「—」，不是 0 B/s");
    }

    #[test]
    fn a_restart_that_rewinds_delivered_starts_over_instead_of_going_negative() {
        let mut r = Rates::default();
        r.observe("a", 50_000, true, at(0));
        r.observe("a", 60_000, true, at(1000));
        assert!(r.speed("a").is_some());
        // 重试：新的 attempt 从很小的 delivered 开始
        r.observe("a", 100, true, at(1200));
        assert_eq!(r.speed("a"), None, "回退要重新起算，不能算负速度");
        r.observe("a", 1100, true, at(2200));
        assert!(r.speed("a").is_some(), "重新起算之后要能继续出数");
    }

    #[test]
    fn total_is_the_sum_of_the_running_tasks() {
        let mut r = Rates::default();
        for id in ["a", "b"] {
            r.observe(id, 0, true, at(0));
            r.observe(id, 1024, true, at(1000));
        }
        let s = r.speed("a").unwrap() + r.speed("b").unwrap();
        assert!((r.total() - s).abs() < 0.01);
    }

    #[test]
    fn tasks_that_left_the_snapshot_are_forgotten() {
        let mut r = Rates::default();
        r.observe("a", 0, true, at(0));
        r.observe("a", 1024, true, at(1000));
        r.retain(["other".to_string()]);
        assert_eq!(r.speed("a"), None);
        assert_eq!(r.total(), 0.0);
        assert!(r.history("a").is_empty());
    }

    /// 曲线：每个有效窗口推一个点，攒满就丢最老的。**不是**越攒越长。
    #[test]
    fn the_history_keeps_the_newest_points_and_drops_the_oldest() {
        let mut r = Rates::default();
        r.observe("a", 0, true, at(0));
        for i in 1..=(HISTORY_CAP as u64 + 5) {
            r.observe("a", i * 1024, true, at(i * 1000));
        }
        let h = r.history("a");
        assert_eq!(h.len(), HISTORY_CAP, "环满了就丢掉最老的");
        assert!(h.iter().all(|v| *v > 0.0));
    }

    /// 卡顿只看"有没有新字节"，不看速率大小 —— 限速到 1 KiB/s 的任务在动，不算卡。
    #[test]
    fn stalling_is_about_new_bytes_not_about_small_speeds() {
        let mut r = Rates::default();
        r.observe("a", 0, true, at(0));
        // 每秒 1 KiB，一直动
        for i in 1..=30 {
            r.observe("a", i * 1024, true, at(i * 1000));
        }
        assert_eq!(r.stalled("a", at(30_000)), None, "一直在动就不算卡住");

        // 之后 20 秒没有新字节
        r.observe("a", 30 * 1024, true, at(50_000));
        let idle = r.stalled("a", at(50_000)).expect("该报卡住了");
        assert!(idle >= STALL_AFTER, "卡住时长从最后一次有进展算起");
    }

    #[test]
    fn a_paused_task_is_never_reported_as_stalled() {
        let mut r = Rates::default();
        r.observe("a", 0, true, at(0));
        r.observe("a", 1024, true, at(1000));
        r.observe("a", 1024, false, at(2000)); // 暂停
        assert_eq!(r.stalled("a", at(60_000)), None, "暂停不是卡住");
        assert!(r.history("a").is_empty(), "暂停之后不该还挂着曲线");
    }
}
