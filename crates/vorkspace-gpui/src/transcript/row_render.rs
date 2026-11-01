//! Rendering one presentation row, including code blocks and activity phases.
use super::*;

impl TranscriptView {
    pub(super) fn highlight(&mut self, fence: &Arc<Fence>, cx: &mut Context<Self>) {
        if self.highlights.contains_key(&fence.key) || self.highlighting.contains(&fence.key) {
            return;
        }
        // Replace an obsolete queued version of a growing fence before it starts.
        self.highlight_queue.retain(|queued| {
            let obsolete =
                fence.language == queued.language && fence.code.starts_with(queued.code.as_ref());
            if obsolete {
                self.highlighting.remove(&queued.key);
            }
            !obsolete
        });
        if self.highlight_queue.len() >= 8 {
            return;
        }
        self.highlighting.insert(fence.key);
        self.highlight_queue.push_back(fence.clone());
        self.pump_highlights(cx);
    }
    pub(super) fn pump_highlights(&mut self, cx: &mut Context<Self>) {
        if self.highlight_active {
            return;
        }
        let Some(fence) = self.highlight_queue.pop_front() else {
            return;
        };
        self.highlight_active = true;
        let epoch = self.highlight_epoch;
        let key = fence.key;
        let bytes = fence.code.len();
        let code = fence.code.to_string();
        let language = fence.language.clone();
        let task = cx.background_executor().spawn(async move {
            crate::syntax::highlight(HighlightRequest {
                source: &code,
                path: None,
                fence_tag: Some(&language),
            })
            .ok()
            .map(Arc::new)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.highlight_epoch != epoch {
                    return;
                }
                this.highlighting.remove(&key);
                this.highlight_active = false;
                let bytes = if result.is_some() { bytes } else { 0 };
                while this.highlight_order.len() >= 100 || this.highlight_bytes + bytes > 500_000 {
                    let Some((key, bytes)) = this.highlight_order.pop_front() else {
                        break;
                    };
                    this.highlights.remove(&key);
                    this.highlight_bytes -= bytes;
                }
                // Oversized fences stay readable without retaining their tokens.
                this.highlight_order.push_back((key, bytes.min(500_000)));
                this.highlight_bytes += bytes.min(500_000);
                this.highlights
                    .insert(key, if bytes <= 500_000 { result } else { None });
                this.pump_highlights(cx);
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn code(
        &mut self,
        fence: Arc<Fence>,
        lines: std::ops::Range<usize>,
        ix: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.highlight(&fence, cx);
        let first = lines.start == 0;
        let last = lines.end == fence.lines.len();
        let key = fence.key;
        let code = fence.code.clone();
        let copied = self.copied == Some(key);
        let file_link = fence.path.as_ref().map(|raw| {
            let (path, location) =
                crate::file_link::file_location(raw, self.file_link_base.as_deref().unwrap_or(""));
            let (line, column) = location.unwrap_or((fence.start_line, 1));
            format!("{path}:{line}:{column}")
        });
        div()
            .relative()
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .rounded_t(px(if first { 10. } else { 0. }))
            .rounded_b(px(if last { 10. } else { 0. }))
            .border_x_1()
            .when(first, |d| d.border_t_1())
            .when(last, |d| d.border_b_1())
            .border_color(content(0.1))
            .bg(content(0.06))
            .when(first, |d| {
                d.child(
                    div()
                        .when(!fence.label.is_empty(), |d| d.min_h(px(36.)))
                        .when(fence.label.is_empty(), |d| {
                            d.absolute().top(px(6.)).right(px(6.))
                        })
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(px(12.))
                        .text_color(content(0.65))
                        .when(!fence.label.is_empty(), |d| {
                            let extension = match fence.language.as_str() {
                                "rust" => "rs",
                                "javascript" => "js",
                                "typescript" => "ts",
                                "python" => "py",
                                "bash" | "shell" => "sh",
                                "markdown" => "md",
                                other => other,
                            };
                            let file = if fence.label.contains('.') {
                                fence.label.clone()
                            } else {
                                format!("code.{extension}")
                            };
                            d.child(crate::activity::file_icon(&file, 16.))
                        })
                        .child(
                            div()
                                .id(("code-file-link", ix))
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .when_some(file_link, |d, path| {
                                    let menu_path = path.clone();
                                    d.role(Role::Link)
                                        .cursor_pointer()
                                        .hover(|s| s.text_color(rgb(0x7dd3fc)))
                                        .on_click(move |event, window, cx| {
                                            if event.click_count() == 1 {
                                                window.dispatch_action(
                                                    Box::new(crate::file_link::OpenPath(
                                                        path.clone(),
                                                    )),
                                                    cx,
                                                );
                                            }
                                        })
                                        .on_mouse_down(
                                            gpui::MouseButton::Right,
                                            move |event, window, cx| {
                                                markdown::link_menu(
                                                    &menu_path,
                                                    event.position,
                                                    window,
                                                    cx,
                                                );
                                                cx.stop_propagation();
                                            },
                                        )
                                })
                                .child(fence.label.clone()),
                        )
                        .child(
                            icon_button(
                                "copy-code",
                                if copied { "Copied" } else { "Copy code" },
                                if copied { "check" } else { "copy" },
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        code.strip_suffix("\r\n")
                                            .or_else(|| code.strip_suffix('\n'))
                                            .unwrap_or(&code)
                                            .to_owned(),
                                    ));
                                    this.copied = Some(key);
                                    cx.notify();
                                    cx.spawn(async move |this, cx| {
                                        cx.background_executor()
                                            .timer(std::time::Duration::from_millis(1500))
                                            .await;
                                        let _ = this.update(cx, |this, cx| {
                                            if this.copied == Some(key) {
                                                this.copied = None;
                                                cx.notify();
                                            }
                                        });
                                    })
                                    .detach();
                                },
                            )),
                        ),
                )
            })
            .child(
                div()
                    .id(("code-scroll", ix))
                    .overflow_x_scroll()
                    .w_full()
                    .py(px(10.))
                    .pr_2()
                    .when(first && !fence.label.is_empty(), |d| {
                        d.border_t_1().border_color(content(0.1))
                    })
                    .child(markdown::code_lines_selected(
                        &fence,
                        lines,
                        self.highlights.get(&key).and_then(|v| v.as_deref()),
                        self.search_match.as_ref().map_or("", |(_, q)| q.as_str()),
                        self.search_match
                            .as_ref()
                            .is_some_and(|(start, _)| *start == self.rows[ix].key.0),
                        self.markdown_selection(
                            self.rows[ix].key,
                            self.messages[self.rows[ix].message].start,
                            self.messages[self.rows[ix].message].role == "assistant",
                            cx,
                        ),
                    )),
            )
            .into_any_element()
    }
    pub(super) fn row(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.rendered += 1;
        if let Some(group) = self.rows[ix].phase_group()
            && self.windowed_phase(group)
        {
            // Keep source/list indices stable for message anchors, selection
            // and reconciliation. The phase header owns these rows' layout
            // while live; settled, opened history is virtualized normally.
            return div().h_0().into_any_element();
        }
        if let Body::Phase { group, active, .. } = self.rows[ix].body
            && self.windowed_phase(group)
        {
            return self.phase_window(ix, group, active, window, cx);
        }
        match self.rows[ix].body {
            Body::OlderHistory => return self.older_history_row(cx),
            Body::SavedDraft => return self.saved_draft_row(cx),
            Body::Pending(i) => return self.pending_row(i, cx),
            Body::Launch(id) => return self.launch_row(ix, id, cx),
            Body::Review { turn } => return self.review_row(turn),
            _ => {}
        }
        let group = self.motions.get(&self.rows[ix].key).and_then(|m| m.group);
        if let Some(group) = group
            && self
                .phase_layouts
                .get(&group)
                .is_none_or(|layout| layout.width != self.viewport.size.width)
        {
            // Measure once per content/width change while the body moves.
            // Retain individual list items so settled history stays virtualized.
            let indices: Vec<_> = self
                .rows
                .iter()
                .enumerate()
                .filter(|(_, row)| {
                    self.motions
                        .get(&row.key)
                        .is_some_and(|m| m.group == Some(group))
                })
                .map(|(ix, _)| ix)
                .collect();
            let mut layout = PhaseLayout {
                width: self.viewport.size.width,
                height: 0.,
                rows: HashMap::new(),
            };
            for ix in indices {
                let natural = self.row_body(ix, false, window, cx).layout_as_root(
                    size(
                        AvailableSpace::Definite(layout.width),
                        AvailableSpace::MinContent,
                    ),
                    window,
                    cx,
                );
                let height = f32::from(natural.height);
                layout
                    .rows
                    .insert(self.rows[ix].key, (layout.height, height));
                layout.height += height;
            }
            self.phase_layouts.insert(group, layout);
        }
        self.row_body(ix, true, window, cx)
    }
    pub(super) fn windowed_phase(&self, group: usize) -> bool {
        self.windowed_phases.contains(&group)
    }
    pub(super) fn phase_window(
        &mut self,
        ix: usize,
        group: usize,
        active: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let header = self.row_body(ix, true, window, cx);
        let open = self.phases.get(&group).copied().unwrap_or(active);
        let indices: Vec<_> = (ix + 1..self.rows.len())
            .take_while(|&i| self.rows[i].phase_group() == Some(group))
            .collect();
        let progress = indices
            .iter()
            .find_map(|&i| {
                self.motions
                    .get(&self.rows[i].key)
                    .filter(|m| m.group == Some(group))
                    .map(|m| m.value(self.frame_now))
            })
            .unwrap_or((1., 1.));
        let mut steps = div().w_full().min_w_0().flex().flex_col();
        for i in indices {
            steps = steps.child(self.row_body(i, true, window, cx));
        }
        let body = live_phase::live_phase(
            steps.into_any_element(),
            self.phase_scrolls.entry(group).or_default(),
            live_phase::max_height(window),
            active && open && self.visible,
            cx,
        );
        div()
            .id(("phase-window", group))
            .w_full()
            .flex()
            .flex_col()
            .child(header)
            .child(
                div()
                    .w_full()
                    .overflow_hidden()
                    .opacity(progress.1)
                    .child(Disclosure::new(body, self.viewport.size.width, progress.0)),
            )
            .into_any_element()
    }
    pub(super) fn row_body(
        &mut self,
        ix: usize,
        animate: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let row = self.rows[ix].clone();
        let message = &self.messages[row.message];
        let user = message.role == "user";
        let stacked = user && self.bot && self.stacks_on_next(message.start);
        let message_turn = message.turn;
        let error = message.role == "error";
        let reasoning = message.role == "reasoning";
        let message_start = message.start;
        let first = row.first;
        let last = row.last;
        // The final answer is under the receipt even while its expanded work
        // is leaving. Its padding must not change when those rows are removed.
        let after_receipt = message.under_line
            || (ix > 0 && matches!(self.rows[ix - 1].body, Body::WorkLine { .. }));
        let first_prose = row.key.1 == 16;
        let nested_phase = row.in_phase();
        let thought_header = matches!(row.body, Body::Reasoning);
        let attachments = message.attachments.clone();
        let peer = message.peer.clone();
        let attachment_body = (user && first && !attachments.is_empty())
            .then(|| self.attachment_row(&attachments, &format!("sent-{message_start}"), cx));
        let step_opacity = self
            .motions
            .get(&row.key)
            .filter(|m| m.step)
            .map_or(1., |m| m.value(self.frame_now).1);
        let is_tool = matches!(
            row.body,
            Body::Tool { .. } | Body::Phase { .. } | Body::Reasoning
        );
        let look = row_bodies::MessageLook {
            user,
            stacked,
            error,
            reasoning,
            message_start,
            first,
            last,
            after_receipt,
            first_prose,
        };
        let body = match row.body {
            Body::RemotePlan(plan) => self.remote_plan_card(&plan, cx),
            Body::Visual(document) => self.visual_row(message_start, document, cx),
            Body::Interjection(ref note) => self.interjection_row(note, message_start, cx),
            Body::BotCard(ref value) => {
                if value["type"] == "session"
                    && let Some(id) = cards::session(&value["sessionId"])
                    && let Some(view) = self.launches.get(&id)
                {
                    bot_cards::launch(id, Some(view), self.started_at.elapsed().as_secs_f32(), cx)
                } else {
                    bot_cards::render(value, cx)
                }
            }
            Body::OlderHistory
            | Body::Pending(_)
            | Body::Launch(_)
            | Body::Review { .. }
            | Body::SavedDraft => div().into_any_element(),
            Body::Actions { turn, complete } => self.response_actions(turn, complete, cx),
            Body::BotHeader { turn } => self.bot_turn_header(turn),
            Body::WorkLine { turn, expandable } => {
                self.work_line_row(turn, expandable, message_start, cx)
            }
            Body::Phase {
                title,
                group,
                icon: glyph,
                active,
            } => self.phase_row(title, group, glyph, active, cx),
            Body::Tool { call, group, tail } => div()
                .relative()
                .w_full()
                .when(group.is_some(), |d| {
                    d.pl(px(20.)).child(crate::activity::rail(tail))
                })
                .child(
                    div()
                        .opacity(step_opacity)
                        .child(self.tool(*call, message_start, cx)),
                )
                .into_any_element(),
            Body::Reasoning => self.reasoning_row(&row, message_start, nested_phase, cx),
            Body::Markdown(ref md) => self.markdown_row(md, &row, ix, look, cx),
            Body::Plain(ref text)
                if user
                    && self.bot
                    && first
                    && last
                    && attachments.is_empty()
                    && is_emoji_only(text) =>
            {
                // In a Bot's chat a few emoji stand alone: no bubble or actions.
                div()
                    .relative()
                    .text_size(px(60.))
                    .line_height(px(60.))
                    .child(text.trim().to_owned())
                    .children(self.reaction_badge(message_start))
                    .into_any_element()
            }
            Body::Plain(ref text) => {
                self.plain_row(text.clone(), &row, attachment_body, look, window, cx)
            }
        };
        let body = if let Some((_, tail)) = row.thought_phase {
            div()
                .relative()
                .w_full()
                .pl(px(20.))
                .when(!tail || thought_header, |d| {
                    d.child(crate::activity::rail_branch(tail, thought_header))
                })
                .child(body)
                .into_any_element()
        } else {
            body
        };
        let body = if let Some(peer) = peer {
            self.peer_message_body(message_start, &peer, first, last, body)
        } else {
            body
        };
        let key = row.key;
        let receipt = matches!(self.rows[ix].body, Body::WorkLine { .. });
        let inside_tools = row.thought_phase.is_some_and(|(_, tail)| !tail || !last)
            || matches!(
                self.rows[ix].body,
                Body::Tool {
                    group: Some(_),
                    tail: false,
                    ..
                }
            );
        let receipt_open = match self.rows[ix].body {
            Body::WorkLine { turn, .. } => self.work_folds.get(&turn).copied().unwrap_or(false),
            _ => false,
        };
        let prose = matches!(self.rows[ix].body, Body::Markdown(..));
        let rail_branch = matches!(self.rows[ix].body, Body::Phase { .. })
            || (!nested_phase && (thought_header || (prose && first_prose)))
            || matches!(self.rows[ix].body, Body::Tool { group: None, .. });
        // Keep a phase header the same height whether its body is present or
        // leaving. The last tool carries the group's animated bottom padding.
        let card = matches!(self.rows[ix].body, Body::BotCard(_));
        let card_after = card
            && self
                .rows
                .get(ix + 1)
                .is_some_and(|r| matches!(r.body, Body::BotCard(_) | Body::Launch(_)));
        let bottom = if card {
            if card_after { 8. } else { 12. }
        } else if inside_tools || matches!(self.rows[ix].body, Body::Phase { .. }) {
            0.
        } else if receipt || (last && is_tool) {
            4.
        } else if last && !user {
            8.
        } else {
            0.
        };
        let natural = div()
            .relative()
            .w_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .children(if first {
                self.bot_separator(message_start)
            } else {
                None
            })
            .child(
                div()
                    .id(("row", ix))
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .w_full()
                            .max_w(px(COLUMN))
                            .min_w_0()
                            .px_4()
                            .pt(px(if first && !is_tool && !prose {
                                if user {
                                    4.
                                } else if receipt && !self.bot {
                                    12.
                                } else if card && self.bot {
                                    4.
                                } else {
                                    0.
                                }
                            } else {
                                0.
                            }))
                            .pb(px(if row.work {
                                0.
                            } else if self.bot && user {
                                // `pt-1 pb-5`, or `pb-0` when the next message stacks on it.
                                if last && !stacked { 20. } else { 0. }
                            } else if self.bot && receipt {
                                0.
                            } else {
                                bottom
                            }))
                            .text_size(px(14.))
                            .line_height(px(24.))
                            .when(user && self.chat_layout, |d| {
                                d.pl(px(56.)).flex().justify_end()
                            })
                            .when(user && self.bot, |d| d.pr(px(16.)))
                            .child(
                                div()
                                    .relative()
                                    .min_w_0()
                                    .when(user && self.chat_layout, |d| d.max_w(px(576.)))
                                    .when(user && !self.chat_layout, |d| d.w_full())
                                    .when(!user, |d| d.w_full())
                                    .when(row.work, |d| {
                                        d.pl(px(20.)).pb(px(bottom)).when(
                                            !nested_phase || !row.work_tail,
                                            |d| {
                                                d.child(crate::activity::rail_branch(
                                                    row.work_tail,
                                                    rail_branch,
                                                ))
                                            },
                                        )
                                    })
                                    .when(receipt_open, |d| {
                                        d.child(
                                            div()
                                                .absolute()
                                                .left(px(6.))
                                                .top(px(22.))
                                                .bottom(px(-4.))
                                                .w(px(1.))
                                                .bg(rgb(0x323232)),
                                        )
                                    })
                                    .child(body),
                            ),
                    ),
            );
        let outer = div()
            .id(gpui::SharedString::from(format!(
                "motion-{}-{}",
                key.0, key.1
            )))
            .w_full()
            .flex()
            .flex_col();
        let mut opacity = 1.;
        let mut translate_y = 0.;
        let motion = self
            .motions
            .get(&key)
            .filter(|_| animate)
            .filter(|m| m.group.is_none_or(|group| !self.windowed_phase(group)))
            .map(|m| (m.value(self.frame_now), m.step));
        if let Some(((_, fade), step)) = motion {
            opacity = if step { 1. } else { fade };
        }
        if let Some((prompt, born)) = self.prompt_rise
            && message_start >= prompt
        {
            let elapsed = self.frame_now.saturating_duration_since(born).as_secs_f32();
            if message_start == prompt {
                let rise = vorkspace_ui::reveal::ease_move((elapsed / 0.56).min(1.));
                let fade = vorkspace_ui::reveal::ease_out((elapsed / 0.48).min(1.));
                translate_y =
                    (f32::from(self.viewport.size.height) * 0.3 - TOP_INSET).max(0.) * (1. - rise);
                opacity *= fade;
            } else {
                let t = ((elapsed - 0.56) / 0.32).clamp(0., 1.);
                let reveal = vorkspace_ui::reveal::ease_move(t);
                translate_y = 10. * (1. - reveal);
                opacity *= reveal;
            }
        }
        if matches!(self.rows[ix].body, Body::Plain(_) | Body::Markdown(_))
            && let Some(born) = self.entrances.get(&message_start)
        {
            // The identity line and footer persist while only the message body
            // enters, matching React's data-chat-message boundary.
            // useBottomChatMotion's entrance: a sent message rises 16px over
            // 360ms, a reply 10px over 280ms.
            let (rise, duration) = if user { (16., 0.36) } else { (10., 0.28) };
            let t = vorkspace_ui::reveal::ease_move(
                (self
                    .frame_now
                    .saturating_duration_since(*born)
                    .as_secs_f32()
                    / duration)
                    .min(1.),
            );
            translate_y += rise * (1. - t);
            opacity *= t;
        }
        // ListState positions the layout root itself. Translate its child so
        // the motion survives prepaint_as_root without affecting row height.
        let natural = natural.relative().top(px(translate_y));
        let body = if let Some(((mut height, _), _)) = motion {
            if let Some(group) = self.motions[&key].group {
                height = self.phase_layouts[&group].row_progress(key, height);
            }
            Disclosure::new(natural, self.viewport.size.width, height).into_any_element()
        } else {
            natural.into_any_element()
        };
        outer
            .when(motion.is_some(), |d| d.overflow_hidden())
            .when(self.bot || user, |d| {
                d.on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    this.bot_hover(user, message_start, message_turn, key, *hovered, cx)
                }))
            })
            .opacity(opacity)
            .child(body)
            .into_any_element()
    }

    pub(super) fn anchor_row(&self) -> Option<usize> {
        self.anchor_turn.and_then(|prompt| {
            if prompt == SAVED_DRAFT_START {
                self.rows
                    .iter()
                    .position(|row| matches!(row.body, Body::SavedDraft))
            } else {
                self.messages
                    .binary_search_by_key(&prompt, |m| m.start)
                    .ok()
                    .map(|message| self.messages[message].display_start)
            }
        })
    }
    // Only measure enough of the live turn to fill the viewport. Older history
    // stays virtualized, and a long reply needs no additional blank space.
    pub(super) fn footer_height(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Pixels {
        let mut remaining = 0.;
        if let Some(first) = self.anchor_row() {
            remaining = (f32::from(self.viewport.size.height) - TOP_INSET - BOTTOM_INSET).max(0.);
            for ix in first..self.rows.len() {
                if remaining <= 0. {
                    break;
                }
                remaining -= f32::from(self.measure_row_height(ix, window, cx));
            }
        }
        px(BOTTOM_INSET + remaining.max(0.))
    }
    pub(super) fn footer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        div()
            .h(self.footer_height(window, cx))
            .w_full()
            .into_any_element()
    }
}
