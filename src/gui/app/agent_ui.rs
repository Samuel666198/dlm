//! Agent 请求确认的对话（0.6.0 阶段 5）。
//!
//! 本地控制口挡得住别的机器，挡不住本机的 Agent —— 所以 `agent_confirm` 开着时，
//! Agent 的「加任务」只换来一张票据，**人在这里点头**才真正入队（见 [`crate::gui::tickets`]）。
//!
//! ## 为什么允许/拒绝要立刻反映到票据上
//!
//! Agent 那边是**轮询**票据状态的（`GET /api/agent/ticket`）。点了「允许」之后如果只是
//! 发个 `Add` 就完事，Agent 拿到的永远是 `pending_approval`，它会一直等 —— 或者更糟，
//! 自己重试一遍再挂一张票据。所以：批准时先记下 `PendingApproval → (等回执)`，
//! runner 一回任务 id 就立刻写回 `Approved { task_id }`。

use eframe::egui;

use super::App;
use super::theme;
use crate::gui::Ticket;
use crate::gui::tickets::TICKET_TTL_SECS;

impl App {
    /// 把票据状态与设置里的开关对齐，并清掉过期的挂单。每帧调一次。
    pub(super) fn sync_agent_gate(&mut self) {
        let now = crate::queue::now_secs();
        if let Ok(mut g) = self.agent_gate.lock() {
            g.confirm = self.settings.agent_confirm;
            g.tickets.expire(now, TICKET_TTL_SECS);
        }
        self.drain_ticket_replies();
    }

    /// 收「已批准、正在等 runner 回执」的那些票据的结果。
    fn drain_ticket_replies(&mut self) {
        let mut settled: Vec<(usize, Option<String>)> = Vec::new();
        for (i, (id, rx)) in self.ticket_replies.iter().enumerate() {
            match rx.try_recv() {
                Ok(Ok(task_id)) => settled.push((i, Some(task_id))),
                Ok(Err(reason)) => {
                    eprintln!("dlm-gui: 票据 {} 建任务失败：{reason}", id);
                    settled.push((i, None));
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => settled.push((i, None)),
            }
        }
        if settled.is_empty() {
            return;
        }
        let Ok(mut g) = self.agent_gate.lock() else {
            return;
        };
        for (i, task_id) in &settled {
            let (id, _) = &self.ticket_replies[*i];
            match task_id {
                Some(task_id) => {
                    g.tickets.approve(id, task_id);
                }
                // 建任务失败 ⇒ 当拒绝处理：让 Agent 拿到一个终态，别无限等。
                None => {
                    g.tickets.reject(id);
                }
            }
        }
        drop(g);
        for (i, _) in settled.into_iter().rev() {
            self.ticket_replies.remove(i);
        }
    }

    /// 有待确认的票据时弹框。
    pub(super) fn agent_tickets_dialog(&mut self, ctx: &egui::Context) {
        let pending: Vec<(String, String, String, Option<String>)> = {
            let Ok(g) = self.agent_gate.lock() else {
                return;
            };
            g.tickets
                .pending()
                .map(|p| {
                    (
                        p.id.clone(),
                        p.name.clone(),
                        p.url.clone(),
                        p.referer.clone(),
                    )
                })
                .collect()
        };
        if pending.is_empty() {
            return;
        }

        let mut approve: Option<(String, String, Option<String>)> = None;
        let mut reject: Option<String> = None;

        egui::Window::new("Agent 请求下载")
            .resizable(false)
            .collapsible(false)
            .default_width(440.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("有 Agent 想往队列里加任务。点「允许」才会真的开始下载。")
                        .size(11.5)
                        .color(theme::INK_2),
                );
                ui.add_space(8.0);
                for (id, name, url, _referer) in &pending {
                    egui::Frame::new()
                        .fill(theme::SURFACE_2)
                        .inner_margin(egui::Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(name).size(12.5).color(theme::INK));
                            ui.label(
                                egui::RichText::new(url)
                                    .size(10.0)
                                    .monospace()
                                    .color(theme::INK_3),
                            );
                            ui.horizontal(|ui| {
                                if ui.button("允许").clicked() {
                                    approve = Some((id.clone(), name.clone(), None));
                                }
                                if ui.button("拒绝").clicked() {
                                    reject = Some(id.clone());
                                }
                                ui.label(
                                    egui::RichText::new(format!("票据 {id}"))
                                        .size(9.5)
                                        .color(theme::INK_3),
                                );
                            });
                        });
                    ui.add_space(6.0);
                }
                ui.label(
                    egui::RichText::new(format!(
                        "{} 秒内不处理就作废（与「拒绝」分开记：一个是不要，一个是没看见）",
                        TICKET_TTL_SECS
                    ))
                    .size(10.0)
                    .color(theme::INK_3),
                );
            });

        if let Some(id) = reject
            && let Ok(mut g) = self.agent_gate.lock()
        {
            g.tickets.reject(&id);
        }
        if let Some((id, name, referer)) = approve {
            let (reply, reply_rx) = std::sync::mpsc::channel();
            self.send(Ticket::Add {
                url: pending
                    .iter()
                    .find(|(pid, _, _, _)| *pid == id)
                    .map(|(_, _, url, _)| url.clone())
                    .unwrap_or_default(),
                name,
                referer,
                start_at: None,
                on_complete: None,
                reply,
            });
            // 回执到了才把票据置成 Approved（见模块注释：Agent 在轮询这个状态）。
            self.ticket_replies.push((id, reply_rx));
        }
    }
}
