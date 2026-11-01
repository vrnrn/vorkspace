//! Rows for queued prompts, launches and the review card.
use super::*;

impl TranscriptView {
    /// Shows queued messages as bubbles at the end of the chat.
    pub(crate) fn set_pending(&mut self, pending: Vec<PendingMessage>, cx: &mut Context<Self>) {
        if self.pending == pending {
            return;
        }
        let now = Instant::now();
        let animate = self.visible;
        for old in &self.pending {
            if !pending.iter().any(|p| p.id == old.id) {
                self.taken.push(TakenMessage {
                    text: old.text.clone(),
                    removed_at: now,
                    entrance: self.pending_entrances.get(&old.id).copied(),
                });
            }
        }
        self.taken
            .retain(|message| now.duration_since(message.removed_at).as_secs_f32() < 2.);
        let added: Vec<_> = pending
            .iter()
            .filter(|p| !self.pending.iter().any(|o| o.id == p.id))
            .map(|p| p.id.clone())
            .collect();
        self.pending = pending;
        if animate {
            for id in added {
                if let Some(ix) = self.pending.iter().position(|p| p.id == id) {
                    self.pending_entrances
                        .insert(self.pending[ix].id.clone(), now);
                }
            }
            self.follow.until = Some(now + std::time::Duration::from_millis(1200));
        }
        let from = self.source.iter().rposition(|r| r.is_prompt()).unwrap_or(0);
        if self.source.is_empty() {
            self.rows.clear();
            self.messages.clear();
            self.push_pending(0);
            self.list.reset(self.rows.len() + 1);
            if self.list.is_following_tail() || self.rows.len() <= self.pending.len() {
                self.list.scroll_to_end();
            }
        } else {
            self.rebuild(from, cx);
        }
        cx.notify();
    }
    pub(super) fn push_pending(&mut self, at: usize) {
        for i in 0..self.pending.len() {
            self.rows.push(UiRow {
                key: (at, 100_000 + i),
                body: Body::Pending(i),
                message: usize::MAX,
                first: false,
                last: false,
                work: false,
                work_tail: false,
                thought_phase: None,
            });
        }
    }
    /// botSpawnedSessions: sessions a run's `sessions.start` calls accepted.
    pub(super) fn run_launches(&self, turn: usize, end: usize) -> Vec<i64> {
        let mut found = Vec::new();
        let mut at = turn;
        while at < end {
            let next = (at + 1..end).find(|&i| self.source[i].first).unwrap_or(end);
            if self.source[at].role == "bot-launch" {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&self.source[at].text)
                    && let Some(id) = cards::session(&value["sessionId"])
                    && !found.contains(&id)
                {
                    found.push(id);
                }
            } else if self.source[at].role == "tool" {
                let text: String = self.source[at..next]
                    .iter()
                    .map(|r| r.text.as_str())
                    .collect();
                if let Ok(tool) = serde_json::from_str::<ToolCall>(&text)
                    && tool.state == ToolState::Completed
                    && let Ok(input) = serde_json::from_str::<serde_json::Value>(&tool.input)
                    && let Some(action) = input["command"]
                        .as_str()
                        .and_then(vorkspace_core::bot::work::command_action)
                    && matches!(action.as_str(), "sessions.start" | "sessions.send")
                {
                    let receipts = serde_json::from_str::<serde_json::Value>(&tool.output)
                        .map(|v| vec![v])
                        .unwrap_or_else(|_| {
                            tool.output
                                .lines()
                                .filter_map(|line| serde_json::from_str(line.trim()).ok())
                                .collect()
                        });
                    for value in receipts {
                        if value["ok"] == true
                            && let Some(id) = cards::session(&value["result"]["sessionId"])
                                .or_else(|| cards::session(&value["result"]["id"]))
                            && (action == "sessions.start" || self.launches.contains_key(&id))
                            && !found.contains(&id)
                        {
                            found.push(id);
                        }
                    }
                }
            }
            at = next;
        }
        found
    }
    /// The live state of the sessions this Bot launched, for their cards.
    pub(crate) fn set_launches(
        &mut self,
        launches: HashMap<i64, LaunchView>,
        cx: &mut Context<Self>,
    ) {
        if self.launches != launches {
            let structure_changed = self.launches.len() != launches.len()
                || self.launches.keys().any(|id| !launches.contains_key(id));
            self.launches = launches;
            if structure_changed {
                self.rebuild(0, cx);
            } else {
                cx.notify();
            }
        }
    }
    pub(super) fn launch_row(
        &mut self,
        ix: usize,
        id: i64,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let card_after = self
            .rows
            .get(ix + 1)
            .is_some_and(|r| matches!(r.body, Body::Launch(_) | Body::BotCard(_)));
        let first =
            ix == 0 || !matches!(self.rows[ix - 1].body, Body::Launch(_) | Body::BotCard(_));
        let view = self.launches.get(&id).cloned();
        let spin = self.started_at.elapsed().as_secs_f32();
        div()
            .w_full()
            .flex()
            .justify_center()
            .child(
                div()
                    .relative()
                    .w_full()
                    .max_w(px(COLUMN))
                    .px_4()
                    .pt(px(if first { 4. } else { 0. }))
                    .pb(px(if card_after { 8. } else { 12. }))
                    .child(bot_cards::launch(id, view.as_ref(), spin, cx)),
            )
            .into_any_element()
    }
    /// The session's review card, under the latest settled turn only.
    pub(crate) fn set_review(&mut self, review: gpui::AnyView) {
        self.review = Some(review);
    }
    /// The card gained or lost its content; the latest turn makes room for it.
    pub(crate) fn set_review_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        if self.review_shown == shown {
            return;
        }
        self.review_shown = shown;
        if !self.messages.is_empty() {
            let start = self
                .source
                .iter()
                .rposition(|r| r.is_prompt())
                .map_or(0, |i| i + 1);
            self.rebuild(start, cx);
        }
        cx.notify();
    }
    pub(super) fn review_row(&self, turn: usize) -> gpui::AnyElement {
        let latest = self
            .source
            .iter()
            .rposition(|r| r.is_prompt())
            .map_or(0, |i| i + 1);
        let Some(review) = self
            .review
            .clone()
            .filter(|_| !self.streaming && turn == latest)
        else {
            return div().into_any_element();
        };
        div()
            .w_full()
            .flex()
            .justify_center()
            .child(
                div()
                    .relative()
                    .w_full()
                    .max_w(px(COLUMN))
                    .px_4()
                    .pt_1()
                    .pb_2()
                    .child(review),
            )
            .into_any_element()
    }
    /// A queued message's bubble, as UserMessageBlock draws one, with its
    /// delivery line when it could not be sent.
    pub(super) fn pending_row(&mut self, ix: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(message) = self.pending.get(ix).cloned() else {
            return div().into_any_element();
        };
        let stacked = self
            .pending
            .get(ix + 1)
            .is_some_and(|next| !dates::opens_stretch(next.sent_at, message.sent_at));
        let (mut opacity, mut rise) = (1., 0.);
        if let Some(born) = self.pending_entrances.get(&message.id) {
            let t = vorkspace_ui::reveal::ease_move((born.elapsed().as_secs_f32() / 0.36).min(1.));
            opacity = t;
            rise = 16. * (1. - t);
        }
        let retry = message.id.clone();
        let discard = message.id.clone();
        let pending_selector = format!("bot-pending-{}", message.id);
        let retry_selector = format!("bot-pending-retry-{retry}");
        let discard_selector = format!("bot-pending-discard-{discard}");
        let attachments = attachments::Attachment::from_paths(&message.attachments);
        let attachment_body = (!attachments.is_empty())
            .then(|| self.attachment_row(&attachments, &format!("pending-{}", message.id), cx));
        let peer_pending = message.peer.is_some();
        div()
            .id(SharedString::from(format!("bot-pending-{}", message.id)))
            .debug_selector(|| "bot-pending-message".into())
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .opacity(opacity)
            .children(self.pending_separator(ix))
            .child(
                div()
                    .debug_selector(move || pending_selector.clone())
                    .relative()
                    .top(px(rise))
                    .w_full()
                    .max_w(px(COLUMN))
                    .min_w_0()
                    .pl(px(if peer_pending { 32. } else { 72. }))
                    .pr(px(32.))
                    .pt_1()
                    .pb(px(if stacked { 0. } else { 20. }))
                    .flex()
                    .flex_col()
                    .items_end()
                    .when(peer_pending, |d| d.items_start())
                    .text_size(px(14.))
                    .line_height(px(20.))
                    .children(
                        message
                            .peer
                            .as_ref()
                            .map(|peer| self.peer_message_label(peer, message.error.is_none())),
                    )
                    .child(
                        div()
                            .relative()
                            .max_w(px(576.))
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .px_3()
                            .py_2()
                            .child(
                                canvas(
                                    |_, _, _| {},
                                    move |bounds, _, window, _| {
                                        let radius =
                                            if !peer_pending && bounds.size.height <= px(37.) {
                                                f32::from(bounds.size.height) * 0.5
                                            } else {
                                                12.
                                            };
                                        window.paint_quad(
                                            gpui::fill(bounds, user_bubble_fill())
                                                .corner_radii(px(radius)),
                                        );
                                    },
                                )
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full(),
                            )
                            .children(attachment_body.map(|body| {
                                div()
                                    .when(!message.text.is_empty(), |d| d.mb_2())
                                    .child(body)
                            }))
                            .when(!message.text.is_empty(), |d| d.child(message.text.clone())),
                    )
                    .when_some(message.error.clone(), |d, error| {
                        d.child(
                            div()
                                .w_full()
                                .min_w_0()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap_2()
                                .px_3()
                                .pt_1()
                                .text_size(px(12.))
                                .line_height(px(16.))
                                .text_color(content(0.5))
                                .child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "bot-pending-error-{}",
                                            message.id
                                        )))
                                        .min_w_0()
                                        .max_w_full()
                                        .role(Role::Alert)
                                        .child(format!("Not sent · {error}")),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "bot-pending-retry-{retry}"
                                        )))
                                        .debug_selector(move || retry_selector.clone())
                                        .flex_shrink_0()
                                        .role(Role::Button)
                                        .rounded(px(4.))
                                        .px_1()
                                        .py(px(2.))
                                        .text_color(content(0.7))
                                        .hover(|s| s.bg(content(0.08)))
                                        .child("Retry")
                                        .on_click(cx.listener(move |_, _, _, cx| {
                                            cx.emit(BotResponseAction::Queue {
                                                id: retry.clone(),
                                                remove: false,
                                            })
                                        })),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "bot-pending-discard-{discard}"
                                        )))
                                        .debug_selector(move || discard_selector.clone())
                                        .flex_shrink_0()
                                        .role(Role::Button)
                                        .rounded(px(4.))
                                        .px_1()
                                        .py(px(2.))
                                        .text_color(content(0.7))
                                        .hover(|s| s.bg(content(0.08)))
                                        .child("Discard")
                                        .on_click(cx.listener(move |_, _, _, cx| {
                                            cx.emit(BotResponseAction::Queue {
                                                id: discard.clone(),
                                                remove: true,
                                            })
                                        })),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }
    /// isCompactFollowUp: the next message is the user's too, directly or
    /// past a reply that was only a reaction, so it stacks onto this one.
    pub(super) fn stacks_on_next(&self, start: usize) -> bool {
        let mut at = start + 1;
        while at < self.source.len() && !self.source[at].first {
            at += 1;
        }
        while let Some(row) = self.source.get(at) {
            if row.is_prompt() {
                return self.peer_at(at).is_none() && !self.bot_opens_stretch(at);
            }
            let end = (at + 1..self.source.len())
                .find(|&i| self.source[i].first)
                .unwrap_or(self.source.len());
            let text: String = self.source[at..end]
                .iter()
                .map(|r| r.text.as_str())
                .collect();
            if !(row.role == "assistant" && text.trim().starts_with("<reaction>")) {
                return false;
            }
            at = end;
        }
        false
    }
}
