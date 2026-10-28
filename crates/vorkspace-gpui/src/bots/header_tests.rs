use super::*;
use gpui::{ScrollDelta, ScrollWheelEvent, TestAppContext, VisualTestContext, point, size};
use std::sync::{Arc, Mutex};
use vorkspace_core::store::Store;

// Exercise the real Bots -> Conversation observer and layout, without creating
// Bot files, starting a provider, or opening an installed application.
#[gpui::test]
fn bot_header_updates_from_chat_status_and_preserves_composer_and_layout(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(Mutex::new(
        Store::open(&directory.path().join("header.sqlite")).unwrap(),
    ));
    let cwd = directory.path().to_string_lossy().to_string();
    let session = locked(&store).unwrap().create("codex", &cwd).unwrap().id;
    let window = cx.update(|cx| {
        crate::clock::use_test_clock(cx);
        input::bind(cx);
        cx.open_window(Default::default(), |window, cx| {
            window.activate_window();
            cx.new(|cx| Shell::new(store, vec![], cwd, None, cx))
        })
        .unwrap()
    });
    cx.run_until_parked();
    let manager = window
        .update(cx, |shell, _, cx| {
            let manager = shell.bot_manager();
            manager.update(cx, |bots, cx| {
                bots.roster.bots.push(Bot {
                    id: "header-fixture".into(),
                    name: "Sam".into(),
                    session_id: Some(session),
                    ..Default::default()
                });
                bots.active = Some("header-fixture".into());
                bots.ensure_chat("header-fixture", cx);
                cx.notify();
            });
            cx.notify();
            manager
        })
        .unwrap();
    cx.run_until_parked();
    let chat = manager.read_with(cx, |bots, _| bots.chat().unwrap());
    chat.update(cx, |chat, cx| {
        chat.bot_post("user", "Keep my place in this conversation.".into(), cx)
            .unwrap();
        chat.bot_post("assistant", "A retained reply.\n\n".repeat(40), cx)
            .unwrap();
        // A short final turn keeps a signature in the virtualized viewport;
        // the long preceding answer supplies actual scrollable history.
        chat.bot_post("user", "And keep my draft.".into(), cx)
            .unwrap();
        chat.bot_post("assistant", "This reply keeps its signature.".into(), cx)
            .unwrap();
    });
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    for width in [900., 560.] {
        visual.simulate_resize(size(px(width), px(680.)));
        visual.run_until_parked();
        visual.refresh().unwrap();
        let header = visual.debug_bounds("bot-chat-header").unwrap();
        let face = visual.debug_bounds("bot-chat-header-idle").unwrap();
        let name = visual.debug_bounds("bot-chat-header-name").unwrap();
        let scrim = visual.debug_bounds("bot-chat-mask-end").unwrap();
        let transcript = visual.debug_bounds("bot-chat-transcript").unwrap();
        let composer = visual.debug_bounds("bot-composer").unwrap();
        assert_eq!(header.size.height, px(header::HEIGHT));
        assert_eq!(face.center().x, header.center().x);
        assert_eq!(name.center().x, header.center().x);
        assert!(header.top() >= px(TITLE_BAR_HEIGHT));
        assert_eq!(face.top(), transcript.top() + px(8.));
        assert_eq!(face.size, size(px(64.), px(64.)));
        assert!(face.bottom() <= name.top() && name.bottom() <= header.bottom());
        assert_eq!(header.top(), transcript.top());
        assert_eq!(header.size.width, transcript.size.width);
        assert!(scrim.size.width <= px(256.) && scrim.size.width < transcript.size.width);
        assert_eq!(scrim.center().x, face.center().x);
        assert!(transcript.bottom() <= composer.top());

        // Selection and draft focus belong to the retained Conversation; a
        // decorative header has no hitbox and cannot consume composer clicks.
        visual.simulate_click(composer.center(), Default::default());
        visual.simulate_input("Unsent header draft");
        visual.run_until_parked();
        // Typing can legitimately wrap the retained draft at narrow widths.
        // Compare status transitions against the resulting composer geometry.
        let composer = visual.debug_bounds("bot-composer").unwrap();
        let draft = chat.read_with(visual, |chat, cx| chat.input.read(cx).content.clone());
        let rows = chat.read_with(visual, |chat, cx| chat.bot_rows(cx).to_vec());
        let signature = visual.debug_bounds("bot-signature-pill");
        visual.simulate_event(ScrollWheelEvent {
            position: face.center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(120.))),
            ..Default::default()
        });
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds("bot-chat-header").unwrap(), header);

        // Notify only the Conversation, as real turns do. Bots' existing
        // observation must refresh idle -> working -> attention -> working -> idle.
        for (busy, attention, selector) in [
            (true, false, "bot-chat-header-working"),
            (true, true, "bot-chat-header-needs-you"),
            (true, false, "bot-chat-header-working"),
            (false, false, "bot-chat-header-idle"),
        ] {
            chat.update(visual, |chat, cx| {
                chat.busy = busy;
                chat.settings.usage_limit = attention.then(Default::default);
                cx.notify();
            });
            visual.run_until_parked();
            assert!(visual.debug_bounds(selector).is_some());
            assert_eq!(visual.debug_bounds("bot-chat-header").unwrap(), header);
            assert_eq!(visual.debug_bounds("bot-composer").unwrap(), composer);
            window
                .update(visual, |_, window, cx| {
                    let chat = chat.read(cx);
                    assert_eq!(chat.input.read(cx).content, draft);
                    assert!(chat.input.focus_handle(cx).contains_focused(window, cx));
                    assert_eq!(chat.bot_rows(cx), rows);
                })
                .unwrap();
        }
        assert!(signature.is_some(), "message signatures remain present");
        // The Channels and settings buttons no longer sit over the chat.
        assert!(visual.debug_bounds("bot-settings").is_none());
    }

    // In short panes existing attention notices may reflow/shrink the composer.
    // The identity's contract is to remain clipped to the transcript in every
    // state, rather than impose a new composer geometry on those controls.
    visual.simulate_resize(size(px(560.), px(240.)));
    for (busy, attention) in [(false, false), (true, false), (true, true)] {
        chat.update(visual, |chat, cx| {
            chat.busy = busy;
            chat.settings.usage_limit = attention.then(Default::default);
            cx.notify();
        });
        visual.run_until_parked();
        visual.refresh().unwrap();
        let transcript = visual.debug_bounds("bot-chat-transcript").unwrap();
        let composer = visual.debug_bounds("bot-composer").unwrap();
        let header = visual.debug_bounds("bot-chat-header").unwrap();
        assert_eq!(header.top(), transcript.top());
        assert!(transcript.bottom() <= composer.top());
    }
}

#[gpui::test]
fn bot_header_passes_selection_clicks_and_scroll_to_overlapped_chat(cx: &mut TestAppContext) {
    struct Surface {
        input: Entity<Input>,
        scroll: gpui::ScrollHandle,
        clicks: usize,
        decoration: Option<bool>,
        header: Option<Entity<header::ChatHeader>>,
    }
    impl gpui::Render for Surface {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(header::chat_surface(
                    div()
                        .id("header-underlay-scroll")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .child(
                            div()
                                .h(px(1000.))
                                .relative()
                                .child(
                                    div()
                                        .debug_selector(|| "header-underlay-input".into())
                                        .absolute()
                                        .top(px(32.))
                                        .left(px(30.))
                                        .w(px(300.))
                                        .child(self.input.clone()),
                                )
                                .child(
                                    div()
                                        .id("header-underlay-control")
                                        .absolute()
                                        .top(px(74.))
                                        .w_full()
                                        .h(px(22.))
                                        .on_click(cx.listener(|this, _, _, _| this.clicks += 1)),
                                ),
                        )
                        .into_any_element(),
                    &Bot::default(),
                    "idle",
                    self.decoration,
                    &mut self.header,
                    cx,
                ))
        }
    }
    let window = cx.update(|cx| {
        crate::clock::use_test_clock(cx);
        input::bind(cx);
        cx.open_window(Default::default(), |window, cx| {
            window.activate_window();
            let input = cx.new(Input::new);
            input.update(cx, |input, cx| {
                input.set_text(
                    "Selectable transcript text underneath the floating Bot face.".into(),
                    cx,
                )
            });
            cx.new(|_| Surface {
                input,
                scroll: Default::default(),
                clicks: 0,
                decoration: Some(false),
                header: None,
            })
        })
        .unwrap()
    });
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(size(px(360.), px(240.)));
    visual.run_until_parked();
    visual.refresh().unwrap();
    let face = visual.debug_bounds("bot-chat-header-idle").unwrap();
    let name = visual.debug_bounds("bot-chat-header-name").unwrap();
    let viewport = visual.debug_bounds("bot-chat-transcript").unwrap();
    let field = visual.debug_bounds("header-underlay-input").unwrap();
    // Compare the actual shared layout with and without decoration: neither
    // the viewport nor the interactive content may acquire a reserved band.
    for decoration in [None, Some(false)] {
        window
            .update(visual, |surface, _, cx| {
                surface.decoration = decoration;
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert_eq!(
            visual.debug_bounds("bot-chat-transcript").unwrap(),
            viewport
        );
        assert_eq!(visual.debug_bounds("header-underlay-input").unwrap(), field);
    }
    let start = face.center() - point(px(12.), px(0.));
    let end = face.center() + point(px(12.), px(0.));
    visual.simulate_mouse_down(start, MouseButton::Left, Default::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Default::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Default::default());
    window
        .update(visual, |surface, window, cx| {
            let input = surface.input.read(cx);
            assert!(input.focus_handle(cx).contains_focused(window, cx));
            assert!(
                !input.selection_utf16().is_empty(),
                "selection drag passes through the face"
            );
        })
        .unwrap();
    visual.simulate_click(name.center(), Default::default());
    window
        .update(visual, |surface, _, _| {
            assert_eq!(
                surface.clicks, 1,
                "click passes through the plain name label"
            );
            assert_eq!(surface.scroll.offset().y, px(0.));
        })
        .unwrap();
    visual.simulate_event(ScrollWheelEvent {
        position: face.center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(-120.))),
        ..Default::default()
    });
    visual.run_until_parked();
    window
        .update(visual, |surface, _, _| {
            assert!(
                surface.scroll.offset().y < px(0.),
                "wheel reaches the overlapped scroll view"
            );
        })
        .unwrap();
    assert_eq!(visual.debug_bounds("bot-chat-header-idle").unwrap(), face);
}

#[test]
#[ignore = "Requires Metal and VOKKY_BOT_HEADER_CAPTURE_DIR"]
fn native_bot_header_three_state_capture() {
    struct Surface {
        bot: Bot,
        status: &'static str,
        light: bool,
        transcript: Entity<crate::transcript::TranscriptView>,
        input: Entity<Input>,
        header: Option<Entity<header::ChatHeader>>,
    }
    impl gpui::Render for Surface {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .font_family(crate::fonts::UI)
                .text_color(content(1.))
                .bg(rgb(if self.light { 0xf3dce8 } else { 0x45243c }))
                .relative()
                .flex()
                .flex_col()
                .child({
                    let light = self.light;
                    gpui::canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| {
                            // Diagnostic wallpaper stays visible through the cutout.
                            let color = rgb(if light { 0xc372a5 } else { 0xbc629b }).opacity(0.2);
                            for x in (0..bounds.size.width.as_f32().ceil() as usize).step_by(8) {
                                window.paint_quad(gpui::fill(
                                    gpui::Bounds::new(
                                        point(bounds.left() + px(x as f32), bounds.top()),
                                        size(px(1.), bounds.size.height),
                                    ),
                                    color,
                                ));
                            }
                            for y in (0..bounds.size.height.as_f32().ceil() as usize).step_by(8) {
                                window.paint_quad(gpui::fill(
                                    gpui::Bounds::new(
                                        point(bounds.left(), bounds.top() + px(y as f32)),
                                        size(bounds.size.width, px(1.)),
                                    ),
                                    color,
                                ));
                            }
                        },
                    )
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                })
                .child(header::chat_surface(
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(self.transcript.clone())
                        .into_any_element(),
                    &self.bot,
                    self.status,
                    Some(true),
                    &mut self.header,
                    cx,
                ))
                .child(
                    div()
                        .mx_3()
                        .mb_3()
                        .p_3()
                        .rounded_lg()
                        .flex_shrink_0()
                        .bg(content(0.03))
                        .child(self.input.clone()),
                )
        }
    }
    let output =
        std::path::PathBuf::from(std::env::var_os("VOKKY_BOT_HEADER_CAPTURE_DIR").unwrap());
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
        let theme = if light { "light" } else { "dark" };
        crate::theme::configure(
            &crate::settings::Preferences {
                theme: theme.into(),
                ..Default::default()
            },
            false,
            true,
        );
        for status in ["idle", "working", "needs-you"] {
            for (width, height) in [(900., 600.), (360., 600.), (360., 140.)] {
                let window = cx
                .open_window(size(px(width), px(height)), |_, cx| {
                    let bot = Bot {
                        id: "header-capture".into(),
                        name: "Sam".into(),
                        mascot: "cat".into(),
                        color: 0x9b66dc,
                        ..Default::default()
                    };
                    let transcript = cx.new(|cx| {
                        let mut view = crate::transcript::TranscriptView::new();
                        view.bot_mode_with_look(avatar::BotLookConfig::from_bot(&bot), &bot.label());
                        view.install_prepared_history(
                            crate::transcript::PreparedHistory::new(
                                vec![
                                    vorkspace_core::transcript::Row {
                                        role: "user".into(),
                                        text: "Keep the conversation easy to read.".into(),
                                        first: true,
                                        sent_at: Some(1_000),
                                    },
                                    vorkspace_core::transcript::Row {
                                        role: "assistant".into(),
                                        text: "The face stays fixed while messages scroll behind it. Selection, links and message actions stay usable.\n\n".repeat(12),
                                        first: true,
                                        sent_at: Some(2_000),
                                    },
                                ],
                                true,
                            ),
                            cx,
                        );
                        view
                    });
                    let input = cx.new(Input::new);
                    cx.new(|_| Surface { bot, status, light, transcript, input, header: None })
                })
                .unwrap();
                cx.run_until_parked();
                crate::clock::settle_headless_animations(window, &mut cx);
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(output.join(format!("{status}-{theme}-{width:.0}x{height:.0}.png")))
                    .unwrap();
                cx.update_window(window.into(), |_, window, _| window.remove_window())
                    .unwrap();
            }
        }
    }
    crate::theme::configure(&crate::settings::Preferences::default(), false, true);
}
