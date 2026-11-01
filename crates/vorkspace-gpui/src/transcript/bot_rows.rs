//! Bot turn headers, reactions and prompt actions.
use super::*;

impl TranscriptView {
    /// TurnDuration with `revealOnHover`: copy, save as note and quote in
    /// the turn's fixed gap, then its time, shown while the turn is hovered.
    pub(super) fn bot_actions(
        &mut self,
        turn: usize,
        output: String,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let copied = self.copied_response.is_some_and(|(id, _)| id == turn);
        let saved = self.note_feedback.get(&turn).is_some_and(|r| r.is_ok());
        let pending = self.note_pending.contains(&turn);
        let opacity = match self.hovered_turn {
            Some((hovered, since)) if hovered == turn => {
                (since.elapsed().as_secs_f32() / HOVER_FADE).min(1.)
            }
            _ if copied || pending => 1.,
            _ => 0.,
        };
        // Group identity owns the actions, but accounting belongs to one
        // provider run. Select from represented messages before looking up its
        // metadata: a legacy continuation must not inherit an older receipt.
        let info = self
            .messages
            .iter()
            .rev()
            .find(|message| {
                message.turn == turn && !matches!(message.role.as_str(), "user" | "bot-system")
            })
            .and_then(|message| self.turns.get(&message.run))
            .cloned()
            .unwrap_or_default();
        let metrics = info.metrics.filter(|metrics| metrics.reported());
        let dot = || {
            div()
                .size(px(3.))
                .flex_shrink_0()
                .rounded_full()
                .bg(content(0.25))
        };
        let empty = output.is_empty();
        let mut actions = div().flex().flex_shrink_0().items_center().gap_1();
        if empty {
            actions = actions.child(icon("check", 14.).text_color(content(0.4)));
        } else {
            let text = output.clone();
            let quote = output.clone();
            actions = actions
                .child(
                    response_actions::button(
                        turn,
                        "bot-copy-response",
                        if copied { "Copied" } else { "Copy response" },
                        if copied { "check" } else { "response-copy" },
                        false,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                        let born = Instant::now();
                        this.copied_response = Some((turn, born));
                        cx.notify();
                        cx.spawn(async move |this, cx| {
                            cx.background_executor()
                                .timer(std::time::Duration::from_secs(2))
                                .await;
                            let _ = this.update(cx, |this, cx| {
                                if this.copied_response == Some((turn, born)) {
                                    this.copied_response = None;
                                    cx.notify();
                                }
                            });
                        })
                        .detach();
                    })),
                )
                .child(
                    response_actions::button(
                        turn,
                        "bot-note-response",
                        if saved {
                            "Saved to Notes"
                        } else if pending {
                            "Saving note…"
                        } else {
                            "Save as note"
                        },
                        if saved { "check" } else { "response-note" },
                        false,
                    )
                    .when(pending, |d| d.opacity(0.4))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.note_pending.insert(turn) {
                            this.note_feedback.remove(&turn);
                            cx.emit(ResponseRequest::SaveNote {
                                turn,
                                text: output.clone(),
                                selection: false,
                            });
                            cx.notify();
                        }
                    })),
                )
                .child(
                    response_actions::button(
                        turn,
                        "bot-quote-response",
                        "Quote response",
                        "comment-add",
                        false,
                    )
                    .on_click(cx.listener(move |_, _, window, cx| {
                        cx.emit(BotResponseAction::Quote {
                            text: quote.clone(),
                            window: window.window_handle(),
                        })
                    })),
                );
        }
        if metrics.is_some() {
            // Reserve the complete strip so the sibling clock cannot overlap
            // the buttons during intrinsic flex measurement.
            actions = actions
                .w(px(if empty { 14. } else { 74. } + 29.))
                .child(self.response_metrics_button(turn, cx));
        }
        div()
            .id(("bot-turn-actions", turn))
            .h(px(24.))
            .mb_1()
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(10.))
            .overflow_hidden()
            .text_size(px(14.))
            .text_color(content(0.4))
            .opacity(opacity)
            .child(actions)
            .when_some(info.completed_at, |d, time| {
                d.child(dot()).child(
                    div()
                        .flex_shrink_0()
                        .text_color(content(0.35))
                        .child(vorkspace_core::response::local_clock(time)),
                )
            })
            .children(self.response_metrics_tooltip(turn, &info))
            .into_any_element()
    }
    /// The Bot signs a group of turns; each run owns its inline work receipt.
    pub(super) fn bot_turn_header(&self, turn: usize) -> gpui::AnyElement {
        let Some((look, name)) = self.bot_look.clone() else {
            return div().into_any_element();
        };
        let pill = div()
            .debug_selector(|| "bot-signature-pill".into())
            .flex()
            .min_w_0()
            .flex_shrink_0()
            .max_w(px(320.))
            .items_center()
            .gap_1p5()
            .rounded_full()
            .bg(content(0.07))
            .py(px(2.))
            .pl_2()
            .pr_3()
            .child(div().size(px(14.)).flex_shrink_0().child(
                crate::bots::avatar::avatar_with_look(look, "idle", 14., true),
            ))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(content(0.8))
                    .child(name),
            );
        div()
            .id(("bot-turn-header", turn))
            .flex()
            .min_w_0()
            .items_center()
            .gap_1p5()
            .py_1()
            .text_size(px(14.))
            .line_height(px(20.))
            .text_color(content(0.5))
            .child(pill)
            .children(self.peer_response_label(turn))
            .into_any_element()
    }
    /// BotReactionBadge, on the top left corner of the message it answers.
    pub(super) fn reaction_badge(&self, prompt: usize) -> Option<gpui::AnyElement> {
        let (emoji, born) = self.reactions.get(&prompt)?;
        // agent-chat-bubble-in: it lands with a small bounce.
        let (scale, opacity) = born.map_or((1., 1.), |born| {
            let t = (born.elapsed().as_secs_f32() / 0.32).min(1.);
            (
                0.9 + 0.1 * vorkspace_ui::reveal::bezier(t, 0.2, 1.4, 0.4, 1.),
                (t / 0.4).min(1.),
            )
        });
        let height = 22. * scale;
        Some(
            div()
                .id(("bot-reaction", prompt))
                .role(Role::Image)
                .absolute()
                .top(px(-10. - (height - 22.) / 2.))
                .left(px(-10.))
                .h(px(height))
                .px(px(7. * scale))
                .flex()
                .items_center()
                .rounded(px(8. * scale))
                .bg(content(0.17))
                .border_1()
                .border_color(content(0.1))
                .text_size(px(11. * scale))
                .line_height(px(11. * scale))
                .opacity(opacity)
                .aria_label(format!("Reacted {emoji}"))
                .child(emoji.clone())
                .into_any_element(),
        )
    }
    /// The sent message's actions, under its bubble while it is hovered.
    pub(super) fn prompt_actions(
        &mut self,
        start: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let end = (start + 1..self.source.len())
            .find(|&i| self.source[i].first)
            .unwrap_or(self.source.len());
        let text: String = self.source[start..end]
            .iter()
            .map(|r| r.text.as_str())
            .collect();
        let copied = self.copied.is_some_and(|id| id == start as u64);
        let sent_at = self.source[start].sent_at;
        let stamp_anchor = self
            .action_anchors
            .entry((start, "sent-at"))
            .or_default()
            .clone();
        div()
            .id(("prompt-actions", start))
            .absolute()
            .top_full()
            .right_0()
            .pt_1()
            .px_3()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.hovered_prompt_actions = Some(start);
                } else if this.hovered_prompt_actions == Some(start) {
                    this.hovered_prompt_actions = None;
                }
                cx.notify();
            }))
            .flex()
            .items_center()
            .gap_1()
            .child(
                response_actions::button(
                    start,
                    "bot-copy-message",
                    if copied { "Copied" } else { "Copy message" },
                    if copied { "check" } else { "response-copy" },
                    false,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                    this.copied = Some(start as u64);
                    cx.notify();
                })),
            )
            .when(
                !self.bot
                    && !self.side_question
                    && !self.streaming
                    && vorkspace_core::provider::supports_rewind(
                        vorkspace_core::provider::Kind::from_id(&self.provider),
                    )
                    && self
                        .source
                        .iter()
                        .rposition(|row| row.first && row.role == "user")
                        == Some(start),
                |d| {
                    d.child(
                        response_actions::button(
                            start,
                            "edit-last-message",
                            "Edit last message",
                            "pencil",
                            false,
                        )
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(ResponseRequest::EditLastTurn { turn: start })
                        })),
                    )
                },
            )
            .when_some(sent_at, |d, at| {
                let label = dates::full_stamp(at);
                d.child(
                    div()
                        .id(("message-sent-at", start))
                        .debug_selector(|| "message-sent-at".into())
                        .ml_1()
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .text_color(content(0.4))
                        .aria_label(label.clone())
                        .child(stamp_anchor.probe())
                        .child(vorkspace_core::response::local_clock(at))
                        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            this.action_hover(start, "sent-at", *hovered, cx)
                        })),
                )
                .when(self.hovered_action == Some((start, "sent-at")), |d| {
                    d.child(
                        popover(
                            ("message-time-tooltip", start),
                            Anchor::Slot(stamp_anchor.clone()),
                        )
                        .side(Side::Top)
                        .padding(8.)
                        .child(div().text_size(px(12.)).child(label)),
                    )
                })
            })
            .into_any_element()
    }
    /// `.transcript-turn:hover` shows the reply's actions; hovering a prompt
    /// shows only its own.
    pub(super) fn bot_hover(
        &mut self,
        prompt: bool,
        start: usize,
        turn: usize,
        key: (usize, usize),
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        if prompt {
            if hovered {
                self.hovered_prompt = Some(start);
            } else if self.hovered_prompt == Some(start) {
                self.hovered_prompt = None;
            }
        } else if hovered {
            self.hover_rows.insert(key, turn);
        } else {
            self.hover_rows.remove(&key);
        }
        let current = if !prompt && hovered {
            Some(turn)
        } else {
            self.hovered_turn
                .map(|(t, _)| t)
                .filter(|t| self.hover_rows.values().any(|v| v == t))
                .or_else(|| self.hover_rows.values().next().copied())
        };
        if current != self.hovered_turn.map(|(t, _)| t) {
            self.hovered_turn = current.map(|t| (t, Instant::now()));
        }
        cx.notify();
    }
    /// Whether `at` sits in a completion run that continues the turn above,
    /// returning the turn that opens the group: groupBotChatTurns.
    pub(super) fn bot_group_start(&self, mut turn: usize) -> usize {
        let mut visited = Vec::new();
        let group = loop {
            if let Some(group) = self.bot_groups.borrow().get(&turn).copied() {
                break group;
            }
            visited.push(turn);
            match bot_continues(&self.source, turn) {
                Some(previous) => turn = previous,
                None => break turn,
            }
        };
        let mut groups = self.bot_groups.borrow_mut();
        for turn in visited {
            groups.insert(turn, group);
        }
        group
    }
}
