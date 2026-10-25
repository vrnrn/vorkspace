use super::*;
use gpui::TestAppContext;

fn row(role: &str, text: &str) -> Row {
    Row {
        role: role.into(),
        text: text.into(),
        first: true,
        sent_at: Some(1_000),
    }
}

fn tool(id: &str) -> Row {
    row(
        "tool",
        &serde_json::to_string(&ToolCall::new(id, "Read")).unwrap(),
    )
}

// Compare the actual presentation, including nested phases and work rails,
// against the session projection. Bot identity is its only additional row.
fn projection(view: &TranscriptView) -> Vec<(usize, String, bool, bool, Option<usize>)> {
    view.rows
        .iter()
        .filter_map(|row| {
            let body = match &row.body {
                Body::BotHeader { .. } => return None,
                Body::Plain(text) => text.to_string(),
                Body::Markdown(md) => markdown::row_text(md),
                Body::Tool { call, group, tail } => format!("{call:?}/{group:?}/{tail}"),
                Body::Phase {
                    title,
                    group,
                    active,
                    icon,
                } => format!("{title}/{group}/{active}/{icon}"),
                Body::WorkLine { turn, expandable } => format!("work/{turn}/{expandable}"),
                Body::Reasoning => "reasoning".into(),
                Body::Actions { turn, complete } => format!("actions/{turn}/{complete}"),
                _ => panic!("unexpected row in work projection"),
            };
            Some((row.key.0, body, row.work, row.work_tail, row.phase_group()))
        })
        .collect()
}

#[gpui::test]
fn bot_work_matches_sessions_while_streaming_settled_and_reloaded(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let session = cx.new(|_| TranscriptView::new());
    let bot = cx.new(|_| {
        let mut view = TranscriptView::new();
        view.bot_mode("cat", 0xe86b5f, "BotCat");
        view
    });
    for view in [&session, &bot] {
        view.update(cx, |view, cx| {
            view.model = "GPT-6-Luna".into();
            view.streaming(true, cx);
            view.patch(
                0,
                vec![
                    row("user", "Check this"),
                    row("assistant", "Checking the files."),
                ],
                cx,
            );
            view.patch(
                2,
                vec![
                    tool("read-a"),
                    row("reasoning", "Compare the paths"),
                    tool("read-b"),
                ],
                cx,
            );
            assert!(
                view.rows
                    .iter()
                    .any(|r| r.key.0 == 1 && matches!(r.body, Body::Markdown(_)))
            );
            assert_eq!(
                view.rows
                    .iter()
                    .filter(|r| matches!(r.body, Body::Tool { .. }))
                    .count(),
                2
            );
            assert!(view.rows.iter().any(|r| matches!(r.body, Body::Reasoning)));
            assert!(view.rows.iter().any(|r| matches!(
                r.body,
                Body::Phase {
                    group: 2,
                    active: true,
                    ..
                }
            )));
            view.toggle_phase(2, false, cx);
            assert!(!view.rows.iter().any(|r| r.in_phase()));
            view.toggle_phase(2, true, cx);
            view.patch(5, vec![row("assistant", "")], cx);
            assert_eq!(
                view.rows
                    .iter()
                    .filter(|r| matches!(r.body, Body::Tool { .. }))
                    .count(),
                2,
                "an empty answer must not hide the live work"
            );
        });
    }
    assert_eq!(
        bot.read_with(cx, |view, _| projection(view)),
        session.read_with(cx, |view, _| projection(view))
    );
    for view in [&session, &bot] {
        view.update(cx, |view, cx| {
            view.patch(5, vec![row("assistant", "Everything checks out.")], cx);
            assert!(view.streaming, "the receipt is available before completion");
            assert!(view.rows.iter().any(|r| matches!(
                r.body,
                Body::WorkLine {
                    turn: 1,
                    expandable: true
                }
            )));
            assert!(
                view.rows
                    .iter()
                    .any(|r| r.key.0 == 5 && matches!(r.body, Body::Markdown(_)))
            );
            assert!(
                !view
                    .rows
                    .iter()
                    .any(|r| matches!(r.body, Body::Tool { .. }))
            );
            view.work_folds.insert(1, true);
            view.rebuild(1, cx);
            assert!(view.rows.iter().any(|r| r.key.0 == 1 && r.work));
            assert_eq!(
                view.rows
                    .iter()
                    .filter(|r| matches!(r.body, Body::Tool { .. }))
                    .count(),
                2
            );
            assert!(
                view.rows
                    .iter()
                    .any(|r| matches!(r.body, Body::Reasoning) && r.in_phase())
            );
            view.streaming(false, cx);
            assert_eq!(view.work_folds.get(&1), Some(&true));
            assert_eq!(view.phases.get(&2), Some(&true));
            assert!(
                !view.list.is_following_tail(),
                "incoming patches honor a disclosure's reading position"
            );
        });
    }
    assert_eq!(
        bot.read_with(cx, |view, _| projection(view)),
        session.read_with(cx, |view, _| projection(view))
    );
    let (source, turns) = bot.read_with(cx, |view, _| (view.source.clone(), view.turns.clone()));
    let reloaded = cx.new(|cx| {
        let mut view = TranscriptView::new();
        view.bot_mode("cat", 0xe86b5f, "BotCat");
        view.turns = turns;
        view.install_prepared_history(PreparedHistory::new(source.clone(), true), cx);
        assert_eq!(view.source, source);
        assert!(view.rows.iter().any(|r| matches!(r.body, Body::WorkLine { expandable: true, .. })));
        assert!(!view.rows.iter().any(|r| matches!(r.body, Body::Tool { .. })));
        assert_eq!(view.turns[&1].model, "GPT-6-Luna");
        view.work_folds.insert(1, true);
        view.phases.insert(2, true);
        view.expanded.insert(3);
        view.rebuild(1, cx);
        assert!(view.rows.iter().any(|r| r.key.0 == 3 && matches!(&r.body, Body::Markdown(md) if markdown::row_text(md) == "Compare the paths")));
        view.expanded.remove(&3);
        view.rebuild(3, cx);
        view
    });
    assert_eq!(
        reloaded.read_with(cx, |view, _| projection(view)),
        bot.read_with(cx, |view, _| projection(view))
    );
}

#[gpui::test]
fn bot_inline_disclosures_preserve_reaction_final_reply_and_saved_work(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let source = vec![
        row("user", "Write the report"),
        row("assistant", "<reaction>👍</reaction>"),
        row(
            "bot-document",
            r#"{"type":"document","documentId":"report","title":"Report","kind":"html"}"#,
        ),
        row("assistant", "Checking the report."),
        tool("read"),
        row("assistant", "Written and checked."),
        row(
            "bot-launch",
            r#"{"sessionId":7,"title":"Review","cwd":"/repo","harness":"codex"}"#,
        ),
    ];
    let view = cx.new(|_| TranscriptView::new());
    view.update(cx, |view, cx| {
        view.bot_mode("cat", 0xe86b5f, "BotCat");
        view.install_prepared_history(PreparedHistory::new(source.clone(), true), cx);
        for open in [false, true, false] {
            view.work_folds.insert(1, open);
            view.rebuild(1, cx);
            assert!(
                view.rows.iter().any(|r| matches!(
                    r.body,
                    Body::WorkLine {
                        turn: 1,
                        expandable: true
                    }
                )),
                "leading reactions and saved references must not swallow the receipt"
            );
            assert_eq!(
                view.rows
                    .iter()
                    .filter(|r| matches!(r.body, Body::Tool { .. }))
                    .count(),
                usize::from(open)
            );
            let answer = view
                .rows
                .iter()
                .position(|r| r.key.0 == 5 && matches!(r.body, Body::Markdown(_)))
                .unwrap();
            let document = view
                .rows
                .iter()
                .position(|r| matches!(r.body, Body::BotCard(_)))
                .unwrap();
            let launch = view
                .rows
                .iter()
                .position(|r| matches!(r.body, Body::Launch(7)))
                .unwrap();
            assert!(answer < document && document < launch);
            assert_eq!(
                view.rows
                    .iter()
                    .filter(|r| matches!(r.body, Body::BotCard(_)))
                    .count(),
                1
            );
            assert_eq!(
                view.rows
                    .iter()
                    .filter(|r| matches!(r.body, Body::Launch(7)))
                    .count(),
                1
            );
            assert!(
                view.rows
                    .iter()
                    .filter(|r| matches!(r.body, Body::BotCard(_) | Body::Launch(_)))
                    .all(|r| !r.work)
            );
            assert_eq!(view.reactions[&0].0, "👍");
            assert!(
                view.rows.iter().filter(|r| r.key.0 == 0).all(|r| !r.work),
                "the user's prompt stays outside the work trail"
            );
            assert_eq!(view.source, source);
        }
    });
}

#[gpui::test]
fn bot_continuation_search_and_pinned_reports_use_their_own_run(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let mut failed = ToolCall::new("agent", "Agent");
    failed.state = ToolState::Failed;
    failed.panel = Some(Panel::Agent(Default::default()));
    failed.output = "Review failed".into();
    let view = cx.new(|_| TranscriptView::new());
    view.update(cx, |view, cx| {
        view.bot_mode("cat", 0xe86b5f, "BotCat");
        view.patch(
            0,
            vec![
                row("user", "Start the review"),
                row("assistant", "Review started."),
                row("bot-system", "Review finished"),
                row("assistant", "Checking the report."),
                tool("read-report"),
                row("tool", &serde_json::to_string(&failed).unwrap()),
                row("assistant", "The review needs another attempt."),
            ],
            cx,
        );
        assert_eq!(view.bot_group_start(3), 1);
        let pinned = view
            .rows
            .iter()
            .position(|r| r.key.0 == 5 && matches!(r.body, Body::Tool { .. }))
            .unwrap();
        let answer = view
            .rows
            .iter()
            .position(|r| r.key.0 == 6 && matches!(r.body, Body::Markdown(_)))
            .unwrap();
        let receipt = view
            .rows
            .iter()
            .position(|r| matches!(r.body, Body::WorkLine { turn: 3, .. }))
            .unwrap();
        assert!(
            receipt < pinned && pinned < answer,
            "a failed delegated report stays by its continuation's answer"
        );
        assert!(!view.rows[pinned].work);
        assert!(!view.rows.iter().any(|r| r.key.0 == 4));
        view.reveal_search(4, "Read", cx);
        assert_eq!(view.work_folds.get(&3), Some(&true));
        assert!(
            view.rows
                .iter()
                .any(|r| r.key.0 == 4 && matches!(r.body, Body::Tool { .. }))
        );
        assert!(view.rows.iter().any(|r| r.key.0 == 3 && r.work));
        assert_eq!(
            view.rows
                .iter()
                .filter(|r| matches!(r.body, Body::BotHeader { .. }))
                .count(),
            1
        );
        assert!(!view.list.is_following_tail());
    });
}
