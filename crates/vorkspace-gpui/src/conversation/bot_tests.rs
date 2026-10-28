use super::*;
use gpui::{TestAppContext, VisualTestContext, WindowHandle, size};
use std::{cell::RefCell, rc::Rc, sync::Mutex};
use vorkspace_core::{bot::peers::Peer, store::Store};

fn message(id: &str, text: &str, internal: bool) -> Message {
    Message {
        prepared_prompt: None,
        id: id.into(),
        text: text.into(),
        internal,
        peer: None,
        sent_at: None,
        error: None,
        attachments: Vec::new(),
    }
}

fn peer_message() -> Message {
    Message {
        prepared_prompt: None,
        peer: Some(Peer {
            from: "merlin".into(),
            hop: 1,
            expect_reply: true,
        }),
        ..message(
            "peer",
            "<bot_peer_message>\nFrom: {\"id\":\"merlin\",\"name\":\"Merlin\",\"hop\":1}\nYour final reply is sent back automatically.\n</bot_peer_message>\n\nPlease review the change.",
            true,
        )
    }
}

fn chat(cx: &mut TestAppContext) -> WindowHandle<Conversation> {
    cx.update(|cx| {
        crate::clock::use_test_clock(cx);
        input::bind(cx);
        let store = Store::open(Path::new(":memory:")).unwrap();
        let summary = store.create("codex", "/private/tmp/bot-queue-ui").unwrap();
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut view = Conversation::new(summary, Arc::new(Mutex::new(store)), cx);
                view.loaded = true;
                view.configure_bot(
                    Bot {
                        name: "Sam".into(),
                        ..Default::default()
                    },
                    "/private/tmp".into(),
                    Default::default(),
                    cx,
                );
                view.bot_post("assistant", "Checking the change.".into(), cx)
                    .unwrap();
                view.input
                    .update(cx, |input, cx| input.set_text("Unsent draft".into(), cx));
                view
            })
        })
        .unwrap()
    })
}

#[gpui::test]
fn canceling_draft_edit_restores_saved_mcp_context(cx: &mut TestAppContext) {
    let window = chat(cx);
    let original = "Use @mcp/docs";
    let prepared = "MCP context: docs\n\nUse @mcp/docs";
    window
        .update(cx, |view, _, cx| {
            let mut state = view.follow_ups.state.clone();
            state.revision += 1;
            state.draft = Some(vorkspace_core::follow_up::Message {
                id: "saved-context".into(),
                text: original.into(),
                prepared_prompt: Some(prepared.into()),
                ..Default::default()
            });
            locked(&view.store)
                .unwrap()
                .accept_follow_ups(view.summary.id, &state)
                .unwrap();
            view.follow_ups.persisted = state.revision;
            view.follow_ups.state = state;
            let draft = view.follow_ups.state.draft.clone();
            view.transcript
                .update(cx, |transcript, cx| transcript.set_saved_draft(draft, cx));
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(size(px(900.), px(680.)));
    visual.run_until_parked();
    let edit = visual.debug_bounds("edit-message-draft").unwrap().center();
    visual.simulate_click(edit, Default::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("No token");
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            let edited = view.follow_ups.state.draft.as_ref().unwrap();
            assert_eq!(edited.text, "No token");
            assert!(edited.prepared_prompt.is_none());
            locked(&view.store)
                .unwrap()
                .save_setting(
                    &format!("bot-delivered:{}:{}", view.summary.id, edited.id),
                    "1",
                )
                .unwrap();
        })
        .unwrap();
    let cancel = visual.debug_bounds("cancel-draft-edit").unwrap().center();
    visual.simulate_click(cancel, Default::default());
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            let saved = locked(&view.store)
                .unwrap()
                .follow_ups(view.summary.id)
                .unwrap()
                .draft
                .unwrap();
            assert_eq!(saved.text, original);
            assert_eq!(saved.prepared_prompt.as_deref(), Some(prepared));
            assert_ne!(saved.id, "saved-context", "cancel publishes a new version");
        })
        .unwrap();
}

#[gpui::test]
fn successive_draft_edits_survive_an_intermediate_delivery_receipt(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("draft-versions.sqlite");
    let attachment = dir.path().join("attachment.txt");
    std::fs::write(&attachment, "saved attachment").unwrap();
    let store = Store::open(&path).unwrap();
    let summary = store.create("codex", "/tmp").unwrap();
    let session = summary.id;
    let mut roster = vorkspace_core::bot::Roster::default();
    roster.bots.push(Bot {
        id: "bot".into(),
        session_id: Some(session),
        ..Default::default()
    });
    roster.save(&store).unwrap();
    let mut state = store.follow_ups(session).unwrap();
    state.revision += 1;
    state.draft = Some(vorkspace_core::follow_up::Message {
        id: "original".into(),
        text: "Original".into(),
        attachments: vec![attachment],
        ..Default::default()
    });
    state.retain_attachments(session, store.data_dir()).unwrap();
    store.accept_follow_ups(session, &state).unwrap();
    store
        .save_setting(
            &format!("bot-draft-origin:{session}"),
            &serde_json::json!({"id":"original", "revision":1}).to_string(),
        )
        .unwrap();
    let window = cx.update(|cx| {
        input::bind(cx);
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut view = Conversation::new(summary, Arc::new(Mutex::new(store)), cx);
                view.loaded = true;
                view.configure_bot(Bot::default(), dir.path().into(), Default::default(), cx);
                view.follow_ups.persisted = state.revision;
                view.follow_ups.state = state.clone();
                view.transcript.update(cx, |transcript, cx| {
                    transcript.set_saved_draft(state.draft.clone(), cx)
                });
                view
            })
        })
        .unwrap()
    });
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(size(px(900.), px(680.)));
    visual.run_until_parked();
    let edit = visual.debug_bounds("edit-message-draft").unwrap().center();
    visual.simulate_click(edit, Default::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("Intermediate");
    visual.run_until_parked();

    // The second connection queues exactly the intermediate snapshot while
    // the first window keeps the same editor open.
    let mut other = Store::open(&path).unwrap();
    let stale_state = other.follow_ups(session).unwrap();
    let intermediate = stale_state.draft.as_ref().unwrap().clone();
    assert_eq!(intermediate.text, "Intermediate");
    assert_ne!(intermediate.id, "original");
    let queued = Message {
        id: intermediate.id.clone(),
        text: intermediate.text.clone(),
        prepared_prompt: intermediate.prepared_prompt.clone(),
        attachments: intermediate
            .attachments
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        ..message(&intermediate.id, &intermediate.text, false)
    };
    let mut roster = vorkspace_core::bot::Roster::load(&other).unwrap();
    roster.get_mut("bot").unwrap().outbox.push(queued.clone());
    roster.save(&other).unwrap();

    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("Final version");
    visual.run_until_parked();
    let final_draft = other.follow_ups(session).unwrap().draft.unwrap();
    assert_eq!(final_draft.text, "Final version");
    assert_ne!(final_draft.id, intermediate.id);
    let marker: serde_json::Value = serde_json::from_str(
        &other
            .setting(&format!("bot-draft-origin:{session}"))
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(marker["id"], intermediate.id);
    assert_eq!(marker["nextId"], final_draft.id);
    assert_eq!(final_draft.attachments.len(), 1);
    assert_eq!(
        std::fs::read(&final_draft.attachments[0]).unwrap(),
        b"saved attachment"
    );
    let mut conflicting = stale_state;
    conflicting.revision += 1;
    conflicting.draft.as_mut().unwrap().text = "Concurrent edit".into();
    assert!(other.accept_follow_ups(session, &conflicting).is_err());
    assert_eq!(
        other.follow_ups(session).unwrap().draft.unwrap(),
        final_draft
    );

    assert!(
        other
            .claim_bot_message("bot", &queued, "owner-one")
            .unwrap()
    );
    assert!(
        !other
            .claim_bot_message("bot", &queued, "owner-two")
            .unwrap()
    );
    let mut transcript = vorkspace_core::transcript::Transcript::default();
    transcript.begin("user");
    transcript.append(&queued.text);
    other
        .write_rows_with_message(session, 0, &transcript.rows, Some(&queued.id))
        .unwrap();
    window
        .update(visual, |view, _, cx| {
            view.bot_draft_outbox_saved(&queued, cx);
            assert_eq!(view.follow_ups.state.draft.as_ref(), Some(&final_draft));
        })
        .unwrap();
    drop(other);
    let reopened = Store::open(&path).unwrap();
    assert!(
        reopened
            .setting(&format!("bot-delivered:{session}:{}", intermediate.id))
            .unwrap()
            .is_some()
    );
    assert_eq!(
        reopened.follow_ups(session).unwrap().draft.unwrap(),
        final_draft
    );
    assert!(
        !reopened
            .claim_bot_message("bot", &queued, "owner-three")
            .unwrap()
    );
}

#[gpui::test]
fn bot_plus_menu_and_saved_draft_card_share_session_actions(cx: &mut TestAppContext) {
    let window = chat(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(size(px(900.), px(680.)));
    visual.run_until_parked();
    let add = visual.debug_bounds("composer-add").unwrap();
    visual.simulate_click(add.center(), Default::default());
    visual.run_until_parked();
    for selector in [
        "menu-action-upload",
        "menu-action-draft",
        "menu-action-queue",
    ] {
        assert!(
            visual.debug_bounds(selector).is_some(),
            "missing {selector}"
        );
    }
    for selector in [
        "menu-action-plan",
        "menu-action-operator",
        "menu-action-orchestrator",
    ] {
        assert!(
            visual.debug_bounds(selector).is_none(),
            "unexpected {selector}"
        );
    }
    let draft = visual.debug_bounds("menu-action-draft").unwrap();
    visual.simulate_click(draft.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |view, _, cx| {
            assert_eq!(view.input.read(cx).content, "/draft Unsent draft");
            view.send(cx);
        })
        .unwrap();
    visual.run_until_parked();
    crate::clock::settle_animations(window, visual);
    assert!(visual.debug_bounds("edit-message-draft").is_some());
    assert!(visual.debug_bounds("remove-message-draft").is_some());
    assert!(visual.debug_bounds("send-message-draft").is_some());
    for width in [260., 360., 900.] {
        visual.simulate_resize(size(px(width), px(680.)));
        visual.run_until_parked();
        crate::clock::settle_animations(window, visual);
        for selector in [
            "edit-message-draft",
            "remove-message-draft",
            "send-message-draft",
        ] {
            let bounds = visual.debug_bounds(selector).unwrap();
            assert!(
                bounds.left() >= px(0.) && bounds.right() <= px(width),
                "{selector} escapes the {width}px pane: {bounds:?}"
            );
        }
    }
    let original_id = window
        .read_with(visual, |view, _| {
            view.follow_ups.state.draft.as_ref().unwrap().id.clone()
        })
        .unwrap();
    let edit = visual.debug_bounds("edit-message-draft").unwrap();
    visual.simulate_click(edit.center(), Default::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("Edited draft");
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            let db = locked(&view.store).unwrap();
            db.save_setting(
                &format!("bot-delivered:{}:{original_id}", view.summary.id),
                "1",
            )
            .unwrap();
            assert_eq!(
                db.follow_ups(view.summary.id).unwrap().draft.unwrap().text,
                "Edited draft",
                "an in-progress edit survives an older version's delivery receipt"
            );
        })
        .unwrap();
    let save = visual.debug_bounds("save-draft-edit").unwrap();
    visual.simulate_click(save.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |view, _, cx| {
            assert_eq!(
                view.follow_ups.state.draft.as_ref().unwrap().text,
                "Edited draft"
            );
            assert_eq!(
                locked(&view.store)
                    .unwrap()
                    .follow_ups(view.summary.id)
                    .unwrap()
                    .draft
                    .unwrap()
                    .text,
                "Edited draft"
            );
            view.input
                .update(cx, |input, cx| input.set_text("Keep composing".into(), cx));
        })
        .unwrap();
    let remove = visual.debug_bounds("remove-message-draft").unwrap();
    visual.simulate_click(remove.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |view, _, cx| {
            assert!(view.follow_ups.state.draft.is_none());
            assert_eq!(view.input.read(cx).content, "Keep composing");
        })
        .unwrap();
}

#[gpui::test]
fn saved_bot_draft_survives_reopen_and_transfers_the_same_attachment_and_identity(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("draft.sqlite");
    let source = dir.path().join("attachment.png");
    image::RgbaImage::from_pixel(100, 80, image::Rgba([255, 255, 255, 255]))
        .save(&source)
        .unwrap();
    let store = Store::open(&path).unwrap();
    let summary = store.create("codex", "/private/tmp/bot-draft").unwrap();
    let session = summary.id;
    let window = cx.update(|cx| {
        input::bind(cx);
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut view = Conversation::new(summary, Arc::new(Mutex::new(store)), cx);
                view.loaded = true;
                view.configure_bot(Bot::default(), dir.path().into(), Default::default(), cx);
                view
            })
        })
        .unwrap()
    });
    cx.run_until_parked();
    let owner = window
        .update(cx, |view, _, cx| {
            view.input.update(cx, |input, cx| {
                input.set_text("/draft Original message".into(), cx)
            });
            view.attachments.push(source.clone());
            view.capture_composer(cx);
            view.composer_draft.flush().unwrap();
            cx.entity()
        })
        .unwrap();
    let lightbox = cx.update(|cx| {
        cx.open_window(Default::default(), |window, cx| {
            cx.new(|cx| {
                crate::image_lightbox::ImageLightbox::new(
                    crate::image_lightbox::Open {
                        source: crate::image_lightbox::Source::Path(source.clone()),
                        alt: "Draft image".into(),
                        draft_session: Some(session),
                    },
                    Some(owner.clone()),
                    window,
                    cx,
                )
            })
        })
        .unwrap()
    });
    let visual = &mut VisualTestContext::from_window(lightbox.into(), cx);
    visual.run_until_parked();
    visual.refresh().unwrap();
    let brush = visual.debug_bounds("annotation-brush").unwrap().center();
    visual.simulate_click(brush, Default::default());
    let bounds = visual.debug_bounds("lightbox-image").unwrap();
    let start = gpui::point(bounds.left() + px(25.), bounds.top() + px(40.));
    let end = gpui::point(bounds.left() + px(75.), bounds.top() + px(40.));
    visual.simulate_mouse_down(start, gpui::MouseButton::Left, Default::default());
    visual.simulate_mouse_move(end, Some(gpui::MouseButton::Left), Default::default());
    visual.simulate_mouse_up(end, gpui::MouseButton::Left, Default::default());
    let close = visual.debug_bounds("close-image-preview").unwrap().center();
    visual.simulate_click(close, Default::default());
    visual.run_until_parked();
    let annotated = owner.read_with(visual, |view, _| view.attachments[0].clone());
    assert_eq!(
        image::open(&annotated)
            .unwrap()
            .into_rgba8()
            .get_pixel(50, 40)
            .0,
        [255, 59, 48, 255]
    );
    lightbox
        .update(visual, |_, window, _| window.remove_window())
        .unwrap();
    window
        .update(cx, |view, _, cx| {
            view.send(cx);
            assert!(view.follow_ups.state.draft.is_some());
            view.app_flush_draft(cx).unwrap();
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    drop(owner);
    std::fs::write(&source, "changed bytes").unwrap();
    let store = Store::open(&path).unwrap();
    let summary = store
        .sessions()
        .unwrap()
        .into_iter()
        .find(|s| s.id == session)
        .unwrap();
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut view = Conversation::new(summary, Arc::new(Mutex::new(store)), cx);
                view.configure_bot(Bot::default(), dir.path().into(), Default::default(), cx);
                view.load(cx);
                view
            })
        })
        .unwrap()
    });
    cx.run_until_parked();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(size(px(900.), px(680.)));
    visual.run_until_parked();
    crate::clock::settle_animations(window, visual);
    let edit = visual.debug_bounds("edit-message-draft").unwrap().center();
    visual.simulate_click(edit, Default::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("Edited message");
    visual.run_until_parked();
    let save = visual.debug_bounds("save-draft-edit").unwrap().center();
    visual.simulate_click(save, Default::default());
    visual.run_until_parked();
    let events = Rc::new(RefCell::new(Vec::new()));
    let observed = events.clone();
    let _subscription = window
        .update(cx, |view, _, cx| {
            let subscription = cx.subscribe(&cx.entity(), move |_, _, event: &Submitted, _| {
                observed.borrow_mut().push((
                    event.draft_id.clone(),
                    event.text.clone(),
                    event.attachments.clone(),
                    event.queue,
                    event.prepared_prompt.clone(),
                ));
            });
            let saved = view.follow_ups.state.draft.as_ref().unwrap();
            assert_eq!(saved.text, "Edited message");
            assert_eq!(
                image::open(&saved.attachments[0])
                    .unwrap()
                    .into_rgba8()
                    .get_pixel(50, 40)
                    .0,
                [255, 59, 48, 255]
            );
            view.follow_ups
                .state
                .draft
                .as_mut()
                .unwrap()
                .prepared_prompt = Some("Saved MCP context\n\nEdited message".into());
            view.input.update(cx, |input, cx| {
                input.set_text("New unsent message".into(), cx)
            });
            view.attachments.push(source.clone());
            view.bot_ui.queue = true;
            view.draft_action(true, cx);
            assert!(view.follow_ups.state.draft.is_some());
            subscription
        })
        .unwrap();
    cx.run_until_parked();
    let first = events.borrow()[0].clone();
    assert_eq!(first.1, "Edited message");
    assert_eq!(
        image::open(&first.2[0])
            .unwrap()
            .into_rgba8()
            .get_pixel(50, 40)
            .0,
        [255, 59, 48, 255]
    );
    assert!(first.3);
    assert_eq!(
        first.4.as_deref(),
        Some("Saved MCP context\n\nEdited message")
    );
    window
        .update(cx, |view, _, cx| {
            view.bot_draft_outbox_failed(first.0.as_ref().unwrap(), "disk full", cx);
            view.draft_action(false, cx);
            assert!(view.follow_ups.state.draft.is_some());
            view.draft_action(true, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(events.borrow()[1], first);
    window
        .update(cx, |view, _, cx| {
            view.bot_draft_outbox_saved(
                &Message {
                    prepared_prompt: first.4.clone(),
                    id: first.0.clone().unwrap(),
                    text: first.1.clone(),
                    attachments: first
                        .2
                        .iter()
                        .map(|p| p.to_string_lossy().to_string())
                        .collect(),
                    peer: None,
                    sent_at: None,
                    internal: false,
                    error: None,
                },
                cx,
            );
            assert!(view.follow_ups.state.draft.is_none());
            assert_eq!(view.input.read(cx).content, "New unsent message");
            assert_eq!(view.attachments.as_slice(), std::slice::from_ref(&source));
        })
        .unwrap();
}

#[gpui::test]
fn older_outbox_ack_cannot_remove_an_edited_draft_from_another_connection(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ownership.sqlite");
    let store = Store::open(&path).unwrap();
    let summary = store.create("codex", "/tmp").unwrap();
    let id = summary.id;
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut view = Conversation::new(summary, Arc::new(Mutex::new(store)), cx);
                view.loaded = true;
                view.configure_bot(Bot::default(), dir.path().into(), Default::default(), cx);
                view
            })
        })
        .unwrap()
    });
    cx.run_until_parked();
    let old = window
        .update(cx, |view, _, cx| {
            view.input
                .update(cx, |input, cx| input.set_text("/draft Original".into(), cx));
            view.send(cx);
            view.follow_ups.state.draft.clone().unwrap()
        })
        .unwrap();
    let other = Store::open(&path).unwrap();
    let mut newer = other.follow_ups(id).unwrap();
    newer.revision += 1;
    newer.draft.as_mut().unwrap().id = "edited-version".into();
    newer.draft.as_mut().unwrap().text = "Newer edit".into();
    other.accept_follow_ups(id, &newer).unwrap();
    window
        .update(cx, |view, _, cx| {
            view.bot_draft_outbox_saved(&message(&old.id, &old.text, false), cx);
            assert!(
                view.follow_ups.state.draft.is_some(),
                "a stale view must not clear its draft"
            );
        })
        .unwrap();
    assert_eq!(
        other.follow_ups(id).unwrap().draft.unwrap().text,
        "Newer edit"
    );
}

#[gpui::test]
fn restart_consumes_only_the_composer_revision_that_became_a_saved_bot_draft(
    cx: &mut TestAppContext,
) {
    for (open, newer) in [
        (Store::open as fn(&Path) -> Result<Store, String>, false),
        (Store::open, true),
        (Store::open_vorkspace, false),
        (Store::open_vorkspace, true),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let store = open(&dir.path().join("crash.sqlite")).unwrap();
        let summary = store.create("codex", "/tmp").unwrap();
        let payload = serde_json::json!({"cwd":"/tmp", "text":"/draft recover me", "selectionStart":17, "selectionEnd":17, "attachments":[], "mcpTags":[]});
        let revision = store.save_composer_draft(summary.id, 0, &payload).unwrap();
        store
            .save_setting(
                &format!("bot-draft-origin:{}", summary.id),
                &serde_json::json!({"id":"draft-crash", "revision":revision}).to_string(),
            )
            .unwrap();
        let mut state = store.follow_ups(summary.id).unwrap();
        state.revision += 1;
        state.draft = Some(vorkspace_core::follow_up::Message {
            id: "draft-crash".into(),
            text: "recover me".into(),
            ..Default::default()
        });
        store.accept_follow_ups(summary.id, &state).unwrap();
        if newer {
            store
                .save_composer_draft(summary.id, revision, &payload)
                .unwrap();
        }
        let persisted_id = store.persisted_session_id(summary.id).unwrap();
        drop(store);
        let store = open(&dir.path().join("crash.sqlite")).unwrap();
        let reopened_id = store.resolve_session_id(&persisted_id).unwrap();
        let summary = store
            .sessions()
            .unwrap()
            .into_iter()
            .find(|session| session.id == reopened_id)
            .unwrap();
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| {
                    let mut view = Conversation::new(summary, Arc::new(Mutex::new(store)), cx);
                    view.configure_bot(Bot::default(), dir.path().into(), Default::default(), cx);
                    view.load(cx);
                    view
                })
            })
            .unwrap()
        });
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                assert_eq!(
                    view.follow_ups.state.draft.as_ref().unwrap().text,
                    "recover me"
                );
                assert_eq!(
                    view.input.read(cx).content,
                    if newer { "/draft recover me" } else { "" }
                );
            })
            .unwrap();
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
    }
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "Requires Metal and VORKSPACE_BOT_DRAFT_CAPTURE_DIR"]
fn native_bot_draft_and_add_menu_capture() {
    struct Capture(Entity<Conversation>);
    impl gpui::Render for Capture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().bg(pane_fill()).child(self.0.clone())
        }
    }
    let output = PathBuf::from(std::env::var_os("VORKSPACE_BOT_DRAFT_CAPTURE_DIR").unwrap());
    assert!(output.is_absolute() && output.is_dir());
    let mut cx = gpui::HeadlessAppContext::with_platform(
        Arc::new(gpui_apple::AppleTextSystem::new()),
        Arc::new(crate::assets::Assets),
        || {
            Ok(Some(Box::new(
                gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
            )))
        },
    );
    crate::clock::use_test_clock(&cx);
    for light in [false, true] {
        let prefs = crate::settings::Preferences {
            theme: if light { "light" } else { "dark" }.into(),
            ..Default::default()
        };
        crate::theme::configure(&prefs, false, true);
        cx.update(|cx| {
            cx.set_global(prefs);
            input::bind(cx);
        });
        let store = Store::open(Path::new(":memory:")).unwrap();
        let summary = store.create("codex", "/tmp").unwrap();
        let captured_chat = Rc::new(RefCell::new(None));
        let captured = captured_chat.clone();
        let window = cx
            .open_window(size(px(900.), px(680.)), |_, cx| {
                let chat = cx.new(|cx| {
                    let mut view = Conversation::new(summary, Arc::new(Mutex::new(store)), cx);
                    view.loaded = true;
                    view.configure_bot(
                        Bot {
                            name: "Sam".into(),
                            ..Default::default()
                        },
                        "/tmp".into(),
                        Default::default(),
                        cx,
                    );
                    view.bot_post("assistant", "What would you like me to work on?".into(), cx)
                        .unwrap();
                    view
                });
                *captured.borrow_mut() = Some(chat.clone());
                cx.new(|_| Capture(chat))
            })
            .unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, _, cx| {
            captured_chat
                .borrow()
                .as_ref()
                .unwrap()
                .update(cx, |chat, cx| {
                    chat.input.update(cx, |input, cx| {
                        input.set_text("/draft Review the next change".into(), cx)
                    });
                    chat.send(cx);
                    assert!(chat.follow_ups.state.draft.is_some());
                });
        })
        .unwrap();
        cx.run_until_parked();
        crate::clock::settle_headless_animations(window, &mut cx);
        let theme = if light { "light" } else { "dark" };
        cx.capture_screenshot(window.into())
            .unwrap()
            .save(output.join(format!("bot-draft-{theme}.png")))
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            captured_chat
                .borrow()
                .as_ref()
                .unwrap()
                .update(cx, |chat, cx| {
                    chat.toggle_menu(ComposerMenu::Add, window, cx)
                });
        })
        .unwrap();
        cx.run_until_parked();
        crate::clock::settle_headless_animations(window, &mut cx);
        cx.capture_screenshot(window.into())
            .unwrap()
            .save(output.join(format!("bot-menu-{theme}.png")))
            .unwrap();
        cx.update_window(window.into(), |_, window, _| window.remove_window())
            .unwrap();
    }
    crate::theme::configure(&Default::default(), false, true);
}

#[gpui::test]
fn typing_keeps_the_unchanged_bot_transcript_rendered(cx: &mut TestAppContext) {
    let window = chat(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(size(px(900.), px(680.)));
    window
        .update(visual, |view, window, cx| {
            window.focus(&view.input.focus_handle(cx), cx);
        })
        .unwrap();
    visual.run_until_parked();
    // Establish the nonempty composer and its final geometry before measuring.
    visual.simulate_input("a");
    visual.run_until_parked();
    let (transcript, draft, source, renders) = window
        .read_with(visual, |view, cx| {
            let transcript = view.transcript.read(cx);
            (
                view.transcript.clone(),
                view.input.read(cx).content.to_string(),
                transcript.source_rows().to_vec(),
                transcript.render_passes,
            )
        })
        .unwrap();
    for _ in 0..60 {
        visual.simulate_input("a");
        visual.run_until_parked();
    }
    transcript.read_with(visual, |view, _| {
        assert_eq!(view.render_passes, renders);
        assert_eq!(view.source_rows(), source);
    });
    window
        .update(visual, |view, window, cx| {
            assert_eq!(
                view.input.read(cx).content.as_ref(),
                draft + &"a".repeat(60)
            );
            assert!(view.input.focus_handle(cx).is_focused(window));
        })
        .unwrap();

    window
        .update(visual, |view, _, cx| {
            view.bot_post("assistant", "A newly published reply.".into(), cx)
                .unwrap();
        })
        .unwrap();
    visual.run_until_parked();
    transcript.read_with(visual, |view, _| {
        assert!(view.render_passes > renders);
        assert_eq!(
            view.source_rows().last().unwrap().text,
            "A newly published reply."
        );
    });
}

#[gpui::test]
fn header_ticks_do_not_notify_the_chat_or_rebuild_its_transcript(cx: &mut TestAppContext) {
    use std::cell::Cell;
    let window = chat(cx);
    window
        .update(cx, |view, _, cx| view.show_bot_header(true, cx))
        .unwrap();
    cx.run_until_parked();
    let (header, transcript) = window
        .read_with(cx, |view, _| {
            (view.bot_ui.header.clone().unwrap(), view.transcript.clone())
        })
        .unwrap();
    let chat = window.entity(cx).unwrap();
    let notifications = Rc::new(Cell::new(0));
    let observed = notifications.clone();
    let _subscription =
        cx.update(|cx| cx.observe(&chat, move |_, _| observed.set(observed.get() + 1)));
    let renders = transcript.read_with(cx, |view, _| view.render_passes);
    for _ in 0..60 {
        // The real animation timer notifies precisely this retained owner.
        // Perpetual timers stay suppressed on the deterministic test clock.
        header.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
    }
    assert_eq!(notifications.get(), 0);
    assert_eq!(
        transcript.read_with(cx, |view, _| view.render_passes),
        renders
    );
    window
        .read_with(cx, |view, _| {
            assert_eq!(view.bot_ui.header.as_ref().unwrap(), &header);
        })
        .unwrap();
}

#[gpui::test]
fn automatic_updates_stay_quiet_and_peer_delivery_keeps_one_transcript_message(
    cx: &mut TestAppContext,
) {
    let window = chat(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    for width in [360., 900.] {
        visual.simulate_resize(size(px(width), px(600.)));
        window
            .update(visual, |view, _, cx| {
                view.busy = false;
                view.bot_ui.pending.clear();
                view.bot_ui.delivered = None;
                view.sync_bot_pending(cx);
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        let composer = visual.debug_bounds("composer-box").unwrap();
        window
            .update(visual, |view, _, cx| {
                view.busy = true;
                view.bot_ui.pending = vec![
                    message("background", "[Delegated work completed]", true),
                    peer_message(),
                    message("human", "Check recovery next", false),
                ];
                view.sync_bot_pending(cx);
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        assert!(visual.debug_bounds("bot-background-update-error").is_none());
        assert!(visual.debug_bounds("bot-pending-peer").is_some());
        assert!(visual.debug_bounds("bot-pending-human").is_some());
        assert_eq!(visual.debug_bounds("composer-box").unwrap(), composer);

        window
            .update(visual, |view, _, cx| {
                view.busy = false;
                view.bot_post("bot-system", peer_message().text, cx)
                    .unwrap();
                view.bot_ui.delivered = Some("peer".into());
                view.sync_bot_pending(cx);
                view.busy = true;
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        assert!(visual.debug_bounds("bot-pending-peer").is_none());
        assert!(visual.debug_bounds("bot-peer-message-label").is_some());
        assert!(visual.debug_bounds("bot-background-update-error").is_none());
        assert_eq!(visual.debug_bounds("composer-box").unwrap(), composer);
        window
            .update(visual, |view, _, cx| {
                assert_eq!(
                    view.bot_ui.pending.len(),
                    3,
                    "presentation must preserve the pending messages"
                );
                assert_eq!(view.input.read(cx).content.as_ref(), "Unsent draft");
            })
            .unwrap();
    }
}

#[gpui::test]
fn background_and_peer_errors_show_recovery_controls_once(cx: &mut TestAppContext) {
    let window = chat(cx);
    let events = Rc::new(RefCell::new(Vec::new()));
    let _subscriptions = window
        .update(cx, |view, _, cx| {
            let background = events.clone();
            let peer = events.clone();
            let subscriptions = (
                cx.subscribe(&cx.entity(), move |_, _, event: &QueueChanged, _| {
                    background
                        .borrow_mut()
                        .push((event.id.clone(), event.remove));
                }),
                cx.subscribe(
                    &view.transcript,
                    move |_, _, event: &crate::transcript::BotResponseAction, _| {
                        if let crate::transcript::BotResponseAction::Queue { id, remove } = event {
                            peer.borrow_mut().push((id.clone(), *remove));
                        }
                    },
                ),
            );
            let mut background = message("background", "[Delegated work completed]", true);
            background.error = Some("Couldn't deliver the background update".into());
            let mut peer = peer_message();
            peer.error = Some("Stopped before the peer reply completed".into());
            view.bot_ui.pending = vec![background, peer];
            view.bot_ui.delivered = Some("peer".into());
            view.sync_bot_pending(cx);
            cx.notify();
            subscriptions
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(size(px(360.), px(600.)));
    visual.run_until_parked();
    crate::clock::settle_animations(window, visual);
    assert!(visual.debug_bounds("bot-background-update-error").is_some());
    assert!(visual.debug_bounds("bot-pending-peer").is_some());
    for selector in [
        "bot-queue-retry",
        "bot-queue-discard",
        "bot-pending-retry-peer",
        "bot-pending-discard-peer",
    ] {
        let button = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("missing {selector}"));
        assert!(
            button.left() >= px(0.) && button.right() <= px(360.),
            "{selector} must fit the viewport: {button:?}"
        );
        visual.simulate_click(button.center(), Default::default());
        visual.run_until_parked();
        crate::clock::settle_animations(window, visual);
    }
    assert_eq!(
        *events.borrow(),
        vec![
            ("background".into(), false),
            ("background".into(), true),
            ("peer".into(), false),
            ("peer".into(), true),
        ]
    );
    window
        .update(visual, |view, _, cx| {
            view.bot_ui.pending.retain(|message| message.peer.is_some());
            view.sync_bot_pending(cx);
            cx.notify();
        })
        .unwrap();
    visual.run_until_parked();
    crate::clock::settle_animations(window, visual);
    assert!(visual.debug_bounds("bot-background-update-error").is_none());
    assert!(visual.debug_bounds("bot-pending-retry-peer").is_some());
    assert!(visual.debug_bounds("bot-pending-discard-peer").is_some());
}

#[gpui::test]
fn only_the_users_own_message_steers_a_running_bot_turn(cx: &mut TestAppContext) {
    let window = chat(cx);
    let (controls, received) = std::sync::mpsc::channel();
    let failures = Rc::new(RefCell::new(Vec::new()));
    window
        .update(cx, |view, _, cx| {
            let seen = failures.clone();
            cx.subscribe(&cx.entity(), move |_, _, event: &SteerFailed, _| {
                seen.borrow_mut()
                    .push((event.id.clone(), event.error.clone()));
            })
            .detach();
            view.busy = true;
            view.controls = Some(controls);
            let user = Message {
                prepared_prompt: Some("Saved MCP context\n\nUse the other approach".into()),
                attachments: vec!["/tmp/plan.png".into()],
                ..message("user", "Use the other approach", false)
            };
            assert!(
                !view.bot_steer(&user, cx),
                "a turn that has not reported readiness keeps the message queued"
            );
            view.follow_ups.ready = true;
            assert!(!view.bot_steer(&message("report", "Completion", true), cx));
            assert!(!view.bot_steer(&peer_message(), cx));
            assert!(view.bot_steer(&user, cx));
            assert!(
                !view.bot_steer(&message("next", "And this", false), cx),
                "one steer is in flight at a time"
            );
            assert!(view.bot_delivered().is_none());
            view.follow_up_delivered("user", cx);
            assert_eq!(view.bot_delivered(), Some("user"));
            assert!(view.bot_steer(&message("next", "And this", false), cx));
            view.fail_follow_up("next", "turn is closing".into(), cx);
            assert!(view.follow_ups.sending.is_none());
        })
        .unwrap();
    let Decision::Steer(steer) = received.try_recv().unwrap().decision else {
        panic!("expected steering");
    };
    assert_eq!(steer.id, "user");
    assert_eq!(steer.text, "Use the other approach");
    assert_eq!(
        steer.prepared_prompt.as_deref(),
        Some("Saved MCP context\n\nUse the other approach")
    );
    assert_eq!(steer.attachments, [PathBuf::from("/tmp/plan.png")]);
    assert!(matches!(received.try_recv().unwrap().decision, Decision::Steer(m) if m.id == "next"));
    assert!(received.try_recv().is_err());
    assert_eq!(
        *failures.borrow(),
        [("next".to_owned(), "turn is closing".to_owned())]
    );
}

#[gpui::test]
fn a_final_update_keeps_the_accepted_steer_receipt(cx: &mut TestAppContext) {
    let window = chat(cx);
    let (controls, _received) = std::sync::mpsc::channel();
    window
        .update(cx, |view, _, cx| {
            view.busy = true;
            view.controls = Some(controls);
            view.follow_ups.ready = true;
            assert!(view.bot_steer(&message("steer", "Change course", false), cx));
            // A stop can skip the intermediate acknowledgement. The final
            // update then carries the turn's initial receipt and the steer.
            view.apply_receipts(Some("initial".into()), vec!["steer".into()], Vec::new(), cx);
            assert_eq!(view.bot_delivered(), Some("steer"));
            assert!(view.follow_ups.sending.is_none());
        })
        .unwrap();
}

fn card_fixture(
    cx: &mut TestAppContext,
) -> (
    WindowHandle<Conversation>,
    Database,
    i64,
    usize,
    serde_json::Value,
) {
    let window = chat(cx);
    let (store, id, index, card) = window.update(cx, |view, _, cx| {
        let card = serde_json::json!({"id":"refresh-card","type":"pr","repo":"synthetic/repo","number":1,"title":"Old"});
        let index = view.bot_rows(cx).len();
        view.bot_post("bot-card", card.to_string(), cx).unwrap();
        (view.store.clone(), view.summary.id, index, card)
    }).unwrap();
    (window, store, id, index, card)
}

#[gpui::test]
fn card_save_admission_stays_responsive_and_publishes_latest_at_the_current_row(
    cx: &mut TestAppContext,
) {
    let (window, store, id, index, card) = card_fixture(cx);
    let guard = locked(&store).unwrap();
    window
        .update(cx, |view, _, cx| {
            // This would deadlock if admission took the Store mutex on the UI.
            view.bot_patch_card(
                index,
                "refresh-card",
                view.submission_generation,
                card.clone(),
                card.clone(),
                cx,
            )
            .unwrap();
            assert!(view.bot_ui.card_versions.is_empty());
            for title in ["One", "Two"] {
                let mut next = card.clone();
                next["title"] = serde_json::json!(title);
                view.bot_patch_card(
                    index,
                    "refresh-card",
                    view.submission_generation,
                    card.clone(),
                    next,
                    cx,
                )
                .unwrap();
            }
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&view.bot_rows(cx)[index].text).unwrap(),
                card
            );
            view.input.update(cx, |input, cx| {
                input.set_text("Still responsive".into(), cx)
            });
            // A reprojected row position cannot redirect the completion's identity.
            let mut rows = view.bot_rows(cx).to_vec();
            rows.insert(
                0,
                vorkspace_core::transcript::Row {
                    role: "assistant".into(),
                    text: "Synthetic prefix".into(),
                    first: true,
                    sent_at: None,
                },
            );
            view.transcript.update(cx, |t, cx| t.patch(0, rows, cx));
        })
        .unwrap();
    drop(guard);
    cx.update(|cx| crate::runtime::card_persistence::finish_for_test(cx));
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            let (_, saved) = card_at(view.bot_rows(cx), index, "refresh-card").unwrap();
            assert_eq!(saved["title"], "Two");
            assert_eq!(view.bot_rows(cx)[0].text, "Synthetic prefix");
            assert_eq!(view.input.read(cx).content, "Still responsive");
        })
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            &locked(&store).unwrap().load(id).unwrap().rows[index].text
        )
        .unwrap()["title"],
        "Two"
    );
}

#[gpui::test]
fn committed_card_refresh_survives_a_new_submission_and_the_next_refresh(cx: &mut TestAppContext) {
    for advance_before_commit in [true, false] {
        let (window, store, session, index, original) = card_fixture(cx);
        let mut committed = original.clone();
        committed["title"] = serde_json::json!("Committed");
        let guard = locked(&store).unwrap();
        let generation = window
            .update(cx, |view, _, cx| {
                let generation = view.submission_generation;
                view.bot_patch_card(
                    index,
                    "refresh-card",
                    generation,
                    original.clone(),
                    committed.clone(),
                    cx,
                )
                .unwrap();
                if advance_before_commit {
                    view.submission_generation += 1;
                }
                generation
            })
            .unwrap();
        drop(guard);
        // Wait for the actual worker's commit without handling the UI receipt
        // or closing the worker that must perform the following refresh.
        cx.update(|cx| crate::runtime::card_persistence::wait_for_saves_for_test(cx));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(
                &locked(&store).unwrap().load(session).unwrap().rows[index].text
            )
            .unwrap(),
            committed
        );
        window
            .update(cx, |view, _, cx| {
                assert_eq!(
                    card_at(view.bot_rows(cx), index, "refresh-card").unwrap().1,
                    original
                );
                if !advance_before_commit {
                    view.submission_generation += 1;
                }
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                assert_eq!(
                    card_at(view.bot_rows(cx), index, "refresh-card").unwrap().1,
                    committed
                );
                assert!(view.bot_ui.card_versions.is_empty());
                assert_eq!(view.input.read(cx).content, "Unsent draft");
                // A fetch begun in the old submission is still inadmissible,
                // even though it now happens to match the reconciled base.
                let mut delayed = committed.clone();
                delayed["title"] = serde_json::json!("Obsolete fetch");
                view.bot_patch_card(
                    index,
                    "refresh-card",
                    generation,
                    committed.clone(),
                    delayed,
                    cx,
                )
                .unwrap();
                assert!(view.bot_ui.card_versions.is_empty());
                let mut latest = committed.clone();
                latest["title"] = serde_json::json!("Latest");
                let expected = card_at(view.bot_rows(cx), index, "refresh-card").unwrap().1;
                view.bot_patch_card(
                    index,
                    "refresh-card",
                    view.submission_generation,
                    expected,
                    latest,
                    cx,
                )
                .unwrap();
                assert!(!view.bot_ui.card_versions.is_empty());
            })
            .unwrap();
        cx.update(|cx| crate::runtime::card_persistence::wait_for_saves_for_test(cx));
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                assert_eq!(
                    card_at(view.bot_rows(cx), index, "refresh-card").unwrap().1["title"],
                    "Latest"
                );
                assert!(view.bot_ui.card_versions.is_empty());
            })
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(
                &locked(&store).unwrap().load(session).unwrap().rows[index].text
            )
            .unwrap()["title"],
            "Latest"
        );
    }
    cx.update(|cx| crate::runtime::card_persistence::finish_for_test(cx));
}

#[gpui::test]
fn card_save_completion_keeps_replacements_and_reconciles_new_submissions(cx: &mut TestAppContext) {
    let first = card_fixture(cx);
    let second = card_fixture(cx);
    let guard1 = locked(&first.1).unwrap();
    let guard2 = locked(&second.1).unwrap();
    for (fixture, replacement) in [(&first, true), (&second, false)] {
        let (window, _, _, index, card) = fixture;
        window
            .update(cx, |view, _, cx| {
                let mut next = card.clone();
                next["title"] = serde_json::json!("New");
                view.bot_patch_card(
                    *index,
                    "refresh-card",
                    view.submission_generation,
                    card.clone(),
                    next,
                    cx,
                )
                .unwrap();
                if replacement {
                    let mut row = view.bot_rows(cx)[*index].clone();
                    let mut new_card = card.clone();
                    new_card["id"] = serde_json::json!("replacement");
                    row.text = new_card.to_string();
                    view.transcript
                        .update(cx, |t, cx| t.patch(*index, vec![row], cx));
                } else {
                    view.submission_generation += 1;
                }
            })
            .unwrap();
    }
    drop(guard1);
    drop(guard2);
    cx.update(|cx| crate::runtime::card_persistence::finish_for_test(cx));
    cx.run_until_parked();
    for (fixture, replacement) in [(&first, true), (&second, false)] {
        let (window, store, id, index, _) = fixture;
        window
            .update(cx, |view, _, cx| {
                let visible: serde_json::Value =
                    serde_json::from_str(&view.bot_rows(cx)[*index].text).unwrap();
                assert_eq!(visible["title"], if replacement { "Old" } else { "New" });
                assert_eq!(
                    visible["id"],
                    if replacement {
                        "replacement"
                    } else {
                        "refresh-card"
                    }
                );
            })
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(
                &locked(store).unwrap().load(*id).unwrap().rows[*index].text
            )
            .unwrap()["title"],
            "New"
        );
    }
}

#[gpui::test]
fn delayed_pr_reply_is_rejected_at_the_owning_card_before_admission(cx: &mut TestAppContext) {
    let (window, store, id, index, card) = card_fixture(cx);
    let mut newer = card.clone();
    newer["title"] = serde_json::json!("Newer");
    let row = locked(&store)
        .unwrap()
        .refresh_bot_card(id, &card, &newer)
        .unwrap()
        .unwrap();
    window
        .update(cx, |view, _, cx| {
            view.transcript
                .update(cx, |t, cx| t.patch(index, vec![row], cx));
            let mut delayed = card.clone();
            delayed["title"] = serde_json::json!("Delayed");
            view.bot_patch_card(
                index,
                "refresh-card",
                view.submission_generation,
                card,
                delayed,
                cx,
            )
            .unwrap();
            assert!(view.bot_ui.card_versions.is_empty());
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&view.bot_rows(cx)[index].text).unwrap(),
                newer
            );
        })
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            &locked(&store).unwrap().load(id).unwrap().rows[index].text
        )
        .unwrap(),
        newer
    );
}

#[gpui::test]
fn card_refresh_from_an_older_submission_is_rejected_before_store_admission(
    cx: &mut TestAppContext,
) {
    let (window, store, _, index, card) = card_fixture(cx);
    let guard = locked(&store).unwrap();
    window
        .update(cx, |view, _, cx| {
            let generation = view.submission_generation;
            view.submission_generation += 1;
            let mut delayed = card.clone();
            delayed["title"] = serde_json::json!("Delayed");
            view.bot_patch_card(index, "refresh-card", generation, card.clone(), delayed, cx)
                .unwrap();
            assert!(view.bot_ui.card_versions.is_empty());
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&view.bot_rows(cx)[index].text).unwrap(),
                card
            );
        })
        .unwrap();
    drop(guard);
}

#[gpui::test]
fn bot_plus_offers_upload_and_queue_and_queue_applies_to_one_message(cx: &mut TestAppContext) {
    let window = chat(cx);
    let sent = Rc::new(RefCell::new(Vec::new()));
    let output = sent.clone();
    let _events = window
        .update(cx, |_, _, cx| {
            cx.subscribe(&cx.entity(), move |_, _, event: &Submitted, _| {
                output.borrow_mut().push((event.text.clone(), event.queue))
            })
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("composer-add").is_some());
    assert!(visual.debug_bounds("composer-queue-mode").is_none());
    window
        .update(visual, |view, window, cx| {
            let mut events = Vec::new();
            let (_, items, _, _) = view.menu_items(ComposerMenu::Add, &mut events, window, cx);
            let ids: Vec<_> = items
                .iter()
                .filter_map(|item| match item {
                    MenuItem::Action(action) => Some(action.id.to_string()),
                    _ => None,
                })
                .collect();
            assert_eq!(ids, ["upload", "draft", "queue"]);
            view.menu_choice("queue", cx);
            assert!(view.bot_ui.queue);
        })
        .unwrap();
    visual.run_until_parked();
    assert!(visual.debug_bounds("composer-queue-mode").is_some());
    window
        .update(visual, |view, _, cx| {
            view.send(cx);
            assert!(!view.bot_ui.queue, "queueing applies to one message");
            assert_eq!(view.input.read(cx).content, "Unsent draft");
            let id = view.bot_ui.direct_sending.as_ref().unwrap().0.clone();
            view.bot_draft_outbox_saved(&message(&id, "Unsent draft", false), cx);
            view.input
                .update(cx, |input, cx| input.set_text("Next".into(), cx));
            view.send(cx);
        })
        .unwrap();
    assert_eq!(
        *sent.borrow(),
        [
            ("Unsent draft".to_owned(), true),
            ("Next".to_owned(), false)
        ]
    );
}

#[gpui::test]
fn direct_bot_send_retains_recovery_until_outbox_ack_and_reuses_retry_identity(
    cx: &mut TestAppContext,
) {
    let window = chat(cx);
    cx.run_until_parked();
    let id = window
        .update(cx, |view, _, cx| {
            view.input.update(cx, |input, cx| {
                input.set_text("Keep this message".into(), cx)
            });
            view.settings.plan = true;
            view.bot_ui.queue = true;
            view.send(cx);
            assert_eq!(view.input.read(cx).content, "Keep this message");
            let pending = view.bot_ui.direct_sending.as_ref().unwrap();
            assert!(!pending.4);
            let db = locked(&view.store).unwrap();
            assert_eq!(
                db.composer_recoveries(view.summary.id).unwrap()[0].payload["text"],
                "Keep this message"
            );
            let recovery = db.composer_recoveries(view.summary.id).unwrap();
            assert_eq!(recovery[0].payload["botSubmission"]["mode"], "plan");
            assert_eq!(recovery[0].payload["botSubmission"]["queue"], true);
            pending.0.clone()
        })
        .unwrap();
    window
        .update(cx, |view, _, cx| {
            view.bot_draft_outbox_failed(&id, "disk failure", cx);
            assert_eq!(view.input.read(cx).content, "Keep this message");
            assert!(view.bot_ui.direct_sending.as_ref().unwrap().4);
            view.send(cx);
            assert_eq!(view.bot_ui.direct_sending.as_ref().unwrap().0, id);
            view.input
                .update(cx, |input, cx| input.select_utf16(0..0, cx));
            view.bot_draft_outbox_saved(&message(&id, "Keep this message", false), cx);
            assert!(view.input.read(cx).content.is_empty());
            assert!(
                locked(&view.store)
                    .unwrap()
                    .composer_recoveries(view.summary.id)
                    .unwrap()
                    .is_empty()
            );
        })
        .unwrap();
}

#[gpui::test]
fn reopened_direct_submission_reuses_identity_and_intent_after_outbox_publication(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.db");
    let image = dir.path().join("annotated.png");
    std::fs::write(&image, b"annotated pixels").unwrap();
    let store = Store::open_vorkspace(&path).unwrap();
    let summary = store.create("codex", "/project").unwrap();
    let session = summary.id;
    let db = Arc::new(Mutex::new(store));
    let create = |cx: &mut gpui::App, summary: vorkspace_core::store::Session| {
        cx.new(|cx| {
            let mut view = Conversation::new(summary, db.clone(), cx);
            view.loaded = true;
            view.configure_bot(Bot::default(), dir.path().into(), Default::default(), cx);
            view
        })
    };
    let first = cx.update(|cx| create(cx, summary.clone()));
    cx.run_until_parked();
    let sent = Rc::new(RefCell::new(Vec::new()));
    let observed = sent.clone();
    let subscription = first.update(cx, |view, cx| {
        let sub = cx.subscribe(&cx.entity(), move |_, _, event: &Submitted, _| {
            observed.borrow_mut().push((
                event.draft_id.clone().unwrap(),
                event.text.clone(),
                event.attachments.clone(),
                event.queue,
                event.mode.clone(),
            ));
        });
        view.input
            .update(cx, |input, cx| input.set_text("Send this image".into(), cx));
        view.attachments.push(image.clone());
        view.settings.plan = true;
        view.bot_ui.queue = true;
        view.send(cx);
        sub
    });
    cx.run_until_parked();
    let original = sent.borrow()[0].clone();
    let queued = Message {
        prepared_prompt: None,
        id: original.0.clone(),
        text: original.1.clone(),
        attachments: vec![image.to_string_lossy().into_owned()],
        peer: None,
        sent_at: None,
        internal: false,
        error: None,
    };
    let mut roster = vorkspace_core::bot::Roster::default();
    roster.bots.push(Bot {
        id: "bot".into(),
        session_id: Some(session),
        outbox: vec![queued.clone()],
        ..Default::default()
    });
    roster.save(&locked(&db).unwrap()).unwrap();
    assert!(
        locked(&db)
            .unwrap()
            .claim_bot_message("bot", &queued, "first-owner")
            .unwrap()
    );
    drop(subscription);
    drop(first);
    let reopened = cx.update(|cx| create(cx, summary));
    cx.run_until_parked();
    let observed = sent.clone();
    let _subscription = reopened.update(cx, |view, cx| {
        let sub = cx.subscribe(&cx.entity(), move |_, _, event: &Submitted, _| {
            observed.borrow_mut().push((
                event.draft_id.clone().unwrap(),
                event.text.clone(),
                event.attachments.clone(),
                event.queue,
                event.mode.clone(),
            ));
        });
        view.send(cx);
        sub
    });
    cx.run_until_parked();
    assert_eq!(sent.borrow()[1], original);
    assert!(
        !locked(&db)
            .unwrap()
            .claim_bot_message("bot", &queued, "second-owner")
            .unwrap()
    );
    reopened.update(cx, |view, cx| {
        view.bot_draft_outbox_saved(&queued, cx);
        assert!(view.input.read(cx).content.is_empty());
        assert!(
            locked(&view.store)
                .unwrap()
                .composer_recoveries(session)
                .unwrap()
                .is_empty()
        );
    });
}

#[gpui::test]
fn failed_composer_clear_keeps_the_submission_recovery(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.db");
    let store = Store::open_vorkspace(&path).unwrap();
    let summary = store.create("codex", "/project").unwrap();
    let id = summary.id;
    let chat = cx.new(|cx| {
        let mut chat = Conversation::new(summary, Arc::new(Mutex::new(store)), cx);
        chat.loaded = true;
        chat.configure_bot(Bot::default(), dir.path().into(), Default::default(), cx);
        chat
    });
    cx.run_until_parked();
    let pending = chat.update(cx, |chat, cx| {
        chat.input
            .update(cx, |input, cx| input.set_text("Pending".into(), cx));
        chat.send(cx);
        chat.bot_ui.direct_sending.as_ref().unwrap().0.clone()
    });
    let other = Store::open_vorkspace(&path).unwrap();
    let saved = other.composer_draft(id).unwrap().unwrap();
    other
        .save_composer_draft(id, saved.revision, &saved.payload)
        .unwrap();
    chat.update(cx, |chat, cx| {
        chat.bot_draft_outbox_saved(&message(&pending, "Pending", false), cx);
        assert!(chat.composer_draft.error.is_some());
    });
    assert!(
        other
            .composer_recoveries(id)
            .unwrap()
            .iter()
            .any(|record| record.owner == pending)
    );
}
