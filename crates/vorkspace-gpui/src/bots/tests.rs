use super::*;
use gpui::{TestAppContext, WindowHandle};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, mpsc};
use vorkspace_core::store::Store;
#[path = "memory_tests.rs"]
mod memory_tests;
struct Setup {
    window: WindowHandle<Shell>,
    manager: Entity<Bots>,
    id: String,
    project: String,
    session: i64,
    root: PathBuf,
    store: Database,
}

#[gpui::test]
fn channel_settings_are_anchored_and_keep_the_conversation_and_draft_visible(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        crate::shell::bind_keys(cx);
        input::bind(cx);
        crate::popover::bind(cx);
        crate::composer_completion::bind(cx);
    });
    let s = setup(cx);
    let id = s.manager.update(cx, |m, cx| {
        let mut next = m.channels.state.clone();
        let id = next
            .create("settings-test", "Old topic", vec![s.id.clone()], &m.roster)
            .unwrap();
        next.post(
            &id,
            Some(m.roster.get(&s.id).unwrap()),
            "Keep this conversation",
            "note",
            0,
            &[],
            &m.roster,
        )
        .unwrap();
        m.commit_channels(next, cx).unwrap();
        m.channels.open = true;
        m.channels.selected = Some(id.clone());
        cx.notify();
        id
    });
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    for (width, height) in [(1400., 900.), (900., 650.)] {
        visual.simulate_resize(gpui::size(px(width), px(height)));
        visual.run_until_parked();
        visual.refresh().unwrap();
        let messages = visual.debug_bounds("channel-messages").unwrap();
        let composer = visual.debug_bounds("channel-composer").unwrap();
        assert!(visual.debug_bounds("close-channels").is_none());
        let gear = visual.debug_bounds("channel-settings").unwrap();
        visual.simulate_click(gear.center(), Default::default());
        visual.run_until_parked();
        visual.refresh().unwrap();
        let menu = visual.debug_bounds("channel-settings-menu").unwrap();
        assert!(menu.size.width <= px(400.) && menu.size.height < px(height));
        assert!(menu.right() <= px(width) && menu.bottom() <= px(height));
        assert_eq!(messages, visual.debug_bounds("channel-messages").unwrap());
        assert_eq!(composer, visual.debug_bounds("channel-composer").unwrap());
        s.manager.update(visual, |m, cx| {
            m.fields[&format!("channel-edit:{id}:topic")]
                .update(cx, |input, cx| input.set_text("Updated topic".into(), cx));
            m.fields[&format!("channel-draft:{id}")].update(cx, |input, cx| {
                input.set_text("Unsent channel draft".into(), cx)
            });
        });
        visual.run_until_parked();
        let save = visual.debug_bounds("save-channel").unwrap();
        visual.simulate_click(save.center(), Default::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("channel-settings-menu").is_none());
        s.manager.read_with(visual, |m, cx| {
            assert_eq!(m.channels.state.get(&id).unwrap().topic, "Updated topic");
            assert_eq!(m.channels.state.get(&id).unwrap().messages.len(), 1);
            assert_eq!(
                m.fields[&format!("channel-draft:{id}")].read(cx).content,
                "Unsent channel draft"
            );
        });
        let gear = visual.debug_bounds("channel-settings").unwrap();
        visual.simulate_click(gear.center(), Default::default());
        visual.run_until_parked();
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        assert!(visual.debug_bounds("channel-settings-menu").is_none());
    }
}

#[gpui::test]
fn peer_delivery_is_durable_replies_once_and_accepts_the_next_message(cx: &mut TestAppContext) {
    let s = setup(cx);
    s.manager.update(cx, |m, cx| {
        m.roster.bots.push(Bot {
            id: "peer-beta".into(),
            name: "Beta".into(),
            projects: vec![s.project.clone()],
            ..Default::default()
        });
        m.ensure_chat("peer-beta", cx);
        m.save(cx);
        m.chats[&s.id].update(cx, |v, _| v.busy = true);
    });
    cx.run_until_parked();
    s.manager.update(cx, |m, cx| {
        m.chats["peer-beta"].update(cx, |v, _| v.busy = true);
    });
    let input = json!({"to":"Beta","message":"Review the approach"});
    let first = call(&s, "peer-one", "bots.send", input.clone(), cx)
        .recv()
        .unwrap()
        .unwrap();
    assert_eq!(
        call(&s, "peer-one", "bots.send", input, cx)
            .recv()
            .unwrap()
            .unwrap(),
        first
    );
    s.manager.update(cx, |m, cx| {
        let saved = Roster::load(&locked(&s.store).unwrap()).unwrap();
        let sent = saved.get("peer-beta").unwrap().outbox[0].clone();
        assert_eq!(saved.get("peer-beta").unwrap().outbox.len(), 1);
        assert_eq!(sent.peer.as_ref().unwrap().hop, 1);
        let recipient = m.chats["peer-beta"].clone();
        recipient.update(cx, |v, cx| {
            v.busy = false;
            v.bot_post("bot-system", sent.text.clone(), cx).unwrap();
            v.bot_post("assistant", "The approach is sound.".into(), cx)
                .unwrap();
        });
        m.inflight.insert("peer-beta".into(), sent);
        m.chat_changed("peer-beta", cx);
        assert!(m.roster.get("peer-beta").unwrap().outbox.is_empty());
        assert!(!m.inflight.contains_key("peer-beta"));
        assert_eq!(recipient.read(cx).bot_status(), "idle");
        let reply = &m.roster.get(&s.id).unwrap().outbox[0];
        assert!(reply.text.ends_with("The approach is sound."));
        assert!(!reply.peer.as_ref().unwrap().expect_reply);
        assert_eq!(reply.peer.as_ref().unwrap().hop, 2);
        recipient.update(cx, |v, _| v.busy = true);
    });
    cx.run_until_parked();
    call(
        &s,
        "peer-two",
        "bots.send",
        json!({"to":"Beta","message":"Check recovery next","expectReply":false}),
        cx,
    )
    .recv()
    .unwrap()
    .unwrap();
    s.manager.read_with(cx, |m, _| {
        assert_eq!(m.roster.get("peer-beta").unwrap().outbox.len(), 1);
        assert!(
            m.roster.get("peer-beta").unwrap().outbox[0]
                .text
                .ends_with("Check recovery next")
        );
    });
}

#[gpui::test]
fn switching_from_peer_to_channel_keeps_the_autonomous_budget(cx: &mut TestAppContext) {
    let s = setup(cx);
    s.manager.update(cx, |m, cx| {
        let mut sent = Message {
            peer: Some(bot::peers::Peer {
                from: "peer".into(),
                hop: 4,
                expect_reply: false,
            }),
            id: "hop-cap".into(),
            text: "Review".into(),
            attachments: Vec::new(),
            internal: true,
            sent_at: None,
            error: None,
        };
        m.inflight.insert(s.id.clone(), sent.clone());
        m.chats[&s.id].update(cx, |v, _| v.busy = true);
        assert_eq!(m.collaboration_hop(s.session, cx), 4);
        sent.peer.as_mut().unwrap().hop = 3;
        m.inflight.insert(s.id.clone(), sent);
        assert_eq!(m.collaboration_hop(s.session, cx), 3);
    });
    let result = call(
        &s,
        "hop-channel",
        "channels.create",
        json!({"name":"hop-test","members":[s.id]}),
        cx,
    )
    .recv()
    .unwrap()
    .unwrap();
    let id = result["id"].as_str().unwrap();
    call(
        &s,
        "hop-post",
        "channels.post",
        json!({"channel":id,"message":"End of chain"}),
        cx,
    )
    .recv()
    .unwrap()
    .unwrap();
    s.manager.read_with(cx, |m, _| {
        let channel = m.channels.state.get(id).unwrap();
        assert_eq!(channel.messages[0]["hop"], 3);
        assert!(channel.deliveries.is_empty()); // The author never wakes itself.
    });
}

#[gpui::test]
fn submitted_peer_messages_pause_after_reopening_instead_of_replaying(cx: &mut TestAppContext) {
    let s = setup(cx);
    s.manager.update(cx, |m, cx| {
        m.roster.bots.push(Bot {
            id: "recovery-sender".into(),
            name: "Sender".into(),
            ..Default::default()
        });
        bot::peers::enqueue(
            &mut m.roster,
            "recovery-sender",
            &s.id,
            "A submitted request",
            "recovery",
            1,
            true,
        )
        .unwrap();
        let sent = &m.roster.get(&s.id).unwrap().outbox[0];
        locked(&s.store)
            .unwrap()
            .save_setting(&format!("bot-delivered:{}:{}", s.session, sent.id), "yes")
            .unwrap();
        m.roster.save(&locked(&s.store).unwrap()).unwrap();
        m.chats.remove(&s.id);
        m.ensure_chat(&s.id, cx);
    });
    cx.run_until_parked();
    s.manager.read_with(cx, |m, cx| {
        let message = &m.roster.get(&s.id).unwrap().outbox[0];
        assert!(message.error.as_deref().unwrap().contains("Interrupted"));
        assert_eq!(message.peer.as_ref().unwrap().hop, 2);
        assert!(!m.chats[&s.id].read(cx).busy);
        assert!(!m.inflight.contains_key(&s.id));
    });
}

#[gpui::test]
fn cold_peer_recovery_loads_inactive_recipients_and_pauses_submitted_work(cx: &mut TestAppContext) {
    for (submitted, human_head) in [(false, false), (false, true), (true, false), (true, true)] {
        let s = setup(cx);
        let (owner, prefs, receipt) = s.manager.update(cx, |m, _| {
            m.roster.bots.extend([
                Bot {
                    id: "cold-sender".into(),
                    name: "Sender".into(),
                    ..Default::default()
                },
                Bot {
                    id: "unrelated-idle".into(),
                    name: "Idle".into(),
                    ..Default::default()
                },
            ]);
            if human_head {
                m.roster.get_mut(&s.id).unwrap().outbox.push(Message {
                    id: "cold-human".into(),
                    text: "Already queued human work".into(),
                    peer: None,
                    error: None,
                    internal: false,
                    sent_at: None,
                    attachments: Vec::new(),
                });
            }
            bot::peers::enqueue(
                &mut m.roster,
                "cold-sender",
                &s.id,
                "Resume this request",
                "cold-request",
                0,
                true,
            )
            .unwrap();
            let receipt = m
                .roster
                .get(&s.id)
                .unwrap()
                .outbox
                .last()
                .unwrap()
                .id
                .clone();
            let db = locked(&s.store).unwrap();
            let mut settings = db.agent_settings(s.session).unwrap().unwrap_or_default();
            // Hold the restored owner at an existing admission gate. This
            // tests cold recovery without launching a real provider process.
            settings.usage_limit = Some(Default::default());
            db.save_agent_settings(s.session, &settings).unwrap();
            if submitted {
                db.save_setting(&format!("bot-delivered:{}:{receipt}", s.session), "yes")
                    .unwrap();
            }
            m.roster.save(&db).unwrap();
            (m.owner.clone(), m.prefs.clone(), receipt)
        });
        let restored = cx.new(|cx| {
            Bots::new(
                s.store.clone(),
                owner,
                prefs,
                Some("cold-sender".into()),
                AnchorSlot::default(),
                cx,
            )
        });
        cx.run_until_parked();
        restored.read_with(cx, |m, cx| {
            assert_eq!(m.active.as_deref(), Some("cold-sender"));
            assert!(
                m.chats.contains_key(&s.id),
                "an inactive recipient must load without being opened"
            );
            assert!(
                !m.chats.contains_key("unrelated-idle"),
                "idle Bots need no new owner"
            );
            assert!(!m.chats[&s.id].read(cx).busy);
            assert!(!m.inflight.contains_key(&s.id));
            let message = m
                .roster
                .get(&s.id)
                .unwrap()
                .outbox
                .iter()
                .find(|message| message.id == receipt)
                .unwrap();
            assert_eq!(message.id, receipt);
            if submitted {
                assert!(message.error.as_deref().unwrap().contains("Interrupted"));
            } else {
                assert!(message.error.is_none());
                assert!(m.persisted_messages.contains(&receipt));
            }
        });
        if !submitted {
            restored.update(cx, |m, _| {
                m.chats.remove(&s.id);
                m.prefs.bots = false;
            });
            gpui::AnyWindowHandle::from(s.window)
                .update(cx, |_, window, cx| {
                    restored.update(cx, |m, cx| m.tick(window, cx))
                })
                .unwrap();
            cx.run_until_parked();
            restored.read_with(cx, |m, _| {
                assert!(
                    m.chats.contains_key(&s.id),
                    "hiding Bots does not abandon accepted work"
                )
            });
        }
        let saved = Roster::load(&locked(&s.store).unwrap()).unwrap();
        assert_eq!(
            saved
                .get(&s.id)
                .unwrap()
                .outbox
                .iter()
                .find(|message| message.id == receipt)
                .unwrap()
                .error
                .is_some(),
            submitted
        );
    }
}

#[gpui::test]
fn usage_resume_preserves_uncertain_peer_requests_until_explicit_retry(cx: &mut TestAppContext) {
    use vorkspace_core::store::SubmissionInput;
    let s = setup(cx);
    let (old, text, peer) = s.manager.update(cx, |m, cx| {
        m.roster.bots.push(Bot {
            id: "usage-sender".into(),
            name: "Sender".into(),
            ..Default::default()
        });
        bot::peers::enqueue(
            &mut m.roster,
            "usage-sender",
            &s.id,
            "Finish this request",
            "usage-request",
            0,
            true,
        )
        .unwrap();
        let message = &mut m.roster.get_mut(&s.id).unwrap().outbox[0];
        message.error = Some("Usage limit; inspect the uncertain turn".into());
        let result = (
            message.id.clone(),
            message.text.clone(),
            message.peer.clone(),
        );
        m.roster.get_mut(&s.id).unwrap().outbox.push(Message {
            id: "human-queued".into(),
            text: "Queued human follow-up".into(),
            error: Some("Usage limit".into()),
            peer: None,
            internal: false,
            sent_at: None,
            attachments: Vec::new(),
        });
        m.chats[&s.id].update(cx, |v, _| v.busy = true);
        m.save(cx);
        result
    });
    let input = |attempt: &str, turn: &str| SubmissionInput {
        attempt_id: attempt.into(),
        user_turn_id: turn.into(),
        session_id: String::new(),
        payload_json: "{}".into(),
        user_turn: None,
    };
    {
        let db = locked(&s.store).unwrap();
        assert!(
            db.reserve_local_submission(s.session, input("uncertain-peer-attempt", &old))
                .unwrap()
                .created
        );
        assert!(
            db.dispatch_local_submission("uncertain-peer-attempt")
                .unwrap()
                .claimed
        );
        db.settle_local_submission(
            "uncertain-peer-attempt",
            "uncertain",
            Some("Usage limit".into()),
        )
        .unwrap();
    }
    cx.run_until_parked();
    s.manager.update(cx, |m, cx| {
        m.chats[&s.id].update(cx, |v, cx| {
            v.settings.usage_limit = Some(Default::default());
            v.busy = false;
            v.resume_usage(cx);
            // Hold execution before deferred recovery events are delivered.
            v.busy = true;
        })
    });
    cx.run_until_parked();
    let fresh = s.manager.update(cx, |m, cx| {
        let queue = &m.roster.get(&s.id).unwrap().outbox;
        assert_eq!(queue[0].id, old);
        assert_eq!(queue[0].text, text);
        assert_eq!(queue[0].peer, peer);
        assert!(queue[0].error.as_deref().unwrap().contains("uncertain"));
        assert!(
            queue[1].error.is_none(),
            "ordinary usage recovery remains available"
        );
        assert!(m.roster.get("usage-sender").unwrap().outbox.is_empty());
        m.queue_changed(&s.id, &old, false, cx);
        let fresh = m.roster.get(&s.id).unwrap().outbox[0].id.clone();
        assert_ne!(fresh, old);
        assert!(
            !m.persisted_messages.contains(&fresh),
            "Retry must await ordered persistence"
        );
        fresh
    });
    cx.run_until_parked();
    s.manager.read_with(cx, |m, _| {
        let retry = &m.roster.get(&s.id).unwrap().outbox[0];
        assert!(m.persisted_messages.contains(&fresh));
        assert!(retry.error.is_none());
        assert_eq!(retry.text, text);
        assert_eq!(retry.peer, peer);
    });
    let db = locked(&s.store).unwrap();
    assert!(
        db.reserve_local_submission(s.session, input("replay-old-peer", &old))
            .is_err()
    );
    assert!(
        db.reserve_local_submission(s.session, input("fresh-peer-attempt", &fresh))
            .unwrap()
            .created
    );
    assert_eq!(
        db.local_submissions(s.session)
            .unwrap()
            .iter()
            .find(|r| r.attempt_id == "uncertain-peer-attempt")
            .unwrap()
            .phase,
        "uncertain"
    );
}

#[gpui::test]
fn peer_history_app_calls_page_public_text_without_loading_a_peer_conversation(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    s.manager.update(cx, |m, _| {
        let mut db = locked(&s.store).unwrap();
        let session = db.create("codex", &s.project).unwrap();
        db.write_rows(
            session.id,
            0,
            &[
                vorkspace_core::transcript::Row {
                    role: "assistant".into(),
                    text: "Earlier reply".into(),
                    first: true,
                    sent_at: None,
                },
                vorkspace_core::transcript::Row {
                    role: "tool".into(),
                    text: "Private output".repeat(100_000),
                    first: true,
                    sent_at: None,
                },
                vorkspace_core::transcript::Row {
                    role: "assistant".into(),
                    text: "Latest reply".into(),
                    first: true,
                    sent_at: None,
                },
            ],
        )
        .unwrap();
        m.roster.bots.push(Bot {
            id: "read-peer".into(),
            name: "Reader target".into(),
            session_id: Some(session.id),
            ..Default::default()
        });
    });
    let result = call(
        &s,
        "bounded-peer-read",
        "bots.read",
        json!({"bot":"read-peer","limit":2,"maxChars":100}),
        cx,
    )
    .recv()
    .unwrap()
    .unwrap();
    assert_eq!(result["bot"]["id"], "read-peer");
    assert_eq!(
        result["messages"],
        json!([{"role":"assistant","text":"Latest reply","truncated":false}])
    );
    assert_eq!(result["before"], 1);
    assert_eq!(result["hasMore"], true);
    s.manager
        .read_with(cx, |m, _| assert!(!m.chats.contains_key("read-peer")));
    let own = call(
        &s,
        "reject-self-peer-read",
        "bots.read",
        json!({"bot":s.id}),
        cx,
    )
    .recv()
    .unwrap();
    assert!(own.unwrap_err().contains("own conversation"));
}

#[gpui::test]
fn stopped_peer_turn_keeps_its_request_and_never_forwards_a_late_partial_reply(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    s.manager.update(cx, |m, cx| {
        m.roster.bots.push(Bot {
            id: "stop-sender".into(),
            name: "Sender".into(),
            ..Default::default()
        });
        bot::peers::enqueue(
            &mut m.roster,
            "stop-sender",
            &s.id,
            "Review this",
            "stop-request",
            0,
            true,
        )
        .unwrap();
        let sent = m.roster.get(&s.id).unwrap().outbox[0].clone();
        m.chats[&s.id].update(cx, |v, cx| {
            v.bot_post("bot-system", sent.text.clone(), cx).unwrap();
            v.bot_post("assistant", "Unfinished answer".into(), cx)
                .unwrap();
            v.busy = true;
        });
        m.inflight.insert(s.id.clone(), sent);
        m.save(cx);
    });
    cx.run_until_parked();
    s.manager.update(cx, |m, cx| {
        m.chats[&s.id].update(cx, |v, cx| v.bot_stop(cx));
        m.chat_changed(&s.id, cx);
        assert!(
            m.inflight.contains_key(&s.id),
            "Stop must wait for worker settlement"
        );
        assert!(m.roster.get("stop-sender").unwrap().outbox.is_empty());
        assert_eq!(m.roster.get(&s.id).unwrap().outbox.len(), 1);
    });
    cx.run_until_parked();
    s.manager.update(cx, |m, cx| {
        m.chats[&s.id].update(cx, |v, cx| {
            assert!(v.notification_was_cancelled());
            v.app_settle_stopped_turn(cx);
            v.bot_post("assistant", "Late partial answer".into(), cx)
                .unwrap();
        });
        m.chat_changed(&s.id, cx);
        m.chat_changed(&s.id, cx);
        assert!(!m.inflight.contains_key(&s.id));
        assert!(m.roster.get("stop-sender").unwrap().outbox.is_empty());
        let queue = &m.roster.get(&s.id).unwrap().outbox;
        assert_eq!(queue.len(), 1);
        assert!(queue[0].error.as_deref().unwrap().starts_with("Stopped"));
        assert!(
            m.chats[&s.id]
                .read(cx)
                .bot_rows(cx)
                .iter()
                .any(|row| row.text == "Late partial answer")
        );
    });
    cx.run_until_parked();
    let saved = Roster::load(&locked(&s.store).unwrap()).unwrap();
    assert!(saved.get("stop-sender").unwrap().outbox.is_empty());
    assert!(
        saved.get(&s.id).unwrap().outbox[0]
            .error
            .as_deref()
            .unwrap()
            .starts_with("Stopped")
    );
}

#[gpui::test]
fn channel_workers_use_the_live_bot_model_for_new_and_reused_lanes(cx: &mut TestAppContext) {
    let s = setup(cx);
    let id = s.manager.update(cx, |m, cx| {
        m.chats[&s.id].update(cx, |v, cx| {
            v.bot_config(
                Kind::Codex,
                Some("selected-codex-model".into()),
                HashMap::new(),
                cx,
            )
            .unwrap();
            assert_ne!(
                v.settings.model,
                v.bot_model(),
                "exercise the stale loaded Settings field"
            );
        });
        let mut next = m.channels.state.clone();
        let id = next
            .create("live-model", "", vec![s.id.clone()], &m.roster)
            .unwrap();
        m.commit_channels(next, cx).unwrap();
        m.channels.inspecting = Some((id.clone(), s.id.clone()));
        m.channel_tick(cx);
        id
    });
    cx.run_until_parked();
    s.manager.update(cx, |m, cx| {
        let key = (id.clone(), s.id.clone());
        assert_eq!(
            m.channels.views[&key].read(cx).bot_model().as_deref(),
            Some("selected-codex-model")
        );
        m.chats[&s.id].update(cx, |v, cx| {
            v.bot_config(
                Kind::Claude,
                Some("selected-claude-model".into()),
                HashMap::new(),
                cx,
            )
            .unwrap();
        });
        let primary = m.chats[&s.id].read(cx);
        let provider = Kind::from_id(&primary.summary.provider);
        let configuration = primary.bot_execution_settings();
        m.channels.views[&key].update(cx, |worker, cx| {
            assert!(worker.inherit_bot_configuration(provider, &configuration, cx))
        });
        let worker = m.channels.views[&key].read(cx);
        assert_eq!(worker.summary.provider, Kind::Claude.id());
        assert_eq!(worker.bot_model().as_deref(), Some("selected-claude-model"));
        assert!(
            !worker.busy,
            "configuration inheritance must not itself start provider execution"
        );
        assert!(m.channels.state.get(&id).unwrap().deliveries.is_empty());
    });
}

#[gpui::test]
fn channel_workers_refresh_member_permissions_account_and_model_without_losing_history(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let summary = locked(&s.store)
        .unwrap()
        .create("codex", &s.project)
        .unwrap();
    let worker = cx.new(|cx| Conversation::new(summary, s.store.clone(), cx));
    worker.update(cx, |v, cx| v.load(cx));
    cx.run_until_parked();
    worker.update(cx, |v, cx| {
        v.bot_post("assistant", "Previous channel reply".into(), cx)
            .unwrap();
        locked(&s.store)
            .unwrap()
            .provider_id(v.summary.id, "previous-account-thread")
            .unwrap();
        v.summary.provider_id = Some("previous-account-thread".into());
        v.settings.usage_limit = Some(Default::default());
        let settings = Settings {
            account: vorkspace_core::accounts::Profile::new("work", &s.root),
            access: AccessMode::ReadOnly,
            model: Some("chosen-model".into()),
            effort: Some("high".into()),
            service_tier: Some("priority".into()),
            ..Default::default()
        };
        assert!(v.inherit_bot_configuration(Kind::Codex, &settings, cx));
        assert_eq!(v.settings.access, AccessMode::ReadOnly);
        assert_eq!(v.settings.account.id, "work");
        assert!(v.settings.usage_limit.is_none());
        assert!(v.summary.provider_id.is_none());
        assert!(
            locked(&s.store)
                .unwrap()
                .bot_session(v.summary.id)
                .unwrap()
                .unwrap()
                .provider_id
                .is_none()
        );
        assert!(
            v.settings
                .handoff
                .as_ref()
                .unwrap()
                .brief
                .contains("Previous channel reply")
        );
        assert_eq!(v.bot_model(), settings.model);
        assert_eq!(v.settings.effort.as_deref(), Some("high"));
        assert_eq!(v.settings.service_tier.as_deref(), Some("priority"));
        assert_eq!(
            v.bot_rows(cx).last().unwrap().text,
            "Previous channel reply"
        );
        v.busy = true;
        assert!(!v.inherit_bot_configuration(Kind::Codex, &Settings::default(), cx));
        assert_eq!(v.settings.account.id, "work");
    });
}

#[gpui::test]
fn channel_receipts_survive_retry_and_unreadable_history_is_never_replaced(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let created = call(
        &s,
        "channel-create",
        "channels.create",
        json!({"name":"Review","members":[s.id]}),
        cx,
    )
    .recv()
    .unwrap()
    .unwrap();
    let id = created["id"].as_str().unwrap();
    let posted = call(
        &s,
        "channel-post",
        "channels.post",
        json!({"channel":id,"message":"A durable note"}),
        cx,
    )
    .recv()
    .unwrap()
    .unwrap();
    let retry = call(
        &s,
        "channel-post",
        "channels.post",
        json!({"channel":id,"message":"A durable note"}),
        cx,
    )
    .recv()
    .unwrap()
    .unwrap();
    assert_eq!(posted, retry);
    assert!(
        call(
            &s,
            "channel-post",
            "channels.post",
            json!({"channel":id,"message":"Changed input"}),
            cx
        )
        .recv()
        .unwrap()
        .is_err()
    );
    let before = locked(&s.store)
        .unwrap()
        .setting("bot-channels-native-v1")
        .unwrap();
    s.manager.update(cx, |m, cx| {
        assert_eq!(m.channels.state.get(id).unwrap().messages.len(), 1);
        m.channels.load_error = Some("unreadable saved history".into());
        assert!(
            m.channel_action(
                "channels.create",
                &json!({"name":"Another","members":[s.id]}),
                None,
                "new",
                cx
            )
            .is_err()
        );
    });
    assert_eq!(
        before,
        locked(&s.store)
            .unwrap()
            .setting("bot-channels-native-v1")
            .unwrap()
    );
}

#[gpui::test]
fn channels_keep_drafts_when_hidden_and_render_in_the_native_shell(cx: &mut TestAppContext) {
    let s = setup(cx);
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    s.manager.update(visual, |m, cx| {
        let result = m
            .channel_action(
                "channels.create",
                &json!({"name":"Design","members":[s.id]}),
                None,
                "create",
                cx,
            )
            .unwrap();
        let id = result["id"].as_str().unwrap().to_string();
        let key = format!("channel-draft:{id}");
        locked(&m.store)
            .unwrap()
            .save_setting(&key, "Keep this unsent draft")
            .unwrap();
        m.channels.selected = Some(id);
        m.channels.open = true;
        cx.notify();
    });
    visual.run_until_parked();
    visual.refresh().unwrap();
    s.manager.read_with(visual, |m, cx| {
        let id = m.channels.selected.as_ref().unwrap();
        assert_eq!(
            m.fields[&format!("channel-draft:{id}")].read(cx).content,
            "Keep this unsent draft"
        );
    });
    s.manager.update(visual, |m, cx| {
        m.channels.open = false;
        cx.notify();
    });
    visual.run_until_parked();
    s.manager.update(visual, |m, cx| {
        m.channels.open = true;
        cx.notify();
    });
    visual.run_until_parked();
    visual.refresh().unwrap();
    s.manager.read_with(visual, |m, cx| {
        let id = m.channels.selected.as_ref().unwrap();
        assert_eq!(
            m.fields[&format!("channel-draft:{id}")].read(cx).content,
            "Keep this unsent draft"
        );
    });
}

#[gpui::test]
fn channel_composer_fits_both_sizes_and_failed_sends_preserve_the_draft(cx: &mut TestAppContext) {
    cx.update(input::bind);
    let s = setup(cx);
    let id = s.manager.update(cx, |m, cx| {
        let mut next = m.channels.state.clone();
        let id = next
            .create("layout", "", vec![s.id.clone()], &m.roster)
            .unwrap();
        next.post(
            &id,
            Some(m.roster.get(&s.id).unwrap()),
            "## Heading\n\n- Item",
            "layout-note",
            0,
            &[],
            &m.roster,
        )
        .unwrap();
        m.commit_channels(next, cx).unwrap();
        m.channels.open = true;
        m.channels.selected = Some(id.clone());
        // Keep this UI test local: admitted turns cannot start a provider.
        m.channels.loading.insert((id.clone(), s.id.clone()));
        cx.notify();
        id
    });
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    for (width, height) in [(1400., 900.), (900., 650.)] {
        visual.simulate_resize(gpui::size(px(width), px(height)));
        visual.run_until_parked();
        visual.refresh().unwrap();
        let surface = visual.debug_bounds("channel-surface").unwrap();
        let composer = visual.debug_bounds("channel-composer").unwrap();
        let send = visual.debug_bounds("send-channel").unwrap();
        assert!(composer.left() >= surface.left() && composer.right() <= surface.right());
        assert!(send.bottom() <= surface.bottom());
        assert!(send.size.width < px(100.));
        assert!(visual.debug_bounds("save-channel").is_none());
    }
    let cached = s.manager.read_with(visual, |m, _| {
        m.channels.message_cache.values().next().unwrap().clone()
    });
    let handle: gpui::AnyWindowHandle = s.window.into();
    handle
        .update(visual, |_, window, cx| {
            s.manager.update(cx, |m, cx| {
                let input = m.fields[&format!("channel-draft:{id}")].clone();
                input.update(cx, |i, cx| i.set_text("Keep this draft".into(), cx));
                m.channels.load_error = Some("Injected unavailable storage".into());
                window.focus(&input.focus_handle(cx), cx);
            })
        })
        .unwrap();
    visual.run_until_parked();
    visual.simulate_keystrokes("shift-enter");
    visual.run_until_parked();
    s.manager.read_with(visual, |m, cx| {
        assert!(
            m.fields[&format!("channel-draft:{id}")]
                .read(cx)
                .content
                .contains('\n')
        );
        assert!(Arc::ptr_eq(
            &cached,
            m.channels.message_cache.values().next().unwrap()
        ));
    });
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    s.manager.update(visual, |m, cx| {
        assert!(
            !m.fields[&format!("channel-draft:{id}")]
                .read(cx)
                .content
                .is_empty()
        );
        assert_eq!(m.channels.state.get(&id).unwrap().messages.len(), 1);
        m.channels.load_error = None;
    });
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    s.manager.read_with(visual, |m, cx| {
        assert!(
            m.fields[&format!("channel-draft:{id}")]
                .read(cx)
                .content
                .is_empty()
        );
        assert_eq!(m.channels.state.get(&id).unwrap().messages.len(), 2);
        assert_eq!(
            locked(&s.store)
                .unwrap()
                .setting(&format!("channel-draft:{id}"))
                .unwrap()
                .as_deref(),
            Some("")
        );
    });
}

#[gpui::test]
fn stopped_channel_work_cannot_publish_a_late_reply(cx: &mut TestAppContext) {
    let s = setup(cx);
    s.manager.update(cx, |m, cx| {
        let mut next = m.channels.state.clone();
        let id = next
            .create("stop-test", "", vec![s.id.clone()], &m.roster)
            .unwrap();
        next.post(&id, None, "Inspect this", "stop-request", 0, &[], &m.roster)
            .unwrap();
        let delivery = &mut next.get_mut(&id).unwrap().deliveries[0];
        delivery.state = "running".into();
        let delivery_id = delivery.id.clone();
        m.commit_channels(next, cx).unwrap();
        let key = (id.clone(), s.id.clone());
        m.channels.views.insert(key.clone(), m.chats[&s.id].clone());
        m.channels.starts.insert(delivery_id.clone(), 0);
        m.stop_channel_delivery(&id, &delivery_id, cx);
        let view = m.chats[&s.id].clone();
        view.update(cx, |v, cx| {
            v.bot_post("assistant", "Late reply".into(), cx).unwrap()
        });
        m.channel_finished(&key, cx);
        assert_eq!(m.channels.state.get(&id).unwrap().messages.len(), 1);
        assert_eq!(
            m.channels.state.get(&id).unwrap().deliveries[0].state,
            "interrupted"
        );
        assert_eq!(
            bot::channels::Channels::load(&locked(&s.store).unwrap())
                .unwrap()
                .get(&id)
                .unwrap()
                .messages
                .len(),
            1
        );
    });
}

#[gpui::test]
fn missing_document_uses_error_cards_without_moving_content(cx: &mut TestAppContext) {
    let s = setup(cx);
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    visual.refresh().unwrap();
    let body = visual.debug_bounds("main-pane-frame").unwrap();
    s.manager.update(visual, |m, cx| {
        m.failed_reports.insert("failed-report".into());
        m.open_document("deleted-document", cx);
    });
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert_eq!(
        s.manager
            .read_with(visual, |m, _| m.error.clone())
            .as_deref(),
        Some("Document unavailable. It may have been deleted.")
    );
    let error = visual.debug_bounds("bot-error").unwrap();
    let viewport = s
        .window
        .update(visual, |_, w, _| w.viewport_size())
        .unwrap();
    assert_eq!(error.top(), px(12.));
    assert_eq!(error.right(), viewport.width - px(12.));
    assert_eq!(error.size.width, px(360.));
    assert_eq!(visual.debug_bounds("main-pane-frame").unwrap(), body);

    let close = visual.debug_bounds("bot-error-close").unwrap();
    visual.simulate_click(close.center(), Default::default());
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("bot-error").is_none());
    s.manager.read_with(visual, |m, _| {
        assert!(m.error.is_none());
        assert!(m.failed_reports.is_empty());
        assert!(
            m.settings.is_some(),
            "dismissing an error keeps settings open"
        );
        assert_eq!(m.active.as_deref(), Some(s.id.as_str()));
    });
}

#[gpui::test]
fn saved_desktop_bot_opens_in_actual_manager_without_replacing_its_conversation(
    cx: &mut TestAppContext,
) {
    let db = Store::open_vorkspace(std::path::Path::new(":memory:")).unwrap();
    let session = db.create("codex", "/saved-project").unwrap();
    let original = db.persisted_session_id(session.id).unwrap();
    let settings = Settings {
        model: Some("gpt-6.1-sol".into()),
        access: AccessMode::Auto,
        ..Default::default()
    };
    db.configure_agent(session.id, "codex", &settings).unwrap();
    let mut roster = Roster::default();
    roster.bots.push(Bot {
        id: "desktop-original-bot".into(),
        session_id: Some(session.id),
        name: "Big Boss".into(),
        mascot: "ghost".into(),
        color: 0xe25a9e,
        projects: vec!["/saved-project".into()],
        ..Default::default()
    });
    roster.save(&db).unwrap();
    let summaries = db.sessions().unwrap();
    let store = Arc::new(Mutex::new(db));
    let database = store.clone();
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| Shell::new(database, summaries, "/saved-project".into(), None, cx))
        })
        .unwrap()
    });
    cx.run_until_parked();
    let manager = window
        .update(cx, |shell, _, _| shell.bot_manager())
        .unwrap();
    window
        .update(cx, |_, window, cx| {
            manager.update(cx, |m, cx| m.open("desktop-original-bot", window, cx));
        })
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        manager.update(cx, |m, cx| {
            m.ensure_chat("desktop-original-bot", cx);
            let bot = m.current().unwrap();
            assert_eq!(bot.id, "desktop-original-bot");
            assert_eq!(bot.session_id, Some(session.id));
            assert_eq!(bot.name, "Big Boss");
            assert_eq!(bot.mascot, "ghost");
            let chat = m.chats[&bot.id].read(cx);
            assert_eq!(chat.summary.id, session.id);
            assert_eq!(chat.summary.provider, "codex");
            assert_eq!(chat.settings.model.as_deref(), Some("gpt-6.1-sol"));
            assert_eq!(m.chats.len(), 1);
        });
    });
    let db = locked(&store).unwrap();
    assert_eq!(db.sessions().unwrap().len(), 1);
    assert_eq!(db.persisted_session_id(session.id).unwrap(), original);
}

#[gpui::test]
fn settings_reset_restores_only_bot_soul_and_name(cx: &mut TestAppContext) {
    let s = setup(cx);
    bot::files::save(&s.root, &s.id, None, "SOUL.md", "# Custom soul", None).unwrap();
    bot::files::save(&s.root, &s.id, None, "MEMORY.md", "Keep this memory", None).unwrap();
    let before = cx.update(|cx| {
        s.manager.update(cx, |m, cx| {
            m.roster.get_mut(&s.id).unwrap().name = "Custom name".into();
            m.save(cx);
            m.roster.get(&s.id).unwrap().clone()
        })
    });
    cx.run_until_parked();
    let _reset = cx.update(|cx| s.manager.update(cx, |m, cx| m.reset_defaults(&s.id, cx)));
    cx.run_until_parked();
    let roster = Roster::load(&locked(&s.store).unwrap()).unwrap();
    let after = roster.get(&s.id).unwrap();
    assert!(after.name.is_empty());
    assert_eq!(after.session_id, before.session_id);
    assert_eq!(after.projects, before.projects);
    assert_eq!(after.habit_sessions, before.habit_sessions);
    let files = bot::files::load(&s.root, &s.id, None).unwrap();
    assert_eq!(files.soul.as_deref(), Some(bot::DEFAULT_SOUL));
    assert_eq!(files.memory, "Keep this memory");
}

#[gpui::test]
fn detail_pages_slide_over_the_overview_and_back_without_recreating_content(
    cx: &mut TestAppContext,
) {
    for kind in ["markdown", "html", "session"] {
        cx.update(|cx| cx.set_reduce_motion(true));
        let s = setup(cx);
        let session = s
            .window
            .update(cx, |shell, _, _| shell.workspace.active.unwrap())
            .unwrap();
        s.window
            .update(cx, |_, _, cx| {
                s.manager.update(cx, |m, cx| {
                    m.settings = None;
                    m.overview_tab = OverviewTab::Documents;
                    m.roster
                        .get_mut(&s.id)
                        .unwrap()
                        .documents
                        .push(bot::Document {
                            id: "slide-document".into(),
                            kind: kind.into(),
                            title: "Slide document".into(),
                            summary: String::new(),
                            body: "# Document\n\nKeep this reader alive.".into(),
                            created_at: 1,
                            updated_at: 1,
                        });
                    cx.notify();
                });
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
        visual.run_until_parked();
        visual.update(|_, cx| cx.set_reduce_motion(false));
        let action = if kind == "session" {
            json!({"type":"session", "sessionId":session})
        } else {
            json!({"type":"document", "documentId":"slide-document"})
        };
        s.window
            .update(visual, |_, _, cx| {
                s.manager
                    .update(cx, |m, cx| m.card_action(&s.id, &action, cx))
            })
            .unwrap();
        visual.run_until_parked();
        let page: &'static str = Box::leak(
            format!(
                "bot-detail-page-{}",
                if kind == "session" {
                    format!("session:{session}")
                } else {
                    "document:slide-document".into()
                }
            )
            .into_boxed_str(),
        );
        let advance = |visual: &mut gpui::VisualTestContext, ms| {
            let details = s.manager.read_with(visual, |m, _| m.details.clone());
            details.update(visual, |d, cx| d.elapsed(Duration::from_millis(ms), cx));
            visual
                .update_window(s.window.into(), |_, w, cx| {
                    w.simulate_next_frame(cx);
                    w.draw(cx).clear(cx);
                })
                .unwrap();
        };
        advance(visual, 70);
        let stack = visual.debug_bounds("bot-detail-stack").unwrap();
        let incoming = visual.debug_bounds(page).unwrap();
        assert!(incoming.left() > stack.left() && incoming.left() < stack.right());
        assert_eq!(
            incoming.size, stack.size,
            "sliding pages keep their full layout size"
        );
        assert!(visual.debug_bounds("bot-detail-base").unwrap().left() < stack.left());
        advance(visual, 1000);
        assert_eq!(visual.debug_bounds(page).unwrap().left(), stack.left());
        let reader = s.manager.read_with(visual, |m, cx| match kind {
            "html" => m.html_documents["slide-document"].entity_id(),
            "session" => m
                .owner
                .upgrade()
                .unwrap()
                .read(cx)
                .bot_session_entity(session)
                .unwrap()
                .entity_id(),
            _ => m.documents["slide-document"].entity_id(),
        });
        assert_eq!(
            s.manager
                .read_with(visual, |m, cx| m.native_detail_content(cx)),
            kind == "html"
        );
        let back = visual.debug_bounds("bot-page-back").unwrap().center();
        visual.simulate_click(back, Default::default());
        visual.run_until_parked();
        advance(visual, 70);
        let outgoing = visual
            .debug_bounds(page)
            .expect("Back keeps the outgoing page mounted");
        assert!(outgoing.left() > stack.left() && outgoing.left() < stack.right());
        advance(visual, 1000);
        assert!(visual.debug_bounds(page).is_none());
        assert_eq!(
            visual.debug_bounds("bot-detail-base").unwrap().left(),
            stack.left()
        );
        assert!(
            !s.manager
                .read_with(visual, |m, cx| m.native_detail_content(cx))
        );

        visual.update(|_, cx| cx.set_reduce_motion(true));
        s.window
            .update(visual, |_, _, cx| {
                s.manager
                    .update(cx, |m, cx| m.card_action(&s.id, &action, cx))
            })
            .unwrap();
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds(page).unwrap().left(), stack.left());
        s.manager.read_with(visual, |m, cx| {
            let reopened = match kind {
                "html" => m.html_documents["slide-document"].entity_id(),
                "session" => m
                    .owner
                    .upgrade()
                    .unwrap()
                    .read(cx)
                    .bot_session_entity(session)
                    .unwrap()
                    .entity_id(),
                _ => m.documents["slide-document"].entity_id(),
            };
            assert_eq!(reader, reopened);
        });
        visual.update(|_, cx| cx.set_reduce_motion(false));
        s.window
            .update(visual, |_, w, cx| {
                s.manager.update(cx, |m, cx| {
                    m.close(cx);
                    m.open(&s.id, w, cx);
                });
            })
            .unwrap();
        visual.run_until_parked();
        assert!(
            visual.debug_bounds(page).is_none(),
            "returning to a Bot must not replay an old page's exit"
        );
        assert_eq!(
            visual.debug_bounds("bot-detail-base").unwrap().left(),
            stack.left()
        );
    }
}
#[gpui::test]
fn a_closed_creation_window_rehomes_the_same_unsaved_bot(cx: &mut TestAppContext) {
    let s = setup(cx);
    let creation_window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| cx.new(|_| gpui::EmptyView))
            .unwrap()
    });
    let id = creation_window
        .update(cx, |_, window, cx| {
            s.manager.update(cx, |m, cx| {
                m.create(vec![s.project.clone()], window, cx);
                let id = m.creation.as_ref().unwrap().bot.id.clone();
                m.fields["bot-name"]
                    .update(cx, |field, cx| field.set_text("Recovered bot".into(), cx));
                m.creation.as_mut().unwrap().soul = "Keep my unsaved soul".into();
                id
            })
        })
        .unwrap();
    cx.run_until_parked();
    creation_window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    s.window
        .update(cx, |_, window, cx| {
            s.manager
                .update(cx, |m, cx| m.create(Vec::new(), window, cx));
        })
        .unwrap();
    cx.run_until_parked();
    s.window
        .update(cx, |_, window, cx| {
            s.manager.update(cx, |m, cx| {
                let draft = m.creation.as_ref().unwrap();
                assert_eq!(draft.window.window_id(), window.window_handle().window_id());
                assert_eq!(draft.bot.id, id);
                assert_eq!(draft.bot.name, "Recovered bot");
                assert_eq!(draft.soul, "Keep my unsaved soul");
                assert_eq!(m.roster.bots.len(), 1);
                assert!(m.fields["bot-name"].focus_handle(cx).is_focused(window));
                m.confirm_creation(window, cx);
                assert!(m.creation.is_none());
                assert_eq!(m.roster.bots.len(), 2);
                assert_eq!(m.roster.get(&id).unwrap().name, "Recovered bot");
            });
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        bot::files::load(&s.root, &id, None)
            .unwrap()
            .soul
            .as_deref(),
        Some("Keep my unsaved soul")
    );
}

fn setup(cx: &mut TestAppContext) -> Setup {
    let dir = std::env::temp_dir().join(format!("native-bots-{}", bot::new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let project = dir.to_string_lossy().to_string();
    let store = Arc::new(Mutex::new(Store::open(&dir.join("native.sqlite")).unwrap()));
    let db = store.clone();
    let cwd = project.clone();
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| Shell::new(db, Vec::new(), cwd, None, cx))
        })
        .unwrap()
    });
    cx.run_until_parked();
    let manager = window
        .update(cx, |shell, _, cx| {
            shell.workspace.right_panel_open = true;
            cx.notify();
            shell.bot_manager()
        })
        .unwrap();
    let handle: gpui::AnyWindowHandle = window.into();
    handle
        .update(cx, |_, w, cx| {
            manager.update(cx, |m, cx| m.create_named(vec![project.clone()], w, cx))
        })
        .unwrap();
    cx.run_until_parked();
    let (id, session, root) = cx.update(|cx| {
        let m = manager.read(cx);
        let bot = m.current().unwrap();
        (bot.id.clone(), bot.session_id.unwrap(), m.root.clone())
    });
    Setup {
        window,
        manager,
        id,
        project,
        session,
        root,
        store,
    }
}
fn call(
    s: &Setup,
    request: &str,
    action: &str,
    input: Value,
    cx: &mut TestAppContext,
) -> mpsc::Receiver<Result<Value, String>> {
    let (reply, rx) = mpsc::channel();
    let call = bot::bridge::Call {
        session: s.session,
        request_id: request.into(),
        action: action.into(),
        input,
        reply,
    };
    let handle: gpui::AnyWindowHandle = s.window.into();
    handle
        .update(cx, |_, w, cx| {
            s.manager.update(cx, |m, cx| m.app_call(call, w, cx))
        })
        .unwrap();
    cx.run_until_parked();
    rx
}
#[gpui::test]
fn settings_match_react_dialog_and_left_navigation_geometry(cx: &mut TestAppContext) {
    let s = setup(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    let dialog = visual.debug_bounds("bot-settings-dialog").unwrap();
    let nav = visual.debug_bounds("bot-settings-navigation").unwrap();
    let viewport = s
        .window
        .update(visual, |_, w, _| w.viewport_size())
        .unwrap();
    assert_eq!(
        dialog.size.width + px(2.),
        px(880.).min(viewport.width - px(24.))
    );
    assert!((dialog.center().x - viewport.width / 2.).abs() <= px(1.));
    assert_eq!(nav.size.width, px(192.));
    for section in ["General", "Soul", "Memory"] {
        let row = visual
            .debug_bounds(Box::leak(
                format!("bot-settings-{section}").into_boxed_str(),
            ))
            .unwrap();
        let icon = visual
            .debug_bounds(Box::leak(
                format!("bot-settings-icon-{section}").into_boxed_str(),
            ))
            .unwrap();
        let label = visual
            .debug_bounds(Box::leak(
                format!("bot-settings-label-{section}").into_boxed_str(),
            ))
            .unwrap();
        assert_eq!(row.size.height, px(32.));
        assert_eq!(icon.left() - row.left(), px(10.));
        assert_eq!(label.left() - icon.right(), px(10.));
    }
    assert_eq!(
        visual.debug_bounds("bot-look").unwrap().size,
        gpui::size(px(80.), px(80.))
    );
    assert_eq!(
        visual.debug_bounds("bot-name-field").unwrap().size,
        gpui::size(px(160.), px(32.))
    );
}

#[gpui::test]
fn settings_color_popover_follows_its_mascot_when_the_window_resizes(cx: &mut TestAppContext) {
    let s = setup(cx);
    cx.update(|cx| {
        crate::shell::bind_keys(cx);
        crate::popover::bind(cx);
    });
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    let face = visual.debug_bounds("bot-look").unwrap();
    visual.simulate_click(face.center(), Default::default());
    visual.run_until_parked();
    for size in [
        gpui::size(px(1300.), px(900.)),
        gpui::size(px(1050.), px(760.)),
    ] {
        visual.simulate_resize(size);
        visual.run_until_parked();
        let face = visual.debug_bounds("bot-look").unwrap();
        let picker = visual.debug_bounds("bot-settings-look-picker").unwrap();
        assert!((picker.center().x - face.center().x).abs() <= px(1.));
        assert!(picker.top() >= face.bottom());
    }
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    visual.update(|_, cx| {
        let m = s.manager.read(cx);
        assert!(m.picker.is_none());
        assert!(m.settings.is_some());
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    visual.update(|_, cx| assert!(s.manager.read(cx).settings.is_none()));
}

#[gpui::test]
fn memory_editor_saves_with_enter_and_stays_open_for_the_next_fact(cx: &mut TestAppContext) {
    let s = setup(cx);
    cx.update(input::bind);
    s.window
        .update(cx, |_, window, cx| {
            s.manager.update(cx, |m, cx| {
                m.settings = Some(SettingsSection::Memory);
                m.begin_memory_editor(
                    memory_ui::MemoryTarget {
                        bot: s.id.clone(),
                        topic: None,
                        original: None,
                    },
                    window,
                    cx,
                );
                cx.notify();
            })
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    s.window
        .update(visual, |_, w, cx| {
            let m = s.manager.read(cx);
            let field = m.fields[&m.memory_editor.as_ref().unwrap().key].clone();
            w.focus(&field.focus_handle(cx), cx);
        })
        .unwrap();
    visual.simulate_input("Keep UI aligned with React.");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert!(
        bot::files::read(&s.root, &s.id, None, "MEMORY.md")
            .unwrap()
            .text
            .unwrap()
            .contains("Keep UI aligned with React.")
    );
    visual.update(|_, cx| {
        let m = s.manager.read(cx);
        let editor = m.memory_editor.as_ref().unwrap();
        assert!(editor.target.original.is_none());
        assert!(m.fields[&editor.key].read(cx).content.is_empty());
    });
}

#[gpui::test]
fn bot_work_disclosure_stays_in_chat_and_leaves_details_on_their_page(cx: &mut TestAppContext) {
    let s = setup(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    s.window
        .update(cx, |shell, window, cx| {
            shell.workspace.right_panel_open = false;
            s.manager.update(cx, |manager, cx| {
                manager.settings = None;
                manager.focus_active(window, cx);
                manager.select_overview(OverviewTab::Documents, cx);
                let transcript = manager.chats[&s.id].update(cx, |chat, cx| {
                    chat.summary.has_messages = true;
                    cx.notify();
                    chat.transcript.clone()
                });
                transcript.update(cx, |view, cx| {
                    let tool = vorkspace_core::tool::ToolCall::new("read", "Read");
                    let row = |role: &str, text: &str| vorkspace_core::transcript::Row {
                        role: role.into(),
                        text: text.into(),
                        first: true,
                        sent_at: None,
                    };
                    view.replace_history(
                        vec![
                            row("user", "Check the files"),
                            row("assistant", "Checking now."),
                            row("tool", &serde_json::to_string(&tool).unwrap()),
                            row("assistant", "All checked."),
                        ],
                        cx,
                    );
                });
            });
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("tool-2").is_none(),
        "completed work starts folded"
    );
    for open in [true, false, true] {
        let receipt = visual
            .debug_bounds("work-fold-1")
            .expect("inline work receipt");
        visual.simulate_click(receipt.center(), Default::default());
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds("tool-2").is_some(), open);
        assert!(visual.debug_bounds("bot-activity").is_none());
        s.window
            .update(visual, |shell, _, cx| {
                assert!(
                    !shell.workspace.right_panel_open,
                    "disclosure must not open the sidebar"
                );
                let manager = s.manager.read(cx);
                assert!(manager.overview_tab == OverviewTab::Documents);
                assert!(manager.route.is_none());
            })
            .unwrap();
    }
}

#[gpui::test]
fn overview_pages_use_the_shared_reveal(cx: &mut TestAppContext) {
    let s = setup(cx);
    s.window
        .update(cx, |_, window, cx| {
            s.manager.update(cx, |m, cx| {
                m.select_overview(OverviewTab::Documents, cx);
                assert!(m.overview_reveal.is_some());
                m.select_overview(OverviewTab::Sessions, cx);
                assert!(m.overview_tab == OverviewTab::Sessions);
                m.select_overview(OverviewTab::Documents, cx);
                assert!(m.overview_tab == OverviewTab::Documents);
                cx.set_reduce_motion(true);
                assert!(
                    m.overview_reveal
                        .as_ref()
                        .unwrap()
                        .progress(OverviewTab::Documents, window, cx)
                        .is_none()
                );
            });
        })
        .unwrap();
}

#[gpui::test]
fn note_actions_preserve_tags_when_updating_the_body(cx: &mut TestAppContext) {
    let s = setup(cx);
    let note = call(
        &s,
        "note-create",
        "notes.write",
        json!({"title":"QA note","body":"First","tags":["work","bot"]}),
        cx,
    )
    .try_recv()
    .unwrap()
    .unwrap();
    let updated = call(
        &s,
        "note-update",
        "notes.write",
        json!({"id":note["id"],"title":"QA note","body":"Second"}),
        cx,
    )
    .try_recv()
    .unwrap()
    .unwrap();
    assert_eq!(updated["tags"], json!(["work", "bot"]));
    assert_eq!(updated["body"], "Second");
}
#[gpui::test]
fn identity_and_context_editors_survive_switches_and_save_to_their_own_files(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    cx.update(|cx| {
        s.manager.update(cx, |m, cx| {
            m.settings = Some(SettingsSection::Soul);
            let bot = m.current().unwrap().clone();
            let _ = m.soul_page(&bot, cx);
            let key = format!("file:{}:SOUL.md", s.id);
            m.fields[&key].update(cx, |f, cx| {
                f.set_text("# Soul\nKeep replies concise.\n".into(), cx)
            });
            m.save_open_file(cx);
        })
    });
    cx.run_until_parked();
    assert_eq!(
        bot::files::read(&s.root, &s.id, None, "SOUL.md")
            .unwrap()
            .text
            .as_deref(),
        Some("# Soul\nKeep replies concise.\n")
    );
    let saved = Roster::load(&locked(&s.store).unwrap()).unwrap();
    assert_eq!(saved.bots.len(), 1);
    assert_eq!(saved.bots[0].session_id, Some(s.session));
    assert!(saved.hidden_sessions.contains(&s.session));
    let handle: gpui::AnyWindowHandle = s.window.into();
    handle
        .update(cx, |_, w, cx| {
            s.manager
                .update(cx, |m, cx| m.create_named(vec![s.project.clone()], w, cx))
        })
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        s.manager.update(cx, |m, cx| {
            let bot = m.current().unwrap().clone();
            assert_ne!(bot.id, s.id);
            assert_ne!(bot.mascot, m.roster.get(&s.id).unwrap().mascot);
            m.settings = Some(SettingsSection::Soul);
            let _ = m.soul_page(&bot, cx);
            let key = format!("file:{}:SOUL.md", bot.id);
            assert!(
                !m.fields[&key]
                    .read(cx)
                    .content
                    .contains("Keep replies concise")
            );
        })
    });
}
#[gpui::test]
fn app_actions_are_project_scoped_and_mutation_retries_are_idempotent(cx: &mut TestAppContext) {
    let s = setup(cx);
    assert!(
        call(
            &s,
            "outside",
            "sessions.list",
            json!({"project":"/not-assigned"}),
            cx
        )
        .try_recv()
        .unwrap()
        .is_err()
    );
    let input = json!({"name":"Morning digest","instructions":"Summarize changes without editing","schedule":{"kind":"daily","time":"09:00"}});
    let first = call(&s, "habit", "habits.add", input.clone(), cx)
        .try_recv()
        .unwrap()
        .unwrap();
    let retry = call(&s, "habit", "habits.add", input, cx)
        .try_recv()
        .unwrap()
        .unwrap();
    assert_eq!(first, retry);
    assert_eq!(habits::load(&s.root, &s.id).unwrap().len(), 1);
    assert!(
        call(&s, "habit", "habits.add", json!({"name":"different"}), cx)
            .try_recv()
            .unwrap()
            .is_err()
    );
    let start=call(&s,"draft","sessions.start",json!({"project":s.project,"prompt":"Review the project","title":"Review draft","draft":true}),cx).try_recv().unwrap().unwrap();
    let child = start["sessionId"].as_i64().unwrap();
    cx.update(|cx| {
        let m = s.manager.read(cx);
        assert_eq!(m.roster.get(&s.id).unwrap().launches.len(), 1);
        let owner = m.owner.upgrade().unwrap();
        let view = owner.read(cx).bot_session_entity(child).unwrap();
        assert_eq!(view.read(cx).input.read(cx).content, "Review the project");
        assert!(!view.read(cx).busy);
        assert!(m.hidden_session(child));
    });
}
#[gpui::test]
fn app_commands_recheck_deferred_ownership_and_protect_cold_and_self_targets(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    for action in ["sessions.stop", "sessions.archive", "sessions.delete"] {
        assert!(
            call(&s, action, action, json!({"sessionId":s.session}), cx)
                .try_recv()
                .unwrap()
                .unwrap_err()
                .contains("calling")
        );
    }
    let cold = locked(&s.store)
        .unwrap()
        .create("codex", &s.project)
        .unwrap()
        .id;
    s.manager.update(cx, |manager, _| {
        manager.roster.hidden_sessions.push(cold);
        manager.roster.save(&locked(&s.store).unwrap()).unwrap();
    });
    assert!(
        call(
            &s,
            "cold-delete",
            "sessions.delete",
            json!({"sessionId":cold}),
            cx
        )
        .try_recv()
        .unwrap()
        .is_err()
    );
    assert!(
        locked(&s.store)
            .unwrap()
            .bot_session(cold)
            .unwrap()
            .is_some()
    );
    let target = locked(&s.store)
        .unwrap()
        .create("codex", &s.project)
        .unwrap()
        .id;
    let (reply, rx) = mpsc::channel();
    s.window
        .update(cx, |_, window, cx| {
            s.manager.update(cx, |manager, cx| {
                manager.app_call(
                    bot::bridge::Call {
                        session: s.session,
                        request_id: "deferred-delete".into(),
                        action: "sessions.delete".into(),
                        input: json!({"sessionId":target}),
                        reply,
                    },
                    window,
                    cx,
                );
                // Ownership changes after admission but before the deferred effect.
                manager.roster.hidden_sessions.push(target);
            });
        })
        .unwrap();
    cx.run_until_parked();
    assert!(rx.try_recv().unwrap().unwrap_err().contains("protected"));
    assert!(
        locked(&s.store)
            .unwrap()
            .bot_session(target)
            .unwrap()
            .is_some()
    );
    assert!(
        locked(&s.store)
            .unwrap()
            .bot_session(s.session)
            .unwrap()
            .is_some()
    );
}
#[gpui::test]
fn background_delegation_keeps_controls_when_hidden_from_the_sidebar(cx: &mut TestAppContext) {
    let s = setup(cx);
    let child = call(
        &s,
        "background-start",
        "sessions.start",
        json!({"prompt":"Initial saved draft","draft":true,"reveal":false}),
        cx,
    )
    .try_recv()
    .unwrap()
    .unwrap()["sessionId"]
        .as_i64()
        .unwrap();
    let view = cx.update(|cx| {
        let manager = s.manager.read(cx);
        assert!(
            !manager
                .roster
                .get(&s.id)
                .unwrap()
                .show_started_sessions_in_sidebar
        );
        assert!(manager.hidden_session(child));
        assert!(!manager.protected_app_target(child, cx));
        manager
            .owner
            .upgrade()
            .unwrap()
            .read(cx)
            .bot_session_entity(child)
            .unwrap()
    });
    view.update(cx, |view, cx| {
        view.input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        view.app_flush_draft(cx).unwrap();
        view.settings.usage_limit = Some(Default::default());
    });
    assert!(
        call(
            &s,
            "background-draft",
            "sessions.draft",
            json!({"sessionId":child,"prompt":"Draft in the background"}),
            cx
        )
        .try_recv()
        .unwrap()
        .is_ok()
    );
    view.update(cx, |view, cx| {
        assert_eq!(view.input.read(cx).content, "Draft in the background");
        view.input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        view.app_flush_draft(cx).unwrap();
    });
    assert!(
        call(
            &s,
            "background-send",
            "sessions.send",
            json!({"sessionId":child,"prompt":"Durable background follow-up"}),
            cx
        )
        .try_recv()
        .unwrap()
        .is_ok()
    );
    assert_eq!(
        locked(&s.store)
            .unwrap()
            .follow_ups(child)
            .unwrap()
            .messages[0]
            .text,
        "Durable background follow-up"
    );
    // Hold the downstream provider through its existing usage-limit gate; these
    // controls exercise real delegation without starting any external process.
    assert!(!view.read_with(cx, |view, _| view.busy));
    assert!(
        call(
            &s,
            "background-stop",
            "sessions.stop",
            json!({"sessionId":child}),
            cx
        )
        .try_recv()
        .unwrap()
        .is_ok()
    );
    assert!(locked(&s.store).unwrap().follow_ups(child).unwrap().paused);
    assert!(
        call(
            &s,
            "background-archive",
            "sessions.archive",
            json!({"sessionId":child}),
            cx
        )
        .try_recv()
        .unwrap()
        .is_ok()
    );
    assert!(
        locked(&s.store)
            .unwrap()
            .bot_session(child)
            .unwrap()
            .unwrap()
            .archived
    );
    assert!(
        call(
            &s,
            "background-delete",
            "sessions.delete",
            json!({"sessionId":child}),
            cx
        )
        .try_recv()
        .unwrap()
        .is_ok()
    );
    assert!(
        locked(&s.store)
            .unwrap()
            .bot_session(child)
            .unwrap()
            .is_none()
    );
    assert!(
        locked(&s.store)
            .unwrap()
            .bot_session(s.session)
            .unwrap()
            .is_some()
    );
}
#[gpui::test]
fn cold_background_delegation_controls_do_not_require_a_visible_or_live_view(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let child = locked(&s.store)
        .unwrap()
        .create("codex", &s.project)
        .unwrap();
    let id = child.id;
    s.manager.update(cx, |manager, cx| {
        let bot = manager.roster.get_mut(&s.id).unwrap();
        bot.show_started_sessions_in_sidebar = false;
        bot.launches.push(bot::Launch {
            session_id: id,
            cwd: child.cwd,
            title: child.title,
            harness: child.provider,
            model: None,
            origin_turn: 0,
            notify_on_complete: false,
            completed: false,
            dismissed: false,
        });
        manager.roster.save(&locked(&s.store).unwrap()).unwrap();
        assert!(manager.hidden_session(id));
        assert!(!manager.protected_app_target(id, cx));
        assert!(
            manager
                .owner
                .upgrade()
                .unwrap()
                .read(cx)
                .bot_session_entity(id)
                .is_none()
        );
    });
    assert!(
        call(
            &s,
            "cold-background-stop",
            "sessions.stop",
            json!({"sessionId":id}),
            cx
        )
        .try_recv()
        .unwrap()
        .is_ok()
    );
    assert!(
        call(
            &s,
            "cold-background-archive",
            "sessions.archive",
            json!({"sessionId":id}),
            cx
        )
        .try_recv()
        .unwrap()
        .is_ok()
    );
    assert!(
        locked(&s.store)
            .unwrap()
            .bot_session(id)
            .unwrap()
            .unwrap()
            .archived
    );
    assert!(
        call(
            &s,
            "cold-background-delete",
            "sessions.delete",
            json!({"sessionId":id}),
            cx
        )
        .try_recv()
        .unwrap()
        .is_ok()
    );
    assert!(locked(&s.store).unwrap().bot_session(id).unwrap().is_none());
}
#[gpui::test]
fn app_command_unknown_admission_does_not_repeat_a_mutation(cx: &mut TestAppContext) {
    let s = setup(cx);
    let input = json!({"name":"Only once","instructions":"Read status","schedule":{"kind":"daily","time":"09:00"}});
    locked(&s.store)
        .unwrap()
        .admit_app_command(s.session, "unknown", "habits.add", &input)
        .unwrap();
    assert!(
        call(&s, "unknown", "habits.add", input, cx)
            .try_recv()
            .unwrap()
            .unwrap_err()
            .contains("unconfirmed")
    );
    assert!(habits::load(&s.root, &s.id).unwrap().is_empty());
}
#[gpui::test]
fn queued_messages_are_saved_before_dispatch_and_do_not_run_while_blocked(cx: &mut TestAppContext) {
    let s = setup(cx);
    let message = bot::new_id();
    cx.update(|cx| {
        s.manager.update(cx, |m, cx| {
            m.roster.get_mut(&s.id).unwrap().outbox.push(Message {
                peer: None,
                sent_at: None,
                id: message.clone(),
                text: "Keep this queued".into(),
                internal: false,
                error: Some("Retry required".into()),
                attachments: Vec::new(),
            });
            m.save(cx);
            m.configure_chat(&s.id, cx);
        })
    });
    cx.run_until_parked();
    let saved = Roster::load(&locked(&s.store).unwrap()).unwrap();
    assert_eq!(saved.bots[0].outbox[0].id, message);
    assert_eq!(
        locked(&s.store)
            .unwrap()
            .load(s.session)
            .unwrap()
            .rows
            .len(),
        0
    );
}

#[gpui::test]
fn chat_composer_keeps_attach_under_the_box_and_opens_the_model_list(cx: &mut TestAppContext) {
    let s = setup(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    let handle: gpui::AnyWindowHandle = s.window.into();
    handle
        .update(cx, |_, w, cx| {
            s.manager.update(cx, |m, cx| m.dismiss_settings(w, cx));
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    let field = visual.debug_bounds("bot-composer").unwrap();
    let send = visual.debug_bounds("bot-send").unwrap();
    let attach = visual.debug_bounds("bot-attach").unwrap();
    let model = visual.debug_bounds("bot-model").unwrap();
    // BotComposer with controls: send inside the box, attach and the model
    // on the row beneath it, the model at its right end.
    assert!(field.contains(&send.center()));
    assert!(attach.top() >= field.bottom());
    assert!(model.top() >= field.bottom());
    assert!(model.left() > attach.right());
    assert_eq!(send.size, gpui::size(px(26.), px(26.)));
    assert_eq!(attach.size, gpui::size(px(26.), px(26.)));
    let chat = visual.update(|_, cx| s.manager.read(cx).chat().unwrap());
    handle
        .update(visual, |_, w, cx| {
            chat.update(cx, |c, cx| c.toggle_model_menu(w, cx))
        })
        .unwrap();
    visual.run_until_parked();
    visual.update(|_, cx| {
        let (models, menu) = chat.read(cx).bot_menus_open();
        assert!(models, "the trigger opens the model list itself");
        assert!(!menu, "not the session composer's settings menu");
    });
}

#[gpui::test]
fn bot_details_share_the_original_titlebar_and_project_tools_use_the_same_edge(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    cx.update(|cx| {
        cx.set_reduce_motion(true);
        crate::shell::bind_keys(cx);
    });
    let session = s
        .window
        .update(cx, |shell, window, cx| {
            assert!(shell.workspace.classic_layout);
            s.manager.update(cx, |manager, cx| {
                manager.settings = None;
                manager.focus_active(window, cx);
                cx.notify();
            });
            shell.workspace.active.unwrap()
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    let panel = visual.debug_bounds("right-panel-frame").unwrap();
    let tabs = visual.debug_bounds("bot-overview-tabs").unwrap();
    let toggle = visual.debug_bounds("toggle-right-panel").unwrap();
    let title_tabs = visual.debug_bounds("title-tab-strip").unwrap();
    assert!((tabs.top() - title_tabs.top()).abs() <= px(0.5));
    assert_eq!(tabs.size.height, px(TITLE_BAR_HEIGHT));
    assert_eq!(tabs.left(), panel.left() + px(6.));
    assert!(
        visual.debug_bounds("bot-activity").is_none(),
        "work is disclosed in chat"
    );
    let search = visual.debug_bounds("titlebar-file-search").unwrap();
    assert_eq!(search.left() - tabs.right(), px(6.));
    assert_eq!(toggle.left() - search.right(), px(4.));
    assert!(tabs.bottom() <= panel.top() + px(TITLE_BAR_HEIGHT));
    assert_eq!(
        visual.debug_bounds("bot-session-list").unwrap().top(),
        panel.top() + px(TITLE_BAR_HEIGHT)
    );
    assert!(
        visual.debug_bounds("bot-overview-toggle").is_none(),
        "only the shell owns the details toggle"
    );
    assert!(
        visual.debug_bounds("sidebar-frame").is_none(),
        "a Bot owns the main surface beside the project rail"
    );
    assert!(title_tabs.right() <= tabs.left());
    let selector = Box::leak(format!("title-tab-{session}").into_boxed_str());
    let tab = visual.debug_bounds(selector).unwrap();
    visual.simulate_click(tab.center(), Default::default());
    visual.run_until_parked();
    let rail = visual.debug_bounds("classic-project-rail").unwrap();
    let sidebar = visual.debug_bounds("sidebar-frame").unwrap();
    assert_eq!(
        sidebar.left(),
        visual.debug_bounds("main-pane-frame").unwrap().right()
    );
    assert_eq!(
        visual.debug_bounds("main-pane-frame").unwrap().left(),
        rail.right()
    );
    assert!(visual.debug_bounds("classic-workspace-tabs").is_some());
    assert!(visual.debug_bounds("right-panel-frame").is_none());
    assert!(visual.debug_bounds("toggle-right-panel").is_none());
    s.window
        .update(visual, |shell, window, cx| {
            assert_eq!(shell.workspace.active, Some(session));
            assert!(
                shell.workspace.right_panel_open,
                "leaving the Bot retains its details preference"
            );
            s.manager
                .update(cx, |manager, cx| manager.open(&s.id, window, cx));
        })
        .unwrap();
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds("toggle-right-panel").unwrap(), toggle);
    assert_eq!(visual.debug_bounds("right-panel-frame").unwrap(), panel);
}

#[gpui::test]
fn titlebar_details_tabs_switch_pages_and_keep_habit_actions(cx: &mut TestAppContext) {
    let s = setup(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    s.window
        .update(cx, |shell, _, cx| {
            shell.workspace.right_panel_width = 400.;
            s.manager.update(cx, |m, cx| {
                m.settings = None;
                cx.notify();
            })
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    for (selector, tab) in [
        ("bot-overview-Documents", OverviewTab::Documents),
        ("bot-overview-Habits", OverviewTab::Habits),
    ] {
        let button = visual.debug_bounds(selector).unwrap();
        visual.simulate_click(button.center(), Default::default());
        visual.run_until_parked();
        assert!(s.manager.read_with(visual, |m, _| m.overview_tab == tab));
        assert!(visual.debug_bounds(selector).unwrap().size.width > px(26.));
    }
    let add = visual.debug_bounds("bot-new-habit").unwrap();
    visual.simulate_click(add.center(), Default::default());
    visual.run_until_parked();
    assert!(
        s.manager
            .read_with(visual, |m, _| matches!(m.route, Some(Route::NewHabit)))
    );
    assert!(visual.debug_bounds("bot-overview-tabs").is_some());
    let sessions = visual.debug_bounds("bot-overview-Sessions").unwrap();
    visual.simulate_click(sessions.center(), Default::default());
    visual.run_until_parked();
    assert!(s.manager.read_with(visual, |m, _| m.route.is_none()
        && m.overview_tab == OverviewTab::Sessions));
    assert!(visual.debug_bounds("bot-activity").is_none());
    for (selector, expected) in [
        ("bot-overview-Changes", OverviewTab::Changes),
        ("bot-overview-Sessions", OverviewTab::Sessions),
    ] {
        let tab = visual.debug_bounds(selector).unwrap();
        visual.simulate_click(tab.center(), Default::default());
        visual.run_until_parked();
        assert!(
            s.manager
                .read_with(visual, |m, _| m.overview_tab == expected)
        );
    }
}

#[gpui::test]
fn original_details_divider_resizes_and_closed_panels_keep_the_corner(cx: &mut TestAppContext) {
    let s = setup(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    s.window
        .update(cx, |shell, _, cx| {
            assert!(shell.workspace.classic_layout);
            s.manager.update(cx, |manager, cx| {
                manager.settings = None;
                cx.notify();
            });
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    for width in [220., 280., 400., 500.] {
        s.window
            .update(visual, |shell, _, cx| {
                shell.workspace.right_panel_width = width;
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        let panel = visual.debug_bounds("right-panel-frame").unwrap();
        let divider = visual.debug_bounds("right-panel-resize").unwrap();
        assert!((divider.center().x - panel.left() - px(0.5)).abs() <= px(0.5));
        assert_eq!(panel.size.width, px(width));
        let tabs = visual.debug_bounds("bot-overview-tabs").unwrap();
        let search = visual.debug_bounds("titlebar-file-search").unwrap();
        let toggle = visual.debug_bounds("toggle-right-panel").unwrap();
        assert!(
            tabs.right() <= search.left() && search.right() < toggle.left(),
            "Bot header actions stay available at the minimum details width"
        );
    }
    let start = visual.debug_bounds("right-panel-resize").unwrap().center();
    let end = start - gpui::point(px(40.), px(0.));
    visual.simulate_mouse_down(start, MouseButton::Left, Default::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Default::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Default::default());
    visual.run_until_parked();
    s.window
        .update(visual, |shell, _, _| {
            assert_eq!(shell.workspace.right_panel_width, 540.);
        })
        .unwrap();
    assert_eq!(
        visual.debug_bounds("right-panel-frame").unwrap().size.width,
        px(540.)
    );
    s.window
        .update(visual, |shell, _, cx| {
            shell
                .git
                .update(cx, |git, cx| git.set_project("~".into(), cx));
            cx.notify();
        })
        .unwrap();
    for _ in 0..2 {
        visual.run_until_parked();
        let open = visual.debug_bounds("toggle-right-panel").unwrap();
        visual.simulate_click(open.center(), Default::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("bot-overview-tabs").is_none());
        assert!(visual.debug_bounds("bot-overview-panel").is_none());
        assert_eq!(visual.debug_bounds("toggle-right-panel").unwrap(), open);
        let saved = locked(&s.store).unwrap().ui_state().unwrap().unwrap();
        let saved: crate::workspace::Workspace = serde_json::from_str(&saved).unwrap();
        assert!(!saved.right_panel_open);
        assert_eq!(saved.right_panel_width, 540.);
        visual.simulate_click(open.center(), Default::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("bot-overview-tabs").is_some());
        assert_eq!(
            visual.debug_bounds("right-panel-frame").unwrap().size.width,
            px(540.)
        );
    }
}

#[gpui::test]
fn opening_a_session_card_preserves_preview_state_when_returning_to_the_original_workspace(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    let (session, preview) = s
        .window
        .update(cx, |shell, _, cx| {
            s.manager.update(cx, |manager, cx| {
                manager.settings = None;
                cx.notify();
            });
            let session = shell.workspace.active.unwrap();
            let preview = shell.bot_ensure_session(session, cx).unwrap();
            preview.update(cx, |session, cx| {
                session.input.update(cx, |input, cx| {
                    input.set_text("Keep this preview draft".into(), cx)
                })
            });
            (session, preview)
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("sidebar-frame").is_none());
    let toggle = visual.debug_bounds("toggle-right-panel").unwrap();
    visual.simulate_click(toggle.center(), Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("right-panel-frame").is_none());
    s.manager.update(visual, |manager, cx| {
        manager.card_action(&s.id, &json!({"type":"session","sessionId":session}), cx)
    });
    visual.run_until_parked();
    let details = visual.debug_bounds("right-panel-frame").unwrap();
    assert!(s.manager.read_with(
        visual,
        |manager, _| matches!(manager.route,Some(Route::Session(id)) if id==session)
    ));
    let selector = Box::leak(format!("title-tab-{session}").into_boxed_str());
    let open = visual.debug_bounds(selector).unwrap();
    visual.simulate_click(open.center(), Default::default());
    visual.run_until_parked();
    let rail = visual.debug_bounds("classic-project-rail").unwrap();
    let sidebar = visual.debug_bounds("sidebar-frame").unwrap();
    assert_eq!(
        sidebar.left(),
        visual.debug_bounds("main-pane-frame").unwrap().right()
    );
    assert_eq!(
        visual.debug_bounds("main-pane-frame").unwrap().left(),
        rail.right()
    );
    assert!(
        visual.debug_bounds("right-panel-frame").is_none(),
        "ordinary sessions use the shared right-hand sidebar"
    );
    s.window
        .update(visual, |shell, window, cx| {
            assert_eq!(shell.workspace.active, Some(session));
            assert!(
                shell.workspace.right_panel_open,
                "Bot details stay enabled for the next Bot visit"
            );
            let actual = shell.bot_ensure_session(session, cx).unwrap();
            assert_eq!(actual.entity_id(), preview.entity_id());
            assert_eq!(
                actual.read(cx).input.read(cx).content,
                "Keep this preview draft"
            );
            assert!(
                actual
                    .read(cx)
                    .input
                    .read(cx)
                    .focus_handle(cx)
                    .contains_focused(window, cx)
            );
            s.manager
                .update(cx, |manager, cx| manager.open(&s.id, window, cx));
        })
        .unwrap();
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds("right-panel-frame").unwrap(), details);
    assert_eq!(
        preview.read_with(visual, |session, cx| session
            .input
            .read(cx)
            .content
            .to_string()),
        "Keep this preview draft"
    );
}

#[gpui::test]
fn long_habit_draft_keeps_fields_and_schedule_inside_the_scrolling_page(cx: &mut TestAppContext) {
    let s = setup(cx);
    cx.update(|cx| {
        cx.set_reduce_motion(true);
        input::bind(cx);
        crate::popover::bind(cx);
    });
    s.window
        .update(cx, |shell, _, cx| {
            shell.workspace.right_panel_width = 220.;
            s.manager.update(cx, |m, cx| {
                m.settings = None;
                m.overview_tab = OverviewTab::Habits;
                m.route = Some(Route::NewHabit);
                cx.notify();
            });
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    s.manager.update(visual, |m, cx| {
        m.fields["habit-name"].update(cx, |f, cx| {
            f.set_text(
                "A long but valid habit name that fills the narrow panel".into(),
                cx,
            )
        });
        m.fields["habit-instructions"].update(cx, |f, cx| {
            f.set_text(
                "Report useful updates on the current project.\n".repeat(65),
                cx,
            )
        });
        m.fields["habit-kind"].update(cx, |f, cx| f.set_text("weekly".into(), cx));
        cx.notify();
    });
    visual.run_until_parked();
    let body = visual.debug_bounds("bot-habit-editor").unwrap();
    let name = visual.debug_bounds("bot-habit-name-field").unwrap();
    let schedule = visual.debug_bounds("bot-habit-schedule").unwrap();
    let prompt = visual.debug_bounds("bot-habit-prompt-field").unwrap();
    let create = visual.debug_bounds("bot-habit-save").unwrap();
    assert!(name.size.height >= px(36.));
    assert!(schedule.top() >= name.bottom());
    assert!(prompt.top() > schedule.bottom());
    assert!(prompt.size.height >= px(300.));
    assert!(prompt.size.height <= px(328.));
    for bounds in [name, schedule, prompt] {
        assert!(bounds.left() >= body.left());
        assert!(bounds.right() <= body.right() + px(1.));
    }
    assert!(create.bottom() <= body.top());
    let day = visual.debug_bounds("bot-habit-weekday-picker").unwrap();
    assert!(day.left() >= schedule.left());
    assert!(day.right() <= schedule.right() + px(1.));
    visual.simulate_click(day.center(), Default::default());
    visual.run_until_parked();
    // Escape returns focus to the trigger; arrow keys reopen the list with
    // the current weekday highlighted, and Enter retains that selection.
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(s.manager.read_with(visual, |m, _| m.picker.is_none()));
    visual.simulate_keystrokes("down");
    visual.run_until_parked();
    assert!(s.manager.read_with(visual, |m, _| m.picker.is_some()));
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert_eq!(
        s.manager
            .read_with(visual, |m, cx| m.value("habit-weekday", cx)),
        "1"
    );
    assert!(s.manager.read_with(visual, |m, _| m.picker.is_none()));
    visual.simulate_click(day.center(), Default::default());
    visual.run_until_parked();
    let saturday = visual.debug_bounds("menu-action-6").unwrap();
    visual.simulate_click(saturday.center(), Default::default());
    visual.run_until_parked();
    assert_eq!(
        s.manager
            .read_with(visual, |m, cx| m.value("habit-weekday", cx)),
        "6"
    );
    // Invalid schedules keep the full draft in place instead of navigating away.
    s.manager.update(visual, |m, cx| {
        m.fields["habit-time"].update(cx, |f, cx| f.set_text("25:00".into(), cx));
    });
    visual.run_until_parked();
    visual.simulate_click(create.center(), Default::default());
    visual.run_until_parked();
    assert!(
        s.manager
            .read_with(visual, |m, _| m.route == Some(Route::NewHabit))
    );
    assert!(s.manager.read_with(visual, |m, cx| {
        m.fields["habit-instructions"].read(cx).content.len() > 2000
    }));
    assert!(habits::load(&s.root, &s.id).unwrap().is_empty());
    s.manager.update(visual, |m, cx| {
        m.fields["habit-time"].update(cx, |f, cx| f.set_text("09:00".into(), cx));
    });
    visual.run_until_parked();
    visual.simulate_click(create.center(), Default::default());
    visual.run_until_parked();
    let saved = habits::load(&s.root, &s.id).unwrap();
    assert_eq!(saved.len(), 1);
    assert!(saved[0].instructions.len() > 2000);
    assert_eq!(saved[0].schedule.schedule_kind, "weekly");
    assert_eq!(saved[0].schedule.day_of_week, 6);
    assert!(s.manager.read_with(visual, |m, _| m.route.is_none()));
}

#[gpui::test]
fn habit_detail_preserves_long_content_history_and_actions_without_edit_fields(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    let mut h = Habit::new(
        "A long habit title that should truncate before the action buttons".into(),
        "Review changes and report anything that needs attention.".into(),
        habits::Schedule::default(),
        vorkspace_core::time::now_ms(),
    )
    .unwrap();
    h.id = "layout-history".into();
    h.runs.push(habits::Run {
        at: vorkspace_core::time::now_ms() - 60_000,
        duration_ms: 72_000,
        outcome: "posted".into(),
        report: Some("**Result**\n\nThe project is ready to review.".into()),
        error: None,
    });
    habits::update(&s.root, &s.id, |all| {
        all.push(h.clone());
        Ok(())
    })
    .unwrap();
    s.window
        .update(cx, |shell, _, cx| {
            shell.workspace.right_panel_width = 280.;
            s.manager.update(cx, |m, cx| {
                m.settings = None;
                m.overview_tab = OverviewTab::Habits;
                m.habits = vec![h.clone()];
                m.route = Some(Route::Habit(h.id.clone()));
                cx.notify();
            });
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    assert!(
        s.manager
            .read_with(visual, |m, _| !m.fields.contains_key("habit-name"))
    );
    let body = visual.debug_bounds("bot-habit-editor").unwrap();
    let description = visual.debug_bounds("bot-habit-instructions").unwrap();
    let row_id = format!("bot-habit-history-{}-{}", h.id, h.runs[0].at);
    let row = visual
        .debug_bounds(Box::leak(row_id.into_boxed_str()))
        .unwrap();
    assert!(row.top() > description.bottom());
    assert!(description.right() <= body.right());
    for selector in ["bot-habit-run", "bot-habit-toggle", "bot-habit-delete"] {
        let action = visual.debug_bounds(selector).unwrap();
        assert!(action.bottom() <= body.top());
        assert!(action.right() <= body.right());
    }
    visual.simulate_click(row.center(), Default::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("bot-habit-report-{}-{}", h.id, h.runs[0].at).into_boxed_str()
            ))
            .is_some()
    );
    let toggle = visual.debug_bounds("bot-habit-toggle").unwrap();
    visual.simulate_click(toggle.center(), Default::default());
    visual.run_until_parked();
    assert!(!habits::load(&s.root, &s.id).unwrap()[0].enabled);
    assert!(
        s.manager
            .read_with(visual, |m, _| m.route == Some(Route::Habit(h.id.clone())))
    );
    let remove = visual.debug_bounds("bot-habit-delete").unwrap();
    visual.simulate_click(remove.center(), Default::default());
    visual.run_until_parked();
    assert!(habits::load(&s.root, &s.id).unwrap().is_empty());
    assert!(s.manager.read_with(visual, |m, _| m.route.is_none()));
}

#[gpui::test]
fn new_habit_click_transfers_composer_focus_after_the_animated_page_mounts(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    cx.update(|cx| cx.set_reduce_motion(false));
    s.window
        .update(cx, |_, window, cx| {
            s.manager.update(cx, |m, cx| {
                m.settings = None;
                m.overview_tab = OverviewTab::Habits;
                m.route = None;
                m.focus_active(window, cx);
                cx.notify();
            });
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    s.window
        .update(visual, |_, window, cx| {
            s.manager.read_with(cx, |m, cx| {
                assert!(
                    m.chat()
                        .unwrap()
                        .read(cx)
                        .input
                        .focus_handle(cx)
                        .is_focused(window)
                );
            });
        })
        .unwrap();
    let advance_focus_frame = |visual: &mut gpui::VisualTestContext| {
        visual
            .update_window(s.window.into(), |_, window, cx| {
                // Use the untyped window context: callbacks and drawing may
                // update the root Shell, so it must not already be borrowed.
                // Parking executor tasks does not deliver platform frame callbacks.
                // Flush the requested next frame before drawing its focused field.
                assert!(window.simulate_next_frame(cx) > 0);
                window.draw(cx).clear(cx);
            })
            .unwrap();
        visual.run_until_parked();
    };
    // Follow the actual header action, with the old composer still focused.
    let add = visual.debug_bounds("bot-new-habit").unwrap();
    visual.simulate_click(add.center(), Default::default());
    visual.run_until_parked();
    advance_focus_frame(visual);
    s.window
        .update(visual, |_, window, cx| {
            s.manager.read_with(cx, |m, cx| {
                assert_eq!(m.route, Some(Route::NewHabit));
                assert!(m.fields["habit-name"].focus_handle(cx).is_focused(window));
                assert!(
                    !m.chat()
                        .unwrap()
                        .read(cx)
                        .input
                        .focus_handle(cx)
                        .is_focused(window)
                );
            });
        })
        .unwrap();
    let page = "bot-detail-page-new-habit";
    assert!(visual.debug_bounds(page).is_some());
    // Complete the real page transition, then leave/reopen through its controls.
    for reopen in [false, true] {
        visual
            .update_window(s.window.into(), |_, window, cx| {
                let details = s.manager.read(cx).details.clone();
                details.update(cx, |v, cx| v.elapsed(std::time::Duration::from_secs(1), cx));
                window.draw(cx).clear(cx);
            })
            .unwrap();
        let back = visual.debug_bounds("bot-page-back").unwrap();
        visual.simulate_click(back.center(), Default::default());
        visual.run_until_parked();
        assert!(s.manager.read_with(visual, |m, _| m.route.is_none()));
        if !reopen {
            let add = visual.debug_bounds("bot-new-habit").unwrap();
            visual.simulate_click(add.center(), Default::default());
            visual.run_until_parked();
            advance_focus_frame(visual);
            s.window
                .update(visual, |_, window, cx| {
                    s.manager.read_with(cx, |m, cx| {
                        assert!(m.fields["habit-name"].focus_handle(cx).is_focused(window))
                    });
                })
                .unwrap();
        }
    }
}

#[gpui::test]
fn sidebar_bot_only_marks_unread_replies_and_swaps_the_mark_for_options_on_hover(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    s.window
        .update(cx, |_, _, cx| {
            s.manager.update(cx, |m, cx| {
                m.settings = None;
                m.close(cx);
            });
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    let unread_selector: &'static str =
        Box::leak(format!("bot-rail-unread-{}", s.id).into_boxed_str());
    let options_selector: &'static str =
        Box::leak(format!("bot-rail-options-{}", s.id).into_boxed_str());
    let row_selector: &'static str = Box::leak(format!("bot-rail-{}", s.id).into_boxed_str());
    let away = gpui::point(px(5.), px(5.));
    visual.simulate_mouse_move(away, None, Default::default());
    visual.run_until_parked();
    visual.refresh().unwrap();
    let row = visual.debug_bounds(row_selector).unwrap();
    assert!(visual.debug_bounds(unread_selector).is_none());
    assert!(visual.debug_bounds(options_selector).is_none());
    // Check the painted output too: an idle Bot used to paint a gray
    // status dot here, even with no unread reply and no pointer over it.
    s.window
        .update(visual, |_, window, _| {
            let scale = window.scale_factor();
            assert!(!window.painted_quads().iter().any(|quad| {
                let bounds = quad.bounds;
                bounds.left().0 >= f32::from(row.right() - px(28.)) * scale
                    && bounds.right().0 <= f32::from(row.right() - px(4.)) * scale
                    && bounds.top().0 >= f32::from(row.top()) * scale
                    && bounds.bottom().0 <= f32::from(row.bottom()) * scale
                    && (3. * scale..=8. * scale).contains(&bounds.size.width.0)
                    && (3. * scale..=8. * scale).contains(&bounds.size.height.0)
            }));
        })
        .unwrap();

    let post = |role, text: &str, visual: &mut gpui::VisualTestContext| {
        s.window
            .update(visual, |_, _, cx| {
                let chat = s.manager.read(cx).chats[&s.id].clone();
                chat.update(cx, |v, cx| v.bot_post(role, text.into(), cx))
                    .unwrap();
            })
            .unwrap();
        visual.run_until_parked();
        visual.refresh().unwrap();
    };
    post("user", "My question", visual);
    assert!(visual.debug_bounds(unread_selector).is_none());
    post("assistant", "A new reply", visual);
    let unread = visual
        .debug_bounds(unread_selector)
        .expect("an unseen assistant reply has an accent circle");
    assert_eq!(unread.size, gpui::size(px(6.), px(6.)));
    assert!(visual.debug_bounds(options_selector).is_none());
    let before_hover = visual.debug_bounds(row_selector).unwrap();
    visual.simulate_mouse_move(before_hover.center(), None, Default::default());
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(
        visual.debug_bounds(unread_selector).is_none(),
        "options replace the unread circle on hover"
    );
    let options = visual.debug_bounds(options_selector).unwrap();
    assert_eq!(options.size, gpui::size(px(24.), px(24.)));
    assert_eq!(options.center(), unread.center());
    assert_eq!(visual.debug_bounds(row_selector).unwrap(), before_hover);
    visual.simulate_click(options.center(), Default::default());
    visual.run_until_parked();
    s.manager.read_with(visual, |m, _| {
        assert!(m.menu.is_some(), "the hover action opens Bot options");
        assert!(m.active.is_none(), "options must not open the Bot");
    });

    s.window
        .update(visual, |_, window, cx| {
            s.manager.update(cx, |m, cx| {
                m.menu = None;
                m.open(&s.id, window, cx);
            });
        })
        .unwrap();
    visual.simulate_mouse_move(away, None, Default::default());
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds(unread_selector).is_none());
    s.window
        .update(visual, |_, _, cx| {
            s.manager.update(cx, |m, cx| {
                assert_eq!(
                    m.roster.get(&s.id).unwrap().seen_rows,
                    m.chats[&s.id].read(cx).bot_rows(cx).len()
                );
                m.close(cx);
            });
        })
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(
        visual.debug_bounds(unread_selector).is_none(),
        "a reply stays read when the Bot closes"
    );
}

#[gpui::test]
fn sidebar_right_click_opens_bot_menu_outside_the_bot_and_edits_its_look(cx: &mut TestAppContext) {
    cx.update(input::bind);
    let s = setup(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    // Leave the Bot, as when a session is open: only the rail remains.
    s.window
        .update(cx, |_, _, cx| {
            s.manager.update(cx, |m, cx| {
                m.settings = None;
                m.active = None;
                cx.notify();
            })
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    let row = s
        .manager
        .read_with(visual, |m, _| m.rail_bounds[&s.id].get())
        .expect("the sidebar lists the Bot");
    let at = row.center();
    visual.simulate_event(gpui::MouseDownEvent {
        position: at,
        button: MouseButton::Right,
        modifiers: Default::default(),
        click_count: 1,
        first_mouse: false,
    });
    visual.simulate_event(gpui::MouseUpEvent {
        position: at,
        button: MouseButton::Right,
        modifiers: Default::default(),
        click_count: 1,
    });
    visual.run_until_parked();
    // BotMenu opens beside the pointer with 5 3D face styles, shape dropdown,
    // texture toggle switch, grid scale slider, and nine circular presets.
    let first = visual
        .debug_bounds("bot-mascot-0")
        .expect("right-click opens the Bot's menu");
    let last = visual.debug_bounds("bot-mascot-4").unwrap();
    assert_eq!(first.top(), last.top());
    assert_eq!(first.size.height, px(32.));
    assert!(first.left() >= at.x);
    let shape_dd = visual.debug_bounds("bot-shape-dropdown").unwrap();
    visual.simulate_click(shape_dd.center(), Default::default());
    visual.run_until_parked();
    let shape = visual.debug_bounds("bot-shape-4").unwrap();
    visual.simulate_click(shape.center(), Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("bot-custom-color").is_none());
    assert_eq!(
        visual.debug_bounds("bot-color-0").unwrap().top(),
        visual.debug_bounds("bot-color-8").unwrap().top()
    );
    let pink = visual.debug_bounds("bot-color-4").unwrap().center();
    visual.simulate_click(pink, Default::default());
    visual.run_until_parked();
    let bg_btn = visual.debug_bounds("bot-bg-color-button").unwrap();
    let fg_btn = visual.debug_bounds("bot-fg-color-button").unwrap();
    assert_eq!(bg_btn.top(), fg_btn.top());
    assert!(bg_btn.bottom() <= visual.debug_bounds("bot-color-0").unwrap().top());
    assert!(visual.debug_bounds("menu-action-delete").is_none());
    let settings_row = visual.debug_bounds("menu-action-settings").unwrap();
    let card = s.manager.read_with(visual, |m, cx| {
        m.menu
            .as_ref()
            .unwrap()
            .1
            .read(cx)
            .surface_region()
            .get()
            .unwrap()
    });
    let name_bounds = visual.debug_bounds("bot-options-name").unwrap();
    assert_eq!(name_bounds.top() - card.top(), px(9.));
    assert_eq!(card.bottom() - settings_row.bottom(), px(9.));
    let die = visual.debug_bounds("bot-randomize-seed").unwrap();
    let grid_slider = visual.debug_bounds("bot-grid-slider").unwrap();
    assert!(die.bottom() <= grid_slider.top());
    visual.simulate_click(bg_btn.center(), Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("bot-custom-color-picker").is_some());
    let (color, badge_shape) = s.manager.read_with(visual, |m, _| {
        let bot = m.roster.get(&s.id).unwrap();
        (bot.color, bot.shape.clone())
    });
    assert_eq!(color, bot::COLORS_BOT[4]);
    assert_eq!(badge_shape, "hex");
    // Opening Background image from the Bot menu keeps the effect dropdown above the modal.
    s.manager.update(visual, |m, cx| {
        m.roster.get_mut(&s.id).unwrap().background =
            Some(vorkspace_core::project_appearance::Background {
                path: concat!(env!("CARGO_MANIFEST_DIR"), "/assets/vorkspace.png").into(),
                ..Default::default()
            });
        m.save(cx);
    });
    let bg_action = visual.debug_bounds("menu-action-background").unwrap();
    visual.simulate_click(bg_action.center(), Default::default());
    visual.run_until_parked();
    assert!(
        s.manager
            .read_with(visual, |m, _| m.menu.is_none() && m.background.is_some())
    );
    let effect_trigger = visual.debug_bounds("project-background-effect").unwrap();
    visual.simulate_click(effect_trigger.center(), Default::default());
    visual.run_until_parked();
    let blur_option = visual.debug_bounds("project-settings-Haze").unwrap();
    visual.simulate_click(blur_option.center(), Default::default());
    visual.run_until_parked();
    assert_eq!(
        s.manager.read_with(visual, |m, _| {
            m.roster
                .get(&s.id)
                .unwrap()
                .background
                .as_ref()
                .unwrap()
                .effect
        }),
        vorkspace_core::project_appearance::Effect::GradientBlur
    );
    assert!(s.manager.read_with(visual, |m, _| m.background.is_some()));
    let close_bg = visual.debug_bounds("close-project-background").unwrap();
    visual.simulate_click(close_bg.center(), Default::default());
    visual.run_until_parked();
    assert!(s.manager.read_with(visual, |m, _| m.background.is_none()));
}

#[gpui::test]
fn new_bot_setup_focuses_name_and_cancel_keeps_storage_and_current_bot_unchanged(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    cx.update(input::bind);
    let before = locked(&s.store).unwrap().setting("bot-roster").unwrap();
    let sessions = locked(&s.store).unwrap().sessions().unwrap().len();
    let original_files = bot::files::load(&s.root, &s.id, None).unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    s.window
        .update(visual, |_, window, cx| {
            s.manager.update(cx, |manager, cx| {
                manager.settings = None;
                manager.create(vec![s.project.clone()], window, cx);
                assert_eq!(manager.active.as_deref(), Some(s.id.as_str()));
                assert_eq!(manager.roster.bots.len(), 1);
                assert!(
                    manager.fields["bot-name"]
                        .focus_handle(cx)
                        .is_focused(window)
                );
                manager.confirm_creation(window, cx);
                assert!(manager.creating());
            })
        })
        .unwrap();
    visual.run_until_parked();
    s.window
        .update(visual, |shell, window, cx| {
            shell.focus_active(window, cx);
            assert!(
                s.manager.read(cx).fields["bot-name"]
                    .focus_handle(cx)
                    .is_focused(window)
            );
        })
        .unwrap();
    assert!(visual.debug_bounds("bot-create").is_some());
    assert!(visual.debug_bounds("bot-reset").is_none());
    visual.simulate_input("   ");
    visual.run_until_parked();
    let create = visual.debug_bounds("bot-create").unwrap();
    visual.simulate_click(create.center(), Default::default());
    visual.run_until_parked();
    let draft_id = s.manager.read_with(visual, |manager, _| {
        assert_eq!(manager.roster.bots.len(), 1);
        manager.creation.as_ref().unwrap().bot.id.clone()
    });
    let soul = visual.debug_bounds("bot-settings-Soul").unwrap();
    visual.simulate_click(soul.center(), Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("bot-soul-page").is_some());
    assert!(visual.debug_bounds("bot-create").is_some());
    s.manager.update(visual, |manager, cx| {
        manager.fields[&format!("file:{draft_id}:SOUL.md")].update(cx, |field, cx| {
            field.set_text("Unsaved setup soul".into(), cx)
        });
    });
    assert!(visual.debug_bounds("bot-settings-Memory").is_none());
    let general = visual.debug_bounds("bot-settings-General").unwrap();
    visual.simulate_click(general.center(), Default::default());
    visual.run_until_parked();
    s.manager.read_with(visual, |manager, _| {
        assert_eq!(
            manager.creation.as_ref().unwrap().soul,
            "Unsaved setup soul"
        );
    });
    assert_eq!(
        before,
        locked(&s.store).unwrap().setting("bot-roster").unwrap()
    );
    assert_eq!(
        sessions,
        locked(&s.store).unwrap().sessions().unwrap().len()
    );
    assert!(!s.root.join(&draft_id).exists());
    let general = visual.debug_bounds("bot-settings-General").unwrap();
    visual.simulate_click(general.center(), Default::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    s.manager.read_with(visual, |manager, _| {
        assert!(!manager.creating());
        assert_eq!(manager.active.as_deref(), Some(s.id.as_str()));
        assert_eq!(manager.roster.bots.len(), 1);
    });
    assert_eq!(
        before,
        locked(&s.store).unwrap().setting("bot-roster").unwrap()
    );
    assert_eq!(
        sessions,
        locked(&s.store).unwrap().sessions().unwrap().len()
    );
    let files = bot::files::load(&s.root, &s.id, None).unwrap();
    assert_eq!(files.soul, original_files.soul);
    assert_eq!(files.memory, original_files.memory);
    assert!(!s.root.join(&draft_id).exists());
}

#[gpui::test]
fn create_named_bot_commits_once_with_draft_choices_and_existing_settings_show_reset(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    cx.update(input::bind);
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    s.window
        .update(visual, |_, window, cx| {
            s.manager
                .update(cx, |manager, cx| manager.create(Vec::new(), window, cx))
        })
        .unwrap();
    visual.run_until_parked();
    visual.simulate_input("  Ada 🧭  ");
    visual.run_until_parked();
    let draft_id = s.manager.read_with(visual, |manager, _| {
        manager.creation.as_ref().unwrap().bot.id.clone()
    });
    let soul = visual.debug_bounds("bot-settings-Soul").unwrap();
    visual.simulate_click(soul.center(), Default::default());
    visual.run_until_parked();
    s.manager.update(visual, |manager, cx| {
        manager.fields[&format!("file:{draft_id}:SOUL.md")].update(cx, |field, cx| {
            field.set_text("# Soul\nKeep replies concise.\n".into(), cx)
        });
    });
    assert!(visual.debug_bounds("bot-create").is_some());
    assert!(!s.root.join(&draft_id).exists());
    s.manager.update(visual, |manager, cx| {
        let id = manager.creation.as_ref().unwrap().bot.id.clone();
        manager.edit_settings_bot(
            &id,
            |bot| {
                bot.mascot = "cat".into();
                bot.projects.push(s.project.clone());
                bot.show_started_sessions_in_sidebar = true;
            },
            cx,
        );
        let draft = manager.creation.as_mut().unwrap();
        draft.provider = Kind::Claude;
        draft.settings.model = Some("chosen-model".into());
        draft.settings.effort = Some("high".into());
        draft.settings.access = AccessMode::ReadOnly;
        draft.settings.fast = true;
        assert_eq!(draft.bot.name, "Ada 🧭");
    });
    visual.run_until_parked();
    let create = visual.debug_bounds("bot-create").unwrap();
    visual.simulate_click(create.center(), Default::default());
    visual.run_until_parked();
    let (id, session) = s.manager.read_with(visual, |manager, cx| {
        assert!(!manager.creating());
        assert_eq!(manager.roster.bots.len(), 2);
        let bot = manager.current().unwrap();
        assert_eq!(bot.name, "Ada 🧭");
        assert_eq!(bot.mascot, "cat");
        assert_eq!(bot.projects, vec![s.project.clone()]);
        assert!(bot.show_started_sessions_in_sidebar);
        assert!(bot.initial_configuration.is_none());
        let view = manager.chat().unwrap();
        assert_eq!(view.read(cx).summary.provider, "claude");
        (bot.id.clone(), bot.session_id.unwrap())
    });
    let saved = Roster::load(&locked(&s.store).unwrap()).unwrap();
    assert_eq!(saved.get(&id).unwrap().name, "Ada 🧭");
    let settings = locked(&s.store)
        .unwrap()
        .agent_settings(session)
        .unwrap()
        .unwrap();
    assert_eq!(settings.model.as_deref(), Some("chosen-model"));
    assert_eq!(settings.effort.as_deref(), Some("high"));
    assert_eq!(settings.access, AccessMode::ReadOnly);
    assert!(settings.fast);
    let files = bot::files::load(&s.root, &id, None).unwrap();
    assert_eq!(
        files.soul.as_deref(),
        Some("# Soul\nKeep replies concise.\n")
    );
    s.window
        .update(visual, |_, window, cx| {
            s.manager.update(cx, |manager, cx| {
                manager.confirm_creation(window, cx);
                assert_eq!(manager.roster.bots.len(), 2);
                manager.settings = Some(SettingsSection::General);
                cx.notify();
            })
        })
        .unwrap();
    visual.run_until_parked();
    assert!(visual.debug_bounds("bot-reset").is_some());
    assert!(visual.debug_bounds("bot-create").is_none());
    let soul = visual.debug_bounds("bot-settings-Soul").unwrap();
    visual.simulate_click(soul.center(), Default::default());
    visual.run_until_parked();
    s.manager.update(visual, |manager, cx| {
        manager.fields[&format!("file:{id}:SOUL.md")].update(cx, |field, cx| {
            field.set_text("Updated after creation".into(), cx)
        });
        manager.save_open_file(cx);
    });
    visual.run_until_parked();
    assert_eq!(
        bot::files::load(&s.root, &id, None)
            .unwrap()
            .soul
            .as_deref(),
        Some("Updated after creation")
    );
}

#[gpui::test]
fn new_bot_footer_remains_visible_at_narrow_window_sizes(cx: &mut TestAppContext) {
    let s = setup(cx);
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    s.window
        .update(visual, |_, window, cx| {
            s.manager
                .update(cx, |manager, cx| manager.create(Vec::new(), window, cx))
        })
        .unwrap();
    for size in [
        gpui::size(px(1100.), px(800.)),
        gpui::size(px(640.), px(500.)),
    ] {
        visual.simulate_resize(size);
        visual.run_until_parked();
        let dialog = visual.debug_bounds("bot-settings-dialog").unwrap();
        let footer = visual.debug_bounds("bot-settings-footer").unwrap();
        let create = visual.debug_bounds("bot-create").unwrap();
        assert!(create.bottom() <= dialog.bottom());
        assert!(create.top() >= footer.top());
        assert!(create.right() <= dialog.right());
        let look = visual.debug_bounds("bot-look").unwrap();
        let name = visual.debug_bounds("bot-name-field").unwrap();
        assert!(name.top() >= look.bottom());
        if size.width >= px(780.) {
            let model = visual.debug_bounds("bot-model-settings").unwrap();
            assert!(model.left() > name.right());
            let identity_center = (look.top() + name.bottom()) / 2.;
            assert!((identity_center - model.center().y).abs() <= px(1.));
        }
    }
}
