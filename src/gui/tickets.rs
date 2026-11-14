//! Agent 下发的**票据**（0.6.0 阶段 5）：Agent 想加任务，先回到对话里问人。
//!
//! ## 为什么需要这一层
//!
//! 本地控制口是 `127.0.0.1 + token` —— 挡住的是别的机器，挡不住**本机的 Agent**。
//! 一个模型完全可以在你不在的时候把 200 GB 拖下来。所以：`agent_confirm` 开着时，
//! Agent 的「加任务」只换来一张**待确认票据**，真正的任务要等人点头才建。
//!
//! ## 状态机
//!
//! ```text
//! (新) --request--> Pending --approve(task_id)--> Approved
//!                       \--reject--> Rejected
//!                       \--超时----> Expired
//! ```
//!
//! 只往前走的三个终态；`Pending` 之外都不再变化（重复 approve 返回 false，
//! 免得同一张票据建出两个任务）。
//!
//! ## 为什么是"小表"而不是队列
//!
//! 同时挂着的票据最多十几张，线性扫一遍就够了；用 `HashMap` 反而要自己管顺序，
//! 而界面按"谁先来的"展示更自然。

use serde::Serialize;

/// 票据默认多久没处理就作废。5 分钟：够人看见弹窗，也不至于半夜攒一堆。
pub const TICKET_TTL_SECS: u64 = 300;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum State {
    /// 等人在界面上点「允许」。
    PendingApproval,
    /// 已批准，`task_id` 是真的队列 id。
    Approved { task_id: String },
    /// 人点了「拒绝」。
    Rejected,
    /// 超时作废 —— 与"拒绝"分开：一个是我不要，一个是我没看见。
    Expired,
}

impl State {
    /// 终态（不再变化）。
    pub fn is_final(&self) -> bool {
        !matches!(self, State::PendingApproval)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Pending {
    pub id: String,
    pub url: String,
    pub name: String,
    pub referer: Option<String>,
    pub created_at: u64,
    pub state: State,
}

#[derive(Debug, Default)]
pub struct Tickets {
    slots: Vec<Pending>,
    next_id: u64,
}

impl Tickets {
    /// 挂一张待确认票据，返回它的 id。
    ///
    /// id 用**会话内自增**而不是随机串：同一个 GUI 进程里不会撞，而且日志里好对。
    /// （票据本来就只在内存里活着 —— 关掉程序和它的票据一起消失，这是对的：
    /// 一个挂着"允许下载"的票据不该跨重启活下来。）
    pub fn request(&mut self, url: &str, name: &str, referer: Option<String>, now: u64) -> String {
        self.next_id += 1;
        let id = format!("tk{:04}", self.next_id);
        self.slots.push(Pending {
            id: id.clone(),
            url: url.to_string(),
            name: name.to_string(),
            referer,
            created_at: now,
            state: State::PendingApproval,
        });
        id
    }

    /// 批准：记下真正建出来的任务 id。**只有 `Pending` 能批准**。
    pub fn approve(&mut self, id: &str, task_id: &str) -> bool {
        match self.slots.iter_mut().find(|p| p.id == id) {
            Some(p) if !p.state.is_final() => {
                p.state = State::Approved {
                    task_id: task_id.to_string(),
                };
                true
            }
            _ => false,
        }
    }

    /// 拒绝。
    pub fn reject(&mut self, id: &str) -> bool {
        match self.slots.iter_mut().find(|p| p.id == id) {
            Some(p) if !p.state.is_final() => {
                p.state = State::Rejected;
                true
            }
            _ => false,
        }
    }

    pub fn state(&self, id: &str) -> Option<&State> {
        self.slots.iter().find(|p| p.id == id).map(|p| &p.state)
    }

    /// 还等着人处理的那些（界面按这个画）。
    pub fn pending(&self) -> impl Iterator<Item = &Pending> {
        self.slots.iter().filter(|p| !p.state.is_final())
    }

    /// 有没有等着处理的票据（界面据此决定要不要弹确认框）。
    pub fn has_pending(&self) -> bool {
        self.pending().next().is_some()
    }

    /// 把超过 `ttl` 的挂单作废，返回作废了几张。
    pub fn expire(&mut self, now: u64, ttl: u64) -> usize {
        let mut n = 0;
        for p in self.slots.iter_mut() {
            if !p.state.is_final() && now.saturating_sub(p.created_at) >= ttl {
                p.state = State::Expired;
                n += 1;
            }
        }
        n
    }

    /// 清掉已经结案的旧记录（界面确认完之后调用）。
    pub fn prune_final(&mut self) {
        self.slots.retain(|p| !p.state.is_final());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_waits_for_a_human() {
        let mut t = Tickets::default();
        let id = t.request("https://a/x.zip", "x.zip", None, 100);
        assert_eq!(t.state(&id), Some(&State::PendingApproval));
        assert!(t.has_pending());
        assert_eq!(t.pending().count(), 1);
    }

    #[test]
    fn approving_twice_does_not_make_two_tasks() {
        let mut t = Tickets::default();
        let id = t.request("https://a/x.zip", "x.zip", None, 100);
        assert!(t.approve(&id, "task-1"));
        assert!(
            !t.approve(&id, "task-2"),
            "同一张票据只许建一个任务（否则点两下就下两份）"
        );
        assert_eq!(
            t.state(&id),
            Some(&State::Approved {
                task_id: "task-1".to_string()
            })
        );
        assert!(!t.has_pending());
    }

    #[test]
    fn a_rejected_ticket_cannot_be_approved_later() {
        let mut t = Tickets::default();
        let id = t.request("https://a/x.zip", "x.zip", None, 100);
        assert!(t.reject(&id));
        assert!(!t.approve(&id, "task-1"), "拒绝之后不许翻案");
        assert_eq!(t.state(&id), Some(&State::Rejected));
    }

    /// 过期与拒绝分开记：一个是我不要，一个是我没看见。
    #[test]
    fn stale_tickets_expire_instead_of_being_rejected() {
        let mut t = Tickets::default();
        let id = t.request("https://a/x.zip", "x.zip", None, 100);
        assert_eq!(t.expire(100 + TICKET_TTL_SECS - 1, TICKET_TTL_SECS), 0);
        assert_eq!(t.expire(100 + TICKET_TTL_SECS, TICKET_TTL_SECS), 1);
        assert_eq!(t.state(&id), Some(&State::Expired));
        assert!(!t.has_pending());
    }

    #[test]
    fn ids_do_not_collide_within_a_session() {
        let mut t = Tickets::default();
        let a = t.request("https://a/1.zip", "1.zip", None, 0);
        let b = t.request("https://a/2.zip", "2.zip", None, 0);
        assert_ne!(a, b);
        assert!(b.starts_with("tk"));
    }

    #[test]
    fn final_tickets_can_be_pruned() {
        let mut t = Tickets::default();
        let id = t.request("https://a/x.zip", "x.zip", None, 0);
        t.reject(&id);
        t.prune_final();
        assert_eq!(t.state(&id), None, "结案的记录清掉，表不会越攒越长");
    }
}
