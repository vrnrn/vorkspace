
use super::*;
use chrono::TimeZone;

fn at(day: u32, hour: u32, minute: u32) -> u64 {
    Local
        .with_ymd_and_hms(2026, 10, day, hour, minute, 0)
        .unwrap()
        .timestamp_millis() as u64
}

#[test]
fn stretches_match_react_and_cross_midnight_in_local_time() {
    assert!(!opens_stretch(None, None));
    assert!(opens_stretch(Some(at(5, 9, 0)), None));
    assert!(!opens_stretch(Some(at(5, 10, 0)), Some(at(5, 9, 0))));
    assert!(opens_stretch(Some(at(5, 10, 1)), Some(at(5, 9, 0))));
    assert!(opens_stretch(Some(at(6, 0, 1)), Some(at(5, 23, 59))));
    let today = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
    assert_eq!(day_label(today, today), "Today");
    assert_eq!(day_label(today.pred_opt().unwrap(), today), "Yesterday");
    assert_eq!(
        day_label(NaiveDate::from_ymd_opt(2025, 12, 30).unwrap(), today),
        "Tue, Dec 30, 2025"
    );
    assert_eq!(
        day_label(NaiveDate::from_ymd_opt(2026, 2, 3).unwrap(), today),
        "Tue, Feb 3"
    );
}

#[gpui::test]
fn bot_dates_survive_streaming_and_split_reactions_and_completion_runs(
    cx: &mut gpui::TestAppContext,
) {
    vorkspace_ui::clock::use_test_clock(cx);
    let view = cx.new(|_| {
        let mut view = TranscriptView::new();
        view.bot_mode("cat", 0xe86b5f, "BotCat");
        view
    });
    view.update(cx, |view, cx| {
        let row = |role: &str, text: &str, sent_at| Row {
            role: role.into(),
            text: text.into(),
            first: true,
            sent_at,
        };
        view.patch(
            0,
            vec![
                row("user", "Yesterday", Some(at(4, 23, 55))),
                row("assistant", "<reaction>👍</reaction>", Some(at(4, 23, 56))),
                row("user", "Today", Some(at(5, 0, 5))),
                row("assistant", "Done", Some(at(5, 0, 6))),
                row("bot-system", "Finished", Some(at(5, 0, 10))),
                row("assistant", "Follow-up", Some(at(5, 0, 11))),
                row("bot-system", "A later result", Some(at(5, 4, 0))),
                row("assistant", "Later reply", Some(at(5, 4, 1))),
            ],
            cx,
        );
        assert_eq!(view.bot_stamp(0), Some(at(4, 23, 55)));
        assert_eq!(view.bot_stamp(2), Some(at(5, 0, 5)));
        assert_eq!(view.bot_stamp(3), None, "no duplicate before the reply");
        assert_eq!(
            view.bot_stamp(5),
            None,
            "nearby completion continues the same stretch"
        );
        assert_eq!(view.bot_stamp(7), Some(at(5, 4, 0)));
        assert!(
            !view.stacks_on_next(0),
            "a new day breaks reaction stacking"
        );
        assert_eq!(view.bot_group_start(5), 3);
        assert_eq!(
            view.bot_group_start(7),
            7,
            "a long break starts a new group"
        );
        view.streaming(true, cx);
        view.patch(8, vec![row("user", "Still today", Some(at(5, 4, 10)))], cx);
        assert_eq!(view.bot_stamp(8), None);
        view.patch(9, vec![row("assistant", "Reply", Some(at(5, 4, 11)))], cx);
        view.streaming(false, cx);
        assert_eq!(view.bot_stamp(8), None);
        assert_eq!(view.bot_stamp(0), Some(at(4, 23, 55)));
        view.set_pending(
            vec![PendingMessage {
                id: "tomorrow".into(),
                text: "Tomorrow".into(),
                error: None,
                sent_at: Some(at(6, 0, 5)),
                attachments: Vec::new(),
                peer: None,
            }],
            cx,
        );
        assert!(view.pending_separator(0).is_some());
        view.streaming(true, cx);
        view.patch(10, vec![row("user", "Tomorrow", Some(at(6, 0, 5)))], cx);
        view.set_pending(Vec::new(), cx);
        assert_eq!(
            view.bot_stamp(10),
            Some(at(6, 0, 5)),
            "delivery keeps the queued message's date"
        );
        view.bot = false;
        assert_eq!(view.bot_stamp(0), None, "normal sessions keep their layout");
    });
}

#[gpui::test]
fn bot_renders_date_pills_and_removes_the_session_outline(cx: &mut gpui::TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let window = cx.update(|cx| {
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(gpui::WindowBounds::Windowed(Bounds::new(
                    gpui::point(px(0.), px(0.)),
                    size(px(1100.), px(700.)),
                ))),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|cx| {
                    let mut view = TranscriptView::new();
                    let now = vorkspace_core::time::now_ms();
                    view.patch(
                        0,
                        vec![
                            Row {
                                role: "user".into(),
                                text: "Yesterday's message".into(),
                                first: true,
                                sent_at: Some(now - 86_400_000),
                            },
                            Row {
                                role: "assistant".into(),
                                text: "First reply".into(),
                                first: true,
                                sent_at: None,
                            },
                            Row {
                                role: "user".into(),
                                text: "Today's message".into(),
                                first: true,
                                sent_at: Some(now),
                            },
                            Row {
                                role: "assistant".into(),
                                text: "Second reply".into(),
                                first: true,
                                sent_at: None,
                            },
                        ],
                        cx,
                    );
                    view
                })
            },
        )
        .unwrap()
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("prompt-outline").is_some());
    assert!(visual.debug_bounds("bot-day-separator").is_none());
    window
        .update(visual, |view, _, cx| {
            view.bot_mode("cat", 0xe86b5f, "BotCat");
            view.rebuild(0, cx);
        })
        .unwrap();
    visual.run_until_parked();
    assert!(visual.debug_bounds("prompt-outline").is_none());
    assert!(visual.debug_bounds("bot-day-separator").is_some());
}
