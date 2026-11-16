//! 批量选择：一套**只认 id 集合**的纯逻辑，界面只负责调它。
//!
//! ## 为什么单独拆出来
//!
//! 0.7.0 起列表里同时存在两套"选中"：
//!
//! - `App::selected`：**正在右栏查看的那一条**（详情跟着它走）；
//! - `App::batch`：**批量操作的集合**（可能十几条，详情不看它们）。
//!
//! 这两套搅在一起是最容易出错的地方：点一下行到底该改哪个？勾了复选框右栏要不要跳？
//! 所以判定全部写成纯函数，界面只做三件事——调函数、发票据、重画。边界因此能被用例钉住。
//!
//! ## 规则（界面照此实现，不再各自发挥）
//!
//! | 操作 | `selected`（看详情） | `batch`（批量） |
//! |---|---|---|
//! | 单击行 | 该行 | 不变 |
//! | Ctrl+单击行 | 该行 | 翻转该行 |
//! | 勾复选框 | 不变 | 翻转该行 |
//! | 表头复选框 | 不变 | 全选可见项 / 全部取消 |
//! | Esc | 清空 | 清空 |
//!
//! 一条都不例外：**单击行不改 batch**。否则"我想看看这条的详情"会顺手把它拖进批量删除。

use std::collections::BTreeSet;

/// 批量集合的类型别名。`BTreeSet` 而不是 `HashSet`：遍历顺序稳定，发票据的顺序也可复现。
pub(crate) type Batch = BTreeSet<String>;

/// 翻转一条。
pub(crate) fn toggle(batch: &mut Batch, id: &str) {
    if !batch.remove(id) {
        batch.insert(id.to_string());
    }
}

/// 表头的复选框：可见项**全在里面**就取消全部，否则补成全选。
///
/// 只看"可见项"：筛了「失败」之后点全选，不该把看不见的已完成也拉进来。
pub(crate) fn toggle_all(batch: &mut Batch, visible: &[String]) {
    if visible.iter().all(|id| batch.contains(id)) {
        for id in visible {
            batch.remove(id);
        }
    } else {
        batch.extend(visible.iter().cloned());
    }
}

/// 表头复选框该显示成"已全选"吗。空列表不算全选。
pub(crate) fn all_selected(batch: &Batch, visible: &[String]) -> bool {
    !visible.is_empty() && visible.iter().all(|id| batch.contains(id))
}

/// 清掉已经不存在的任务（删掉之后，批量里不能留着幽灵 id）。
pub(crate) fn prune(batch: &mut Batch, alive: &[String]) {
    batch.retain(|id| alive.iter().any(|a| a == id));
}

/// 按**列表顺序**把批量集合排成要发票据的目标。
///
/// 顺序很重要：用户看到的先后就是动作发生的先后（"继续"按这个次序入队），
/// 这里用列表顺序而不是集合顺序，避免"我勾的三条按 id 字典序启动"。
pub(crate) fn targets(batch: &Batch, visible_order: &[String]) -> Vec<String> {
    visible_order
        .iter()
        .filter(|id| batch.contains(*id))
        .cloned()
        .collect()
}

/// 删几条才值得再问一句。
///
/// 单条删除不确认：用户在行上点「删除」、上下文条上点「删除」，对象就在眼前，
/// 多问一句只是拖慢。**两条以上要问** —— 那是"一次点掉一批"，误触成本完全不同
/// （丢的是断点进度，不是重下一次就有的小文件）。
pub(crate) fn needs_batch_confirm(count: usize) -> bool {
    count >= 2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn toggling_one_id_flips_only_that_id() {
        let mut b = Batch::new();
        toggle(&mut b, "a");
        assert!(b.contains("a"));
        toggle(&mut b, "a");
        assert!(!b.contains("a"), "再 flip 一次应当取消");
    }

    #[test]
    fn select_all_only_covers_the_visible_rows() {
        let mut b = Batch::new();
        let visible = ids(&["a", "b"]);
        // 「已完成」被筛掉了，不在 visible 里，所以点全选不该把它拉进来。
        toggle_all(&mut b, &visible);
        assert_eq!(b.len(), 2);
        assert!(!b.contains("hidden-done"));
    }

    #[test]
    fn select_all_when_everything_is_selected_clears_them() {
        let mut b = Batch::new();
        let visible = ids(&["a", "b"]);
        toggle_all(&mut b, &visible);
        assert!(all_selected(&b, &visible));
        toggle_all(&mut b, &visible);
        assert!(b.is_empty(), "已经全选时再点一次应当全部取消");
        assert!(!all_selected(&b, &visible));
    }

    #[test]
    fn an_empty_list_is_never_all_selected() {
        let b = Batch::new();
        assert!(
            !all_selected(&b, &[]),
            "空列表不该显示成「已全选」（否则表头那个勾会一直在）"
        );
    }

    #[test]
    fn pruning_removes_ids_that_no_longer_exist() {
        let mut b = ids(&["a", "b", "c"]).into_iter().collect::<Batch>();
        prune(&mut b, &ids(&["a", "c"]));
        assert!(!b.contains("b"), "删掉的任务要从批量里清掉");
        assert_eq!(b.len(), 2);
    }

    #[test]
    fn targets_follow_the_list_order_not_the_id_order() {
        // 勾的时候先勾 c 后勾 a，但列表顺序是 a、b、c —— 发票据要按列表顺序。
        let mut b = Batch::new();
        toggle(&mut b, "c");
        toggle(&mut b, "a");
        let visible = ids(&["a", "b", "c"]);
        assert_eq!(targets(&b, &visible), ids(&["a", "c"]));
    }

    #[test]
    fn targets_skip_rows_that_are_filtered_out() {
        let mut b = Batch::new();
        toggle(&mut b, "a");
        toggle(&mut b, "b");
        // 现在只剩 a 可见（b 被筛掉了）—— 批量动作只作用于看得见的。
        let visible = ids(&["a"]);
        assert_eq!(targets(&b, &visible), ids(&["a"]));
    }

    /// 一条不问、两条起才问。
    #[test]
    fn only_multi_item_deletes_ask_for_confirmation() {
        assert!(!needs_batch_confirm(0), "空集合没得问");
        assert!(
            !needs_batch_confirm(1),
            "单条删除与上下文条一致，不多问一句"
        );
        assert!(needs_batch_confirm(2));
        assert!(needs_batch_confirm(37));
    }
}
