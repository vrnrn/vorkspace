//! Rendered GPUI geometry and input regressions. These use the test platform,
//! not an AppKit window or a screenshot of the installed application.
use super::*;
use gpui::{TestAppContext, VisualTestContext, WindowHandle, size};

const SCALES: [u32; 7] = [50, 75, 90, 100, 125, 150, 200];

fn settle(visual: &mut VisualTestContext) {
    visual.run_until_parked();
    // GPUI deliberately defers zoom requested during Render until the next
    // platform frame. Deliver that frame explicitly; don't use wall-clock sleeps.
    visual.update(|w, cx| {
        w.simulate_next_frame(cx);
        w.draw(cx).clear(cx);
    });
    visual.run_until_parked();
}

fn activate(visual: &mut VisualTestContext, key: &str) {
    // simulate_keystrokes sends only KeyDown; native keyboard click activation
    // requires a matching KeyUp on the same focused control.
    let keystroke = gpui::Keystroke::parse(key).unwrap();
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    visual.simulate_event(gpui::KeyUpEvent { keystroke });
}

fn platform_bounds(
    window: WindowHandle<Shell>,
    visual: &mut VisualTestContext,
    selector: &'static str,
) -> Bounds<Pixels> {
    let bounds = visual.debug_bounds(selector).expect(selector);
    window
        .update(visual, |_, w, _| w.logical_to_platform_bounds(bounds))
        .unwrap()
}

fn assert_titlebar(
    window: WindowHandle<Shell>,
    visual: &mut VisualTestContext,
    percent: u32,
    fullscreen: bool,
) {
    let scale = percent as f32 / 100.;
    let toggle = platform_bounds(window, visual, "toggle-sidebar");
    let back = platform_bounds(window, visual, "visit-back");
    let forward = platform_bounds(window, visual, "visit-forward");
    let nav = platform_bounds(window, visual, "titlebar-navigation");
    // Hit areas snap to device pixels. At 90%, for example, a 21.6pt
    // nominal width becomes 21.5pt on the test platform's 2x display.
    let pixel = window
        .update(visual, |_, w, _| 1. / w.platform_scale_factor())
        .unwrap();
    let near = |a: Pixels, b: f32| assert!((a.as_f32() - b).abs() <= pixel, "{a:?} != {b}");
    let native = cfg!(target_os = "macos") && !fullscreen;
    let safe = if native { 78. } else { 10. * scale };
    near(toggle.left(), safe);
    for button in [toggle, back, forward] {
        near(button.size.width, 24. * scale);
        near(button.size.height, 24. * scale);
        near(button.center().y, 20. * scale);
        if native {
            assert!(button.left() >= px(66. + 12. - 0.02));
        }
    }
    assert!(toggle.right() <= back.left() + px(0.02));
    assert!(back.right() <= forward.left() + px(0.02));
    assert!(
        forward.right() <= nav.right() + px(0.02),
        "{percent}% fullscreen={fullscreen}: forward={forward:?} nav={nav:?}"
    );
    assert!(visual.debug_bounds("toggle-project-rail").is_none());
    if visual
        .debug_bounds("classic-project-rail-divider")
        .is_some()
    {
        let divider = platform_bounds(window, visual, "classic-project-rail-divider");
        near(divider.top(), 40. * scale);
        if native {
            assert!(divider.top() >= px(20. * scale + 7.));
        }
    }
    let tabs = platform_bounds(window, visual, "title-tab-strip");
    assert!(
        tabs.left() + px(pixel) >= nav.right(),
        "{tabs:?} overlaps {nav:?}"
    );
    // The strip's background can snap a device pixel across the column edge.
    // Its actual tab/new-tab hit areas must stay entirely clear of navigation.
    let selectors = window
        .update(visual, |shell, _, cx| {
            shell
                .visible_tabs(cx)
                .into_iter()
                .map(|tab| match tab {
                    TitleTab::Session(id) => format!("title-tab-{id}"),
                    TitleTab::Bot { id, .. } => format!("bot-title-tab-{id}"),
                    TitleTab::App(view) => format!("app-title-tab-{}", view.label()),
                    TitleTab::Search => "search-title-tab".into(),
                })
                .chain(std::iter::once("new-tab".into()))
                .collect::<Vec<_>>()
        })
        .unwrap();
    for selector in selectors {
        let bounds = platform_bounds(window, visual, Box::leak(selector.into_boxed_str()));
        assert!(
            bounds.left() + px(0.02) >= nav.right(),
            "tab hit area {bounds:?} overlaps {nav:?}"
        );
    }
    window
        .update(visual, |shell, w, _| {
            assert!((w.ui_scale() - scale).abs() < f32::EPSILON);
            assert_eq!(shell.fullscreen, fullscreen);
            // In addition to the divider's layout, inspect painted borders: a
            // full-height rail border must not reappear through native controls.
            if native {
                let dpi = w.platform_scale_factor();
                assert!(!w.painted_quads().iter().any(|quad| {
                    let x = quad.bounds.right().0 / dpi;
                    quad.border_widths.right.0 > 0.
                        && quad.border_color.a > 0.
                        && (12. ..=66.).contains(&x)
                        && quad.bounds.top().0 / dpi < 20. * scale + 7.
                        && quad.bounds.bottom().0 / dpi > 20. * scale - 7.
                }));
            }
        })
        .unwrap();
}

#[gpui::test]
fn zoomed_titlebar_keeps_native_controls_navigation_and_tabs_separate(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let (window, _, _) = tests::window(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    // A minimum-width rail exercises navigation's padding budget at low zoom;
    // a wider rail keeps history at the expanded rail's edge.
    for dpi in [1., 2.] {
        visual.simulate_scale_factor_change(dpi);
        for width in [160., 300.] {
            for percent in SCALES {
                for mode in ["expanded", "compact", "hidden"] {
                    window
                        .update(visual, |shell, _, cx| {
                            shell.workspace.project_rail_width = width;
                            shell.workspace.project_rail_open = mode == "expanded";
                            shell.workspace.preferences.collapsed_project_rail =
                                if mode == "hidden" {
                                    "hidden"
                                } else {
                                    "compact"
                                }
                                .into();
                            shell.workspace.preferences.ui_scale = percent;
                            shell.invalidate_chrome(cx);
                            cx.notify();
                        })
                        .unwrap();
                    for fullscreen in [false, true, false] {
                        window
                            .update(visual, |_, w, _| {
                                if w.is_fullscreen() != fullscreen {
                                    w.toggle_fullscreen();
                                }
                                w.refresh();
                            })
                            .unwrap();
                        settle(visual);
                        assert_titlebar(window, visual, percent, fullscreen);
                        if mode == "hidden" {
                            assert!(
                                visual
                                    .debug_bounds("classic-project-rail-divider")
                                    .is_none()
                            );
                        }
                    }
                }
            }
        }
    }
}

// Workspace restoration selects the classic body. Render the retained inset
// titlebar branch directly so its inline navigation uses the same scale rule.
struct InsetTitlebar(Entity<Shell>);
impl Render for InsetTitlebar {
    fn render(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.0.update(cx, |shell, cx| {
            shell.fullscreen = w.is_fullscreen();
            shell.titlebar(w, cx)
        })
    }
}

#[gpui::test]
fn inset_titlebar_uses_the_same_native_inset_and_navigation_budget(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let (window, _, _) = tests::window(cx);
    let shell = window
        .update(cx, |_, w, cx| {
            let shell = cx.entity();
            w.replace_root(cx, |_, _| InsetTitlebar(shell.clone()));
            shell
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    for percent in SCALES {
        for mode in ["expanded", "compact", "hidden"] {
            for fullscreen in [false, true, false] {
                visual.update(|w, cx| {
                    w.set_ui_scale(percent as f32 / 100., cx);
                    if w.is_fullscreen() != fullscreen {
                        w.toggle_fullscreen();
                    }
                    shell.update(cx, |shell, cx| {
                        shell.workspace.classic_layout = false;
                        shell.workspace.project_rail_width = 160.;
                        shell.workspace.project_rail_open = mode == "expanded";
                        shell.workspace.preferences.collapsed_project_rail = if mode == "hidden" {
                            "hidden"
                        } else {
                            "compact"
                        }
                        .into();
                        shell.invalidate_chrome(cx);
                        cx.notify();
                    });
                    w.refresh();
                });
                settle(visual);
                let nav = visual.debug_bounds("titlebar-navigation").unwrap();
                let toggle = visual.debug_bounds("toggle-sidebar").unwrap();
                let tabs = visual.debug_bounds("title-tab-strip").unwrap();
                let forward = visual.debug_bounds("visit-forward").unwrap();
                let pixel = visual.update(|w, _| 1. / w.scale_factor());
                assert!(tabs.left() >= nav.right() - px(pixel));
                assert!(forward.right() <= nav.right() + px(0.02));
                let new_tab = visual.debug_bounds("new-tab").unwrap();
                assert!(new_tab.left() >= nav.right());
                visual.update(|w, _| {
                    let toggle = w.logical_to_platform_bounds(toggle);
                    let expected = if cfg!(target_os = "macos") && !fullscreen {
                        78.
                    } else {
                        10. * w.ui_scale()
                    };
                    assert!((toggle.left().as_f32() - expected).abs() < 0.02);
                    assert!(
                        (toggle.center().y.as_f32() - 20. * w.ui_scale()).abs()
                            <= 1. / w.platform_scale_factor()
                    );
                });
            }
        }
    }
}

#[gpui::test]
fn titlebar_bot_and_app_views_keep_the_same_zoomed_navigation(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let (window, _, _) = tests::window(cx);
    let (manager, bot) = window
        .update(cx, |shell, w, cx| {
            let manager = shell.bot_manager();
            let bot = manager.update(cx, |manager, cx| {
                manager.create_named(Vec::new(), w, cx);
                manager.dismiss_settings(w, cx);
                manager.active.clone().unwrap()
            });
            (manager, bot)
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    for percent in SCALES {
        for bot_view in [true, false] {
            for mode in ["expanded", "compact", "hidden"] {
                window
                    .update(visual, |shell, w, cx| {
                        if bot_view {
                            shell.leave_app_views(cx);
                            manager.update(cx, |m, cx| m.open(&bot, w, cx));
                        } else {
                            shell.open_notes(w, cx);
                        }
                        shell.workspace.preferences.ui_scale = percent;
                        shell.workspace.project_rail_width = 160.;
                        shell.workspace.project_rail_open = mode == "expanded";
                        shell.workspace.preferences.collapsed_project_rail = if mode == "hidden" {
                            "hidden"
                        } else {
                            "compact"
                        }
                        .into();
                        shell.invalidate_chrome(cx);
                        cx.notify();
                    })
                    .unwrap();
                settle(visual);
                assert_titlebar(window, visual, percent, false);
            }
        }
    }
}

#[gpui::test]
fn titlebar_zoom_paths_restore_and_keep_real_controls_live(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_reduce_motion(true);
        bind_keys(cx);
        crate::app_menu::init(cx);
    });
    let (window, a, b) = tests::window_with_scale(cx, 75);
    window
        .update(cx, |shell, _, cx| {
            // Title tabs are scoped to the current project. Keep both test tabs in
            // that scope while exercising actual tab and history clicks.
            shell.sessions[&a].update(cx, |session, cx| {
                session.summary.cwd = "/work/vorkspace".into();
                cx.emit(SessionChanged);
            });
            shell.invalidate_chrome(cx);
            cx.notify();
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    // Keep both session tabs visible at 200% while clicking them. The separate
    // geometry matrix exercises narrow rails, including zero-width hidden tabs.
    visual.simulate_resize(size(px(1800.), px(1000.)));
    settle(visual);
    assert_titlebar(window, visual, 75, false);
    let modifier = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    for percent in SCALES {
        window
            .update(visual, |shell, w, cx| {
                shell.scale_interface(
                    percent as i32 - shell.workspace.preferences.ui_scale as i32,
                    w,
                    cx,
                );
                shell.workspace.project_rail_width = 160.;
                shell.workspace.project_rail_open = true;
                shell.focus_workspace(w, cx);
                shell.invalidate_chrome(cx);
                cx.notify();
            })
            .unwrap();
        settle(visual);
        assert_titlebar(window, visual, percent, false);
        let bounds = window.update(visual, |_, w, _| w.window_bounds()).unwrap();
        let toggle = platform_bounds(window, visual, "toggle-sidebar");
        visual.simulate_mouse_move(toggle.center(), None, Default::default());
        visual.simulate_click(toggle.center(), Default::default());
        settle(visual);
        window
            .update(visual, |shell, w, _| {
                assert!(!shell.workspace.project_rail_open);
                assert_eq!(shell.workspace.project_rail_width, 160.);
                assert_eq!(w.window_bounds(), bounds);
            })
            .unwrap();
        assert_titlebar(window, visual, percent, false);
        window
            .update(visual, |shell, w, cx| {
                assert!(shell.project_rail_toggle_focus.is_focused(w));
                w.focus_prev(cx);
                w.focus_next(cx);
                assert!(
                    shell.project_rail_toggle_focus.is_focused(w),
                    "the toggle participates in keyboard tab order"
                );
            })
            .unwrap();
        activate(visual, "enter");
        settle(visual);
        window
            .update(visual, |shell, _, _| {
                assert!(
                    shell.workspace.project_rail_open,
                    "the focused toggle activates with Enter"
                )
            })
            .unwrap();
        activate(visual, "space");
        settle(visual);
        window
            .update(visual, |shell, _, _| {
                assert!(
                    !shell.workspace.project_rail_open,
                    "the focused toggle activates with Space"
                )
            })
            .unwrap();
        visual.simulate_keystrokes(&format!("{modifier}-b"));
        settle(visual);
        window
            .update(visual, |shell, _, _| {
                assert!(shell.workspace.project_rail_open)
            })
            .unwrap();
        // Real title-tab and history clicks, through UI zoom's platform mapping.
        let tab = visual
            .debug_bounds(Box::leak(format!("title-tab-{a}").into_boxed_str()))
            .unwrap();
        let tab = window
            .update(visual, |_, w, _| w.logical_to_platform_bounds(tab))
            .unwrap();
        assert!(
            tab.size.width > px(10.),
            "the tab is clickable at {percent}%: {tab:?}"
        );
        visual.simulate_mouse_move(tab.center(), None, Default::default());
        visual.simulate_click(tab.center(), Default::default());
        settle(visual);
        window
            .update(visual, |shell, _, _| {
                assert_eq!(shell.workspace.active, Some(a))
            })
            .unwrap();
        let back = platform_bounds(window, visual, "visit-back");
        assert_titlebar(window, visual, percent, false);
        visual.simulate_mouse_move(back.center(), None, Default::default());
        visual.simulate_click(back.center(), Default::default());
        settle(visual);
        window
            .update(visual, |shell, _, _| {
                assert_eq!(shell.workspace.active, Some(b))
            })
            .unwrap();
        let forward = platform_bounds(window, visual, "visit-forward");
        visual.simulate_mouse_move(forward.center(), None, Default::default());
        visual.simulate_click(forward.center(), Default::default());
        settle(visual);
        window
            .update(visual, |shell, w, cx| {
                assert_eq!(shell.workspace.active, Some(a));
                shell.open(b, w, cx);
            })
            .unwrap();
    }
    visual.simulate_keystrokes(&format!("{modifier}-0 {modifier}-="));
    settle(visual);
    assert_titlebar(window, visual, 110, false);
    let menus = crate::app_menu::build(&crate::settings::Preferences::default());
    let action = menus
        .iter()
        .flat_map(|m| &m.items)
        .find_map(|item| match item {
            gpui::MenuItem::Action { name, action, .. } if name == "Zoom Out" => {
                Some(action.boxed_clone())
            }
            _ => None,
        })
        .unwrap();
    visual.cx.update(|cx| cx.dispatch_action(action.as_ref()));
    settle(visual);
    assert_titlebar(window, visual, 100, false);
    window
        .update(visual, |shell, w, cx| {
            shell.open_settings(crate::settings::Section::Appearance, w, cx)
        })
        .unwrap();
    for percent in SCALES {
        window
            .update(visual, |shell, _, cx| {
                let mut prefs = shell.workspace.preferences.clone();
                prefs.ui_scale = percent;
                shell.settings.as_ref().unwrap().update(cx, |_, cx| {
                    cx.emit(crate::settings::SettingsEvent::Changed(Box::new(prefs)));
                });
            })
            .unwrap();
        settle(visual);
        let nav = platform_bounds(window, visual, "titlebar-navigation");
        let header = platform_bounds(window, visual, "settings-header");
        assert!(header.left() + px(0.02) >= nav.right());
        let toggle = platform_bounds(window, visual, "toggle-sidebar");
        assert!(
            (toggle.left().as_f32()
                - if cfg!(target_os = "macos") {
                    78.
                } else {
                    10. * percent as f32 / 100.
                })
            .abs()
                < 0.02
        );
    }
}
