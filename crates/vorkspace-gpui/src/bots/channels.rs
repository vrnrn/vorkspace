use super::*;
use serde_json::{Value, json};
use vorkspace_core::bot::channels::Channels;
mod ui;
pub(super) const READ_KEY: &str = "bot-channel-read";

#[derive(Default)]
pub(super) struct ChannelUi {
    pub state: Channels,
    pub read_through: HashMap<String, String>,
    pub load_error: Option<String>,
    pub open: bool,
    pub inspecting: Option<(String, String)>,
    pub selected: Option<String>,
    pub views: HashMap<(String, String), Entity<Conversation>>,
    pub(super) loading: HashSet<(String, String)>,
    pub(super) starts: HashMap<String, usize>,
    pub(super) editing: bool,
    pub(super) settings_anchor: AnchorSlot,
    pub(super) completions: HashMap<String, Entity<crate::composer_completion::Completion>>,
    pub(super) form_members: HashMap<String, HashSet<String>>,
    pub(super) message_cache: HashMap<String, std::sync::Arc<Vec<crate::markdown::MarkdownRow>>>,
    pub(super) scroll: HashMap<String, gpui::ScrollHandle>,
}

impl Bots {
    fn channel_unread(&self, id: &str) -> bool {
        self.channels
            .state
            .get(id)
            .and_then(|c| c.messages.last())
            .and_then(|m| m["id"].as_str())
            .is_some_and(|last| {
                self.channels.read_through.get(id).map(String::as_str) != Some(last)
            })
    }

    fn mark_channel_read(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.channel_unread(id) {
            return;
        }
        let Some(last) = self
            .channels
            .state
            .get(id)
            .and_then(|c| c.messages.last())
            .and_then(|m| m["id"].as_str())
        else {
            return;
        };
        let mut next = self.channels.read_through.clone();
        next.retain(|id, _| self.channels.state.get(id).is_some());
        next.insert(id.into(), last.into());
        let saved = serde_json::to_string(&next)
            .map_err(|e| e.to_string())
            .and_then(|raw| locked(&self.store)?.save_setting(READ_KEY, &raw));
        match saved {
            Ok(()) => {
                self.channels.read_through = next;
                self.changed(cx);
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn mark_visible_channel_read(&mut self, cx: &mut Context<Self>) {
        if self.channels.open
            && !self.channels.editing
            && self.channels.inspecting.is_none()
            && self.parent_window.is_some()
            && self.parent_window == cx.active_window()
            && let Some(id) = self.channels.selected.clone()
        {
            self.mark_channel_read(&id, cx);
        }
    }

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
                let unread = self.channel_unread(&id);
                div()
                    .id(SharedString::from(format!("classic-channel-{id}")))
                    .debug_selector(move || format!("classic-channel-{selector}"))
                    .role(Role::Button)
                    .aria_label(format!(
                        "Channel #{}{}",
                        channel.name,
                        if unread { ", unread messages" } else { "" }
                    ))
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
                    .child(div().w(px(16.)).flex_shrink_0().text_center().child("#"))
                    .child(
                        div()
                            .debug_selector({
                                let id = id.clone();
                                move || format!("channel-label-{id}")
                            })
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(channel.name.clone()),
                    )
                    .when(unread, |d| {
                        d.child(
                            div()
                                .debug_selector({
                                    let id = id.clone();
                                    move || format!("channel-unread-{id}")
                                })
                                .size(px(6.))
                                .flex_shrink_0()
                                .rounded_full()
                                .bg(accent_color()),
                        )
                    })
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
        self.channels.editing = false;
        if self.channels.selected != id {
            self.channels.message_cache.clear();
        }
        self.channels.selected = id;
        self.channels.inspecting = None;
        self.mark_visible_channel_read(cx);
        self.changed(cx);
        let owner = self.owner.clone();
        let manager = cx.weak_entity();
        window.defer(cx, move |window, cx| {
            let _ = owner.update(cx, |shell, cx| shell.open_classic_channels(window, cx));
            let _ = manager.update(cx, |this, cx| {
                if this.channels.selected.is_some() {
                    this.focus_channel_composer(window, cx);
                } else {
                    let name =
                        this.field("channel-new:name", "Channel name", String::new(), false, cx);
                    window.focus(&name.focus_handle(cx), cx);
                }
            });
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
        self.mark_visible_channel_read(cx);
        self.changed(cx);
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn channel_action(
        &mut self,
        action: &str,
        input: &Value,
        author: Option<&Bot>,
        request: &str,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        self.channel_action_with_hop(action, input, author, request, 0, cx)
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn channel_action_with_hop(
        &mut self,
        action: &str,
        input: &Value,
        author: Option<&Bot>,
        request: &str,
        hop: u8,
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
                    let message = next.post(id,author,input["message"].as_str().ok_or("Provide message")?,request,hop,&targets,&self.roster)?;
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
            if !view.read(cx).bot_configuration_ready() || view.read(cx).busy {
                continue;
            }
            let Some(primary) = self
                .chats
                .get(&bot.id)
                .filter(|v| v.read(cx).bot_configuration_ready())
            else {
                self.ensure_chat(&bot.id, cx);
                continue;
            };
            let configuration = primary.read(cx).settings.clone();
            let provider = Kind::from_id(&primary.read(cx).summary.provider);
            let mut context = bot.clone();
            // This lane owns its channel turn, not the primary Bot's queue.
            context.outbox.clear();
            let root = self.root.clone();
            let environment = self
                .bridge
                .as_ref()
                .map(|b| b.environment(view.read(cx).summary.id))
                .unwrap_or_default();
            view.update(cx, |v, cx| v.configure_bot(context, root, environment, cx));
            if !view.update(cx, |view, cx| {
                view.inherit_bot_configuration(provider, &configuration, cx)
            }) {
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
                peer: None,
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
        if !primary.read(cx).bot_configuration_ready() {
            self.channels.loading.remove(&key);
            return;
        }
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
                let valid = this.roster.get(&key.1).is_some()
                    && next.get(&key.0).is_some_and(|c| {
                        (c.member_ids.contains(&key.1)
                            || (saved.is_some() && this.channels.inspecting.as_ref() == Some(&key)))
                            && c.member_sessions.get(&key.1).copied() == saved
                    });
                if !valid {
                    // Only this task owns a newly created, unpublished session.
                    // Existing backing history is never deleted during a race.
                    if saved.is_none() {
                        let _ = locked(&this.store).and_then(|mut db| db.delete(summary.id));
                    }
                    return;
                }
                let channel = next.get_mut(&key.0).unwrap();
                channel.member_sessions.insert(key.1.clone(), summary.id);
                if let Err(e) = this.commit_channels(next, cx) {
                    this.error = Some(e);
                    if saved.is_none() {
                        let _ = locked(&this.store).and_then(|mut db| db.delete(summary.id));
                    }
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
                let mut bot = this.roster.get(&key.1).cloned().unwrap();
                bot.outbox.clear();
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
                this.observations.push(cx.observe(&view, |this, view, cx| {
                    if view.read(cx).bot_ready() {
                        this.channel_tick(cx);
                    }
                }));
                this.channels.views.insert(key, view);
                this.roster.hidden_sessions.push(id);
                this.save(cx);
                this.channel_tick(cx);
            });
        })
        .detach();
    }
    pub(super) fn channel_finished(&mut self, key: &(String, String), cx: &mut Context<Self>) {
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
        let (completed, status) = view.external_outcome(cx);
        let failed = !completed || rows.iter().any(|r| r.role == "error");
        let reply = bot::peers::final_reply(rows);
        let failed = failed || reply.is_err();
        let error = reply.as_ref().err().cloned().unwrap_or(status);
        let text = reply.unwrap_or_default();
        let mut next = self.channels.state.clone();
        if let Some(d) = next
            .get_mut(&key.0)
            .and_then(|c| c.deliveries.iter_mut().find(|d| d.id == delivery.id))
        {
            d.state = if failed { "interrupted" } else { "done" }.into();
            d.error = failed.then(|| format!("{error}\nInspect its conversation before retrying"));
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
        self.channel_surface(window, cx)
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
            manager.roster.bots.push(Bot { id: "reader".into(), name: "Reader".into(), ..Default::default() });
            manager.channels.state.channels.push(Channel {
                id: "team".into(),
                name: "team".into(),
                member_ids: vec!["reader".into()],
                messages: vec![json!({"id":"first","from":{"kind":"bot","botId":"reader"},"text":"First message"})],
                ..Default::default()
            });
            manager.changed(cx);
        });
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.simulate_resize(gpui::size(px(1400.), px(900.)));
        window
            .update(visual, |_, window, _| window.activate_window())
            .unwrap();
        visual.run_until_parked();
        let channel = visual.debug_bounds("classic-channel-team").unwrap();
        let label = visual.debug_bounds("channel-label-team").unwrap();
        let bot = visual.debug_bounds("bot-rail-reader").unwrap();
        let project = visual.debug_bounds("classic-project-label-0").unwrap();
        assert_eq!(label.left(), bot.left() + px(32.));
        assert_eq!(label.left(), project.left());
        assert!(visual.debug_bounds("channel-unread-team").is_some());
        visual.simulate_click(channel.center(), Default::default());
        visual.run_until_parked();
        manager.read_with(visual, |manager, _| {
            assert!(manager.classic_channels_open());
            assert!(manager.active.is_none());
            assert_eq!(manager.channels.selected.as_deref(), Some("team"));
            assert!(!manager.channel_unread("team"));
        });
        assert!(visual.debug_bounds("channel-unread-team").is_none());
        manager.update(visual, |manager, cx| {
            let mut next = manager.channels.state.clone();
            next.get_mut("team").unwrap().messages.push(json!({"id":"visible","from":{"kind":"bot","botId":"reader"},"text":"Visible reply"}));
            manager.commit_channels(next, cx).unwrap();
            assert!(!manager.channel_unread("team"));
        });
        visual.deactivate_window();
        manager.update(visual, |manager, cx| {
            let mut next = manager.channels.state.clone();
            next.get_mut("team").unwrap().messages.push(json!({"id":"background","from":{"kind":"bot","botId":"reader"},"text":"Reply while inactive"}));
            manager.commit_channels(next, cx).unwrap();
            assert!(manager.channel_unread("team"));
            let db = locked(&manager.store).unwrap();
            let seen: HashMap<String, String> = serde_json::from_str(&db.setting(READ_KEY).unwrap().unwrap()).unwrap();
            let restored = Channels::load(&db).unwrap();
            assert_eq!(seen["team"], "visible");
            assert_eq!(restored.get("team").unwrap().messages.last().unwrap()["id"], "background");
        });
        visual.run_until_parked();
        manager.read_with(visual, |manager, cx| {
            assert!(cx.active_window().is_none());
            assert!(manager.channel_unread("team"));
        });
        assert!(visual.debug_bounds("channel-unread-team").is_some());
        window
            .update(visual, |_, window, _| window.activate_window())
            .unwrap();
        visual.refresh().unwrap();
        visual.run_until_parked();
        assert!(visual.debug_bounds("channel-unread-team").is_none());
        assert!(visual.debug_bounds("channel-messages").is_some());
        assert!(visual.debug_bounds("classic-project-rail").is_some());
        assert!(visual.debug_bounds("sidebar-frame").is_none());

        assert!(visual.debug_bounds("close-channels").is_none());
        let close = visual.debug_bounds("classic-project-0").unwrap();
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
        visual.simulate_input("focused-channel-name");
        visual.run_until_parked();
        manager.read_with(visual, |manager, cx| {
            assert_eq!(
                manager.fields["channel-new:name"].read(cx).content,
                "focused-channel-name"
            );
        });
        let project = visual.debug_bounds("classic-project-0").unwrap();
        visual.simulate_click(project.center(), Default::default());
        visual.run_until_parked();
        assert!(!manager.read_with(visual, |manager, _| manager.classic_channels_open()));
        assert!(visual.debug_bounds("sidebar-frame").is_some());
    }
}
