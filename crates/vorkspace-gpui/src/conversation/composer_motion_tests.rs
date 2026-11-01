use super::*;
use gpui::{Bounds, EntityInputHandler, TestAppContext, WindowBounds, WindowOptions, point, size};
use std::{sync::Mutex, time::Duration};
use vorkspace_core::store::Store;

fn window(cx: &mut TestAppContext, width: f32) -> gpui::WindowHandle<Conversation> {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let summary = store.create("codex", "/work/vorkspace").unwrap();
    let store = Arc::new(Mutex::new(store));
    let window = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(width), px(700.)),
                ))),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| Conversation::new(summary, store, cx)),
        )
        .unwrap()
    });
    window
        .update(cx, |c, w, cx| {
            c.loaded = true;
            c.status = "Ready".into();
            w.focus(&c.input.focus_handle(cx), cx);
            c.input.update(cx, |i, cx| {
                i.replace_text_in_range(None, "First message", w, cx)
            });
            cx.notify();
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    window
}
fn begin(
    window: gpui::WindowHandle<Conversation>,
    cx: &mut TestAppContext,
) -> Bounds<gpui::Pixels> {
    let from = window
        .update(cx, |c, _, cx| {
            let from = c.composer_anchor.get().unwrap();
            // Use the first-send presentation path without a provider request.
            c.input.update(cx, |i, cx| {
                i.take(cx);
            });
            c.begin_turn(cx);
            cx.notify();
            from
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    from
}
#[gpui::test]
fn composer_toolbar_selects_one_row_during_resize_and_preserves_draft(cx: &mut TestAppContext) {
    vorkspace_ui::theme::configure(&Default::default(), false, true);
    cx.update(|cx| {
        cx.set_global(crate::settings::Preferences::default());
        vorkspace_ui::clock::use_test_clock(cx);
        input::bind(cx);
        crate::popover::bind(cx);
    });
    let window = window(cx, 1000.);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    let mut saw_compact = false;
    let mut saw_wide = false;
    for width in [1000., 760., 480., 360., 260., 360., 480., 760., 1000.] {
        visual.simulate_resize(size(px(width), px(700.)));
        for started in [false, true] {
            for beside in [false, true] {
                window
                    .update(visual, |view, _, cx| {
                        cx.global_mut::<crate::settings::Preferences>()
                            .model_controls = if beside { "beside" } else { "combined" }.into();
                        view.summary.has_messages = started;
                        view.branch = Some("feature/a-very-long-branch-name".into());
                        view.git = Some(vorkspace_core::git::Snapshot::default());
                        view.model = Some("gpt-6.1-sol".into());
                        view.settings.effort = Some("xhigh".into());
                        view.settings.access = AccessMode::FullAccess;
                        view.catalog = vec![Model {
                            alias_target: None,
                            id: "gpt-6.1-sol".into(),
                            name: "GPT-6.1 Sol".into(),
                            efforts: vec!["high".into(), "xhigh".into()],
                            default_effort: Some("high".into()),
                            fast: false,
                            thinking: false,
                            default: true,
                            tiers: Vec::new(),
                            controls: Vec::new(),
                        }];
                        cx.notify();
                    })
                    .unwrap();
                visual.refresh().unwrap();
                let toolbar = visual.debug_bounds("composer-toolbar").unwrap();
                assert!(
                    toolbar.size.height <= px(30.5),
                    "toolbar wrapped at {width}: {toolbar:?}"
                );
                let add = visual.debug_bounds("composer-add").unwrap();
                let compact = visual.debug_bounds("composer-session-settings");
                if let Some(settings) = compact {
                    saw_compact = true;
                    assert!(visual.debug_bounds("composer-workspace").is_none());
                    assert!(visual.debug_bounds("composer-model").is_none());
                    assert!(visual.debug_bounds("composer-access").is_none());
                    assert!(add.right() <= settings.left());
                    assert!(settings.right() <= toolbar.right());
                    if let Some(context) = visual.debug_bounds("context-meter") {
                        assert!(settings.right() <= context.left());
                        assert!(context.right() <= toolbar.right());
                    }
                } else {
                    saw_wide = true;
                    for name in [
                        "composer-workspace",
                        "composer-branch",
                        "composer-model",
                        "composer-access",
                    ] {
                        let control = visual.debug_bounds(name).unwrap();
                        assert!(
                            control.top() >= toolbar.top() && control.bottom() <= toolbar.bottom()
                        );
                    }
                }
                for _ in 0..3 {
                    visual.refresh().unwrap();
                    assert_eq!(
                        compact.is_some(),
                        visual.debug_bounds("composer-session-settings").is_some()
                    );
                }
                window
                    .update(visual, |view, _, cx| {
                        assert_eq!(view.input.read(cx).content.as_ref(), "First message")
                    })
                    .unwrap();
            }
        }
    }
    assert!(saw_compact && saw_wide);
}
#[gpui::test]
fn compact_settings_keep_existing_actions_focus_and_anchor_on_resize(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_global(crate::settings::Preferences::default());
        vorkspace_ui::clock::use_test_clock(cx);
        input::bind(cx);
        crate::popover::bind(cx);
    });
    let window = window(cx, 360.);
    window
        .update(cx, |c, _, cx| {
            c.git = Some(vorkspace_core::git::Snapshot::default());
            c.branch = Some("main".into());
            c.settings.access = AccessMode::FullAccess;
            c.model = Some("test-model".into());
            c.catalog = vec![Model {
                alias_target: None,
                id: "test-model".into(),
                name: "Test model".into(),
                efforts: vec!["high".into(), "xhigh".into()],
                default_effort: Some("high".into()),
                fast: false,
                thinking: false,
                default: true,
                tiers: Vec::new(),
                controls: Vec::new(),
            }];
            cx.notify();
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    window
        .update(visual, |c, w, cx| w.focus(&c.compact_focus, cx))
        .unwrap();
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert!(visual.debug_bounds("menu-action-workspace:new").is_some());
    assert!(
        visual
            .debug_bounds("menu-action-workspace:existing")
            .is_some()
    );
    assert!(visual.debug_bounds("menu-action-compact:branch").is_some());
    assert!(visual.debug_bounds("menu-action-model").is_some());
    assert!(visual.debug_bounds("menu-action-reasoning").is_some());
    assert!(visual.debug_bounds("menu-action-compact:access").is_some());
    let effort = visual.debug_bounds("menu-action-reasoning").unwrap();
    visual.simulate_mouse_move(effort.center(), None, Default::default());
    visual.run_until_parked();
    let high = visual.debug_bounds("menu-action-effort:xhigh").unwrap();
    visual.simulate_click(high.center(), Default::default());
    window
        .update(visual, |c, _, cx| {
            assert_eq!(c.settings.effort.as_deref(), Some("xhigh"));
            assert_eq!(c.input.read(cx).content.as_ref(), "First message");
        })
        .unwrap();
    visual.refresh().unwrap();
    let settings = visual.debug_bounds("composer-session-settings").unwrap();
    visual.simulate_click(settings.center(), Default::default());
    visual.run_until_parked();
    let new = visual.debug_bounds("menu-action-workspace:new").unwrap();
    visual.simulate_click(new.center(), Default::default());
    window
        .update(visual, |c, w, cx| {
            assert!(c.settings.new_worktree);
            assert!(c.input.focus_handle(cx).is_focused(w));
            assert_eq!(c.input.read(cx).content.as_ref(), "First message");
        })
        .unwrap();
    window
        .update(visual, |c, _, cx| {
            c.git = Some(vorkspace_core::git::Snapshot::default());
            c.branch = Some("main".into());
            c.settings.worktree_base = Some("refs/remotes/origin/main".into());
            cx.notify();
        })
        .unwrap();
    window
        .update(visual, |c, w, cx| {
            let (_, items, _, _) =
                c.menu_items(ComposerMenu::CompactSettings, &mut Vec::new(), w, cx);
            let branch = items.iter().find_map(|item| match item {
                MenuItem::Action(action) if action.id.as_ref() == "compact:branch" => {
                    action.value.as_deref()
                }
                _ => None,
            });
            assert_eq!(branch, Some("origin/main"));
            assert_eq!(
                c.settings.worktree_base.as_deref(),
                Some("refs/remotes/origin/main")
            );
        })
        .unwrap();
    visual.refresh().unwrap();
    let settings = visual.debug_bounds("composer-session-settings").unwrap();
    visual.simulate_click(settings.center(), Default::default());
    visual.run_until_parked();
    let branch = visual.debug_bounds("menu-action-compact:branch").unwrap();
    visual.simulate_click(branch.center(), Default::default());
    window
        .update(visual, |c, _, cx| {
            assert!(
                c.git_picker
                    .as_ref()
                    .is_some_and(|picker| picker.read(cx).mode() == GitMode::Base)
            );
            assert_eq!(c.input.read(cx).content.as_ref(), "First message");
        })
        .unwrap();
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    visual.refresh().unwrap();
    let settings = visual.debug_bounds("composer-session-settings").unwrap();
    visual.simulate_click(settings.center(), Default::default());
    visual.run_until_parked();
    let access = visual.debug_bounds("menu-action-compact:access").unwrap();
    visual.simulate_mouse_move(access.center(), None, Default::default());
    visual.run_until_parked();
    let supervised = visual
        .debug_bounds("menu-action-access:supervised")
        .unwrap();
    visual.simulate_click(supervised.center(), Default::default());
    window
        .update(visual, |c, _, cx| {
            assert_eq!(c.settings.access, AccessMode::Supervised);
            assert_eq!(c.input.read(cx).content.as_ref(), "First message");
        })
        .unwrap();
    visual.refresh().unwrap();
    let settings = visual.debug_bounds("composer-session-settings").unwrap();
    visual.simulate_click(settings.center(), Default::default());
    visual.run_until_parked();
    visual.simulate_resize(size(px(1000.), px(700.)));
    visual.refresh().unwrap();
    window
        .update(visual, |c, _, _| {
            assert!(c.menu_is(ComposerMenu::CompactSettings));
            assert_eq!(c.compact_anchor.get(), c.model_anchor.get());
        })
        .unwrap();
    visual.simulate_keystrokes("escape");
    window
        .update(visual, |c, w, cx| {
            assert!(c.menu.is_none());
            assert!(c.input.focus_handle(cx).is_focused(w));
            assert_eq!(c.input.read(cx).content.as_ref(), "First message");
        })
        .unwrap();
    visual.refresh().unwrap();
    let model = visual.debug_bounds("composer-model").unwrap();
    visual.simulate_click(model.center(), Default::default());
    visual.run_until_parked();
    visual.simulate_resize(size(px(360.), px(700.)));
    visual.refresh().unwrap();
    window
        .update(visual, |c, _, _| {
            assert!(c.menu_is(ComposerMenu::Model));
            assert_eq!(c.model_anchor.get(), c.compact_anchor.get());
        })
        .unwrap();
    visual.simulate_keystrokes("escape");
    window
        .update(visual, |c, w, cx| {
            assert!(c.menu.is_none());
            assert!(c.input.focus_handle(cx).is_focused(w));
            assert_eq!(c.input.read(cx).content.as_ref(), "First message");
        })
        .unwrap();
}
#[gpui::test]
fn compact_bot_composer_keeps_queue_and_bot_settings_separate(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_global(crate::settings::Preferences::default());
        crate::popover::bind(cx);
    });
    let window = window(cx, 220.);
    window
        .update(cx, |view, _, cx| {
            view.configure_bot(
                vorkspace_core::bot::Bot {
                    name: "Sam".into(),
                    ..Default::default()
                },
                "/private/tmp".into(),
                Default::default(),
                cx,
            );
            view.input
                .update(cx, |input, cx| input.set_text("Unsent draft".into(), cx));
        })
        .unwrap();
    window
        .update(cx, |view, _, cx| {
            view.bot_ui.queue = true;
            view.model = Some("test-model".into());
            view.catalog = vec![vorkspace_core::catalog::Model {
                alias_target: None,
                id: "test-model".into(),
                name: "A very long test model name".into(),
                efforts: vec!["high".into(), "xhigh".into()],
                default_effort: Some("high".into()),
                fast: false,
                thinking: false,
                default: true,
                tiers: Vec::new(),
                controls: Vec::new(),
            }];
            cx.notify();
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(size(px(220.), px(680.)));
    visual.refresh().unwrap();
    let settings = visual.debug_bounds("composer-session-settings").unwrap();
    let add = visual.debug_bounds("composer-add").unwrap();
    let context = visual.debug_bounds("context-meter").unwrap();
    assert!(add.right() <= settings.left() && settings.right() <= context.left());
    visual.simulate_click(settings.center(), Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("menu-action-model").is_some());
    assert!(visual.debug_bounds("menu-action-reasoning").is_some());
    assert!(visual.debug_bounds("menu-action-compact:access").is_none());
    assert!(visual.debug_bounds("menu-action-workspace:new").is_none());
    visual.simulate_keystrokes("escape");
    visual.refresh().unwrap();
    visual.simulate_click(add.center(), Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("menu-action-queue").is_some());
    window
        .update(visual, |view, _, cx| {
            assert!(view.bot_ui.queue);
            assert_eq!(view.input.read(cx).content.as_ref(), "Unsent draft");
        })
        .unwrap();
}

#[test]
#[ignore = "Requires Metal and VORKSPACE_COMPOSER_CAPTURE_DIR"]
fn native_narrow_composer_capture() {
    struct Capture(Entity<Conversation>);
    impl gpui::Render for Capture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().bg(pane_fill()).child(self.0.clone())
        }
    }
    let output =
        std::path::PathBuf::from(std::env::var_os("VORKSPACE_COMPOSER_CAPTURE_DIR").unwrap());
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
    vorkspace_ui::clock::use_test_clock(&cx);
    for light in [false, true] {
        let prefs = crate::settings::Preferences {
            theme: if light { "light" } else { "dark" }.into(),
            ..Default::default()
        };
        vorkspace_ui::theme::configure(&prefs, false, true);
        cx.update(|cx| cx.set_global(prefs));
        for width in [260., 360., 480., 760.] {
            let store = Store::open(Path::new(":memory:")).unwrap();
            let summary = store.create("codex", "/work/vorkspace").unwrap();
            let store = Arc::new(Mutex::new(store));
            let window = cx
                .open_window(size(px(width), px(700.)), |_, cx| {
                    let conversation = cx.new(|cx| {
                        let mut view = Conversation::new(summary, store, cx);
                        view.loaded = true;
                        view.status = "Ready".into();
                        view.branch = Some("feature/a-very-long-branch-name".into());
                        view.settings.access = AccessMode::FullAccess;
                        view.model = Some("gpt-6.1-sol".into());
                        view.settings.effort = Some("xhigh".into());
                        view.catalog = vec![Model {
                            alias_target: None,
                            id: "gpt-6.1-sol".into(),
                            name: "GPT-6.1 Sol".into(),
                            efforts: vec!["high".into(), "xhigh".into()],
                            default_effort: Some("high".into()),
                            fast: false,
                            thinking: false,
                            default: true,
                            tiers: Vec::new(),
                            controls: Vec::new(),
                        }];
                        view
                    });
                    cx.new(|_| Capture(conversation))
                })
                .unwrap();
            cx.run_until_parked();
            vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(output.join(format!(
                    "composer-{}-{width}.png",
                    if light { "light" } else { "dark" }
                )))
                .unwrap();
            if width == 360. {
                cx.update_window(window.into(), |root, window, cx| {
                    let conversation = root.downcast::<Capture>().unwrap().read(cx).0.clone();
                    conversation.update(cx, |view, cx| {
                        view.git = Some(vorkspace_core::git::Snapshot::default());
                        view.toggle_menu(ComposerMenu::CompactSettings, window, cx);
                    });
                })
                .unwrap();
                cx.run_until_parked();
                vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(output.join(format!(
                        "composer-menu-{}.png",
                        if light { "light" } else { "dark" }
                    )))
                    .unwrap();
            }
            if width == 260. {
                cx.update_window(window.into(), |root, _, cx| {
                    let conversation = root.downcast::<Capture>().unwrap().read(cx).0.clone();
                    conversation.update(cx, |view, cx| {
                        view.configure_bot(
                            vorkspace_core::bot::Bot {
                                name: "Sam".into(),
                                ..Default::default()
                            },
                            "/private/tmp".into(),
                            Default::default(),
                            cx,
                        );
                        view.bot_ui.queue = true;
                        cx.notify();
                    });
                })
                .unwrap();
                cx.run_until_parked();
                vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(output.join(format!(
                        "bot-composer-{}.png",
                        if light { "light" } else { "dark" }
                    )))
                    .unwrap();
                cx.update_window(window.into(), |root, window, cx| {
                    let conversation = root.downcast::<Capture>().unwrap().read(cx).0.clone();
                    conversation.update(cx, |view, cx| {
                        view.toggle_menu(ComposerMenu::CompactSettings, window, cx)
                    });
                })
                .unwrap();
                cx.run_until_parked();
                vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(output.join(format!(
                        "bot-menu-{}.png",
                        if light { "light" } else { "dark" }
                    )))
                    .unwrap();
            }
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
        }
    }
    vorkspace_ui::theme::configure(&Default::default(), false, true);
}
#[gpui::test]
fn composer_uses_react_surface_and_equal_text_padding_with_attachments(cx: &mut TestAppContext) {
    let preferences = crate::settings::Preferences::default();
    vorkspace_ui::theme::configure(&preferences, false, true);
    cx.update(|cx| {
        cx.set_global(preferences);
        vorkspace_ui::clock::use_test_clock(cx);
    });
    for width in [480., 1000.] {
        let window = window(cx, width);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        for docked in [false, true] {
            for attached in [false, true] {
                window
                    .update(visual, |c, _, cx| {
                        c.summary.has_messages = docked;
                        c.attachments = if attached {
                            vec![std::path::PathBuf::from("/work/notes.txt")]
                        } else {
                            Vec::new()
                        };
                        c.input.update(cx, |input, cx| {
                            input.set_text(
                                "First typed line\nSecond typed line\nThird typed line".into(),
                                cx,
                            );
                        });
                        cx.notify();
                    })
                    .unwrap();
                visual.run_until_parked();
                visual.refresh().unwrap();
                let composer = visual.debug_bounds("composer-box").unwrap();
                let field = visual.debug_bounds("composer-text-field").unwrap();
                let toolbar = visual.debug_bounds("composer-toolbar").unwrap();
                // The column is centred in the pane, not pushed to an edge.
                let left = composer.left();
                let right = px(width) - composer.right();
                assert!(
                    (left - right).abs() <= px(1.),
                    "composer is centred: {left:?} vs {right:?} at {width}px"
                );
                let top = if attached {
                    visual
                        .debug_bounds("composer-attachments")
                        .unwrap()
                        .bottom()
                } else {
                    composer.top() + px(1.)
                };
                // BotComposer's box: 5px around the row plus the field's
                // 4px, and the tools sit under the box, not inside it.
                assert_eq!(field.top() - top, px(9.));
                assert_eq!(composer.bottom() - field.bottom(), px(10.));
                assert_eq!(toolbar.top(), composer.bottom());
                assert_eq!(field.size.height, px(54.));
                window
                    .update(visual, |_, window, _| {
                        let scale = window.scale_factor();
                        let surface = window
                            .painted_quads()
                            .into_iter()
                            .find(|quad| {
                                (quad.corner_radii.top_left.0 / scale - 7.).abs() < 0.1
                                    && quad.background.as_solid() == Some(content(0.03))
                                    && (quad.bounds.size.width.0 / scale
                                        - f32::from(composer.size.width))
                                    .abs()
                                        <= 2.
                            })
                            .expect("the composer paints React's content-3% surface");
                        assert_eq!(
                            surface.border_color,
                            gpui_apple::metal_renderer::backdrop_blur_marker(24. * scale)
                        );
                        assert!(window.painted_quads().iter().any(|quad| {
                            quad.border_color == content(0.2)
                                && (quad.bounds.size.width.0 / scale
                                    - f32::from(composer.size.width))
                                .abs()
                                    < 0.1
                        }));
                    })
                    .unwrap();
            }
        }
    }
}

#[gpui::test]
fn first_submit_moves_the_painted_composer_down_without_resizing_chat(cx: &mut TestAppContext) {
    let window = window(cx, 1000.);
    let from = begin(window, cx);
    let (initial, transcript) = window
        .update(cx, |c, _, cx| {
            (
                c.composer_anchor.get().unwrap(),
                c.transcript.read(cx).list.viewport_bounds(),
            )
        })
        .unwrap();
    assert!(
        (initial.top() - from.top()).abs() < px(1.),
        "the first docked frame must stay at the empty-session slot"
    );
    window
        .update(cx, |c, _, cx| {
            c.composer_motion.elapsed(Duration::from_millis(120));
            cx.notify();
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    let middle = window
        .update(cx, |c, w, cx| {
            let bounds = c.composer_anchor.get().unwrap();
            let painted = w
                .painted_quads()
                .into_iter()
                .find(|q| {
                    (q.corner_radii.top_left.0 / w.scale_factor() - 8.).abs() < 0.1
                        && q.bounds.size.width.0 / w.scale_factor() > 500.
                })
                .expect("the composer box must be painted at its animated position");
            assert!(
                (painted.bounds.top().0 / w.scale_factor() - f32::from(bounds.top())).abs() < 1.
            );
            assert!(c.input.focus_handle(cx).is_focused(w));
            c.composer_motion.elapsed(Duration::from_millis(480));
            cx.notify();
            bounds
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    let docked = window
        .update(cx, |c, _, _| c.composer_anchor.get().unwrap())
        .unwrap();
    assert!(
        middle.top() > from.top() + px(30.),
        "the composer must slide down"
    );
    assert!(
        middle.top() < docked.top() - px(5.),
        "an intermediate frame must not already be docked"
    );
    assert!((docked.bottom() - px(694.)).abs() < px(1.));
    assert_eq!(
        window
            .update(cx, |c, _, cx| c.transcript.read(cx).list.viewport_bounds())
            .unwrap(),
        transcript,
        "composer motion must not relayout the transcript"
    );
}
#[gpui::test]
fn narrow_layout_and_opening_history_do_not_slide(cx: &mut TestAppContext) {
    let narrow = window(cx, 820.);
    narrow
        .update(cx, |c, _, cx| c.set_beside_pane(true, cx))
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    let from = begin(narrow, cx);
    narrow
        .update(cx, |c, _, _| {
            assert!((c.composer_anchor.get().unwrap().top() - from.top()).abs() < px(1.));
        })
        .unwrap();
    let history = window(cx, 1000.);
    let from = history
        .update(cx, |c, _, cx| {
            let from = c.composer_anchor.get().unwrap();
            c.summary.has_messages = true;
            cx.notify();
            from
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    history
        .update(cx, |c, _, _| {
            assert!(c.composer_anchor.get().unwrap().top() > from.top() + px(100.));
        })
        .unwrap();
}
