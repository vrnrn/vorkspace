//! The action bar beneath a response and its tooltips.
use super::*;

impl TranscriptView {
    pub(super) fn action_hover(
        &mut self,
        turn: usize,
        action: &'static str,
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        if hovered {
            self.hovered_action = Some((turn, action));
        } else if self.hovered_action == Some((turn, action)) {
            self.hovered_action = None;
        }
        cx.notify();
    }
    pub(super) fn response_metrics_button(
        &mut self,
        turn: usize,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let anchor = self
            .action_anchors
            .entry((turn, "metrics"))
            .or_default()
            .clone();
        response_actions::button(
            turn,
            "response-metrics",
            "Turn metrics",
            "response-metrics",
            false,
        )
        .role(Role::Image)
        .debug_selector(move || format!("response-metrics-{turn}"))
        .ml(px(3.))
        .child(anchor.probe())
        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            this.action_hover(turn, "metrics", *hovered, cx)
        }))
    }
    pub(super) fn response_metrics_tooltip(
        &self,
        turn: usize,
        info: &vorkspace_core::agent::TurnInfo,
    ) -> Option<crate::popover::Popover> {
        if self.hovered_action != Some((turn, "metrics")) {
            return None;
        }
        let metrics = info.metrics.filter(|metrics| metrics.reported())?;
        let mut summary = Vec::new();
        if let Some(hit) = metrics.cache_hit_percent.filter(|hit| hit.is_finite()) {
            summary.push(format!("Cache hit {hit:.0}%"));
        }
        if info.duration_ms > 0 && metrics.output_tokens > 0 {
            summary.push(format!(
                "Output {:.0} tok/s",
                metrics.output_tokens as f64 / (info.duration_ms as f64 / 1000.)
            ));
        }
        let headline = if summary.is_empty() {
            "Turn tokens".into()
        } else {
            summary.join(" · ")
        };
        let detail = [
            (metrics.input_tokens, "input"),
            (metrics.output_tokens, "output"),
            (metrics.cache_read_tokens, "cached"),
        ]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, label)| format!("{} {label}", vorkspace_core::context::format_tokens(n)))
        .collect::<Vec<_>>()
        .join(" · ");
        self.response_tooltip(turn, "metrics", headline, Some(detail))
    }
    pub(super) fn response_tooltip(
        &self,
        turn: usize,
        action: &'static str,
        headline: String,
        detail: Option<String>,
    ) -> Option<crate::popover::Popover> {
        let anchor = self.action_anchors.get(&(turn, action))?;
        let headline = gpui::StyledText::new(headline);
        let detail = detail.map(gpui::StyledText::new);
        // Retain the actual laid-out text for rendered hover regressions. The
        // test platform does not activate native accessibility callbacks.
        #[cfg(test)]
        {
            let mut layouts = self.tooltip_text.borrow_mut();
            layouts.clear();
            layouts.push(headline.layout().clone());
            layouts.extend(detail.as_ref().map(|text| text.layout().clone()));
        }
        Some(
            popover(("response-tooltip", turn), Anchor::Slot(anchor.clone()))
                .side(Side::Top)
                .max_width(if action == "metrics" { 300. } else { 240. })
                .padding(8.)
                .child(
                    div()
                        .debug_selector(move || format!("response-tooltip-content-{turn}"))
                        .flex()
                        .flex_col()
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .text_color(content(1.))
                        .child(headline)
                        .when_some(detail, |d, detail| {
                            d.child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(content(0.5))
                                    .child(detail),
                            )
                        }),
                ),
        )
    }
    pub(super) fn response_actions(
        &mut self,
        turn: usize,
        complete: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if !complete {
            // A Bot's turn keeps its 1.5rem action gap while it works, and
            // the 4px gap between turns.
            return div()
                .h(px(if self.bot { 28. } else { 38. }))
                .into_any_element();
        }
        let (rows, _) = self.response_slices(turn);
        let output = vorkspace_core::response::prose(rows);
        if self.bot {
            return self.bot_actions(turn, output, cx);
        }
        let copied = self.copied_response.is_some_and(|(id, _)| id == turn);
        let saved = self.note_feedback.get(&turn).is_some_and(|r| r.is_ok());
        let pending = self.note_pending.contains(&turn);
        let info = self.turns.get(&turn).cloned().unwrap_or_default();
        let metrics = info.metrics.filter(|m| m.reported());
        // Reserve the full icon strip before the sibling clock is laid out.
        // Intrinsic flex measurement otherwise reports only its widest button.
        let strip_width = if output.is_empty() { 14. } else { 44. }
            + if self.side_question { 0. } else { 52. }
            + if metrics.is_some() { 29. } else { 0. };
        let mut actions = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .flex_shrink_0()
            .w(px(strip_width));
        if !output.is_empty() {
            let copy_label = if copied { "Copied" } else { "Copy response" };
            let anchor = self
                .action_anchors
                .entry((turn, "copy"))
                .or_default()
                .clone();
            let text = output.clone();
            actions = actions.child(
                response_actions::button(
                    turn,
                    "response-copy",
                    copy_label,
                    if copied { "check" } else { "response-copy" },
                    false,
                )
                .ml(px(-4.))
                .child(anchor.probe())
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    this.action_hover(turn, "copy", *hovered, cx)
                }))
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
            );
            let anchor = self
                .action_anchors
                .entry((turn, "note"))
                .or_default()
                .clone();
            actions = actions.child(
                response_actions::button(
                    turn,
                    "response-note",
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
                .child(anchor.probe())
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    this.action_hover(turn, "note", *hovered, cx)
                }))
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
            );
        } else {
            actions = actions.child(icon("check", 14.).text_color(content(0.4)));
        }
        for (action, label, glyph, kind) in [
            (
                "handoff",
                "Handoff",
                "response-handoff",
                vorkspace_core::response::TransferKind::Handoff,
            ),
            (
                "opinion",
                "Second opinion",
                "response-opinion",
                vorkspace_core::response::TransferKind::SecondOpinion,
            ),
        ]
        .into_iter()
        .filter(|_| !self.side_question)
        {
            let anchor = self
                .action_anchors
                .entry((turn, action))
                .or_default()
                .clone();
            let click_anchor = anchor.clone();
            let active = self.target_action == Some((turn, kind));
            actions = actions.child(
                response_actions::button(turn, glyph, label, glyph, active)
                    .aria_expanded(active)
                    .child(anchor.probe())
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        this.action_hover(turn, action, *hovered, cx)
                    }))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(ResponseRequest::Target {
                            turn,
                            kind,
                            anchor: click_anchor.clone(),
                        })
                    })),
            );
        }
        if metrics.is_some() {
            actions = actions.child(self.response_metrics_button(turn, cx));
        }
        let dot = || {
            div()
                .size(px(3.))
                .flex_shrink_0()
                .rounded_full()
                .bg(content(0.25))
        };
        let mut row = div()
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(10.))
            .pt(px(4.))
            .pb(px(12.))
            .text_size(px(12.))
            .line_height(px(16.))
            .text_color(content(0.4))
            .child(actions)
            .when_some(info.completed_at, |d, time| {
                d.child(dot()).child(
                    div()
                        .flex_shrink_0()
                        .text_color(content(0.35))
                        .child(vorkspace_core::response::local_clock(time)),
                )
            });
        if let Some(Err(error)) = self.note_feedback.get(&turn) {
            row = row.child(
                div()
                    .id(("note-save-error", turn))
                    .role(Role::Alert)
                    .text_size(px(11.))
                    .child(format!("Could not save note. {error}")),
            );
        }
        if let Some((hover_turn, action)) = self.hovered_action
            && hover_turn == turn
        {
            let headline = match action {
                "copy" => Some(if copied { "Copied" } else { "Copy response" }),
                "note" => Some(if saved {
                    "Saved to Notes"
                } else {
                    "Save as note"
                }),
                "handoff" => Some("Handoff"),
                "opinion" => Some("Second opinion"),
                _ => None,
            };
            row = row.children(if let Some(headline) = headline {
                self.response_tooltip(turn, action, headline.into(), None)
            } else {
                self.response_metrics_tooltip(turn, &info)
            });
        }
        row.into_any_element()
    }
}
