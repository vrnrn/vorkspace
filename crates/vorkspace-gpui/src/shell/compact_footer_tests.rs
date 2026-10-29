//! Real footer geometry/input regressions and an opt-in native offscreen fixture.
use super::*;
use crate::{
    accounts::Key, popover::AnchorSlot, resource_pressure::ResourceMonitor,
    usage_footer::UsageFooter,
};
use gpui::{TestAppContext, VisualTestContext, WindowHandle, size};
use vorkspace_core::{store::Store, usage::Snapshot};

fn default_key(kind: Kind) -> Key {
    Key::new(kind, Default::default(), None)
}

// Only the disposable database and in-memory account book are used. Provider
// refresh/auth are disabled by their existing cfg(test) paths.
fn accounts(multiple: bool, cx: &mut App) -> Vec<Key> {
    let center = crate::accounts::get(cx).unwrap();
    center.update(cx, |center, cx| {
        let mut keys = vec![default_key(Kind::Codex)];
        if multiple {
            let account = center.book.add(Kind::Codex, "Work").unwrap();
            keys.push(Key::new(Kind::Codex, center.profile(&account.id), None));
            keys.push(default_key(Kind::Claude));
        }
        keys.sort_by_key(|key| {
            (
                if key.kind == Kind::Codex { 0 } else { 1 },
                key.profile.id.clone(),
            )
        });
        center.usage.clear();
        for key in &keys {
            center
                .usage
                .entry(key.clone())
                .or_default()
                .complete(Ok(Snapshot {
                    updated_at: vorkspace_core::usage::now_ms(),
                    ..Default::default()
                }));
        }
        cx.notify();
        keys
    })
}

fn shell(cx: &mut TestAppContext) -> (tempfile::TempDir, WindowHandle<Shell>) {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(Mutex::new(
        Store::open(&directory.path().join("native.sqlite")).unwrap(),
    ));
    cx.update(|cx| {
        crate::shell::bind_keys(cx);
        crate::usage_footer::bind(cx);
    });
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| Shell::new(store, Vec::new(), String::new(), None, cx))
        })
        .unwrap()
    });
    (directory, window)
}

fn settle(visual: &mut VisualTestContext) {
    visual.run_until_parked();
    visual.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
    });
    visual.run_until_parked();
}

fn chrome(
    window: WindowHandle<Shell>,
    visual: &mut VisualTestContext,
    key: &Key,
) -> (AnchorSlot, FocusHandle) {
    window
        .update(visual, |shell, _, cx| {
            shell.usage_footer.read(cx).subscription_chrome(key)
        })
        .unwrap()
}

fn near(a: Pixels, b: Pixels) {
    assert!((a - b).abs() <= px(0.7), "{a:?} != {b:?}");
}

#[gpui::test]
fn compact_footer_centers_and_fits_through_zoom_accounts_and_expansion(cx: &mut TestAppContext) {
    let (_directory, window) = shell(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(size(px(1400.), px(1000.)));
    for multiple in [false, true] {
        let keys = visual.update(|_, cx| accounts(multiple, cx));
        assert_eq!(keys.len(), if multiple { 3 } else { 1 });
        for scale in [75, 100, 150] {
            // Each scale is a fresh fixture, so the real warning's 60-second
            // notice cooldown does not prevent the next scenario's admission.
            window
                .update(visual, |shell, _, cx| {
                    shell.resources = cx.new(|cx| {
                        let mut monitor = ResourceMonitor::new(cx);
                        monitor.set_classic(true, cx);
                        monitor
                    });
                })
                .unwrap();
            for shown in [false, true, false] {
                window
                    .update(visual, |shell, _, cx| {
                        shell.workspace.preferences.ui_scale = scale;
                        shell.workspace.project_rail_open = false;
                        shell.workspace.preferences.collapsed_project_rail = "compact".into();
                        shell.resources.update(cx, |monitor, cx| {
                            if shown {
                                monitor.saturate(cx);
                            } else {
                                monitor.recover(cx);
                            }
                        });
                        shell.invalidate_chrome(cx);
                        cx.notify();
                    })
                    .unwrap();
                settle(visual);
                let rail = visual.debug_bounds("classic-project-rail").unwrap();
                let settings = visual.debug_bounds("classic-rail-settings").unwrap();
                let center = rail.center().x;
                near(settings.center().x, center);
                near(settings.size.width, px(32.));
                near(settings.size.height, px(32.));
                let warning = visual.debug_bounds("resource-pressure-chip");
                assert_eq!(warning.is_some(), shown);
                let mut controls = warning.into_iter().collect::<Vec<_>>();
                controls.extend(
                    keys.iter()
                        .map(|key| chrome(window, visual, key).0.get().unwrap()),
                );
                controls.push(settings);
                for (ix, bounds) in controls.iter().enumerate() {
                    near(bounds.center().x, center);
                    near(bounds.size.width, px(32.));
                    near(bounds.size.height, px(32.));
                    assert!(bounds.left() >= rail.left() && bounds.right() <= rail.right());
                    if ix > 0 {
                        assert!(controls[ix - 1].bottom() + px(3.) <= bounds.top());
                    }
                }
                let compact_anchor = chrome(window, visual, &keys[0]).0;
                let compact_bounds = compact_anchor.get().unwrap();
                window
                    .update(visual, |shell, _, cx| shell.toggle_project_rail(cx))
                    .unwrap();
                settle(visual);
                let expanded = chrome(window, visual, &keys[0]).0.get().unwrap();
                assert!(expanded.size.width > px(100.) && expanded.size.height > px(32.));
                if shown {
                    assert!(
                        visual
                            .debug_bounds("resource-pressure-chip")
                            .unwrap()
                            .size
                            .width
                            > px(32.)
                    );
                }
                window
                    .update(visual, |shell, _, cx| shell.toggle_project_rail(cx))
                    .unwrap();
                settle(visual);
                near(
                    compact_anchor.get().unwrap().center().x,
                    compact_bounds.center().x,
                );
                near(
                    compact_anchor.get().unwrap().size.width,
                    compact_bounds.size.width,
                );
            }
        }
        let settings = visual.debug_bounds("classic-rail-settings").unwrap();
        for visible in [false, true] {
            window
                .update(visual, |shell, _, cx| {
                    shell.workspace.sidebar_subscriptions_visible = visible;
                    shell.invalidate_chrome(cx);
                    cx.notify();
                })
                .unwrap();
            settle(visual);
            assert_eq!(visual.debug_bounds("subscriptions").is_some(), visible);
            near(
                visual
                    .debug_bounds("classic-rail-settings")
                    .unwrap()
                    .center()
                    .x,
                settings.center().x,
            );
        }
    }
}

fn activate(visual: &mut VisualTestContext, key: &str) {
    let keystroke = gpui::Keystroke::parse(key).unwrap();
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    visual.simulate_event(gpui::KeyUpEvent { keystroke });
    settle(visual);
}

#[gpui::test]
fn compact_footer_keeps_click_keyboard_focus_and_live_popover_anchor(cx: &mut TestAppContext) {
    let (_directory, window) = shell(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let keys = visual.update(|_, cx| accounts(true, cx));
    window
        .update(visual, |shell, _, cx| {
            shell.workspace.project_rail_open = false;
            shell
                .resources
                .update(cx, |monitor, cx| monitor.saturate(cx));
            shell.invalidate_chrome(cx);
            cx.notify();
        })
        .unwrap();
    visual.simulate_resize(size(px(1400.), px(1000.)));
    settle(visual);
    let warning = visual.debug_bounds("resource-pressure-chip").unwrap();
    visual.simulate_click(warning.center(), Default::default());
    settle(visual);
    window
        .update(visual, |shell, window, cx| {
            assert!(shell.resources.read(cx).is_open());
            shell
                .resources
                .update(cx, |monitor, cx| monitor.close(window, cx));
        })
        .unwrap();
    settle(visual);
    activate(visual, "space");
    window
        .update(visual, |shell, window, cx| {
            assert!(shell.resources.read(cx).is_open());
            shell
                .resources
                .update(cx, |monitor, cx| monitor.close(window, cx));
        })
        .unwrap();
    settle(visual);
    for key in &keys {
        let (anchor, trigger) = chrome(window, visual, key);
        visual.simulate_click(anchor.get().unwrap().center(), Default::default());
        settle(visual);
        window
            .update(visual, |shell, _, cx| {
                assert_eq!(
                    shell.usage_footer.read(cx).open_subscription_key(),
                    Some(key)
                )
            })
            .unwrap();
        activate(visual, "escape");
        window
            .update(visual, |_, window, _| assert!(trigger.is_focused(window)))
            .unwrap();
        for activation in ["enter", "space"] {
            activate(visual, activation);
            window
                .update(visual, |shell, _, cx| {
                    assert_eq!(
                        shell.usage_footer.read(cx).open_subscription_key(),
                        Some(key)
                    )
                })
                .unwrap();
            activate(visual, "escape");
        }
        // Keep an account's popover open while the very same anchor slot is
        // relaid out for expansion, then collapse and restore keyboard focus.
        activate(visual, "enter");
        window
            .update(visual, |shell, _, cx| shell.toggle_project_rail(cx))
            .unwrap();
        settle(visual);
        assert!(anchor.get().unwrap().size.width > px(100.));
        window
            .update(visual, |shell, _, cx| shell.toggle_project_rail(cx))
            .unwrap();
        settle(visual);
        near(anchor.get().unwrap().size.width, px(32.));
        window
            .update(visual, |shell, _, cx| {
                assert_eq!(
                    shell.usage_footer.read(cx).open_subscription_key(),
                    Some(key)
                )
            })
            .unwrap();
        activate(visual, "escape");
        window
            .update(visual, |_, window, _| assert!(trigger.is_focused(window)))
            .unwrap();
    }
    // The last subscription hands Tab focus to Settings; Enter keeps its
    // existing action, rather than introducing a second footer event path.
    activate(visual, "tab");
    activate(visual, "enter");
    window
        .update(visual, |shell, _, _| assert!(shell.settings.is_some()))
        .unwrap();
}

#[test]
#[ignore = "Requires Metal and VOKKY_COMPACT_FOOTER_CAPTURE_DIR"]
fn native_compact_footer_capture() {
    struct Surface {
        resources: Entity<ResourceMonitor>,
        usage: Entity<UsageFooter>,
    }
    impl Render for Surface {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .font_family(crate::fonts::UI)
                .bg(rgb(CHAT))
                .child(
                    div()
                        .w(px(COMPACT_WIDTH))
                        .h_full()
                        .pr_px()
                        .bg(shell_fill())
                        .flex()
                        .flex_col()
                        .justify_end()
                        .child(compact_footer(
                            self.resources.clone(),
                            Some(self.usage.clone()),
                            compact_settings_button(),
                        )),
                )
        }
    }
    let output =
        std::path::PathBuf::from(std::env::var_os("VOKKY_COMPACT_FOOTER_CAPTURE_DIR").unwrap());
    assert!(output.is_absolute() && output.is_dir());
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(Mutex::new(
        Store::open(&directory.path().join("native.sqlite")).unwrap(),
    ));
    let mut cx = gpui::HeadlessAppContext::with_platform(
        Arc::new(gpui_apple::AppleTextSystem::new()),
        Arc::new(crate::assets::Assets),
        || {
            Ok(Some(Box::new(
                gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
            )))
        },
    );
    cx.update(|cx| {
        crate::clock::use_test_clock(cx);
        crate::accounts::install(store, cx);
    });
    for multiple in [false, true] {
        cx.update(|cx| {
            accounts(multiple, cx);
        });
        for scale in [75, 100, 150] {
            for shown in [true, false] {
                let window = cx
                    .open_window(size(px(100.), px(360.)), |window, cx| {
                        window.set_ui_scale(scale as f32 / 100., cx);
                        let resources = cx.new(|cx| {
                            let mut monitor = ResourceMonitor::new(cx);
                            monitor.set_classic(true, cx);
                            monitor.set_compact(true, cx);
                            if shown {
                                monitor.saturate(cx);
                            }
                            monitor
                        });
                        let usage = cx.new(|cx| {
                            let mut usage = UsageFooter::new(Kind::Codex, cx);
                            usage.set_compact(true, cx);
                            usage
                        });
                        cx.new(|_| Surface { resources, usage })
                    })
                    .unwrap();
                cx.run_until_parked();
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(output.join(format!(
                        "compact-{}-{}-{scale}.png",
                        if multiple { "multiple" } else { "one" },
                        if shown { "warning" } else { "healthy" },
                    )))
                    .unwrap();
                cx.update_window(window.into(), |_, window, _| window.remove_window())
                    .unwrap();
            }
        }
    }
}

#[test]
#[ignore = "Requires Metal and VOKKY_SIDEBAR_USAGE_CAPTURE_DIR"]
fn native_sidebar_usage_capture() {
    struct Surface {
        usage: Entity<UsageFooter>,
        compact: bool,
    }
    impl Render for Surface {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .bg(rgb(CHAT))
                .font_family(crate::fonts::UI)
                .child(
                    div()
                        .w(px(if self.compact { COMPACT_WIDTH } else { 240. }))
                        .h_full()
                        .bg(shell_fill())
                        .flex()
                        .flex_col()
                        .justify_end()
                        .when(!self.compact, |d| d.px_2())
                        .child(self.usage.clone()),
                )
        }
    }
    let output =
        std::path::PathBuf::from(std::env::var_os("VOKKY_SIDEBAR_USAGE_CAPTURE_DIR").unwrap());
    assert!(output.is_absolute() && output.is_dir());
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(Mutex::new(
        Store::open(&directory.path().join("native.sqlite")).unwrap(),
    ));
    let mut cx = gpui::HeadlessAppContext::with_platform(
        Arc::new(gpui_apple::AppleTextSystem::new()),
        Arc::new(crate::assets::Assets),
        || {
            Ok(Some(Box::new(
                gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
            )))
        },
    );
    cx.update(|cx| {
        crate::clock::use_test_clock(cx);
        crate::accounts::install(store, cx);
        let center = crate::accounts::get(cx).unwrap();
        center.update(cx, |center, cx| {
            center.usage.clear();
            let now = vorkspace_core::usage::now_ms();
            for (kind, weekly) in [(Kind::Codex, 88.), (Kind::Claude, 31.)] {
                center
                    .usage
                    .entry(default_key(kind))
                    .or_default()
                    .complete(Ok(Snapshot {
                        updated_at: now,
                        windows: vec![
                            vorkspace_core::usage::Window {
                                minutes: 300,
                                used_percent: 0.,
                                resets_at: None,
                            },
                            vorkspace_core::usage::Window {
                                minutes: 10_080,
                                used_percent: weekly,
                                resets_at: Some(now + 172_800_000),
                            },
                        ],
                        ..Default::default()
                    }));
            }
            cx.notify();
        });
    });
    for compact in [false, true] {
        let window = cx
            .open_window(size(px(280.), px(300.)), |_, cx| {
                let usage = cx.new(|cx| {
                    let mut usage = UsageFooter::new(Kind::Codex, cx);
                    usage.set_compact(compact, cx);
                    usage
                });
                cx.new(|_| Surface { usage, compact })
            })
            .unwrap();
        cx.run_until_parked();
        cx.capture_screenshot(window.into())
            .unwrap()
            .save(output.join(if compact {
                "usage-compact.png"
            } else {
                "usage-expanded.png"
            }))
            .unwrap();
        cx.update_window(window.into(), |_, window, _| window.remove_window())
            .unwrap();
    }
}
