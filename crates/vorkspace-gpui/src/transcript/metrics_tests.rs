use super::*;
use gpui::{TestAppContext, VisualTestContext, WindowHandle, point};
use vorkspace_core::agent::{Settings, TurnInfo, TurnMetrics};

fn row(role: &str, text: &str) -> Row {
    Row {
        role: role.into(),
        text: text.into(),
        first: true,
        sent_at: Some(1_000),
    }
}

fn info() -> TurnInfo {
    TurnInfo {
        duration_ms: 1_400_000,
        duration_known: true,
        completed_at: Some(1_401_000),
        metrics: Some(TurnMetrics {
            input_tokens: 6_200_000,
            output_tokens: 42_000,
            cache_read_tokens: 6_000_000,
            cache_hit_percent: Some(98.),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn fixture(cx: &mut TestAppContext, bot: bool) -> WindowHandle<TranscriptView> {
    crate::clock::use_test_clock(cx);
    cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut view = TranscriptView::new();
                if bot {
                    view.bot_mode("cat", 0xe86b5f, "BotCat");
                }
                view.smooth_reveal = false;
                view.turns.insert(1, info());
                view.install_prepared_history(
                    PreparedHistory::new(
                        vec![row("user", "Check the reply"), row("assistant", "Checked.")],
                        bot,
                    ),
                    cx,
                );
                view
            })
        })
        .unwrap()
    })
}

fn tooltip_text(visual: &mut VisualTestContext) -> String {
    if visual.debug_bounds("response-tooltip-content-1").is_none()
        && visual.debug_bounds("response-tooltip-content-7").is_none()
    {
        return String::new();
    }
    visual.update(|window, cx| {
        window
            .root::<TranscriptView>()
            .unwrap()
            .unwrap()
            .read(cx)
            .tooltip_text
            .borrow()
            .iter()
            .map(|layout| layout.text())
            .collect::<Vec<_>>()
            .join("\n")
    })
}

fn hover_metrics(
    window: WindowHandle<TranscriptView>,
    visual: &mut VisualTestContext,
    turn: usize,
) -> Bounds<Pixels> {
    crate::clock::settle_animations(window, visual);
    let anchor = window
        .update(visual, |view, _, _| {
            view.action_anchors[&(turn, "metrics")].get().unwrap()
        })
        .unwrap();
    visual.simulate_mouse_move(anchor.center(), None, Default::default());
    // Hover through the real hit target, then advance its finite reveal.
    window
        .update(visual, |view, _, cx| {
            if view.bot {
                assert_eq!(view.hovered_turn.map(|(turn, _)| turn), Some(turn));
                view.hovered_turn =
                    Some((turn, Instant::now() - std::time::Duration::from_secs(1)));
            }
            assert_eq!(view.hovered_action, Some((turn, "metrics")));
            cx.notify();
        })
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
    crate::clock::settle_animations(window, visual);
    anchor
}

#[gpui::test]
fn bot_and_session_metrics_hover_show_the_same_text_and_fit_at_both_widths(
    cx: &mut TestAppContext,
) {
    for bot in [false, true] {
        let window = fixture(cx, bot);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        for width in [900., 320.] {
            visual.simulate_mouse_move(point(px(0.), px(0.)), None, Default::default());
            visual.simulate_resize(size(px(width), px(600.)));
            visual.run_until_parked();
            visual.refresh().unwrap();
            assert!(visual.debug_bounds("response-tooltip-content-1").is_none());
            let anchor = hover_metrics(window, visual, 1);
            let text = tooltip_text(visual);
            assert!(text.contains("Cache hit 98% · Output 30 tok/s"), "{text}");
            assert!(
                text.contains("6.2M input · 42K output · 6M cached"),
                "{text}"
            );
            let content = visual.debug_bounds("response-tooltip-content-1").unwrap();
            assert!(content.size.width > px(0.) && content.size.height >= px(32.));
            assert!(content.left() >= px(8.) && content.right() <= px(width - 8.));
            assert!(
                content.bottom() < anchor.top(),
                "tooltip opens above its icon"
            );
            visual.update(|window, _| {
                let frame = window
                    .painted_quads()
                    .into_iter()
                    .find(|quad| {
                        quad.background == gpui::Background::from(crate::theme::content(0.02))
                    })
                    .expect("painted tooltip glass surface");
                // Inspect the glass fill: GPUI splits transparent border
                // quads into edge fragments that intentionally omit the center.
                // The tooltip must escape the Bot strip's 24px overflow clip.
                assert!(
                    frame.content_mask.bounds.contains(&frame.bounds.center()),
                    "bot={bot}, width={width}, frame={frame:?}"
                );
                assert!(frame.bounds.size.height.0 >= 50. * window.scale_factor());
            });
            visual.simulate_mouse_move(point(px(0.), px(0.)), None, Default::default());
            visual.run_until_parked();
            visual.refresh().unwrap();
            assert!(visual.debug_bounds("response-tooltip-content-1").is_none());
        }
    }
}

#[gpui::test]
fn bot_metrics_use_the_latest_represented_run_in_each_group(cx: &mut TestAppContext) {
    let window = fixture(cx, true);
    window
        .update(cx, |view, _, cx| {
            let mut delegated = ToolCall::new("delegate", "Agent");
            delegated.state = ToolState::Completed;
            delegated.output = "Unrelated child: 999M input · 999M output".into();
            view.patch(
                2,
                vec![
                    row("bot-system", "Review finished"),
                    row("assistant", "Checking the report."),
                    row("tool", &serde_json::to_string(&delegated).unwrap()),
                    row("assistant", "Reviewed."),
                    row("user", "Another question"),
                    row("assistant", "Another answer."),
                ],
                cx,
            );
            assert_eq!(view.bot_group_start(3), 1);
            assert_eq!(view.bot_group_start(7), 7);
            view.turns.insert(
                3,
                TurnInfo {
                    duration_ms: 3_000,
                    completed_at: Some(3_601_000),
                    metrics: Some(TurnMetrics {
                        input_tokens: 1_000,
                        output_tokens: 90,
                        cache_read_tokens: 500,
                        cache_hit_percent: Some(50.),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            );
            view.turns.insert(7, info());
            // Metadata alone, including a truncated or unrelated run, must
            // never choose which receipt the visible reply represents.
            view.turns.insert(99, info());
            cx.notify();
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(size(px(900.), px(900.)));
    visual.run_until_parked();
    visual.refresh().unwrap();
    hover_metrics(window, visual, 1);
    let text = tooltip_text(visual);
    assert!(text.contains("Cache hit 50% · Output 30 tok/s"), "{text}");
    assert!(text.contains("1K input · 90 output · 500 cached"));
    assert!(!text.contains("6.2M input"));

    hover_metrics(window, visual, 7);
    let text = tooltip_text(visual);
    assert!(text.contains("Cache hit 98% · Output 30 tok/s"));
    assert!(!text.contains("1K input · 90 output"));
}

#[gpui::test]
fn bot_latest_run_without_metrics_never_falls_back_even_during_a_stale_hover(
    cx: &mut TestAppContext,
) {
    for latest in [
        None,
        Some(TurnInfo::default()),
        Some(TurnInfo {
            metrics: Some(TurnMetrics::default()),
            ..Default::default()
        }),
    ] {
        let window = fixture(cx, true);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.refresh().unwrap();
        hover_metrics(window, visual, 1);
        assert!(tooltip_text(visual).contains("6.2M input"));
        window
            .update(visual, |view, _, cx| {
                view.patch(
                    2,
                    vec![
                        row("bot-system", "Continue"),
                        row("assistant", "Continued."),
                    ],
                    cx,
                );
                if let Some(info) = latest {
                    view.turns.insert(3, info);
                }
                assert_eq!(view.bot_group_start(3), 1);
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        visual.refresh().unwrap();
        let text = tooltip_text(visual);
        assert!(visual.debug_bounds("response-metrics-1").is_none());
        assert!(!text.contains("6.2M input"));
        assert!(visual.debug_bounds("response-tooltip-content-1").is_none());
    }
}

#[gpui::test]
fn metrics_with_zero_or_legacy_duration_keep_tokens_without_a_made_up_rate(
    cx: &mut TestAppContext,
) {
    for bot in [false, true] {
        let window = fixture(cx, bot);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        for (known, hit) in [
            (false, None),
            (true, None),
            (true, Some(f64::NAN)),
            (true, Some(f64::INFINITY)),
        ] {
            window
                .update(visual, |view, _, cx| {
                    let info = view.turns.get_mut(&1).unwrap();
                    info.duration_ms = 0;
                    info.duration_known = known;
                    info.metrics.as_mut().unwrap().cache_hit_percent = hit;
                    cx.notify();
                })
                .unwrap();
            visual.run_until_parked();
            visual.refresh().unwrap();
            hover_metrics(window, visual, 1);
            let text = tooltip_text(visual);
            assert!(text.contains("Turn tokens"));
            assert!(text.contains("6.2M input · 42K output · 6M cached"));
            assert!(!text.contains("tok/s") && !text.contains("NaN") && !text.contains("inf"));
        }
    }
}

#[gpui::test]
fn streamed_bot_metrics_survive_save_reopen_and_history_install(cx: &mut TestAppContext) {
    let window = fixture(cx, true);
    // The shown output rate divides by elapsed time, so this test reads real time.
    crate::clock::use_test_clock(cx);
    window
        .update(cx, |view, _, cx| {
            view.streaming(true, cx);
            view.patch(
                2,
                vec![
                    row("bot-system", "Continue"),
                    row("assistant", "Final reply."),
                ],
                cx,
            );
            view.metrics(info().metrics.unwrap());
            assert_eq!(view.active_turn(), Some(3));
            assert_eq!(view.turns[&3].metrics, info().metrics);
            view.started = Some(Instant::now() - std::time::Duration::from_secs(1_400));
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(
        visual.debug_bounds("response-metrics-1").is_none(),
        "active replies keep their action gap"
    );
    window
        .update(visual, |view, _, cx| view.streaming(false, cx))
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
    hover_metrics(window, visual, 1);
    assert!(tooltip_text(visual).contains("Cache hit 98% · Output 30 tok/s"));

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fixture.db");
    let id = window
        .update(visual, |view, _, _| {
            let mut store = vorkspace_core::store::Store::open_vorkspace(&path).unwrap();
            let session = store
                .create("codex", directory.path().to_str().unwrap())
                .unwrap();
            store.write_rows(session.id, 0, &view.source).unwrap();
            store
                .configure_agent(
                    session.id,
                    "codex",
                    &Settings {
                        turns: view.turns.clone(),
                        ..Default::default()
                    },
                )
                .unwrap();
            session.id
        })
        .unwrap();
    let store = vorkspace_core::store::Store::open_vorkspace(&path).unwrap();
    let settings = store.agent_settings(id).unwrap().unwrap();
    let history = store.load(id).unwrap();
    assert_eq!(settings.turns[&3].metrics, info().metrics);
    assert!(settings.turns[&3].duration_known);
    window
        .update(visual, |view, _, cx| {
            *view = TranscriptView::new();
            view.bot_mode("cat", 0xe86b5f, "BotCat");
            view.smooth_reveal = false;
            view.turns = settings.turns;
            view.install_prepared_history(PreparedHistory::new(history.rows, true), cx);
        })
        .unwrap();
    visual.simulate_mouse_move(point(px(0.), px(0.)), None, Default::default());
    visual.run_until_parked();
    visual.refresh().unwrap();
    hover_metrics(window, visual, 1);
    let text = tooltip_text(visual);
    assert!(text.contains("Cache hit 98% · Output 30 tok/s"));
    assert!(text.contains("6.2M input · 42K output · 6M cached"));
}

#[gpui::test]
fn bot_metrics_preserve_copy_note_and_quote_actions(cx: &mut TestAppContext) {
    let window = fixture(cx, true);
    let notes = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let quotes = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let _subscriptions = window
        .update(cx, |_, _, cx| {
            let notes = notes.clone();
            let quotes = quotes.clone();
            (
                cx.subscribe(&cx.entity(), move |_, _, event: &ResponseRequest, _| {
                    if let ResponseRequest::SaveNote {
                        turn,
                        text,
                        selection,
                    } = event
                    {
                        notes.borrow_mut().push((*turn, text.clone(), *selection));
                    }
                }),
                cx.subscribe(&cx.entity(), move |_, _, event: &BotResponseAction, _| {
                    if let BotResponseAction::Quote { text, .. } = event {
                        quotes.borrow_mut().push(text.clone());
                    }
                }),
            )
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(size(px(320.), px(600.)));
    visual.run_until_parked();
    visual.refresh().unwrap();
    let anchor = hover_metrics(window, visual, 1);
    // Three existing 22px buttons, 4px gaps, then the metrics' 3px inset.
    let copy = anchor.center() - point(px(81.), px(0.));
    let note = anchor.center() - point(px(55.), px(0.));
    let quote = anchor.center() - point(px(29.), px(0.));
    visual.simulate_click(copy, Default::default());
    visual.run_until_parked();
    visual.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().unwrap(),
            "Checked."
        )
    });
    window
        .update(visual, |view, _, _| {
            assert_eq!(view.copied_response.unwrap().0, 1)
        })
        .unwrap();
    visual.simulate_click(note, Default::default());
    visual.run_until_parked();
    assert_eq!(*notes.borrow(), vec![(1, "Checked.".into(), false)]);
    window
        .update(visual, |view, _, cx| {
            assert!(view.note_pending.contains(&1));
            view.note_result(1, Ok(()), cx);
            assert!(view.note_feedback[&1].is_ok());
        })
        .unwrap();
    visual.simulate_click(quote, Default::default());
    visual.run_until_parked();
    assert_eq!(*quotes.borrow(), vec!["Checked.".to_owned()]);
    hover_metrics(window, visual, 1);
    assert!(tooltip_text(visual).contains("Cache hit 98% · Output 30 tok/s"));
}

// Uses GPUI's existing offscreen native capture path; never opens the shell,
// a provider, or a database. Kept opt-in like the other Metal regressions.
#[test]
#[ignore = "Requires a Metal device and VOKKY_METRICS_CAPTURE_DIR"]
fn native_metrics_hover_capture() {
    struct Surface(gpui::Entity<TranscriptView>);
    impl Render for Surface {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .font_family(crate::fonts::UI)
                .bg(rgb(CHAT))
                .child(self.0.clone())
        }
    }
    let output = std::path::PathBuf::from(std::env::var_os("VOKKY_METRICS_CAPTURE_DIR").unwrap());
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
    for bot in [false, true] {
        for width in [900., 320.] {
            let window = cx
                .open_window(size(px(width), px(360.)), |_, cx| {
                    let transcript = cx.new(|cx| {
                        let mut view = TranscriptView::new();
                        if bot {
                            view.bot_mode("cat", 0xe86b5f, "BotCat");
                        }
                        view.smooth_reveal = false;
                        view.turns.insert(1, info());
                        view.install_prepared_history(
                            PreparedHistory::new(
                                vec![row("user", "Check the reply"), row("assistant", "Checked.")],
                                bot,
                            ),
                            cx,
                        );
                        view
                    });
                    cx.new(|_| Surface(transcript))
                })
                .unwrap();
            cx.run_until_parked();
            cx.update_window(window.into(), |surface, window, cx| {
                let surface = surface.downcast::<Surface>().unwrap();
                let view = surface.read(cx).0.clone();
                let anchor = view.read(cx).action_anchors[&(1, "metrics")].get().unwrap();
                window.simulate_mouse_move(anchor.center(), cx);
                view.update(cx, |view, cx| {
                    assert_eq!(view.hovered_action, Some((1, "metrics")));
                    view.hovered_turn =
                        Some((1, Instant::now() - std::time::Duration::from_secs(1)));
                    cx.notify();
                });
            })
            .unwrap();
            cx.run_until_parked();
            crate::clock::settle_headless_animations(window, &mut cx);
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(output.join(format!(
                    "{}-{width:.0}.png",
                    if bot { "bot" } else { "session" },
                )))
                .unwrap();
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
        }
    }
}
