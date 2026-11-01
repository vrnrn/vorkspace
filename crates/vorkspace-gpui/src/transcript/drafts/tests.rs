
use super::*;
use gpui::TestAppContext;

#[gpui::test]
fn new_drafts_anchor_like_sent_prompts_and_keep_the_anchor_through_updates_and_resize(
    cx: &mut TestAppContext,
) {
    for history in [false, true] {
        let window = cx.update(|cx| {
            cx.open_window(
                gpui::WindowOptions {
                    window_bounds: Some(gpui::WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(900.), px(600.)),
                    ))),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|cx| {
                        let mut view = TranscriptView::new();
                        if history {
                            let mut transcript = vorkspace_core::transcript::Transcript::default();
                            transcript.begin("user");
                            transcript.append("An earlier question");
                            transcript.begin("assistant");
                            transcript.append("An earlier answer");
                            view.patch(0, transcript.rows, cx);
                        }
                        view
                    })
                },
            )
            .unwrap()
        });
        let cx = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        cx.refresh().unwrap();
        window
            .update(cx, |view, _, cx| {
                view.introduce_saved_draft(
                    Draft {
                        id: "new".into(),
                        text: "Save this for later".into(),
                        at: view.source.len(),
                        ..Default::default()
                    },
                    cx,
                );
                assert!(
                    !view.streaming,
                    "saving a draft must not start a provider turn"
                );
                assert_eq!(view.prompt_rise.map(|(at, _)| at), Some(SAVED_DRAFT_START));
                // Measure the final position independently of animation timing.
                view.prompt_rise = None;
            })
            .unwrap();
        cx.refresh().unwrap();
        cx.run_until_parked();
        let assert_anchor = |view: &TranscriptView| {
            let ix = view
                .rows
                .iter()
                .position(|row| matches!(row.body, Body::SavedDraft))
                .unwrap();
            let bounds = view.list.bounds_for_item(ix).unwrap();
            assert!(
                (bounds.top() - view.viewport.top()).abs() < px(1.),
                "a newly saved draft must sit at the same top anchor as a sent prompt: {bounds:?} {:?}",
                view.viewport
            );
            assert!(view.list.is_following_tail());
        };
        window
            .update(cx, |view, window, cx| {
                assert_anchor(view);
                let mut draft = view.saved_draft.clone().unwrap();
                draft.attachments = vec!["/saved/attachment.txt".into()];
                view.set_saved_draft(Some(draft), cx);
                window.resize(size(px(700.), px(760.)));
                cx.notify();
            })
            .unwrap();
        cx.refresh().unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                assert_anchor(view);
                view.visible(false, cx);
                view.visible(true, cx);
                assert_eq!(view.anchor_turn, Some(SAVED_DRAFT_START));
                view.set_saved_draft(None, cx);
                assert_ne!(view.anchor_turn, Some(SAVED_DRAFT_START));
                assert_eq!(view.source.len(), if history { 2 } else { 0 });
            })
            .unwrap();
    }
}

#[gpui::test]
fn restored_drafts_do_not_replay_submission_or_override_the_anchor_preference(
    cx: &mut TestAppContext,
) {
    let view = cx.new(|_| TranscriptView::new());
    view.update(cx, |view, cx| {
        view.set_saved_draft(
            Some(Draft {
                id: "restored".into(),
                text: "already saved".into(),
                ..Default::default()
            }),
            cx,
        );
        assert!(view.anchor_turn.is_none());
        assert!(view.prompt_rise.is_none());
        view.set_saved_draft(None, cx);
        view.apply_preferences(
            &crate::settings::Preferences {
                anchor_prompts: false,
                ..Default::default()
            },
            cx,
        );
        view.introduce_saved_draft(
            Draft {
                id: "new".into(),
                text: "saved without anchoring".into(),
                ..Default::default()
            },
            cx,
        );
        assert!(view.anchor_turn.is_none());
        assert!(view.prompt_rise.is_none());
    });
}

#[gpui::test]
fn saved_draft_keeps_its_position_as_history_grows_and_removal_leaves_history_intact(
    cx: &mut TestAppContext,
) {
    let view = cx.new(|_| TranscriptView::new());
    let row = |role: &str, text: &str| Row {
        role: role.into(),
        text: text.into(),
        first: true,
        sent_at: None,
    };
    view.update(cx, |view, cx| {
        view.patch(
            0,
            vec![row("user", "first"), row("assistant", "answer")],
            cx,
        );
        view.set_saved_draft(
            Some(Draft {
                id: "draft".into(),
                text: "for later".into(),
                at: 2,
                ..Default::default()
            }),
            cx,
        );
        view.patch(
            2,
            vec![
                row("user", "another message"),
                row("assistant", "another answer"),
            ],
            cx,
        );
        let draft = view
            .rows
            .iter()
            .position(|r| matches!(r.body, Body::SavedDraft))
            .unwrap();
        let next = view.rows.iter().position(|r| r.key.0 == 2).unwrap();
        assert!(draft < next);
        assert_eq!(
            view.rows
                .iter()
                .filter(|r| matches!(r.body, Body::SavedDraft))
                .count(),
            1
        );
        let history = view.source.clone();
        view.set_saved_draft(None, cx);
        assert!(
            view.rows
                .iter()
                .all(|r| !matches!(r.body, Body::SavedDraft))
        );
        assert_eq!(view.source, history);
    });
}
