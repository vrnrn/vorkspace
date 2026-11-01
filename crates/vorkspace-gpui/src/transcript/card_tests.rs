use super::*;
use gpui::{TestAppContext, VisualTestContext, point};
use serde_json::{Value, json};

fn row(role: &str, text: impl ToString) -> Row {
    Row {
        role: role.into(),
        text: text.to_string(),
        first: true,
        sent_at: Some(1_000),
    }
}
fn start(id: Value) -> Row {
    let request = format!(
        "start-{}",
        id.as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| id.to_string())
    );
    let mut call = ToolCall::new(&request, "Bash");
    call.input = json!({"command":format!("vorkspace app sessions.start --json {{}} --request-id {request}")}).to_string();
    call.output = json!({"ok":true,"result":{"sessionId":id,"accepted":true},"requestId":request})
        .to_string();
    call.state = ToolState::Completed;
    row("tool", serde_json::to_string(&call).unwrap())
}
fn explicit(id: Value, card: &str) -> Row {
    row(
        "bot-card",
        json!({"id":card,"type":"session","sessionId":id,"title":"Same title","note":"Keep the review instructions","future":{"keep":true}}),
    )
}
fn visible(view: &TranscriptView) -> Vec<((usize, usize), Value)> {
    view.rows
        .iter()
        .filter_map(|r| match &r.body {
            Body::Launch(id) => Some((r.key, json!({"type":"session","sessionId":id}))),
            Body::BotCard(value) => Some((r.key, value.clone())),
            _ => None,
        })
        .collect()
}
fn fixture(cx: &mut TestAppContext, bot: bool) -> gpui::Entity<TranscriptView> {
    vorkspace_ui::clock::use_test_clock(cx);
    cx.new(|_| {
        let mut view = TranscriptView::new();
        if bot {
            view.bot_mode("cat", 0xe86b5f, "BotCat");
        }
        view
    })
}

#[gpui::test]
fn cards_screenshot_two_starts_and_explicit_links_survive_stream_settle_patch_and_reopen(
    cx: &mut TestAppContext,
) {
    let view = fixture(cx, true);
    let initial = vec![
        row("user", "Delegate both fixes"),
        start(json!(7)),
        start(json!(8)),
        row("assistant", "Delegated both fixes."),
    ];
    view.update(cx, |view, cx| {
        view.streaming(true, cx);
        view.patch(0, initial.clone(), cx);
        assert_eq!(
            visible(view).len(),
            2,
            "accepted starts are visible before explicit links"
        );
    });
    let keys = view.read_with(cx, |view, _| {
        visible(view).iter().map(|v| v.0).collect::<Vec<_>>()
    });
    view.update(cx, |view, cx| {
        view.patch(
            4,
            vec![explicit(json!(7), "first"), explicit(json!(8), "second")],
            cx,
        );
        assert_eq!(
            visible(view).len(),
            2,
            "the exact screenshot path has two cards, never four"
        );
        assert_eq!(visible(view).iter().map(|v| v.0).collect::<Vec<_>>(), keys);
        assert_eq!(visible(view)[0].1["id"], "first");
        assert_eq!(visible(view)[0].1["future"]["keep"], true);
        view.streaming(false, cx);
        view.rebuild(5, cx);
        assert_eq!(visible(view).len(), 2);
        view.patch(5, vec![explicit(json!(8), "second-patch")], cx);
        assert_eq!(visible(view).len(), 2);
    });
    TranscriptView::settle_animation_frames(&view, cx);
    let source = view.read_with(cx, |view, _| view.source.clone());
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("history.db");
    let id = {
        let mut store = vorkspace_core::store::Store::open(&path).unwrap();
        let session = store.create("codex", "/repo").unwrap();
        store.write_rows(session.id, 0, &source).unwrap();
        session.id
    };
    let store = vorkspace_core::store::Store::open(&path).unwrap();
    let saved = store.load(id).unwrap().rows;
    assert_eq!(
        saved, source,
        "both durable representations and unknown fields remain intact"
    );
    let reopened = fixture(cx, true);
    reopened.update(cx, |view, cx| {
        view.install_prepared_history(PreparedHistory::new(saved.clone(), true), cx)
    });
    assert_eq!(
        reopened.read_with(cx, |view, _| visible(view)),
        view.read_with(cx, |view, _| visible(view))
    );
    assert_eq!(
        reopened.read_with(cx, |view, _| view.source.clone()),
        source
    );
}

#[gpui::test]
fn cards_reverse_order_replay_group_scope_and_invalid_identity(cx: &mut TestAppContext) {
    for bot in [false, true] {
        let view = fixture(cx, bot);
        view.update(cx, |view, cx| {
            view.patch(
                0,
                vec![
                    row("user", "Delegate"),
                    explicit(json!(7), "a"),
                    explicit(json!(7), "retry"),
                ],
                cx,
            );
            view.patch(
                3,
                vec![
                    start(json!(7)),
                    start(json!(7)),
                    explicit(json!(8), "b"),
                    row("assistant", "Started"),
                ],
                cx,
            );
            assert_eq!(visible(view).len(), 2);
            assert_eq!(
                visible(view)[0].1["id"],
                "a",
                "stable explicit card identity across new arbitrary retry IDs"
            );
            if bot {
                view.patch(
                    7,
                    vec![
                        row("bot-system", "Session finished"),
                        explicit(json!(7), "completion"),
                        row("assistant", "Reviewed"),
                    ],
                    cx,
                );
                assert_eq!(
                    visible(view).len(),
                    2,
                    "completion runs belong to the same response group"
                );
            }
            let end = view.source.len();
            view.patch(
                end,
                vec![
                    row("user", "Link it again"),
                    explicit(json!(7), "later"),
                    explicit(json!(null), "missing-a"),
                    explicit(json!(null), "missing-b"),
                    explicit(json!(""), "empty"),
                    row("bot-card", json!({"type":"choices","options":["A"]})),
                    row("bot-card", json!({"type":"choices","options":["A"]})),
                ],
                cx,
            );
            assert_eq!(
                visible(view).len(),
                8,
                "later intent, malformed references and choices must remain separate"
            );
        });
    }
    let view = fixture(cx, true);
    let desktop = json!("app-child");
    let handle = vorkspace_core::bot::work::session_handle(&desktop).unwrap();
    view.update(cx, |view, cx| {
        view.patch(
            0,
            vec![
                row("user", "Delegate"),
                start(desktop.clone()),
                row("bot-launch", json!({"sessionId":handle})),
                explicit(desktop, "serialized"),
                explicit(json!(handle), "projected"),
            ],
            cx,
        );
        assert_eq!(visible(view).len(), 1);
        assert_eq!(visible(view)[0].1["sessionId"], handle);
    });
    // A numeric-looking desktop ID is still an external ID, not native 7.
    let view = fixture(cx, true);
    let desktop = json!("7");
    let handle = vorkspace_core::bot::work::session_handle(&desktop).unwrap();
    view.update(cx, |view, cx| {
        view.patch(
            0,
            vec![
                row("user", "Link both"),
                start(desktop.clone()),
                explicit(json!(handle), "external"),
                explicit(json!(7), "native"),
            ],
            cx,
        );
        assert_eq!(visible(view).len(), 2);
    });
}

#[gpui::test]
fn cards_peer_prompts_notes_and_habits_are_not_weak_identity_matches(cx: &mut TestAppContext) {
    let view = fixture(cx, true);
    view.update(cx, |view, cx| {
        view.patch(0, vec![row("user", "Link"), explicit(json!(7), "first"), row("assistant", "Linked"), row("bot-system", "<bot_peer_message>\nFrom: {\"id\":\"peer\",\"name\":\"Peer\",\"hop\":1}\nContext\n</bot_peer_message>\n\nPlease link it"), explicit(json!(7), "peer")], cx);
        assert_eq!(visible(view).len(), 2, "a peer prompt opens an independent response");
        view.patch(5, vec![row("bot-note", json!({"type":"document","documentId":"d","note":true})), row("bot-card", json!({"type":"document","documentId":"d"})), row("bot-document", json!({"type":"document"})), row("bot-document", json!({"type":"document"})), row("bot-card", json!({"type":"habit","name":"Same"})), row("bot-card", json!({"type":"habit","name":"Same"}))], cx);
        assert_eq!(visible(view).len(), 8, "notes, malformed projections and habit choices stay distinct");
    });
}

#[gpui::test]
fn cards_replaced_source_anchor_does_not_leave_an_outgoing_duplicate(cx: &mut TestAppContext) {
    let view = fixture(cx, true);
    view.update(cx, |view, cx| {
        view.streaming(true, cx);
        view.patch(
            0,
            vec![
                row("user", "Link"),
                explicit(json!(7), "a"),
                explicit(json!(7), "replay"),
            ],
            cx,
        );
        assert_eq!(visible(view).len(), 1);
        let old_key = visible(view)[0].0;
        view.patch(
            1,
            vec![row("assistant", "Linked"), explicit(json!(7), "replay")],
            cx,
        );
        assert_eq!(
            visible(view).len(),
            1,
            "no duplicate even before the animation clock advances"
        );
        assert_eq!(
            visible(view)[0].0,
            old_key,
            "the resource retains its scroll anchor across source replacement"
        );
        assert!(!view.motions.contains_key(&old_key));
    });
}

#[gpui::test]
fn cards_documents_merge_latest_fields_in_both_orders_without_title_dedup(cx: &mut TestAppContext) {
    for reverse in [false, true] {
        let view = fixture(cx, true);
        let projected = row(
            "bot-document",
            json!({"id":"artifact:d","type":"document","documentId":"d","title":"Same title","kind":"html","summary":"initial"}),
        );
        let explicit = row(
            "bot-card",
            json!({"id":"explicit","type":"document","documentId":"d","title":"Same title","note":"Keep this","action":{"keep":true}}),
        );
        let mut rows = vec![row("user", "Write"), row("assistant", "Written")];
        rows.extend(if reverse {
            vec![explicit, projected]
        } else {
            vec![projected, explicit]
        });
        view.update(cx, |view, cx| {
            view.patch(0, rows, cx);
            let key = visible(view)[0].0;
            assert_eq!(visible(view).len(), 1);
            view.patch(4, vec![row("bot-document", json!({"type":"document","documentId":"d","title":"Latest title","summary":"latest"})), row("bot-document", json!({"type":"document","documentId":"other","title":"Latest title"}))], cx);
            let cards = visible(view);
            assert_eq!(cards.len(), 2);
            assert_eq!(cards[0].0, key);
            assert_eq!(cards[0].1["id"], "explicit");
            assert_eq!(cards[0].1["title"], "Latest title");
            assert_eq!(cards[0].1["summary"], "latest");
            assert_eq!(cards[0].1["kind"], "html");
            assert_eq!(cards[0].1["note"], "Keep this");
            assert_eq!(cards[0].1["action"]["keep"], true);
            view.patch(6, vec![row("bot-document", json!({"type":"document","documentId":"d","summary":null}))], cx);
            assert_eq!(visible(view)[0].1["summary"], "latest", "an omitted projected field must not erase richer metadata");
            let cards = visible(view);
            view.rebuild(5, cx);
            assert_eq!(visible(view), cards);
        });
    }
}

#[gpui::test]
fn cards_many_resources_have_distinct_stable_keys(cx: &mut TestAppContext) {
    let view = fixture(cx, true);
    view.update(cx, |view, cx| {
        let mut rows = vec![row("user", "Delegate"), row("assistant", "Done")];
        for id in 1..=150 {
            rows.push(start(json!(id)));
            rows.push(row(
                "bot-document",
                json!({"type":"document","documentId":format!("d-{id}"),"title":"Same title"}),
            ));
        }
        view.patch(0, rows, cx);
        assert_eq!(visible(view).len(), 300);
        let keys: HashSet<_> = visible(view).iter().map(|r| r.0).collect();
        assert_eq!(keys.len(), 300);
        let before = visible(view);
        view.rebuild(300, cx);
        assert_eq!(visible(view), before);
    });
}

#[gpui::test]
fn cards_shared_history_projection_and_save_preserve_duplicate_source_blocks(
    cx: &mut TestAppContext,
) {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("desktop.db");
    let mut store = vorkspace_core::store::Store::open_vorkspace(&path).unwrap();
    let session = store.create("codex", "/repo").unwrap();
    let blocks = json!([
        {"id":"prompt","role":"user","text":"Delegate and write","future":{"keep":1}},
        {"id":"launch","role":"assistant","text":"Starting","botSpawnedSessions":[{"sessionId":"app-child","title":"Review","cwd":"/repo","model":"gpt-6","harness":"codex","future":2}],"artifactCards":[{"id":"report","kind":"html","title":"Report","summary":"initial","future":3}]},
        {"id":"session-card","role":"assistant","text":"","botCard":{"type":"session","sessionId":"app-child","title":"Review","note":"Keep instructions","future":4},"future":5},
        {"id":"document-card","role":"assistant","text":"","botCard":{"type":"document","documentId":"report","title":"Report","summary":"latest","future":6}},
        {"id":"reply","role":"assistant","text":"Done","future":7}
    ]);
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute("UPDATE sessions SET blocks_json=?1", [blocks.to_string()])
        .unwrap();
    let source = store.load(session.id).unwrap().rows;
    let view = fixture(cx, true);
    view.update(cx, |view, cx| {
        view.install_prepared_history(PreparedHistory::new(source.clone(), true), cx);
        assert_eq!(visible(view).len(), 2);
        assert_eq!(view.source, source);
        let document = visible(view)
            .into_iter()
            .find(|(_, v)| v["type"] == "document")
            .unwrap()
            .1;
        assert_eq!(document["summary"], "latest");
        assert_eq!(document["kind"], "html");
        assert_eq!(document["future"], 6);
    });
    // Saving unrelated prose uses the original source rows, including both
    // representations. Presentation reconciliation is not a durable migration.
    let mut updated = source.clone();
    updated.last_mut().unwrap().text = "Done and reviewed".into();
    store.write_rows(session.id, 0, &updated).unwrap();
    drop(store);
    let store = vorkspace_core::store::Store::open_vorkspace(&path).unwrap();
    let reopened = store.load(session.id).unwrap().rows;
    assert_eq!(reopened, updated);
    // Native save normalizes history into ordinary_blocks; blocks_json is
    // only a legacy cache and is intentionally emptied by that owner.
    let mut query = conn
        .prepare("SELECT block_json FROM ordinary_blocks ORDER BY position")
        .unwrap();
    let saved = query
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| serde_json::from_str::<Value>(&r.unwrap()).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(saved.len(), 5);
    let mut expected = blocks;
    expected[4]["text"] = json!("Done and reviewed");
    // The adapter may update native envelope fields on edited prose, while
    // every original card-bearing block must remain byte-for-value identical.
    for index in 0..4 {
        assert_eq!(saved[index], expected[index]);
    }
    assert_eq!(saved[4]["future"], 7);
    assert_eq!(saved[4]["text"], "Done and reviewed");
    view.update(cx, |view, cx| {
        view.install_prepared_history(PreparedHistory::new(reopened, true), cx);
        assert_eq!(visible(view).len(), 2);
    });
}

#[gpui::test]
fn cards_screenshot_materialization_renders_two_pills_and_routes_clicks(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let window = cx
        .update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| {
                    let mut view = TranscriptView::new();
                    view.bot_mode("cat", 0xe86b5f, "BotCat");
                    view.patch(
                        0,
                        vec![
                            row("user", "Delegate both fixes"),
                            start(json!(7)),
                            start(json!(8)),
                            row("assistant", "Delegated both fixes"),
                            explicit(json!(7), "foreground"),
                            explicit(json!(8), "hang"),
                        ],
                        cx,
                    );
                    view.set_launches(
                        [7, 8]
                            .map(|id| {
                                (
                                    id,
                                    LaunchView {
                                        title: if id == 7 {
                                            "Remove foreground Bot-card writes"
                                        } else {
                                            "Capture local foreground hangs"
                                        }
                                        .into(),
                                        state: "done",
                                        harness: "codex".into(),
                                        cwd: "/repo".into(),
                                    },
                                )
                            })
                            .into_iter()
                            .collect(),
                        cx,
                    );
                    view
                })
            })
        })
        .unwrap();
    let clicks = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let events = clicks.clone();
    let _subscription = window
        .update(cx, |_, _, cx| {
            cx.subscribe(
                &cx.entity(),
                move |_, _, event: &crate::bots::cards::CardAction, _| {
                    events.borrow_mut().push(event.0.clone())
                },
            )
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(size(px(900.), px(900.)));
    vorkspace_ui::clock::settle_animations(window, visual);
    visual.run_until_parked();
    let rebuilds = window
        .update(visual, |view, _, _| view.rebuild_passes)
        .unwrap();
    visual.refresh().unwrap();
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            assert_eq!(
                view.rebuild_passes, rebuilds,
                "painting unchanged cards must not reparse history"
            )
        })
        .unwrap();
    let a = visual.debug_bounds("bot-launch-card-7").unwrap();
    let b = visual.debug_bounds("bot-launch-card-8").unwrap();
    assert_eq!(a.size.height, px(40.));
    assert_eq!(b.size.height, px(40.));
    assert!(a.bottom() <= b.top());
    window
        .update(visual, |view, _, _| assert_eq!(visible(view).len(), 2))
        .unwrap();
    assert!(
        a.top() >= px(0.) && b.bottom() <= px(900.),
        "both pills are inside the viewport: {a:?}, {b:?}"
    );
    visual.simulate_mouse_move(a.center(), None, Default::default());
    visual.run_until_parked();
    visual.simulate_click(a.center(), Default::default());
    visual.run_until_parked();
    let b = visual.debug_bounds("bot-launch-card-8").unwrap();
    visual.simulate_mouse_move(b.center(), None, Default::default());
    visual.run_until_parked();
    visual.simulate_click(b.center(), Default::default());
    visual.run_until_parked();
    assert_eq!(
        clicks
            .borrow()
            .iter()
            .map(|v| v["sessionId"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![7, 8]
    );
    // A stable-selector geometry check and actual click prove the rows reach
    // the user-facing renderer and existing action handlers.
    visual.simulate_mouse_move(point(px(0.), px(0.)), None, Default::default());
}
