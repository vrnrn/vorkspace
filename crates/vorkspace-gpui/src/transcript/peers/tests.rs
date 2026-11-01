
use super::*;
use gpui::TestAppContext;
use vorkspace_core::{
    bot::{Bot, Roster, peers::enqueue},
    store::Store,
    transcript::Transcript,
};

fn prompt(body: &str, reply: bool) -> String {
    let mut roster = Roster::default();
    roster.bots = vec![
        Bot {
            id: "alpha".into(),
            name: "Alpha".into(),
            ..Default::default()
        },
        Bot {
            id: "beta".into(),
            name: "Beta".into(),
            ..Default::default()
        },
    ];
    enqueue(&mut roster, "alpha", "beta", body, "test", 0, reply).unwrap();
    roster.get_mut("beta").unwrap().outbox.remove(0).text
}

#[gpui::test]
fn saved_peer_messages_are_visible_separate_turns_without_exposing_envelopes(
    cx: &mut TestAppContext,
) {
    let body = format!(
        "Hello Beta! 👋\n{}End.",
        "A longer message 🦀\n".repeat(150)
    );
    let peer_prompt = prompt(&body, true);
    let mut transcript = Transcript::default();
    transcript.begin("user");
    transcript.append("Hello");
    transcript.begin("assistant");
    transcript.append("Hello user");
    let incoming = transcript.begin("bot-system");
    transcript.append(&peer_prompt);
    let reply = transcript.begin("assistant");
    transcript.append("Hello Alpha!");
    let literal = transcript.begin("user");
    transcript.append(&peer_prompt);
    let human_reply = transcript.begin("assistant");
    transcript.append("That is literal text from the user");
    for row in &mut transcript.rows {
        row.sent_at = row.first.then_some(1_700_000_000_000);
    }

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("peer-history.db");
    let mut store = Store::open(&path).unwrap();
    let session = store.create("codex", "/project").unwrap().id;
    store.write_rows(session, 0, &transcript.rows).unwrap();
    drop(store);
    let saved = Store::open(&path).unwrap().load(session).unwrap().rows;
    assert_eq!(saved, transcript.rows);

    let view = cx.new(|_| TranscriptView::new());
    view.update(cx, |view, cx| {
        view.bot_mode("cat", 0xffffff, "Beta");
        view.install_prepared_history(PreparedHistory::new(saved, true), cx);
        assert_eq!(view.peer_at(incoming).unwrap().name, "Alpha");
        assert_eq!(
            view.bot_group_start(reply),
            reply,
            "peer turn must not merge into the user's answer"
        );
        assert!(view.peer_response_label(reply).is_some());
        assert!(view.peer_response_label(human_reply).is_none());
        assert!(
            view.peer_at(literal).is_none(),
            "user text cannot claim peer provenance"
        );
        assert!(
            view.bot_separator(incoming).is_none(),
            "same-hour messages do not need a new date"
        );
        assert!(view.bot_separator(reply).is_none());
        let shown: String = view
            .rows
            .iter()
            .filter(|row| view.messages[row.message].start == incoming)
            .filter_map(|row| match &row.body {
                Body::Plain(text) => Some(text.as_ref()),
                _ => None,
            })
            .collect();
        assert_eq!(shown, body);
        assert_eq!(
            view.source, transcript.rows,
            "display must not rewrite provider history"
        );
    });
}

#[gpui::test]
fn queued_failed_and_one_way_messages_keep_their_origin(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let peer_prompt = prompt("Hi Beta!", false);
    let (origin, body) = presentation(&peer_prompt).unwrap();
    assert!(!origin.auto_reply);
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|_| TranscriptView::new())
        })
        .unwrap()
    });
    window
        .update(cx, |view, _, cx| {
            view.bot_mode("cat", 0xffffff, "Beta");
            view.set_pending(
                vec![PendingMessage {
                    id: "peer".into(),
                    text: body.to_owned().into(),
                    peer: Some(origin),
                    error: Some("Interrupted".into()),
                    sent_at: None,
                    attachments: Vec::new(),
                }],
                cx,
            );
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("bot-peer-message-label").is_some());
    assert!(visual.debug_bounds("bot-peer-chip").is_some());
    assert!(
        visual.debug_bounds("bot-peer-chip-avatar").is_none(),
        "an unknown sender keeps the name-only chip"
    );
    let bot = |id: &str| Bot {
        id: id.into(),
        name: "Alpha".into(),
        ..Default::default()
    };
    window
        .update(visual, |view, _, cx| {
            view.set_peer_looks(peer_looks(&[bot("a")]), cx)
        })
        .unwrap();
    visual.run_until_parked();
    assert!(visual.debug_bounds("bot-peer-chip-avatar").is_some());
    window
        .update(visual, |view, _, cx| {
            view.set_peer_looks(peer_looks(&[bot("a"), bot("b")]), cx)
        })
        .unwrap();
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("bot-peer-chip-avatar").is_none(),
        "an ambiguous name must not guess an avatar"
    );
    window
        .update(visual, |view, _, cx| {
            let mut transcript = Transcript::default();
            transcript.begin("bot-system");
            transcript.append(&peer_prompt);
            let turn = transcript.begin("assistant");
            transcript.append("Acknowledged");
            view.set_pending(Vec::new(), cx);
            view.patch(0, transcript.rows, cx);
            assert!(!view.peer_at(0).unwrap().auto_reply);
            assert!(view.peer_response_label(turn).is_some());
            assert!(view.bot_separator(0).is_some());
            assert!(
                view.bot_separator(turn).is_none(),
                "date belongs before the visible incoming message"
            );
        })
        .unwrap();
    visual.run_until_parked();
    assert!(visual.debug_bounds("bot-peer-message-label").is_some());
    assert!(visual.debug_bounds("bot-peer-response-label").is_some());
}

#[test]
fn peer_labels_read_like_chat_without_bot_to_bot_wording() {
    assert_eq!(peer_route("x", false), "→ x");
    assert_eq!(peer_route("x", true), "→ x · Queued");
    assert_eq!(replying_to("y"), "replying to y");
    for text in [peer_route("x", true), replying_to("y")] {
        assert!(!text.contains("Bot-to-bot") && !text.contains("Bot reply"));
    }
}
