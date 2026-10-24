use super::*;
use serde_json::{Value, json};
use vorkspace_core::bot::channels::Channels;

#[derive(Default)]
pub(super) struct ChannelUi {
    pub state: Channels,
    pub load_error: Option<String>,
    pub open: bool,
    pub inspecting: Option<(String, String)>,
    pub selected: Option<String>,
    pub views: HashMap<(String, String), Entity<Conversation>>,
    pub(super) loading: HashSet<(String, String)>,
    pub(super) starts: HashMap<String, usize>,
}

impl Bots {
    pub(crate) fn classic_channels_open(&self) -> bool {
        self.prefs.bots && self.channels.open
    }

    pub(crate) fn close_classic_channels(&mut self, cx: &mut Context<Self>) {
        if self.channels.open {
            self.channels.open = false;
            self.channels.inspecting = None;
            cx.notify();
        }
    }

    /// The original rail lists Channels beside Bots, regardless of which
    /// project's session/file pane happens to be selected.
    pub(crate) fn classic_channels_rail(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if !self.prefs.bots {
            return div().into_any_element();
        }
        div()
            .id("classic-channels")
            .debug_selector(|| "classic-channels".into())
            .px_2()
            .pb_3()
            .child(
                section_label("Channels").child(
                    icon_button("classic-add-channel", "New channel", "plus")
                        .debug_selector(|| "classic-add-channel".into())
                        .focusable()
                        .tab_stop(true)
                        .size(px(20.))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_classic_channel(None, window, cx);
                        })),
                ),
            )
            .children(self.channels.state.channels.iter().map(|channel| {
                let id = channel.id.clone();
                let selector = id.clone();
                let selected = self.channels.open
                    && self.channels.selected.as_deref() == Some(channel.id.as_str());
                let count = channel.member_ids.len();
                div()
                    .id(SharedString::from(format!("classic-channel-{id}")))
                    .debug_selector(move || format!("classic-channel-{selector}"))
                    .role(Role::Button)
                    .aria_label(format!("Channel {}, {count} members", channel.name))
                    .aria_selected(selected)
                    .focusable()
                    .tab_stop(true)
                    .h(px(32.))
                    .min_w_0()
                    .px_2()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(14.))
                    .opacity(if selected { 1. } else { 0.65 })
                    .when(selected, |d| d.bg(content(0.1)))
                    .hover(|s| s.bg(content(0.08)).opacity(1.))
                    .focus(|s| s.bg(content(0.08)).opacity(1.))
                    .child(icon("hash", 16.))
                    .child(
                        div().flex().items_center().children(
                            channel
                                .member_ids
                                .iter()
                                .take(3)
                                .filter_map(|id| self.roster.get(id))
                                .enumerate()
                                .map(|(ix, bot)| {
                                    div()
                                        .size(px(8.))
                                        .rounded_full()
                                        .bg(rgb(bot.color))
                                        .when(ix > 0, |d| d.ml(px(-2.)))
                                }),
                        ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(channel.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(content(0.5))
                            .child(count.to_string()),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_classic_channel(Some(id.clone()), window, cx);
                    }))
            }))
            .into_any_element()
    }

    fn open_classic_channel(
        &mut self,
        id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.channels.open = true;
        self.channels.selected = id;
        self.channels.inspecting = None;
        self.changed(cx);
        let owner = self.owner.clone();
        window.defer(cx, move |window, cx| {
            let _ = owner.update(cx, |shell, cx| shell.open_classic_channels(window, cx));
        });
    }

    pub(super) fn stop_channel_delivery(
        &mut self,
        channel: &str,
        delivery: &str,
        cx: &mut Context<Self>,
    ) {
        let mut next = self.channels.state.clone();
        let Some(d) = next
            .get_mut(channel)
            .and_then(|c| c.deliveries.iter_mut().find(|d| d.id == delivery))
        else {
            return;
        };
        let running = d.state == "running";
        let key = (channel.to_string(), d.bot.clone());
        d.state = "interrupted".into();
        d.error = Some("Stopped by you".into());
        // A failed persistence write must never prevent Stop. The stored running
        // receipt is conservatively interrupted on recovery, never replayed.
        if let Err(error) = self.commit_channels(next.clone(), cx) {
            self.channels.state = next;
            self.error = Some(error);
        }
        if running && let Some(view) = self.channels.views.get(&key) {
            view.update(cx, |v, cx| v.bot_stop(cx));
        }
        cx.notify();
    }

    pub(super) fn commit_channels(
        &mut self,
        next: Channels,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if let Some(error) = &self.channels.load_error {
            return Err(error.clone());
        }
        next.save(&*locked(&self.store)?)?;
        self.channels.state = next;
        cx.notify();
        Ok(())
    }
    pub(super) fn channel_action(
        &mut self,
        action: &str,
        input: &Value,
        author: Option<&Bot>,
        request: &str,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        if let Some(error) = &self.channels.load_error {
            return Err(error.clone());
        }
        bot::api::validate(action, input)?;
        let mut next = self.channels.state.clone();
        let result = match action {
            "channels.list" => json!(next.channels.iter().filter(|c| author.is_none_or(|m| c.member_ids.contains(&m.id))).map(|c| json!({"id":c.id,"name":c.name,"topic":c.topic,"memberIds":c.member_ids})).collect::<Vec<_>>()),
            "channels.create" => {
                let mut members = input["members"].as_array().ok_or("Provide member Bot names")?.iter().map(|v| v.as_str().map(str::to_owned).ok_or("Invalid member".to_string())).collect::<Result<Vec<_>,_>>()?;
                if let Some(m) = author { members.push(m.id.clone()); }
                let id = next.create(input["name"].as_str().ok_or("Provide name")?,input["topic"].as_str().unwrap_or(""), members, &self.roster)?;
                if let Some(author)=author && self.channels.state.get(&id).is_some_and(|c|!c.member_ids.contains(&author.id)) {return Err("This Bot is not a channel member".into());}
                json!({"id":id})
            },
            "channels.read" | "channels.post" => {
                let id = input["channel"].as_str().ok_or("Provide channel")?;
                let channel = next.get(id).ok_or("Unknown channel")?;
                if author.is_some_and(|m| !channel.member_ids.contains(&m.id)) { return Err("This Bot is not a channel member".into()); }
                if action == "channels.read" {
                    let limit = input["limit"].as_u64().unwrap_or(20).min(50) as usize;
                    let rows = channel.messages.iter().rev().take(limit).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>();
                    json!(rows)
                } else {
                    // A channel worker publishes exactly once through its final
                    // reply. Reject recursive same-turn posts through the CLI.
                    let targets = input["to"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect::<Vec<_>>();
                    let message = next.post(id,author,input["message"].as_str().ok_or("Provide message")?,request,0,&targets,&self.roster)?;
                    json!({"accepted":true,"messageId":message})
                }
            },
            _ => return Err("Unknown channel action".into()),
        };
        if !matches!(action, "channels.list" | "channels.read") {
            self.commit_channels(next, cx)?;
        }
        Ok(result)
    }
    pub(super) fn channel_tick(&mut self, cx: &mut Context<Self>) {
        if self.channels.load_error.is_some() {
            return;
        }
        if let Some(key) = self.channels.inspecting.clone()
            && !self.channels.views.contains_key(&key)
            && let Some(bot) = self.roster.get(&key.1).cloned()
        {
            self.load_channel_view(key, bot, cx);
        }
        let queued = self
            .channels
            .state
            .channels
            .iter()
            .flat_map(|c| {
                c.deliveries
                    .iter()
                    .filter(|d| d.state == "queued")
                    .map(|d| (c.id.clone(), d.clone()))
            })
            .collect::<Vec<_>>();
        for (channel, delivery) in queued {
            let key = (channel.clone(), delivery.bot.clone());
            if self.channels.state.get(&channel).is_some_and(|c| {
                c.deliveries
                    .iter()
                    .any(|d| d.bot == delivery.bot && d.state == "running")
            }) {
                continue;
            }
            let Some(bot) = self.roster.get(&delivery.bot).cloned() else {
                continue;
            };
            let Some(view) = self.channels.views.get(&key).cloned() else {
                self.load_channel_view(key, bot, cx);
                continue;
            };
            if !view.read(cx).bot_ready() {
                continue;
            }
            let prompt = self
                .channels
                .state
                .get(&channel)
                .unwrap()
                .prompt(&delivery, &self.roster);
            let mut next = self.channels.state.clone();
            if let Some(d) = next
                .get_mut(&channel)
                .and_then(|c| c.deliveries.iter_mut().find(|d| d.id == delivery.id))
            {
                d.state = "running".into();
            }
            if let Err(error) = self.commit_channels(next, cx) {
                self.error = Some(error);
                break;
            }
            self.channels
                .starts
                .insert(delivery.id.clone(), view.read(cx).bot_rows(cx).len());
            let message = Message {
                id: delivery.id.clone(),
                text: prompt,
                attachments: Vec::new(),
                internal: true,
                sent_at: Some(vorkspace_core::time::now_ms()),
                error: None,
            };
            view.update(cx, |v, cx| {
                v.bot_dispatch(&message, cx);
            });
        }
    }
    fn load_channel_view(&mut self, key: (String, String), bot: Bot, cx: &mut Context<Self>) {
        if !self.channels.loading.insert(key.clone()) {
            return;
        }
        self.ensure_chat(&bot.id, cx);
        let Some(primary) = self.chats.get(&bot.id) else {
            self.channels.loading.remove(&key);
            return;
        };
        let mut settings = primary.read(cx).settings.clone();
        settings.bot = None;
        settings.turns.clear();
        settings.environment.clear();
        settings.attachments.clear();
        let provider = primary.read(cx).summary.provider.clone();
        let saved = self
            .channels
            .state
            .get(&key.0)
            .and_then(|c| c.member_sessions.get(&key.1))
            .copied();
        let store = self.store.clone();
        let cwd = primary.read(cx).summary.cwd.clone();
        let work = cx.background_executor().spawn(async move {
            let db = locked(&store)?;
            if let Some(id) = saved {
                db.bot_session(id)?
                    .ok_or("Saved channel conversation is unavailable".into())
            } else {
                let summary = db.create(&provider, &cwd)?;
                db.configure_agent(summary.id, &provider, &settings)?;
                Ok(summary)
            }
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.channels.loading.remove(&key);
                let summary = match result {
                    Ok(s) => s,
                    Err(e) => {
                        this.error = Some(e);
                        cx.notify();
                        return;
                    }
                };
                let mut next = this.channels.state.clone();
                let Some(channel) = next.get_mut(&key.0) else {
                    return;
                };
                channel.member_sessions.insert(key.1.clone(), summary.id);
                if let Err(e) = this.commit_channels(next, cx) {
                    this.error = Some(e);
                    return;
                }
                let id = summary.id;
                let view = cx.new(|cx| Conversation::new(summary, this.store.clone(), cx));
                let env = this
                    .bridge
                    .as_ref()
                    .map(|b| b.environment(id))
                    .unwrap_or_default();
                let root = this.root.clone();
                view.update(cx, |v, cx| {
                    v.configure_bot(bot, root, env, cx);
                    v.load(cx);
                });
                let observed = key.clone();
                this.observations.push(
                    cx.subscribe(&view, move |this, _, _: &SessionChanged, cx| {
                        this.channel_finished(&observed, cx)
                    }),
                );
                this.channels.views.insert(key, view);
                this.roster.hidden_sessions.push(id);
                this.save(cx);
                this.channel_tick(cx);
            });
        })
        .detach();
    }
    fn channel_finished(&mut self, key: &(String, String), cx: &mut Context<Self>) {
        let Some(view) = self.channels.views.get(key) else {
            return;
        };
        let view = view.read(cx);
        if view.busy {
            cx.notify();
            return;
        }
        let Some(delivery) = self
            .channels
            .state
            .get(&key.0)
            .and_then(|c| {
                c.deliveries
                    .iter()
                    .find(|d| d.bot == key.1 && d.state == "running")
            })
            .cloned()
        else {
            return;
        };
        let Some(start) = self.channels.starts.get(&delivery.id).copied() else {
            return;
        };
        let rows = &view.bot_rows(cx)[start.min(view.bot_rows(cx).len())..];
        let failed = rows.iter().any(|r| r.role == "error");
        let text = rows
            .iter()
            .filter(|r| r.role == "assistant")
            .map(|r| r.text.as_str())
            .collect::<Vec<_>>()
            .join("");
        let mut next = self.channels.state.clone();
        if let Some(d) = next
            .get_mut(&key.0)
            .and_then(|c| c.deliveries.iter_mut().find(|d| d.id == delivery.id))
        {
            d.state = if failed { "interrupted" } else { "done" }.into();
            d.error =
                failed.then(|| "Bot turn failed; inspect its conversation before retrying".into());
        }
        let silent = matches!(
            text.trim().to_ascii_lowercase().as_str(),
            "" | "pass" | "<pass/>" | "<pass></pass>"
        );
        if !failed && !silent {
            let text = text.trim().trim_end_matches("<pass/>").trim();
            if let Err(e) = next.post(
                &key.0,
                self.roster.get(&key.1),
                text,
                &format!("reply:{}", delivery.id),
                delivery.hop,
                &[],
                &self.roster,
            ) {
                if let Some(d) = next
                    .get_mut(&key.0)
                    .and_then(|c| c.deliveries.iter_mut().find(|d| d.id == delivery.id))
                {
                    d.state = "interrupted".into();
                    d.error = Some(e.clone());
                }
                self.error = Some(e);
            }
        }
        match self.commit_channels(next, cx) {
            Ok(()) => {
                self.channels.starts.remove(&delivery.id);
            }
            Err(e) => self.error = Some(e),
        }
        self.channel_tick(cx);
    }
    pub(super) fn channels_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(key) = self.channels.inspecting.clone() {
            let mut panel = div().size_full().flex().flex_col().child(
                panels::button("back-to-channel", "Back to channel").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.channels.inspecting = None;
                        cx.notify();
                    },
                )),
            );
            if let Some(view) = self.channels.views.get(&key).cloned() {
                let transcript = view.read(cx).transcript.clone();
                let id = view.read(cx).summary.id;
                if view.read(cx).busy {
                    panel = panel.child(
                        panels::button("stop-channel-conversation", "Stop")
                            .on_click(cx.listener(move |this, _, _, cx| this.stop_session(id, cx))),
                    );
                }
                panel = panel
                    .child(div().flex_1().min_h_0().child(transcript))
                    .child(view.update(cx, |v, cx| v.bot_attention(window, cx)));
            } else {
                panel = panel.child(
                    self.error
                        .clone()
                        .unwrap_or("Loading saved conversation…".into()),
                );
            }
            return panel.into_any_element();
        }
        let name = self.field("channel-name", "Channel name", String::new(), false, cx);
        let members = self.field(
            "channel-members",
            "Member Bot names, separated by commas",
            String::new(),
            false,
            cx,
        );
        let topic = self.field("channel-topic", "Topic", String::new(), false, cx);
        let mut body=div().size_full().flex().flex_col().p_4().gap_3().text_size(px(13.)).text_color(content(0.85))
            .on_key_down(cx.listener(|this,event:&gpui::KeyDownEvent,_,cx|{if event.keystroke.key=="escape"{this.channels.open=false;this.changed(cx);}}))
            .child(div().flex().items_center().justify_between().child("Bot Channels").child(panels::button("close-channels",if self.active.is_some() {"Back to Bot"} else {"Back to workspace"}).debug_selector(|| "close-channels".into()).on_click(cx.listener(|this,_,_,cx|{this.channels.open=false;this.changed(cx);}))))
            .child(div().flex().gap_2().children(self.channels.state.channels.iter().map(|c|{let id=c.id.clone();panels::button(SharedString::from(format!("channel-{id}")),format!("#{}",c.name)).on_click(cx.listener(move|this,_,_,cx|{this.channels.selected=Some(id.clone());this.changed(cx);}))})))
            .child(div().flex().gap_2().child(div().flex_1().child(name)).child(div().flex_1().child(members)).child(div().flex_1().child(topic)).child(panels::button("create-channel","Create").on_click(cx.listener(|this,_,_,cx|{
                let field=|key:&str|this.fields[key].read(cx).content.clone();
                let input=json!({"name":field("channel-name"),"topic":field("channel-topic"),"members":field("channel-members").split(',').map(str::trim).filter(|s|!s.is_empty()).collect::<Vec<_>>()});
                match this.channel_action("channels.create",&input,None,&bot::new_id(),cx){Ok(v)=>this.channels.selected=v["id"].as_str().map(str::to_owned),Err(e)=>this.error=Some(e)};this.changed(cx);
            }))));
        if let Some(c) = self
            .channels
            .selected
            .as_deref()
            .and_then(|id| self.channels.state.get(id))
            .cloned()
        {
            let mut messages = div()
                .id("channel-messages")
                .debug_selector(|| "channel-messages".into())
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_3();
            for m in &c.messages {
                messages = messages.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_color(content(0.5))
                                .child(m["from"]["name"].as_str().unwrap_or("You").to_string()),
                        )
                        .child(m["text"].as_str().unwrap_or_default().to_string()),
                );
            }
            for d in c.deliveries.iter().filter(|d| d.state != "done") {
                let label = format!(
                    "{}: {}{}",
                    self.roster
                        .get(&d.bot)
                        .map(|b| b.label())
                        .unwrap_or(d.bot.clone()),
                    d.state,
                    d.error
                        .as_ref()
                        .map(|e| format!(" · {e}"))
                        .unwrap_or_default()
                );
                let channel = c.id.clone();
                let delivery = d.id.clone();
                let key = (c.id.clone(), d.bot.clone());
                let inspect = key.clone();
                if c.member_sessions.contains_key(&d.bot) {
                    messages = messages.child(
                        panels::button(
                            SharedString::from(format!("inspect-{}", d.id)),
                            "Inspect conversation",
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.channels.inspecting = Some(inspect.clone());
                            cx.notify();
                        })),
                    );
                }
                let interrupted = d.state == "interrupted";
                messages = messages.child(
                    div().child(label).child(
                        panels::button(
                            SharedString::from(format!("stop-{}", d.id)),
                            if interrupted { "Dismiss" } else { "Stop" },
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let mut next = this.channels.state.clone();
                            if interrupted {
                                if let Some(c) = next.get_mut(&channel) {
                                    c.deliveries.retain(|d| d.id != delivery);
                                }
                                if let Err(error) = this.commit_channels(next, cx) {
                                    this.error = Some(error);
                                }
                                return;
                            }
                            this.stop_channel_delivery(&channel, &delivery, cx);
                            cx.notify();
                        })),
                    ),
                );
                if let Some(view) = self.channels.views.get(&(c.id.clone(), d.bot.clone())) {
                    messages = messages.child(view.update(cx, |v, cx| v.bot_attention(window, cx)));
                }
            }
            for bot in c.member_sessions.keys() {
                let key = (c.id.clone(), bot.clone());
                let label = self
                    .roster
                    .get(bot)
                    .map(|m| m.label())
                    .unwrap_or(bot.clone());
                messages = messages.child(
                    panels::button(
                        SharedString::from(format!("history-{bot}")),
                        format!("Inspect {label}'s conversation"),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.channels.inspecting = Some(key.clone());
                        this.channel_tick(cx);
                        cx.notify();
                    })),
                );
            }
            let input_key = format!("channel-draft:{}", c.id);
            let draft = if self.fields.contains_key(&input_key) {
                String::new()
            } else {
                locked(&self.store)
                    .ok()
                    .and_then(|db| db.setting(&input_key).ok().flatten())
                    .unwrap_or_default()
            };
            let input = self.field(&input_key, "Message the channel", draft, true, cx);
            let target = c.id.clone();
            body = body
                .child(format!("#{} · {}", c.name, c.topic))
                .child(messages)
                .child(div().h(px(100.)).child(input))
                .child(panels::button("send-channel", "Send").on_click(cx.listener(
                    move |this, _, _, cx| {
                        let text = this.fields[&input_key].read(cx).content.clone();
                        let value = json!({"channel":target,"message":text});
                        match this.channel_action("channels.post", &value, None, &bot::new_id(), cx)
                        {
                            Ok(_) => {
                                this.fields[&input_key]
                                    .update(cx, |i, cx| i.set_text(String::new(), cx));
                                this.channel_tick(cx);
                            }
                            Err(e) => this.error = Some(e),
                        };
                        cx.notify();
                    },
                )));
        } else {
            body = body.child("Create a channel to let your Bots work together.");
        }
        if let Some(error) = &self.error {
            body = body.child(error.clone());
        }
        body.into_any_element()
    }
}

#[cfg(test)]
mod classic_rail_tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};
    use std::sync::{Arc, Mutex};
    use vorkspace_core::{bot::channels::Channel, store::Store};

    #[gpui::test]
    fn rail_opens_channels_without_a_selected_bot_and_returns_to_the_workspace(
        cx: &mut TestAppContext,
    ) {
        let root = std::env::temp_dir().join(format!("classic-channel-{}", bot::new_id()));
        std::fs::create_dir_all(&root).unwrap();
        let store = Arc::new(Mutex::new(
            Store::open(&root.join("native.sqlite")).unwrap(),
        ));
        let project = root.to_string_lossy().into_owned();
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| Shell::new(store, Vec::new(), project, None, cx))
            })
            .unwrap()
        });
        cx.run_until_parked();
        let manager = window
            .update(cx, |shell, _, _| shell.bot_manager())
            .unwrap();
        manager.update(cx, |manager, cx| {
            assert!(manager.active.is_none());
            manager.channels.state.channels.push(Channel {
                id: "team".into(),
                name: "team".into(),
                ..Default::default()
            });
            manager.changed(cx);
        });
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.simulate_resize(gpui::size(px(1400.), px(900.)));
        visual.run_until_parked();
        let channel = visual.debug_bounds("classic-channel-team").unwrap();
        visual.simulate_click(channel.center(), Default::default());
        visual.run_until_parked();
        manager.read_with(visual, |manager, _| {
            assert!(manager.classic_channels_open());
            assert!(manager.active.is_none());
            assert_eq!(manager.channels.selected.as_deref(), Some("team"));
        });
        assert!(visual.debug_bounds("channel-messages").is_some());
        assert!(visual.debug_bounds("classic-project-rail").is_some());
        assert!(visual.debug_bounds("sidebar-frame").is_none());

        let close = visual.debug_bounds("close-channels").unwrap();
        visual.simulate_click(close.center(), Default::default());
        visual.run_until_parked();
        assert!(!manager.read_with(visual, |manager, _| manager.classic_channels_open()));
        assert!(visual.debug_bounds("sidebar-frame").is_some());

        let add = visual.debug_bounds("classic-add-channel").unwrap();
        visual.simulate_click(add.center(), Default::default());
        visual.run_until_parked();
        manager.read_with(visual, |manager, _| {
            assert!(manager.classic_channels_open());
            assert!(manager.channels.selected.is_none());
        });
        let project = visual.debug_bounds("classic-project-0").unwrap();
        visual.simulate_click(project.center(), Default::default());
        visual.run_until_parked();
        assert!(!manager.read_with(visual, |manager, _| manager.classic_channels_open()));
        assert!(visual.debug_bounds("sidebar-frame").is_some());
    }
}
