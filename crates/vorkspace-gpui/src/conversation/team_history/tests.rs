
use super::*;
use gpui::{TestAppContext, VisualTestContext, WindowHandle};
use std::{path::PathBuf, sync::Mutex};
use vorkspace_core::{
    bot::{Bot, Message, MinionMetadata, peers::Peer},
    store::Store,
};

fn fixture(cx: &mut TestAppContext, configured: bool, retired: bool) -> WindowHandle<Conversation> {
    vorkspace_ui::clock::use_test_clock(cx);
    let store = Store::open(Path::new(":memory:")).unwrap();
    let summary = store
        .create("codex", "/missing/team-history-fixture")
        .unwrap();
    let parent = Bot {
        id: "parent".into(),
        name: "Parent".into(),
        color: 0x22c55e,
        fg_color: Some(0x052e16),
        ..Default::default()
    };
    let actor = Bot {
        id: "member".into(),
        name: "Scout".into(),
        session_id: Some(summary.id),
        minion: Some(MinionMetadata {
            parent_id: parent.id.clone(),
            role: "Reviewer".into(),
            specialties: vec!["Storage".into()],
            retired,
            ..Default::default()
        }),
        ..Default::default()
    };
    let histories: crate::bots::TeamHistories = Default::default();
    histories.borrow_mut().insert(
        summary.id,
        crate::bots::TeamHistory {
            actor: actor.clone(),
            parent: Some(parent),
        },
    );
    cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut view = Conversation::new(summary, Arc::new(Mutex::new(store)), cx);
                view.loaded = true;
                view.team_histories = Some(histories);
                if configured {
                    view.configure_bot(
                        actor,
                        PathBuf::from("/missing/team-history-root"),
                        Default::default(),
                        cx,
                    );
                }
                view.input
                    .update(cx, |i, cx| i.set_text("Preserved draft".into(), cx));
                view.attachments = vec!["/missing/preserved.png".into()];
                view
            })
        })
    })
    .unwrap()
}

#[gpui::test]
fn team_history_rejects_user_send_voice_quote_retry_and_resume_without_consuming_drafts(
    cx: &mut TestAppContext,
) {
    for configured in [false, true] {
        for retired in [false, true] {
            let window = fixture(cx, configured, retired);
            window
                .update(cx, |view, window, cx| {
                    assert!(view.user_read_only());
                    assert!(!view.voice_composer_ready());
                    let generation = view.submission_generation;
                    let rows = view.transcript.read(cx).source_rows().to_vec();
                    view.send(cx);
                    view.quick_submit("quick".into(), Settings::default(), cx);
                    assert!(view.voice_send("voice".into(), cx).is_err());
                    assert!(view.app_start_message("app".into(), cx).is_err());
                    view.bot_quote("quoted text", window, cx);
                    view.begin_edit_last_turn(0, window, cx);
                    view.draft_action(true, cx);
                    view.launch_prompt("retry".into(), cx);
                    view.settings.usage_limit = Some(Default::default());
                    view.resume_usage(cx);
                    assert!(view.settings.usage_limit.is_some());
                    assert_eq!(view.input.read(cx).content, "Preserved draft");
                    assert_eq!(
                        view.attachments,
                        vec![PathBuf::from("/missing/preserved.png")]
                    );
                    assert_eq!(view.submission_generation, generation);
                    assert_eq!(
                        serde_json::to_value(view.transcript.read(cx).source_rows()).unwrap(),
                        serde_json::to_value(&rows).unwrap()
                    );
                    assert!(
                        locked(&view.store)
                            .unwrap()
                            .follow_ups(view.summary.id)
                            .unwrap()
                            .messages
                            .is_empty()
                    );
                    assert!(!view.busy);
                    let actor = view.team_histories.as_ref().unwrap().borrow()[&view.summary.id]
                        .actor
                        .clone();
                    let look = view.actor_look(&actor);
                    assert!(look.one_eye);
                    assert_eq!((look.bg_color, look.fg_color), (0x22c55e, 0x052e16));
                })
                .unwrap();
            let visual = &mut VisualTestContext::from_window(window.into(), cx);
            visual.refresh().unwrap();
            assert!(visual.debug_bounds("team-read-only-history").is_some());
            assert!(visual.debug_bounds("team-history-attribution").is_some());
            assert!(visual.debug_bounds("composer-box").is_none());
            assert!(visual.debug_bounds("bot-add").is_none());
        }
    }
}

#[gpui::test]
fn restored_team_views_keep_bot_capabilities_and_reject_unowned_dispatch(cx: &mut TestAppContext) {
    let window = fixture(cx, true, false);
    window
        .update(cx, |view, _, _| {
            view.app_environment
                .insert("VORKSPACE_APP_ENDPOINT".into(), "operator-endpoint".into());
            view.app_environment
                .insert("VORKSPACE_APP_TOKEN".into(), "operator-capability".into());
            view.settings
                .environment
                .insert("VORKSPACE_APP_ENDPOINT".into(), "bot-endpoint".into());
            view.settings
                .environment
                .insert("VORKSPACE_APP_TOKEN".into(), "team-capability".into());
            let mut settings = view.settings.clone();
            view.prepare_app_environment(&mut settings);
            assert_eq!(
                settings.environment["VORKSPACE_APP_ENDPOINT"],
                "bot-endpoint"
            );
            assert_eq!(
                settings.environment["VORKSPACE_APP_TOKEN"],
                "team-capability"
            );
            let mut regular = Settings::default();
            view.prepare_app_environment(&mut regular);
            assert_eq!(
                regular.environment["VORKSPACE_APP_ENDPOINT"],
                "operator-endpoint"
            );
        })
        .unwrap();
    let raw = fixture(cx, false, false);
    raw.update(cx, |view, _, cx| {
        let message = Message {
            id: "unowned".into(),
            text: "Must not execute".into(),
            internal: true,
            peer: Some(Peer {
                from: "parent".into(),
                hop: 0,
                expect_reply: true,
            }),
            prepared_prompt: None,
            sent_at: None,
            error: None,
            attachments: vec![],
        };
        assert!(!view.bot_dispatch(&message, cx));
        assert!(!view.busy);
        assert!(view.settings.bot_message_id.is_none());
    })
    .unwrap();
}

#[gpui::test]
fn internal_team_dispatch_keeps_its_admission_while_user_input_is_read_only(
    cx: &mut TestAppContext,
) {
    // An unsupported provider fails locally before any subprocess can start.
    // This asserts owner dispatch reaches launch admission, not provider success.
    let window = fixture(cx, true, false);
    window
        .update(cx, |view, _, cx| {
            view.summary.provider = "removed-provider".into();
            let message = Message {
                id: "team-work".into(),
                text: "Review storage".into(),
                internal: true,
                peer: Some(Peer {
                    from: "parent".into(),
                    hop: 0,
                    expect_reply: true,
                }),
                prepared_prompt: None,
                sent_at: None,
                error: None,
                attachments: Vec::new(),
            };
            assert!(view.bot_dispatch(&message, cx));
            assert!(view.status.contains("no longer supported"));
            assert_eq!(view.settings.bot_message_id.as_deref(), Some("team-work"));
            assert!(
                !view.team_dispatch,
                "the internal grant never leaks to a later user call"
            );
            view.send(cx);
            assert!(view.status.contains("Read-only"));
            assert_eq!(view.input.read(cx).content, "Preserved draft");
        })
        .unwrap();
}
