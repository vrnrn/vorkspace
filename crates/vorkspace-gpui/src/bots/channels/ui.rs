//! Channel conversations use the existing native input, Markdown and approval owners.
use super::*;
use gpui::FontWeight;
use std::sync::Arc;
use vorkspace_ui::clock::Instant;

impl Bots {
    pub(in crate::bots) fn send_channel_message(&mut self, channel: &str, cx: &mut Context<Self>) {
        if self.channel_user_read_only(channel) {
            self.error = Some("Users cannot message Minion team channels".into());
            cx.notify();
            return;
        }
        let key = format!("channel-draft:{channel}");
        let Some(input) = self.fields.get(&key).cloned() else {
            return;
        };
        let text = input.read(cx).content.clone();
        if text.trim().is_empty() {
            return;
        }
        let mut next = self.channels.state.clone();
        let result = (|| {
            if let Some(error) = &self.channels.load_error {
                return Err(error.clone());
            }
            next.post(channel, None, &text, &bot::new_id(), 0, &[], &self.roster)?;
            // A sent message and its cleared draft have one durable publication.
            next.save_with_draft(&*locked(&self.store)?, &key, "")
        })();
        match result {
            Ok(()) => {
                self.channels.state = next;
                self.mark_visible_channel_read(cx);
                self.error = None;
                input.update(cx, |input, cx| input.set_text(String::new(), cx));
                if let Some(scroll) = self.channels.scroll.get(channel) {
                    scroll.scroll_to_bottom();
                }
                self.channel_tick(cx);
            }
            Err(error) => self.error = Some(error),
        }
        self.changed(cx);
    }

    pub(super) fn channel_surface(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let surface = div()
            .id("channel-surface")
            .debug_selector(|| "channel-surface".into())
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(pane_fill().opacity(0.96))
            .text_color(content(0.9))
            .text_size(px(13.));
        if let Some(key) = self.channels.inspecting.clone() {
            let mut body = surface.child(
                div()
                    .h(px(52.))
                    .flex_shrink_0()
                    .px_5()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(content(0.08))
                    .child(
                        panels::button("back-to-channel", "Back to channel").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.channels.inspecting = None;
                                cx.notify();
                            },
                        )),
                    ),
            );
            if let Some(view) = self.channels.views.get(&key).cloned() {
                let transcript = view.read(cx).transcript.clone();
                body = body
                    .child(div().flex_1().min_h_0().child(transcript))
                    .child(view.update(cx, |view, cx| view.bot_attention(window, cx)));
                if view.read(cx).busy {
                    let session = view.read(cx).summary.id;
                    body = body.child(
                        panels::button("stop-channel-conversation", "Stop").on_click(
                            cx.listener(move |this, _, _, cx| this.stop_session(session, cx)),
                        ),
                    );
                }
            } else {
                body = body.child(
                    div().p_6().child(
                        self.error
                            .clone()
                            .unwrap_or("Loading saved conversation…".into()),
                    ),
                );
            }
            return body.into_any_element();
        }
        let selected = self.channels.selected.clone();
        if selected.is_none() {
            let form = self.channel_form(cx);
            return surface
                .child(form)
                .children(self.options_surface(cx))
                .into_any_element();
        }
        let id = selected.unwrap();
        self.mark_visible_channel_read(cx);
        let Some(channel) = self.channels.state.get(&id) else {
            return surface
                .child(div().p_6().child("This channel is unavailable"))
                .into_any_element();
        };
        let name = channel.name.clone();
        let topic = channel.topic.clone();
        let members = channel.member_ids.clone();
        let deliveries = channel.deliveries.clone();
        let sessions = channel.member_sessions.clone();
        let mut retained = HashSet::new();
        let messages = channel
            .messages
            .iter()
            .enumerate()
            .map(|(index, message)| {
                let key = message["id"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("{id}:{index}"));
                retained.insert(key.clone());
                let rows = self
                    .channels
                    .message_cache
                    .entry(key.clone())
                    .or_insert_with(|| {
                        let mut parser = vorkspace_model::markdown_model::MarkdownParser::new();
                        parser.set_text(message["text"].as_str().unwrap_or_default());
                        Arc::new(
                            parser
                                .tree()
                                .blocks
                                .iter()
                                .flat_map(|block| crate::markdown::rows(&block.block))
                                .collect(),
                        )
                    })
                    .clone();
                (
                    key,
                    message["from"].clone(),
                    message["createdAt"].as_u64(),
                    rows,
                )
            })
            .collect::<Vec<_>>();
        self.channels
            .message_cache
            .retain(|key, _| retained.contains(key));
        let scroll = self.channels.scroll.entry(id.clone()).or_default().clone();
        let draft_key = format!("channel-draft:{id}");
        let draft = if self.fields.contains_key(&draft_key) {
            String::new()
        } else {
            locked(&self.store)
                .ok()
                .and_then(|db| db.setting(&draft_key).ok().flatten())
                .unwrap_or_default()
        };
        let input = self.field(&draft_key, "Message the channel", draft, true, cx);
        let read_only = self.channel_user_read_only(&id);
        let bot_mentions = members
            .iter()
            .filter_map(|id| self.roster.get(id))
            .map(|bot| crate::composer_completion::BotMention {
                id: bot.id.clone(),
                name: bot.label(),
                mascot: bot.mascot.clone(),
                color: bot.color,
            })
            .collect::<Vec<_>>();
        let completion = if let Some(completion) = self.channels.completions.get(&id) {
            completion.update(cx, |view, cx| view.set_bots(bot_mentions, cx));
            completion.clone()
        } else {
            let completion = cx.new(|cx| {
                crate::composer_completion::Completion::bot_mentions(
                    input.clone(),
                    bot_mentions,
                    cx,
                )
            });
            self.channels
                .completions
                .insert(id.clone(), completion.clone());
            completion
        };
        let has_draft = !input.read(cx).content.trim().is_empty();
        let title = name.clone();
        let header = div()
            .h(px(58.))
            .flex_shrink_0()
            .px_6()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(content(0.08))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("#{title}")),
                    )
                    .when(!topic.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(px(12.))
                                .text_color(content(0.5))
                                .truncate()
                                .child(topic),
                        )
                    }),
            )
            .child(
                div().flex().items_center().gap_1().children(
                    members
                        .iter()
                        .take(4)
                        .filter_map(|id| self.presentation_bot(id))
                        .map(|bot| avatar::bot_avatar(&bot, "idle", 20., true)),
                ),
            )
            .when(!read_only, |d| {
                d.child(
                    icon_button("channel-settings", "Channel settings", "settings")
                        .debug_selector(|| "channel-settings".into())
                        .child(self.channels.settings_anchor.probe())
                        .aria_expanded(self.channels.editing)
                        .on_click(cx.listener(|this, _, window, cx| {
                            if this.channels.editing {
                                this.dismiss_channel_settings(window, cx);
                                return;
                            }
                            this.channels.editing = true;
                            let channel = this
                                .channels
                                .state
                                .get(this.channels.selected.as_deref().unwrap())
                                .unwrap();
                            let key = format!("channel-edit:{}:name", channel.id);
                            let name = channel.name.clone();
                            let input = this.field(&key, "Channel name", name, false, cx);
                            window.focus(&input.focus_handle(cx), cx);
                            cx.notify();
                        })),
                )
            });
        let mut list = div()
            .id("channel-messages")
            .debug_selector(|| "channel-messages".into())
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .p_6()
            .flex()
            .flex_col()
            .items_center()
            .gap_5();
        if messages.is_empty() {
            list = list.child(
                div().flex_1().flex().items_center().justify_center().child(
                    div()
                        .max_w(px(430.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_2()
                        .text_center()
                        .child(
                            div()
                                .text_size(px(18.))
                                .font_weight(FontWeight::MEDIUM)
                                .child(format!("Start the conversation in #{name}")),
                        )
                        .child(div().text_color(content(0.5)).line_height(px(21.)).child(
                            "Message everyone, or mention a Bot with @Name to address it directly.",
                        )),
                ),
            );
        }
        for (key, author, time, rows) in messages {
            let bot = author["botId"]
                .as_str()
                .and_then(|id| self.presentation_bot(id));
            let label = bot
                .as_ref()
                .map(|b| b.label())
                .unwrap_or_else(|| author["name"].as_str().unwrap_or("You").to_owned());
            let mut heading = div()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM);
            if let Some(bot) = bot {
                heading = heading.child(avatar::bot_avatar(&bot, "idle", 20., true));
            }
            heading = heading.child(label).when_some(time, |d, time| {
                d.child(
                    div()
                        .font_weight(FontWeight::NORMAL)
                        .text_color(content(0.35))
                        .child(vorkspace_core::response::local_clock(time)),
                )
            });
            list = list.child(
                div()
                    .id(SharedString::from(format!("channel-message-{key}")))
                    .w_full()
                    .max_w(px(900.))
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(heading)
                    .children(rows.iter().enumerate().map(|(index, row)| {
                        let body = crate::markdown::body(
                            row,
                            format!("channel-{key}-{index}").into(),
                            crate::markdown::Tone::Normal,
                            None,
                            Instant::now(),
                        );
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .pl(px(row.indent.saturating_sub(1) as f32 * 24.))
                            .when(row.quote > 0, |d| {
                                d.ml_1().pl_4().border_l_4().border_color(content(0.2))
                            })
                            .when(row.indent > 0, |d| {
                                d.child(
                                    div()
                                        .w(px(24.))
                                        .flex_shrink_0()
                                        .child(row.marker.clone().unwrap_or_default()),
                                )
                            })
                            .child(div().flex_1().min_w_0().child(body))
                    })),
            );
        }
        let mut status = div().w_full().max_w(px(900.)).flex().flex_col().gap_2();
        if !sessions.is_empty() {
            status = status.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(content(0.4))
                            .child("Bot conversations"),
                    )
                    .children(sessions.keys().map(|bot| {
                        let key = (id.clone(), bot.clone());
                        let label = self
                            .roster
                            .get(bot)
                            .map(|b| b.label())
                            .unwrap_or_else(|| bot.clone());
                        panels::button(SharedString::from(format!("inspect-member-{bot}")), label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.channels.inspecting = Some(key.clone());
                                this.channel_tick(cx);
                                cx.notify();
                            }))
                    })),
            );
        }
        for delivery in deliveries.iter().filter(|d| d.state != "done") {
            let label = self
                .roster
                .get(&delivery.bot)
                .map(|b| b.label())
                .unwrap_or("Unavailable Bot".into());
            let channel = id.clone();
            let receipt = delivery.id.clone();
            let interrupted = delivery.state == "interrupted";
            let mut row = div()
                .w_full()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(content(0.04))
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(format!("{label} · {}", delivery.state))
                        .when_some(delivery.error.clone(), |d, error| {
                            d.child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(content(0.5))
                                    .child(error),
                            )
                        }),
                );
            if sessions.contains_key(&delivery.bot) {
                let key = (id.clone(), delivery.bot.clone());
                row = row.child(
                    panels::button(
                        SharedString::from(format!("inspect-{}", delivery.id)),
                        "Inspect conversation",
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.channels.inspecting = Some(key.clone());
                        this.channel_tick(cx);
                        cx.notify();
                    })),
                );
            }
            row = row.child(
                panels::button(
                    SharedString::from(format!("stop-{}", delivery.id)),
                    if interrupted { "Dismiss" } else { "Stop" },
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if interrupted {
                        let mut next = this.channels.state.clone();
                        if let Some(c) = next.get_mut(&channel) {
                            c.deliveries.retain(|d| d.id != receipt);
                        }
                        if let Err(error) = this.commit_channels(next, cx) {
                            this.error = Some(error);
                        }
                    } else {
                        this.stop_channel_delivery(&channel, &receipt, cx);
                    }
                    cx.notify();
                })),
            );
            status = status.child(row);
            if let Some(view) = self.channels.views.get(&(id.clone(), delivery.bot.clone())) {
                status = status.child(view.update(cx, |v, cx| v.bot_attention(window, cx)));
            }
        }
        let send = id.clone();
        let focus = input.focus_handle(cx);
        let composer = div()
            .id("channel-composer")
            .debug_selector(|| "channel-composer".into())
            .w_full()
            .relative()
            .child(completion.read(cx).composer_probe())
            .max_w(px(900.))
            .min_w_0()
            .border_1()
            .border_color(composer_border(focus.is_focused(window)))
            .rounded_xl()
            .bg(composer_fill())
            .px_3()
            .pt_3()
            .pb_2()
            .flex()
            .flex_col()
            .gap_2()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                window.focus(&focus, cx)
            })
            .child(completion)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(content(0.4))
                            .child("Enter to send · Shift+Enter for a new line"),
                    )
                    .child(
                        panels::button("send-channel", "Send")
                            .debug_selector(|| "send-channel".into())
                            .bg(accent_color().opacity(if has_draft { 1. } else { 0.3 }))
                            .text_color(gpui::white())
                            .opacity(if has_draft { 1. } else { 0.5 })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.send_channel_message(&send, cx)
                            })),
                    ),
            );
        let mut footer = div()
            .flex_shrink_0()
            .px_6()
            .pt_2()
            .pb_4()
            .flex()
            .flex_col()
            .items_center()
            .gap_3()
            .child(status);
        if let Some(error) = self.channels.load_error.as_ref().or(self.error.as_ref()) {
            footer = footer.child(
                div()
                    .w_full()
                    .max_w(px(900.))
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(content(0.05))
                    .text_color(content(0.7))
                    .child(error.clone()),
            );
        }
        let footer = if read_only {
            footer.child(
                div()
                    .text_size(px(11.))
                    .text_color(content(0.5))
                    .child("Team conversation · Only Bots can send messages"),
            )
        } else {
            footer.child(composer)
        };
        let surface = surface
            .child(header)
            .child(list)
            .child(footer)
            .children(self.options_surface(cx));
        if self.channels.editing {
            let form = self.channel_form(cx);
            let weak = cx.weak_entity();
            surface
                .child(
                    popover(
                        "channel-settings-popover",
                        Anchor::Slot(self.channels.settings_anchor.clone()),
                    )
                    .align(crate::popover::Align::End)
                    .ignore(self.channels.settings_anchor.clone())
                    .width(400.)
                    .max_height((window.viewport_size().height.as_f32() - 120.).clamp(180., 560.))
                    .on_dismiss(move |window, cx| {
                        let _ =
                            weak.update(cx, |this, cx| this.dismiss_channel_settings(window, cx));
                    })
                    .child(form),
                )
                .into_any_element()
        } else {
            surface.into_any_element()
        }
    }

    fn channel_form(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let editing = self
            .channels
            .selected
            .clone()
            .filter(|_| self.channels.editing);
        let existing = editing
            .as_deref()
            .and_then(|id| self.channels.state.get(id));
        if existing.is_some_and(|c| c.user_read_only(&self.roster)) {
            return div()
                .px_3()
                .py_2()
                .text_size(px(12.))
                .text_color(content(0.7))
                .child("Team channels are managed by their Bots.")
                .into_any_element();
        }
        let name = existing.map(|c| c.name.clone()).unwrap_or_default();
        let topic = existing.map(|c| c.topic.clone()).unwrap_or_default();
        let members = existing
            .map(|c| c.member_ids.iter().cloned().collect())
            .unwrap_or_default();
        let prefix = editing
            .as_ref()
            .map(|id| format!("channel-edit:{id}"))
            .unwrap_or("channel-new".into());
        self.channels
            .form_members
            .entry(prefix.clone())
            .or_insert(members);
        let name_key = format!("{prefix}:name");
        let topic_key = format!("{prefix}:topic");
        let name = self.field(&name_key, "Channel name", name, false, cx);
        let topic = self.field(&topic_key, "Channel topic (optional)", topic, false, cx);
        let valid = !name.read(cx).content.trim().is_empty()
            && !self.channels.form_members[&prefix].is_empty();
        let mut bots = div().w_full().flex().flex_col().gap_1();
        for bot in self.roster.bots.iter().filter(|b| !b.is_minion()) {
            let chosen = self.channels.form_members[&prefix].contains(&bot.id);
            let member = bot.id.clone();
            let key = prefix.clone();
            bots = bots.child(
                div()
                    .id(SharedString::from(format!("channel-member-{}", bot.id)))
                    .role(Role::CheckBox)
                    .aria_label(format!("Add {} to channel", bot.label()))
                    .aria_toggled(if chosen {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .w_full()
                    .h(px(42.))
                    .px_3()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_3()
                    .bg(content(if chosen { 0.08 } else { 0.02 }))
                    .hover(|s| s.bg(content(0.1)))
                    .child(avatar::bot_avatar(bot, "idle", 24., true))
                    .child(div().flex_1().child(bot.label()))
                    .child(if chosen { "✓" } else { "" })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let members = this.channels.form_members.entry(key.clone()).or_default();
                        if !members.remove(&member) {
                            members.insert(member.clone());
                        }
                        cx.notify();
                    })),
            );
        }
        if !self.roster.bots.iter().any(|b| !b.is_minion()) {
            bots = bots.child(
                div()
                    .text_color(content(0.5))
                    .child("Create a Bot in the sidebar first."),
            );
        }
        let selected = editing.clone();
        let form_key = prefix.clone();
        let save_name = name_key.clone();
        let save_topic = topic_key.clone();
        let create = panels::button(
            "save-channel",
            if editing.is_some() {
                "Save changes"
            } else {
                "Create channel"
            },
        )
        .debug_selector(|| "save-channel".into())
        .bg(accent_color().opacity(if valid { 1. } else { 0.3 }))
        .text_color(gpui::white())
        .on_click(cx.listener(move |this, _, window, cx| {
            if !valid {
                return;
            }
            if selected
                .as_deref()
                .is_some_and(|id| this.channel_user_read_only(id))
            {
                return;
            }
            let name = this.fields[&save_name].read(cx).content.clone();
            let topic = this.fields[&save_topic].read(cx).content.clone();
            let members = this
                .roster
                .bots
                .iter()
                .filter(|bot| !bot.is_minion())
                .filter(|bot| {
                    this.channels
                        .form_members
                        .get(&form_key)
                        .is_some_and(|ids| ids.contains(&bot.id))
                })
                .map(|bot| bot.id.clone())
                .collect::<Vec<_>>();
            let mut next = this.channels.state.clone();
            let result = if let Some(id) = &selected {
                next.edit(id, &name, &topic, members, &this.roster)
                    .map(|_| id.clone())
            } else {
                next.create(&name, &topic, members, &this.roster)
            };
            match result.and_then(|id| {
                if next
                    .get(&id)
                    .is_some_and(|c| c.user_read_only(&this.roster))
                {
                    return Err("Minion team channels are managed by their Bots".into());
                }
                this.commit_channels(next, cx).map(|_| id)
            }) {
                Ok(id) => {
                    this.channels.selected = Some(id);
                    this.channels.editing = false;
                    this.channels.form_members.remove(&form_key);
                    this.fields.retain(|key, _| !key.starts_with(&form_key));
                    this.error = None;
                    this.focus_channel_composer(window, cx);
                }
                Err(error) => this.error = Some(error),
            }
            this.changed(cx);
        }));
        let cancel = panels::button("close-channels", "Cancel")
            .debug_selector(|| "close-channels".into())
            .on_click(cx.listener(|this, _, window, cx| {
                if this.channels.editing {
                    this.dismiss_channel_settings(window, cx);
                } else {
                    this.close_classic_channels(cx);
                }
                this.changed(cx);
            }));
        let mut form=div().w_full().max_w(px(520.)).flex().flex_col().gap_4()
            .child(div().text_size(px(if editing.is_some(){18.}else{22.})).font_weight(FontWeight::SEMIBOLD).child(if editing.is_some(){"Channel settings"}else{"New channel"}))
            .child(div().text_color(content(0.5)).line_height(px(21.)).child("Give your Bots a shared conversation. Each keeps its own model, account and permissions."))
            .child(div().flex().flex_col().gap_2().child("Name").child(div().px_3().py_2p5().rounded_md().border_1().border_color(content(0.12)).bg(content(0.03)).child(name)))
            .child(div().flex().flex_col().gap_2().child("Topic").child(div().px_3().py_2p5().rounded_md().border_1().border_color(content(0.12)).bg(content(0.03)).child(topic)))
            .child(div().flex().flex_col().gap_2().child("Members").child(bots));
        if let Some(error) = self.channels.load_error.as_ref().or(self.error.as_ref()) {
            form = form.child(div().text_color(content(0.6)).child(error.clone()));
        }
        let form = form.child(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .child(cancel)
                .child(create),
        );
        if editing.is_some() {
            return div()
                .id("channel-settings-menu")
                .debug_selector(|| "channel-settings-menu".into())
                .role(Role::Dialog)
                .aria_label("Channel settings")
                .key_context("ChannelSettings")
                .w_full()
                .min_h_0()
                .overflow_y_scroll()
                .p_4()
                .on_action(
                    cx.listener(|this, _: &crate::shell::DismissMenu, window, cx| {
                        this.dismiss_channel_settings(window, cx);
                        cx.stop_propagation();
                    }),
                )
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        this.dismiss_channel_settings(window, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(form)
                .into_any_element();
        }
        div()
            .id("channel-form-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_6()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .child(form)
            .into_any_element()
    }
    fn dismiss_channel_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = &self.channels.selected {
            let prefix = format!("channel-edit:{id}");
            self.fields.retain(|key, _| !key.starts_with(&prefix));
            self.channels.form_members.remove(&prefix);
        }
        self.channels.editing = false;
        self.focus_channel_composer(window, cx);
        cx.notify();
    }
    pub(in crate::bots) fn focus_channel_composer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.channels.selected.clone() else {
            return;
        };
        if self.channel_user_read_only(&id) {
            return;
        }
        let key = format!("channel-draft:{id}");
        let draft = locked(&self.store)
            .ok()
            .and_then(|db| db.setting(&key).ok().flatten())
            .unwrap_or_default();
        let input = self.field(&key, "Message the channel", draft, true, cx);
        window.focus(&input.focus_handle(cx), cx);
    }
}
