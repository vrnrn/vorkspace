use super::*;
use gpui::{TestAppContext, WindowBounds, WindowOptions, point};

fn row(role: &str, text: &str) -> Row {
    Row {
        role: role.into(),
        text: text.into(),
        first: true,
        sent_at: None,
    }
}
fn settle(view: &mut TranscriptView, cx: &mut Context<TranscriptView>) {
    for reveal in view.reveals.values_mut() {
        reveal.settle(Instant::now());
    }
    view.prompt_rise = None;
    view.motions.clear();
    view.rebuild(0, cx);
}

#[gpui::test]
fn elapsed_clock_has_one_owner_across_restarted_turns_and_hidden_tabs(cx: &mut TestAppContext) {
    let view = cx.new(|_| TranscriptView::new());
    let notifications = std::rc::Rc::new(std::cell::Cell::new(0));
    let _observe = cx.update(|cx| {
        let count = notifications.clone();
        cx.observe(&view, move |_, _| count.set(count.get() + 1))
    });
    // Each previous clock has started sleeping before its turn finishes.
    // None may survive to wake the final replacement turn.
    for _ in 0..20 {
        view.update(cx, |view, cx| view.streaming(true, cx));
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(10));
        view.update(cx, |view, cx| view.streaming(false, cx));
    }
    view.update(cx, |view, cx| view.streaming(true, cx));
    cx.run_until_parked();
    notifications.set(0);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(
        notifications.get(),
        1,
        "one elapsed wakeup per active second"
    );

    let started = view.read_with(cx, |view, _| view.started);
    view.update(cx, |view, cx| view.visible(false, cx));
    cx.run_until_parked();
    notifications.set(0);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(5));
    cx.run_until_parked();
    assert_eq!(notifications.get(), 0, "hidden transcripts need no clock");
    view.update(cx, |view, cx| {
        assert!(view.streaming_clock.is_none());
        view.visible(true, cx);
        assert_eq!(
            view.started, started,
            "showing retains the authoritative start"
        );
    });
    cx.run_until_parked();
    assert_eq!(
        notifications.get(),
        1,
        "showing immediately refreshes elapsed time"
    );
    notifications.set(0);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(notifications.get(), 1);

    view.update(cx, |view, cx| view.streaming(false, cx));
    cx.run_until_parked();
    notifications.set(0);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(2));
    cx.run_until_parked();
    assert_eq!(
        notifications.get(),
        0,
        "completed turns release their clock"
    );
}

#[gpui::test]
fn settled_tab_switches_preserve_rows_layout_and_scroll(cx: &mut TestAppContext) {
    let view = cx.new(|_| TranscriptView::new());
    view.update(cx, |view, cx| {
        view.patch(
            0,
            vec![
                row("user", "Question"),
                row("assistant", "A settled **answer**."),
            ],
            cx,
        );
        settle(view, cx);
        view.list.pause_following_tail();
        view.list.scroll_to(ListOffset {
            item_ix: 1,
            offset_in_item: px(7.),
        });
        let position = view.list.logical_scroll_top();
        let rows = view.rows.as_ptr();
        let rebuilds = view.rebuild_passes;
        view.phase_layouts.insert(
            999,
            PhaseLayout {
                width: px(600.),
                height: 42.,
                rows: HashMap::new(),
            },
        );
        for _ in 0..20 {
            view.visible(false, cx);
            view.animate_frame(cx);
            view.visible(true, cx);
        }
        assert_eq!(view.rebuild_passes, rebuilds);
        assert_eq!(view.rows.as_ptr(), rows);
        assert!(view.phase_layouts.contains_key(&999));
        let restored = view.list.logical_scroll_top();
        assert_eq!(restored.item_ix, position.item_ix);
        assert_eq!(restored.offset_in_item, position.offset_in_item);
        assert!(!view.list.is_following_tail());
    });
}

#[gpui::test]
fn hiding_settles_live_text_and_showing_reuses_hidden_updates(cx: &mut TestAppContext) {
    let view = cx.new(|_| TranscriptView::new());
    view.update(cx, |view, cx| {
        view.streaming = true;
        view.live_start = 1;
        view.patch(
            0,
            vec![
                row("user", "Question"),
                row("assistant", "The complete live answer has arrived."),
            ],
            cx,
        );
        // Reproduce a response still catching up to its received text.
        view.reveals.insert(
            1,
            PacedText::new(
                "The complete live answer has arrived.",
                true,
                Instant::now(),
            ),
        );
        assert!(view.reveals[&1].behind());
        let before = view.rebuild_passes;
        view.visible(false, cx);
        assert_eq!(view.rebuild_passes, before + 1);
        assert!(!view.reveals[&1].behind());
        view.patch(1, vec![row("assistant", "Updated while hidden.")], cx);
        let updated = view.rebuild_passes;
        view.visible(true, cx);
        assert_eq!(view.rebuild_passes, updated);
        assert!(
            view.rows
                .iter()
                .any(|row| matches!(&row.body, Body::Markdown(md)
                if crate::markdown::selection_text(md).contains("Updated while hidden.")))
        );
    });
}

#[test]
fn saved_work_line_uses_turn_provenance_and_never_invents_legacy_model_or_duration() {
    assert_eq!(
        work_title(Some("Saved Model"), Some(9500), false),
        "Saved Model worked for 10s"
    );
    assert_eq!(work_title(None, Some(60000), false), "Worked for 1m");
    assert_eq!(work_title(None, None, false), "Worked");
    assert_eq!(work_title(Some(""), None, false), "Worked");
    assert_eq!(
        work_title(Some("Saved Model"), Some(0), false),
        "Saved Model worked for 1s"
    );
    assert_eq!(
        work_title(Some("Live Model"), None, true),
        "Live Model working…"
    );
}

#[gpui::test]
fn fresh_follow_ups_keep_provider_metrics_and_timing_on_the_original_turn(cx: &mut TestAppContext) {
    let view = cx.new(|_| TranscriptView::new());
    view.update(cx, |view, cx| {
        view.identity("codex", "Chosen Model");
        view.streaming(true, cx);
        view.patch(
            0,
            vec![row("user", "Prompt"), row("assistant", "Working")],
            cx,
        );
        view.record_provider_turn("provider-turn".into());
        view.patch(
            2,
            vec![row("user", "Also this"), row("assistant", "Done")],
            cx,
        );
        view.metrics(vorkspace_core::agent::TurnMetrics {
            output_tokens: 20,
            ..Default::default()
        });
        view.streaming(false, cx);
        assert_eq!(view.turns[&1].model, "Chosen Model");
        assert_eq!(
            view.turns[&1].provider_turn_id.as_deref(),
            Some("provider-turn")
        );
        assert_eq!(view.turns[&1].metrics.unwrap().output_tokens, 20);
        assert!(view.turns[&1].has_duration());
        assert!(view.turns[&1].completed_at.is_some());
        assert_eq!(view.turns[&3].model, "Chosen Model");
        assert!(
            !view.turns[&3].has_duration(),
            "a steer has provenance but no separate run duration"
        );
        assert!(view.turns[&3].metrics.is_none());
    });
}

#[test]
fn phase_reveals_one_clipped_body_and_keeps_visible_steps_at_their_natural_height() {
    let layout = PhaseLayout {
        width: px(600.),
        height: 100.,
        rows: HashMap::from([
            ((1, 2), (0., 20.)),
            ((2, 2), (20., 50.)),
            ((3, 2), (70., 30.)),
        ]),
    };
    assert_eq!(layout.row_progress((1, 2), 0.5), 1.);
    assert_eq!(layout.row_progress((2, 2), 0.5), 0.6);
    assert_eq!(layout.row_progress((3, 2), 0.5), 0.);
    for progress in [0., 0.2, 0.5, 0.9, 1.] {
        let visible: f32 = layout
            .rows
            .iter()
            .map(|(key, (_, height))| height * layout.row_progress(*key, progress))
            .sum();
        assert!((visible - layout.height * progress).abs() < 0.001);
    }
}

#[gpui::test]
fn clicking_a_phase_uses_one_reveal_and_rapid_toggles_preserve_its_progress(
    cx: &mut TestAppContext,
) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|_| TranscriptView::new())
        })
        .unwrap()
    });
    window
        .update(cx, |view, _, cx| {
            let tool = |id: &str| {
                row(
                    "tool",
                    &serde_json::to_string(&ToolCall {
                        id: id.into(),
                        name: "Read".into(),
                        input: format!(r#"{{"file_path":"{id}.rs"}}"#),
                        output: "tool output".into(),
                        state: ToolState::Completed,
                        panel: None,
                        display: None,
                    })
                    .unwrap(),
                )
            };
            view.patch(
                0,
                vec![
                    row("user", "Check"),
                    tool("one"),
                    tool("two"),
                    row("assistant", "Done"),
                ],
                cx,
            );
            view.work_folds.insert(1, true);
            view.rebuild(1, cx);
            view.motions.clear();
            view.toggle_phase(1, true, cx);
            let motions: Vec<_> = view
                .rows
                .iter()
                .filter(|r| r.in_phase())
                .map(|r| view.motions[&r.key].clone())
                .collect();
            assert_eq!(motions.len(), 2, "the two compact steps share the body");
            for motion in &motions {
                assert_eq!(motion.group, Some(1));
                assert_eq!(motion.start, motions[0].start);
                assert!(!motion.step);
            }
            view.toggle_phase(1, false, cx);
            assert!(
                view.rows.iter().any(|r| r.in_phase()),
                "closing retains the body through its exit"
            );
            assert!(
                view.motions
                    .values()
                    .all(|m| m.group == Some(1) && !m.opening)
            );
            view.toggle_phase(1, true, cx);
            assert!(
                view.motions
                    .values()
                    .all(|m| m.group == Some(1) && m.opening)
            );
            assert_eq!(view.phase_toggle, None);
            // Hold a partial reveal long enough to inspect actual list geometry.
            let start = Instant::now() - std::time::Duration::from_secs(25);
            for motion in view.motions.values_mut() {
                *motion = Motion::phase(1, true, start);
                motion.duration = std::time::Duration::from_secs(100);
            }
            view.visible = false;
            cx.notify();
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| {
            let layout = &view.phase_layouts[&1];
            let steps: Vec<_> = view
                .rows
                .iter()
                .enumerate()
                .filter(|(_, r)| r.in_phase())
                .collect();
            let (first_ix, first) = steps.first().unwrap();
            let (last_ix, last) = steps.last().unwrap();
            let first_bounds = view.list.bounds_for_item(*first_ix).unwrap();
            let last_bounds = view.list.bounds_for_item(*last_ix).unwrap();
            assert!(
                (f32::from(first_bounds.size.height) - layout.rows[&first.key].1).abs() < 0.1,
                "a revealed row must retain its natural height, rather than shrink with the group"
            );
            assert!(
                f32::from(last_bounds.size.height) < layout.rows[&last.key].1,
                "the reveal clips the bottom of the body"
            );
        })
        .unwrap();
}

#[gpui::test]
fn closing_a_phase_keeps_its_header_and_final_frame_in_place(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|_| TranscriptView::new())
        })
        .unwrap()
    });
    for width in [900., 320.] {
        cx.simulate_window_resize(window.into(), size(px(width), px(1400.)));
        window
            .update(cx, |view, _, cx| {
                view.visible = false;
                let mut source = vec![
                    row("user", "Check the collapse"),
                    row("assistant", "I will inspect the tools."),
                ];
                source.extend((0..5).map(|ix| {
                    row(
                        "tool",
                        &serde_json::to_string(&ToolCall {
                            id: ix.to_string(),
                            name: "Read".into(),
                            input: format!(r#"{{"file_path":"file-{ix}.rs"}}"#),
                            output: "A tool result that can wrap at a narrower width. ".repeat(3),
                            state: ToolState::Completed,
                            panel: None,
                            display: None,
                        })
                        .unwrap(),
                    )
                }));
                source.push(row("assistant", "The tools have finished."));
                source.push(row("assistant", "Here is the final answer."));
                view.patch(0, source, cx);
                view.work_folds.insert(1, true);
                view.phases.insert(2, true);
                view.expanded.insert(3);
                view.rebuild(0, cx);
                view.list.pause_following_tail();
                view.list.scroll_to(ListOffset {
                    item_ix: 0,
                    offset_in_item: px(0.),
                });
            })
            .unwrap();
        cx.refresh().unwrap();
        let header = window
            .update(cx, |view, _, cx| {
                assert_eq!(view.viewport.size.width, px(width));
                let ix = view
                    .rows
                    .iter()
                    .position(|r| matches!(r.body, Body::Phase { group: 2, .. }))
                    .unwrap();
                let header = view.list.bounds_for_item(ix).unwrap();
                view.visible = true;
                view.toggle_phase(2, false, cx);
                view.visible = false;
                header
            })
            .unwrap();
        cx.refresh().unwrap();
        let mut previous = window
            .update(cx, |view, _, _| {
                let ix = view
                    .rows
                    .iter()
                    .position(|r| matches!(r.body, Body::Phase { group: 2, .. }))
                    .unwrap();
                assert_eq!(
                    view.list.bounds_for_item(ix).unwrap(),
                    header,
                    "closing the body must not move or resize its header"
                );
                (0..view.rows.len())
                    .map(|ix| view.list.bounds_for_item(ix).unwrap())
                    .collect::<Vec<_>>()
            })
            .unwrap();
        for elapsed_ms in [40, 160, 220, 280, 310, 319, 320] {
            window
                .update(cx, |view, _, cx| {
                    let start = Instant::now() - std::time::Duration::from_millis(elapsed_ms);
                    for motion in view.motions.values_mut() {
                        motion.start = start;
                    }
                    view.list.remeasure_items(0..view.rows.len() + 1);
                    cx.notify();
                })
                .unwrap();
            cx.refresh().unwrap();
            previous = window.update(cx, |view, _, _| {
                    let current: Vec<_> = (0..view.rows.len())
                        .map(|ix| view.list.bounds_for_item(ix).unwrap()).collect();
                    for (before, after) in previous.iter().zip(&current) {
                        assert!(after.top() <= before.top() + px(0.1),
                            "closing must not shift rows down at {elapsed_ms}ms: {before:?} vs {after:?}");
                    }
                    current
                }).unwrap();
        }
        let finishing = window
            .update(cx, |view, _, cx| {
                let finishing: Vec<_> = view
                    .rows
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| !r.in_phase())
                    .map(|(ix, r)| (r.key, view.list.bounds_for_item(ix).unwrap()))
                    .collect();
                // Hidden transcripts intentionally ignore queued frames.
                // Drive this final visible frame explicitly for the test.
                view.visible = true;
                view.animate_frame(cx);
                view.visible = false;
                assert!(!view.rows.iter().any(|r| r.in_phase()));
                finishing
            })
            .unwrap();
        cx.refresh().unwrap();
        window
            .update(cx, |view, _, _| {
                for (key, before) in finishing {
                    let ix = view.rows.iter().position(|r| r.key == key).unwrap();
                    assert_eq!(
                        view.list.bounds_for_item(ix).unwrap(),
                        before,
                        "removing the closed rows must not nudge the transcript"
                    );
                }
            })
            .unwrap();
    }
}

#[gpui::test]
fn work_and_phase_disclosures_finish_at_their_natural_height_after_resizing(
    cx: &mut TestAppContext,
) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|_| TranscriptView::new())
        })
        .unwrap()
    });
    window
        .update(cx, |view, _, cx| {
            let tool = |id: &str| {
                row(
                    "tool",
                    &serde_json::to_string(&ToolCall {
                        id: id.into(),
                        name: "Read".into(),
                        input: format!(r#"{{"file_path":"{id}.rs"}}"#),
                        output: String::new(),
                        state: ToolState::Completed,
                        panel: None,
                        display: None,
                    })
                    .unwrap(),
                )
            };
            view.patch(
                0,
                vec![
                    row("user", "Check the layout"),
                    row(
                        "assistant",
                        &"Earlier commentary that wraps onto several lines. ".repeat(8),
                    ),
                    tool("first"),
                    tool("second"),
                    row(
                        "assistant",
                        "The final answer stays below the work.\n\nIts spacing stays fixed.",
                    ),
                ],
                cx,
            );
            view.list.pause_following_tail();
        })
        .unwrap();
    // Exercise first opening, nested tools and reopening at a new width,
    // without relying on any previously painted height.
    for width in [900., 320., 640.] {
        window
            .update(cx, |view, w, cx| {
                w.resize(size(px(width), px(1200.)));
                view.visible = true;
                view.work_folds.insert(1, true);
                view.phases.insert(2, true);
                view.rebuild(1, cx);
                view.motions.clear();
                for row in view.rows.iter().filter(|row| row.work) {
                    view.motions.insert(
                        row.key,
                        Motion::new(
                            true,
                            false,
                            Instant::now() - std::time::Duration::from_secs(1),
                        ),
                    );
                }
                // Retain the finishing frame until its geometry is inspected.
                view.visible = false;
                view.list.scroll_to(ListOffset {
                    item_ix: 0,
                    offset_in_item: px(0.),
                });
                cx.notify();
            })
            .unwrap();
        cx.refresh().unwrap();
        cx.run_until_parked();
        let finishing = window
            .update(cx, |view, _, cx| {
                let bounds: Vec<_> = (0..view.rows.len())
                    .map(|ix| view.list.bounds_for_item(ix).unwrap())
                    .collect();
                view.motions.clear();
                view.list.remeasure_items(0..view.rows.len() + 1);
                cx.notify();
                bounds
            })
            .unwrap();
        cx.refresh().unwrap();
        cx.run_until_parked();
        window.update(cx, |view, _, cx| {
                for (ix, before) in finishing.iter().enumerate() {
                    let after = view.list.bounds_for_item(ix).unwrap();
                    assert!((before.top() - after.top()).abs() < px(0.1));
                    assert!((before.size.height - after.size.height).abs() < px(0.1),
                        "finishing a disclosure must not move the content below it: {before:?} vs {after:?}");
                }
                assert!(view.action_anchors.contains_key(&(1, "copy")));
                assert!(view.action_anchors.contains_key(&(1, "note")));
                assert!(!view.action_anchors.contains_key(&(1, "activity")));
                view.work_folds.insert(1, false);
                view.rebuild(1, cx);
            }).unwrap();
    }
}

#[gpui::test]
fn bot_turn_actions_stay_while_moving_between_its_rows_in_any_event_order(cx: &mut TestAppContext) {
    let view = cx.new(|_| {
        let mut view = TranscriptView::new();
        view.bot_mode("cat", 0xe86b5f, "BotCat");
        view
    });
    view.update(cx, |view, cx| {
        view.patch(
            0,
            vec![row("user", "Hi"), row("assistant", "Hello there")],
            cx,
        );
        // The next row reports its enter before the last one leaves.
        view.bot_hover(false, 1, 1, (1, 16), true, cx);
        view.bot_hover(false, 1, 1, (1, usize::MAX - 1), true, cx);
        view.bot_hover(false, 1, 1, (1, 16), false, cx);
        assert_eq!(view.hovered_turn.map(|(t, _)| t), Some(1));
        view.bot_hover(false, 1, 1, (1, usize::MAX - 1), false, cx);
        assert!(view.hovered_turn.is_none());
    });
}

#[gpui::test]
fn bot_messages_stack_past_a_reaction_and_queued_bubbles_hand_over(cx: &mut TestAppContext) {
    let view = cx.new(|_| {
        let mut view = TranscriptView::new();
        view.bot_mode("cat", 0xe86b5f, "BotCat");
        view
    });
    view.update(cx, |view, cx| {
        view.patch(
            0,
            vec![
                row("user", "nice"),
                row("assistant", "<reaction>👍</reaction>"),
                row("user", "what else"),
            ],
            cx,
        );
        assert!(
            view.stacks_on_next(0),
            "a reaction-only reply keeps the stack"
        );
        assert!(!view.stacks_on_next(2));
        view.set_pending(
            vec![PendingMessage {
                id: "q".into(),
                text: "Queued".into(),
                error: None,
                sent_at: None,
                attachments: Vec::new(),
                peer: None,
            }],
            cx,
        );
        assert!(matches!(view.rows.last().unwrap().body, Body::Pending(0)));
        let born = view.pending_entrances["q"];
        view.streaming(true, cx);
        view.patch(3, vec![row("user", "Queued")], cx);
        view.set_pending(Vec::new(), cx);
        assert_eq!(
            view.entrances.get(&3),
            Some(&born),
            "delivery continues the same entrance without restarting it"
        );
        assert!(!view.rows.iter().any(|r| matches!(r.body, Body::Pending(_))));
        assert_eq!(view.list.item_count(), view.rows.len() + 1);
    });
}

#[gpui::test]
fn bot_turns_merge_follow_ups_keep_inline_work_and_queue_above_the_work(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let view = cx.new(|_| {
        let mut view = TranscriptView::new();
        view.bot_mode("cat", 0xe86b5f, "BotCat");
        view
    });
    let start = serde_json::to_string(&ToolCall {
        id: "t".into(),
        name: "Bash".into(),
        input: r#"{"command":"vorkspace app sessions.start --json {}"}"#.into(),
        output: r#"{"ok":true,"result":{"sessionId":776,"accepted":true}}"#.into(),
        state: ToolState::Completed,
        panel: None,
        display: None,
    })
    .unwrap();
    view.update(cx, |view, cx| {
        view.patch(
            0,
            vec![
                row("user", "Start a Luna session"),
                row("assistant", "I'll start a Luna session."),
                row("tool", &start),
                row("assistant", "Started session 776."),
                row("bot-system", "Session 776 finished"),
                row("assistant", "Luna replied."),
            ],
            cx,
        );
        let headers = view
            .rows
            .iter()
            .filter(|r| matches!(r.body, Body::BotHeader { .. }))
            .count();
        let actions: Vec<_> = view
            .rows
            .iter()
            .filter_map(|r| match r.body {
                Body::Actions { turn, .. } => Some(turn),
                _ => None,
            })
            .collect();
        assert_eq!(
            headers, 1,
            "the follow-up continues the turn under one header"
        );
        assert_eq!(
            actions,
            vec![1],
            "one set of actions, at the end of the group"
        );
        assert!(
            !view.rows.iter().any(|r| r.key == (1, 16)),
            "earlier commentary stays behind the inline receipt"
        );
        assert!(view.rows.iter().any(|r| r.key == (3, 16)));
        assert!(view.rows.iter().any(|r| r.key == (5, 16)));
        assert_eq!(
            view.rows
                .iter()
                .filter(|r| matches!(r.body, Body::WorkLine { .. }))
                .count(),
            2,
            "each run keeps its own work receipt under the shared signature"
        );
        view.work_folds.insert(1, true);
        view.rebuild(1, cx);
        assert!(view.rows.iter().any(|r| r.key == (1, 16) && r.work));
        assert!(
            view.rows
                .iter()
                .any(|r| matches!(r.body, Body::Tool { .. }))
        );
        assert!(
            view.rows
                .iter()
                .any(|r| matches!(r.body, Body::Launch(776))),
            "the accepted launch gets its card"
        );
        view.streaming(true, cx);
        view.patch(6, vec![row("user", "And another thing")], cx);
        view.patch(7, vec![row("assistant", "Working on it")], cx);
    });
    TranscriptView::settle_animation_frames(&view, cx);
    view.update(cx, |view, cx| {
        assert!(
            view.rows.iter().any(|r| r.key == (7, 16)),
            "live prose appears inline"
        );
        view.set_pending(
            vec![PendingMessage {
                id: "q".into(),
                text: "One more".into(),
                error: None,
                sent_at: None,
                attachments: Vec::new(),
                peer: None,
            }],
            cx,
        );
        let pending = view
            .rows
            .iter()
            .position(|r| matches!(r.body, Body::Pending(0)))
            .unwrap();
        let header = view
            .rows
            .iter()
            .rposition(|r| matches!(r.body, Body::WorkLine { .. }))
            .unwrap();
        assert!(
            pending < header,
            "a queued message sits above the running turn's work"
        );
    });
}

#[gpui::test]
fn desktop_work_metadata_renders_once_after_the_reply_and_never_as_prose(cx: &mut TestAppContext) {
    let view = cx.new(|_| {
        let mut view = TranscriptView::new();
        view.bot_mode("cat", 0xe86b5f, "BotCat");
        view
    });
    view.update(cx, |view,cx| {
            view.patch(0,vec![
                row("user","Write a report"),
                row("bot-document",r#"{"id":"artifact:report","type":"document","documentId":"report","title":"Report","kind":"html"}"#),
                row("bot-launch",r#"{"sessionId":7,"title":"Review","cwd":"/repo","harness":"codex"}"#),
                row("assistant","Written and checked."),
            ],cx);
            assert_eq!(view.rows.iter().filter(|r| matches!(r.body,Body::BotCard(_))).count(),1);
            assert_eq!(view.rows.iter().filter(|r| matches!(r.body,Body::Launch(7))).count(),1);
            assert!(!view.rows.iter().any(|r| matches!(&r.body,Body::Plain(text) if text.contains("documentId") || text.contains("sessionId"))));
            assert!(!view.rows.iter().any(|r| r.work));
            let answer=view.rows.iter().position(|r| matches!(r.body,Body::Markdown(_))).unwrap();
            let document=view.rows.iter().position(|r| matches!(r.body,Body::BotCard(_))).unwrap();
            assert!(document > answer);
        });
}

#[gpui::test]
fn bot_document_and_session_cards_are_one_line_like_react(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let start = serde_json::to_string(&ToolCall {
        id: "t".into(),
        name: "Bash".into(),
        input: r#"{"command":"vorkspace app sessions.start --json {}"}"#.into(),
        output: r#"{"ok":true,"result":{"sessionId":7,"accepted":true}}"#.into(),
        state: ToolState::Completed,
        panel: None,
        display: None,
    })
    .unwrap();
    let window = cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(900.), px(600.)),
                    ))),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|cx| {
                        let mut view = TranscriptView::new();
                        view.bot_mode("cat", 0xe86b5f, "BotCat");
                        view.patch(
                            0,
                            vec![
                                row("user", "Start one and write it up"),
                                row("tool", &start),
                                row("assistant", "Started it."),
                                row(
                                    "bot-card",
                                    r#"{"id":"c","type":"document","documentId":"d","title":"Test Document"}"#,
                                ),
                            ],
                            cx,
                        );
                        view.set_launches(
                            HashMap::from([(
                                7,
                                LaunchView {
                                    title: "Fix the composer".into(),
                                    state: "working",
                                    harness: "codex".into(),
                                    cwd: "/tmp/project".into(),
                                },
                            )]),
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
    let document = visual.debug_bounds("bot-artifact-card").unwrap();
    let session = visual.debug_bounds("bot-launch-card-7").unwrap();
    // ArtifactCard: min-h-[38px] and w-fit; LaunchedSessionCard: py-1.5
    // around its project chip, both at most 24rem wide.
    assert_eq!(document.size.height, px(38.));
    assert!(document.size.width < px(384.));
    assert_eq!(session.size.height, px(40.));
    assert!(session.size.width <= px(384.));
}

#[gpui::test]
fn bot_chat_rests_on_the_composer_and_slides_sent_messages_in(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(900.), px(600.)),
                ))),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| {
                    let mut view = TranscriptView::new();
                    view.bot_mode("cat", 0xe86b5f, "BotCat");
                    view
                })
            },
        )
        .unwrap()
    });
    let bubble_top = |w: &mut Window| {
        w.painted_quads()
            .into_iter()
            .filter(|q| {
                q.corner_radii.top_left.0 > 0.
                    && q.corner_radii.top_left.0 * 2. == q.bounds.size.height.0
            })
            .map(|q| q.bounds.top().0 / w.scale_factor())
            .fold(None, |low: Option<f32>, top| {
                Some(low.map_or(top, |l| l.max(top)))
            })
            .expect("single-line bubble is fully rounded")
    };
    // Loaded history neither animates nor starts at the top.
    window
        .update(cx, |view, _, cx| {
            view.patch(0, vec![row("user", "Earlier")], cx)
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, w, _| {
            assert!(view.entrances.is_empty(), "history must not slide in");
            assert!(
                bubble_top(w) > 400.,
                "a short Bot chat sits at the bottom, like justify-end"
            );
        })
        .unwrap();
    window
        .update(cx, |view, _, cx| {
            view.patch(
                0,
                vec![
                    row("user", "Hello"),
                    row("assistant", "<reaction>👍</reaction>"),
                ],
                cx,
            );
            view.streaming(true, cx);
            view.patch(2, vec![row("user", "A sent line")], cx);
            assert!(view.entrances.contains_key(&2), "the sent message enters");
            assert_eq!(view.reactions.get(&0).map(|r| r.0.as_str()), Some("👍"));
            view.entrances
                .insert(2, Instant::now() - std::time::Duration::from_millis(10));
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    let entering = window
        .update(cx, |view, w, _| {
            assert!(
                view.follow.offset > 0.,
                "the chat glides up to the sent message instead of jumping"
            );
            bubble_top(w)
        })
        .unwrap();
    window
        .update(cx, |view, _, cx| {
            view.entrances.clear();
            cx.notify();
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    let settled = window.update(cx, |_, w, _| bubble_top(w)).unwrap();
    assert!(
        entering > settled + 10.,
        "a sent message rises into place ({entering} vs {settled})"
    );
}

#[gpui::test]
fn prompt_rise_moves_the_painted_bubble_without_moving_its_scroll_anchor(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(900.), px(600.)),
                ))),
                ..Default::default()
            },
            |_, cx| cx.new(|_| TranscriptView::new()),
        )
        .unwrap()
    });
    window
        .update(cx, |view, _, cx| {
            view.streaming(true, cx);
            view.patch(
                0,
                vec![row("user", "A single line"), row("assistant", "")],
                cx,
            );
            view.streaming(false, cx);
            view.prompt_rise_pending = false;
            view.prompt_rise = Some((0, Instant::now() - std::time::Duration::from_millis(40)));
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    let (rising_y, scroll_anchor) = window
        .update(cx, |view, w, cx| {
            let bubble = w
                .painted_quads()
                .into_iter()
                .find(|q| {
                    q.corner_radii.top_left.0 > 0.
                        && q.corner_radii.top_left.0 * 2. == q.bounds.size.height.0
                })
                .expect("single-line bubble is fully rounded");
            assert_eq!(
                bubble.bounds.size.height.0 / w.scale_factor(),
                36.,
                "React's 20px line height plus 8px padding above and below"
            );
            let position = bubble.bounds.top().0 / w.scale_factor();
            let anchor = view.list.logical_scroll_top();
            view.prompt_rise = Some((0, Instant::now() - std::time::Duration::from_millis(900)));
            cx.notify();
            (position, anchor)
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, w, _| {
            let bubble = w
                .painted_quads()
                .into_iter()
                .find(|q| {
                    q.corner_radii.top_left.0 > 0.
                        && q.corner_radii.top_left.0 * 2. == q.bounds.size.height.0
                })
                .unwrap();
            let settled_y = bubble.bounds.top().0 / w.scale_factor();
            assert!(
                rising_y > settled_y + 30.,
                "the painted prompt must visibly rise, rather than only fade"
            );
            assert_eq!(
                view.list.logical_scroll_top().item_ix,
                scroll_anchor.item_ix
            );
            assert_eq!(
                view.list.logical_scroll_top().offset_in_item,
                scroll_anchor.offset_in_item
            );
        })
        .unwrap();
}

#[gpui::test]
fn steered_prompts_take_the_top_anchor_without_restarting_the_provider_turn(
    cx: &mut TestAppContext,
) {
    let window = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(900.), px(600.)),
                ))),
                ..Default::default()
            },
            |_, cx| cx.new(|_| TranscriptView::new()),
        )
        .unwrap()
    });
    window
        .update(cx, |view, _, cx| {
            view.patch(
                0,
                vec![
                    row("user", "Earlier question"),
                    row("assistant", "Earlier answer"),
                ],
                cx,
            );
            view.streaming(true, cx);
            view.patch(
                2,
                vec![row("user", "Start working"), row("assistant", "Working")],
                cx,
            );
            settle(view, cx);
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();

    for start in [4, 7] {
        window
            .update(cx, |view, _, cx| {
                view.list.scroll_by(px(-100.));
                view.update_jump(cx);
                assert!(!view.list.is_following_tail());
                let mut continuation = row("user", " and its continuation");
                continuation.first = false;
                view.patch(
                    start,
                    vec![
                        row("user", "Change direction"),
                        continuation,
                        row("assistant", "Following the steer"),
                    ],
                    cx,
                );
                assert_eq!(
                    view.live_start, 2,
                    "steering keeps the provider turn running"
                );
                assert_eq!(view.anchor_turn, Some(start));
                assert_eq!(view.prompt_rise.map(|(at, _)| at), Some(start));
                assert!(view.list.is_following_tail());
                assert!(!view.show_jump);
                settle(view, cx);
            })
            .unwrap();
        cx.refresh().unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, window, cx| {
                let prompt = view.messages.iter().find(|m| m.start == start).unwrap();
                let bounds = view.list.bounds_for_item(prompt.display_start).unwrap();
                assert!(
                    (bounds.top() - view.viewport.top()).abs() < px(1.),
                    "each accepted steer must replace the previous prompt's top anchor"
                );
                window.resize(size(px(700.), px(760.)));
                cx.notify();
            })
            .unwrap();
        cx.refresh().unwrap();
        cx.run_until_parked();
    }

    window
        .update(cx, |view, _, cx| {
            let prompt = view.messages.iter().find(|m| m.start == 7).unwrap();
            let bounds = view.list.bounds_for_item(prompt.display_start).unwrap();
            assert!((bounds.top() - view.viewport.top()).abs() < px(1.));
            view.list.scroll_by(px(-100.));
            let offset = view.list.logical_scroll_top();
            view.patch(0, view.source.clone(), cx);
            assert!(
                !view.list.is_following_tail(),
                "a replay must respect the reader's scroll"
            );
            assert_eq!(view.list.logical_scroll_top().item_ix, offset.item_ix);
            assert_eq!(
                view.list.logical_scroll_top().offset_in_item,
                offset.offset_in_item
            );
            assert!(
                view.prompt_rise.is_none(),
                "a replay must not repeat the entrance"
            );
        })
        .unwrap();
}

#[gpui::test]
fn live_prompt_anchors_respect_history_visibility_and_the_anchor_preference(
    cx: &mut TestAppContext,
) {
    let entity = cx.new(|_| TranscriptView::new());
    entity.update(cx, |view, cx| {
        view.patch(0, vec![row("user", "Restored history")], cx);
        assert!(view.anchor_turn.is_none());
        assert!(view.prompt_rise.is_none());
        view.streaming(true, cx);
        view.visible(false, cx);
        view.patch(1, vec![row("user", "A steer while hidden")], cx);
        assert_eq!(view.anchor_turn, Some(1));
        assert!(view.prompt_rise.is_none());
        view.visible(true, cx);
        view.patch(1, vec![row("user", "A steer while hidden")], cx);
        assert!(
            view.prompt_rise.is_none(),
            "showing a tab must not replay a steer"
        );

        view.apply_preferences(
            &vorkspace_ui::preferences::Preferences {
                anchor_prompts: false,
                ..Default::default()
            },
            cx,
        );
        view.list.pause_following_tail();
        view.patch(2, vec![row("user", "A steer with anchoring disabled")], cx);
        assert!(view.anchor_turn.is_none());
        assert!(view.prompt_rise.is_none());
        assert!(
            view.list.is_following_tail(),
            "a newly sent message still resumes following"
        );
    });
}

#[gpui::test]
fn sent_prompt_stays_at_the_top_then_long_replies_follow_until_scrolled_away(
    cx: &mut TestAppContext,
) {
    let window = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(900.), px(600.)),
                ))),
                ..Default::default()
            },
            |_, cx| cx.new(|_| TranscriptView::new()),
        )
        .unwrap()
    });
    window
        .update(cx, |view, _, cx| {
            view.patch(
                0,
                vec![
                    row("user", "Earlier question"),
                    row("assistant", "Earlier answer"),
                ],
                cx,
            );
            view.streaming(true, cx);
            view.patch(
                2,
                vec![
                    row("user", "The newly sent prompt"),
                    row("assistant", "A short reply"),
                ],
                cx,
            );
            view.streaming(false, cx);
            settle(view, cx);
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, w, cx| {
            {
                let markers: Vec<_> = w
                    .painted_quads()
                    .into_iter()
                    .filter(|q| {
                        q.border_color
                            == gpui_apple::metal_renderer::edge_fade_marker(
                                true,
                                TOP_FADE * w.scale_factor(),
                                BOTTOM_INSET * w.scale_factor(),
                            )
                            || q.border_color
                                == gpui_apple::metal_renderer::edge_fade_marker(
                                    false,
                                    TOP_FADE * w.scale_factor(),
                                    BOTTOM_INSET * w.scale_factor(),
                                )
                    })
                    .collect();
                assert_eq!(
                    markers.len(),
                    2,
                    "both mask boundaries must be painted inside the viewport"
                );
                assert_eq!(markers[0].bounds, markers[1].bounds);
            }
            let prompt = view
                .rows
                .iter()
                .position(|r| view.messages[r.message].start == 2)
                .unwrap();
            let bounds = view.list.bounds_for_item(prompt).unwrap();
            assert!(
                (bounds.top() - view.viewport.top()).abs() < px(1.),
                "prompt row must stay at the top, below the list's fade padding"
            );
            assert!(view.list.is_following_tail());
            w.resize(size(px(700.), px(760.)));
            cx.notify();
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            let prompt = view
                .rows
                .iter()
                .position(|r| view.messages[r.message].start == 2)
                .unwrap();
            assert!(
                (view.list.bounds_for_item(prompt).unwrap().top() - view.viewport.top()).abs()
                    < px(1.),
                "resizing must retain the anchored prompt"
            );
            view.patch(
                3,
                vec![row(
                    "assistant",
                    &"A paragraph of streamed reply text.\n\n".repeat(80),
                )],
                cx,
            );
            settle(view, cx);
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert!(view.list.is_following_tail());
            view.list.scroll_by(px(-150.));
            view.update_jump(cx);
            assert!(view.show_jump);
            let offset = view.list.logical_scroll_top();
            view.patch(
                3,
                vec![row(
                    "assistant",
                    &"A paragraph of streamed reply text.\n\n".repeat(90),
                )],
                cx,
            );
            settle(view, cx);
            assert_eq!(view.list.logical_scroll_top().item_ix, offset.item_ix);
            assert_eq!(
                view.list.logical_scroll_top().offset_in_item,
                offset.offset_in_item
            );
            view.jump_to_bottom(cx);
            assert!(!view.show_jump && view.list.is_following_tail());
        })
        .unwrap();
}
