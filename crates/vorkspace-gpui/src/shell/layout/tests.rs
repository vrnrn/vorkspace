
use super::*;
use crate::{bots::Bots, shell::tests::window};
use gpui::{Focusable, TestAppContext, WindowHandle};
use std::time::Duration;
use vorkspace_ui::clock::Instant;

fn pane_has_left_border(window: WindowHandle<Shell>, visual: &mut gpui::VisualTestContext) -> bool {
    let pane = visual.debug_bounds("main-pane-frame").unwrap();
    window
        .update(visual, |_, window, _| {
            let scale = window.scale_factor();
            window.painted_quads().iter().any(|quad| {
                (quad.bounds.left().0 - pane.left().as_f32() * scale).abs() < 0.5
                    && (quad.bounds.top().0 - pane.top().as_f32() * scale).abs() < 0.5
                    && (quad.bounds.size.width.0 - pane.size.width.as_f32() * scale).abs() < 0.5
                    && (quad.bounds.size.height.0 - pane.size.height.as_f32() * scale).abs() < 0.5
                    && quad.border_widths.left.0 > 0.
                    && quad.border_color.a > 0.
            })
        })
        .unwrap()
}

fn mixed_window(cx: &mut TestAppContext) -> (WindowHandle<Shell>, Entity<Bots>, String, i64) {
    let (window, _, session) = window(cx);
    let (manager, id) = window
        .update(cx, |shell, w, cx| {
            // Channels are experimental and hidden by default.
            shell.workspace.preferences.channels = true;
            shell.apply_preferences(cx);
            let manager = shell.bot_manager();
            manager.update(cx, |m, cx| {
                m.create_named(Vec::new(), w, cx);
                m.dismiss_settings(w, cx);
            });
            let id = manager.read(cx).active.clone().unwrap();
            (manager, id)
        })
        .unwrap();
    cx.run_until_parked();
    (window, manager, id, session)
}

#[gpui::test]
fn the_sidebar_sits_beside_the_rail_by_default_or_switches_right(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, _, session) = window(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1400.), px(900.)));
    window
        .update(visual, |shell, w, cx| {
            shell.open(session, w, cx);
            shell.set_sidebar_visible(true, cx);
            // The sidebar sits beside the rail by default.
            shell.workspace.preferences.sidebar_right = false;
            shell.invalidate_chrome(cx);
            cx.notify();
        })
        .unwrap();
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    let rail = visual.debug_bounds("classic-project-rail").unwrap();
    let sidebar = visual.debug_bounds("sidebar-frame").unwrap();
    let pane = visual.debug_bounds("main-pane-frame").unwrap();
    let strip = visual.debug_bounds("title-tab-strip").unwrap();
    assert_eq!(sidebar.left(), rail.right());
    assert_eq!(pane.left(), sidebar.right());
    assert!(pane_has_left_border(window, visual));
    assert!(
        strip.left() >= pane.left(),
        "tabs head the pane, after the sidebar"
    );
    // Its separator faces the pane.
    let sash = visual.debug_bounds("sidebar-resize").unwrap();
    assert!((sash.center().x - sidebar.right()).abs() <= px(2.5));
    // Switched to the right, it follows the pane.
    window
        .update(visual, |shell, _, cx| {
            shell.workspace.preferences.sidebar_right = true;
            shell.invalidate_chrome(cx);
            cx.notify();
        })
        .unwrap();
    visual.run_until_parked();
    let pane = visual.debug_bounds("main-pane-frame").unwrap();
    let sidebar = visual.debug_bounds("sidebar-frame").unwrap();
    assert_eq!(pane.left(), rail.right());
    assert_eq!(sidebar.left(), pane.right());
    assert!(!pane_has_left_border(window, visual));
    // The rail owns the divider again once the left sidebar closes.
    window
        .update(visual, |shell, _, cx| {
            shell.workspace.preferences.sidebar_right = false;
            shell.set_sidebar_visible(false, cx);
            shell.invalidate_chrome(cx);
            cx.notify();
        })
        .unwrap();
    vorkspace_ui::clock::settle_animations(window, visual);
    assert!(visual.debug_bounds("sidebar-frame").is_none());
    assert!(!pane_has_left_border(window, visual));
}

#[gpui::test]
fn sidebar_project_switcher_clears_navigation_across_rail_modes(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, _, session) = window(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1400.), px(900.)));
    window
        .update(visual, |shell, w, cx| {
            w.activate_window();
            shell.open(session, w, cx);
            shell.set_sidebar_visible(true, cx);
        })
        .unwrap();
    vorkspace_ui::clock::settle_animations(window, visual);
    for right in [false, true] {
        for mode in ["expanded", "compact", "hidden", "expanded"] {
            window
                .update(visual, |shell, _, cx| {
                    shell.workspace.preferences.sidebar_right = right;
                    shell.workspace.project_rail_open = mode == "expanded";
                    if mode != "expanded" {
                        shell.workspace.preferences.collapsed_project_rail = mode.into();
                    }
                    shell.invalidate_chrome(cx);
                    cx.notify();
                })
                .unwrap();
            visual.run_until_parked();
            let navigation = visual.debug_bounds("titlebar-navigation").unwrap();
            let sidebar = visual.debug_bounds("sidebar-frame").unwrap();
            let header = visual.debug_bounds("classic-workspace-header").unwrap();
            if navigation.right() > sidebar.left() {
                assert!(
                    header.top() >= navigation.bottom(),
                    "{mode}, right={right}: {header:?} overlaps {navigation:?}"
                );
            } else {
                assert_eq!(header.top(), sidebar.top(), "{mode}, right={right}");
            }
            // Click the leading edge that used to sit beneath navigation.
            let project = visual.debug_bounds("sidebar-project-button").unwrap();
            visual.simulate_click(
                gpui::point(project.left() + px(12.), project.center().y),
                Default::default(),
            );
            visual.run_until_parked();
            window
                .update(visual, |shell, _, cx| {
                    assert!(
                        shell.menu_open(&super::super::MenuTarget::Header),
                        "{mode}, right={right}: the project switcher remains clickable"
                    );
                    shell.close_menu(cx);
                })
                .unwrap();
            visual.run_until_parked();
        }
    }
}

#[gpui::test]
fn the_active_tab_is_the_selected_pill_of_the_tab_strip(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, _, session) = window(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |shell, w, cx| shell.open(session, w, cx))
        .unwrap();
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            let id = shell.workspace.active_tab().unwrap();
            assert_eq!(
                shell.tab_liquid.selected().as_deref(),
                Some(format!("tab:{id}").as_str())
            );
        })
        .unwrap();
}

#[gpui::test]
fn expanded_pages_have_no_title_bar_but_settings_keeps_its_header_over_them(
    cx: &mut TestAppContext,
) {
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, _, _) = window(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |shell, w, cx| {
            shell.open_connector(crate::workspace::Connector::Github, w, cx)
        })
        .unwrap();
    visual.run_until_parked();
    assert!(visual.debug_bounds("title-tab-strip").is_none());
    assert!(visual.debug_bounds("settings-header").is_none());
    window
        .update(visual, |shell, w, cx| {
            shell.open_settings(crate::settings::Section::General, w, cx)
        })
        .unwrap();
    visual.run_until_parked();
    assert!(visual.debug_bounds("settings-header").is_some());
}

#[gpui::test]
fn search_header_clears_navigation_and_keeps_its_query_across_rail_modes(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, _, _) = window(cx);
    let input = window
        .update(cx, |shell, w, cx| {
            w.activate_window();
            shell.workspace.preferences.sidebar_right = false;
            shell.open_app_search(w, cx);
            let input = shell
                .app_search
                .as_ref()
                .unwrap()
                .view
                .read(cx)
                .input
                .clone();
            input.update(cx, |input, cx| input.set_text("query draft".into(), cx));
            input
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1400.), px(900.)));
    for mode in ["expanded", "compact", "hidden", "expanded"] {
        window
            .update(visual, |shell, w, cx| {
                shell.workspace.project_rail_open = mode == "expanded";
                if mode != "expanded" {
                    shell.workspace.preferences.collapsed_project_rail = mode.into();
                }
                w.focus(&shell.focus, cx);
                shell.invalidate_chrome(cx);
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        let navigation = visual.debug_bounds("titlebar-navigation").unwrap();
        let pane = visual.debug_bounds("main-pane-frame").unwrap();
        let header = visual.debug_bounds("app-search-header").unwrap();
        assert!(!pane_has_left_border(window, visual), "{mode}");
        assert_eq!(
            header.left(),
            pane.left(),
            "{mode}: the rail owns the divider when no left sidebar is drawn"
        );
        if navigation.right() > pane.left() {
            assert!(header.top() >= navigation.bottom(), "{mode}: {header:?}");
        } else {
            assert_eq!(header.top(), pane.top(), "{mode}");
        }
        assert!(visual.debug_bounds("title-tab-strip").is_none(), "{mode}");
        // This query point lies under the overhanging buttons without clearance.
        visual.simulate_click(
            gpui::point(header.left() + px(48.), header.center().y),
            Default::default(),
        );
        visual.run_until_parked();
        window
            .update(visual, |_, w, cx| {
                assert!(input.read(cx).focus_handle(cx).is_focused(w), "{mode}");
                assert_eq!(input.read(cx).content, "query draft", "{mode}");
            })
            .unwrap();
    }
}

#[gpui::test]
fn channels_after_bot_details_keep_a_plain_header_and_full_tab_width(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, manager, _, _) = mixed_window(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1600.), px(1000.)));
    visual.run_until_parked();
    let toggle = visual.debug_bounds("toggle-right-panel").unwrap();
    visual.simulate_click(toggle.center(), Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("bot-overview-panel").is_some());
    assert!(visual.debug_bounds("bot-overview-tabs").is_some());

    let channels = visual.debug_bounds("classic-add-channel").unwrap();
    visual.simulate_click(channels.center(), Default::default());
    visual.run_until_parked();
    assert!(manager.read_with(visual, |manager, _| manager.classic_channels_open()));
    assert!(visual.debug_bounds("right-panel-frame").is_none());
    assert!(visual.debug_bounds("bot-overview-tabs").is_none());
    assert!(visual.debug_bounds("toggle-right-panel").is_none());
    window
        .update(visual, |shell, window, cx| {
            assert!(
                shell.workspace.right_panel_open,
                "the Bot's remembered preference is retained"
            );
            assert_eq!(shell.right_drawn, 0.);
            assert_eq!(shell.corner_room(cx), 42.);
            assert_eq!(
                shell.tabs_width(window, cx),
                1600. - shell.classic_rail_width() - 42.
            );
        })
        .unwrap();
}

#[gpui::test]
fn hidden_workspace_panes_do_not_leave_a_resize_hitbox_over_global_surfaces(
    cx: &mut TestAppContext,
) {
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, manager, bot, session) = mixed_window(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1600.), px(1000.)));
    window
        .update(visual, |shell, window, cx| {
            shell.open(session, window, cx);
            shell.set_sidebar_visible(true, cx);
            cx.notify();
        })
        .unwrap();
    visual.run_until_parked();
    let sash = visual.debug_bounds("sidebar-resize").unwrap().center();
    let width = window
        .update(visual, |shell, _, _| shell.workspace.right_panel_width)
        .unwrap();
    let moved = sash + gpui::point(px(20.), px(0.));
    visual.simulate_mouse_down(sash, MouseButton::Left, Default::default());
    visual.simulate_mouse_move(moved, MouseButton::Left, Default::default());
    visual.simulate_mouse_up(moved, MouseButton::Left, Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            assert_eq!(shell.workspace.right_panel_width, width - 20.)
        })
        .unwrap();

    for surface in ["GitHub", "Notes", "Search", "Bot", "Settings", "Channels"] {
        window
            .update(visual, |shell, window, cx| {
                if shell.settings.is_some() {
                    shell.close_settings(window, cx);
                }
                shell.open(session, window, cx);
                shell.set_sidebar_visible(true, cx);
                match surface {
                    "GitHub" => shell.open_inbox(window, cx),
                    "Notes" => shell.open_notes(window, cx),
                    "Search" => shell.open_app_search(window, cx),
                    "Bot" => manager.update(cx, |manager, cx| manager.open(&bot, window, cx)),
                    "Settings" => {
                        shell.open_settings(crate::settings::Section::General, window, cx)
                    }
                    "Channels" => (),
                    _ => unreachable!(),
                }
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        if surface == "Channels" {
            let add = visual.debug_bounds("classic-add-channel").unwrap();
            visual.simulate_click(add.center(), Default::default());
            visual.run_until_parked();
        }
        assert!(visual.debug_bounds("sidebar-frame").is_none(), "{surface}");
        assert!(visual.debug_bounds("sidebar-resize").is_none(), "{surface}");
        let (old_sash, before) = window
            .update(visual, |shell, _, _| {
                (
                    gpui::point(
                        px(shell.viewport_width - shell.workspace.right_panel_width + 0.5),
                        px(500.),
                    ),
                    shell.workspace.right_panel_width,
                )
            })
            .unwrap();
        visual.simulate_mouse_down(old_sash, MouseButton::Left, Default::default());
        window
            .update(visual, |shell, _, _| {
                assert!(shell.resize.is_none(), "{surface}")
            })
            .unwrap();
        let moved = old_sash + gpui::point(px(30.), px(0.));
        visual.simulate_mouse_move(moved, MouseButton::Left, Default::default());
        visual.simulate_mouse_up(moved, MouseButton::Left, Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert_eq!(shell.workspace.right_panel_width, before, "{surface}")
            })
            .unwrap();
    }
}
#[gpui::test]
fn original_workspace_tabs_fit_sidebar_widths_and_keep_their_actions(cx: &mut TestAppContext) {
    use crate::workspace::SidebarTab;
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, _, _) = window(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1600.), px(900.)));
    for width in [200., 260., 420., 560., 260.] {
        window
            .update(visual, |shell, _, cx| {
                shell.workspace.right_panel_width = width;
                shell.invalidate_chrome(cx);
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        let sidebar = visual.debug_bounds("sidebar-frame").unwrap();
        let selectors = [
            "classic-tab-sessions",
            "classic-tab-explorer",
            "classic-tab-changes",
        ];
        let tabs = selectors.map(|selector| visual.debug_bounds(selector).unwrap());
        assert_eq!(sidebar.size.width, px(width));
        assert!(tabs[0].left() >= sidebar.left());
        assert!(tabs[0].right() <= tabs[1].left());
        assert!(tabs[1].right() <= tabs[2].left());
        assert!(tabs[2].right() <= sidebar.right());
        assert!(tabs.iter().all(|tab| tab.size.width > px(0.)));
        assert!(visual.debug_bounds("right-panel-frame").is_none());
        for (index, expected) in [SidebarTab::Sessions, SidebarTab::Files, SidebarTab::Changes]
            .into_iter()
            .enumerate()
        {
            visual.simulate_click(tabs[index].center(), Default::default());
            visual.run_until_parked();
            window
                .update(visual, |shell, _, _| {
                    assert_eq!(shell.workspace.sidebar_tab(), expected)
                })
                .unwrap();
        }
        let history = visual.debug_bounds("classic-source-history").unwrap();
        visual.simulate_click(history.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert_eq!(shell.workspace.sidebar_tab(), SidebarTab::Graph)
            })
            .unwrap();
    }
}
#[gpui::test]
fn bot_and_session_sidebars_share_width_while_retaining_their_own_visibility(
    cx: &mut TestAppContext,
) {
    use crate::workspace::SidebarTab;
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, manager, bot, session) = mixed_window(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1600.), px(1000.)));
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    assert!(visual.debug_bounds("right-panel-frame").is_none());
    let toggle = visual.debug_bounds("toggle-right-panel").unwrap();
    visual.simulate_click(toggle.center(), Default::default());
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    assert!(visual.debug_bounds("bot-overview-panel").is_some());
    let frame = visual.debug_bounds("right-panel-frame").unwrap();
    let bounds = visual.debug_bounds("right-panel-resize").unwrap();
    assert_eq!(bounds.size.width, px(6.));
    assert!((bounds.center().x - frame.left() - px(0.5)).abs() <= px(0.5));
    for x in [bounds.left() + px(0.5), bounds.right() - px(0.5)] {
        let point = gpui::point(x, bounds.center().y);
        visual.simulate_mouse_down(point, MouseButton::Left, Default::default());
        window
            .update(visual, |shell, _, _| assert!(shell.right_resize.is_some()))
            .unwrap();
        visual.simulate_mouse_up(point, MouseButton::Left, Default::default());
        visual.run_until_parked();
        vorkspace_ui::clock::settle_animations(window, visual);
    }
    let handle = visual.debug_bounds("right-panel-resize").unwrap().center();
    let end = handle - gpui::point(px(30.), px(0.));
    visual.simulate_mouse_down(handle, MouseButton::Left, Default::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Default::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Default::default());
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    let expanded = window
        .update(visual, |shell, _, _| shell.workspace.right_panel_width)
        .unwrap();
    let handle = visual.debug_bounds("right-panel-resize").unwrap().center();
    let end = handle + gpui::point(px(30.), px(0.));
    visual.simulate_mouse_down(handle, MouseButton::Left, Default::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Default::default());
    window
        .update(visual, |shell, _, _| {
            assert_eq!(shell.workspace.right_panel_width, expanded - 30.)
        })
        .unwrap();
    visual.simulate_mouse_up(
        end + gpui::point(px(20.), px(0.)),
        MouseButton::Left,
        Default::default(),
    );
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    let width = window
        .update(visual, |shell, w, cx| {
            assert!(shell.right_resize.is_none());
            vorkspace_ui::clock::use_test_clock(cx);
            let width = shell.workspace.right_panel_width;
            shell.open(session, w, cx);
            shell.select_classic_sidebar(SidebarTab::Files, w, cx);
            width
        })
        .unwrap();
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    assert!(visual.debug_bounds("right-panel-frame").is_none());
    assert!(visual.debug_bounds("bot-overview-panel").is_none());
    let sidebar = visual.debug_bounds("sidebar-frame").unwrap();
    let explorer = visual.debug_bounds("explorer-header").unwrap();
    assert_eq!(sidebar.size.width, px(width));
    assert_eq!(sidebar.right(), px(1600.));
    assert!(explorer.left() >= sidebar.left() && explorer.right() <= sidebar.right());
    let handle = visual.debug_bounds("sidebar-resize").unwrap().center();
    let end = handle - gpui::point(px(40.), px(0.));
    visual.simulate_mouse_down(handle, MouseButton::Left, Default::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Default::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Default::default());
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    let width = width + 40.;
    let session_sidebar = visual.debug_bounds("sidebar-frame").unwrap();
    assert_eq!(session_sidebar.size.width, px(width));
    window
        .update(visual, |_, w, cx| {
            manager.update(cx, |manager, cx| manager.open(&bot, w, cx))
        })
        .unwrap();
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    for _ in 0..3 {
        assert_eq!(
            visual.debug_bounds("right-panel-frame").unwrap(),
            session_sidebar
        );
        visual
            .update_window(window.into(), |_, w, cx| w.simulate_next_frame(cx))
            .unwrap();
    }
    vorkspace_ui::clock::use_test_clock(cx);
    // One sidebar fits at this size. Counting the hidden session sidebar
    // alongside Bot details must not change its visibility preference.
    visual.simulate_resize(gpui::size(px(1050.), px(1000.)));
    window
        .update(visual, |shell, w, cx| {
            assert!(shell.workspace.sidebar_visible);
            shell.open(session, w, cx);
        })
        .unwrap();
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    let session_sidebar = visual.debug_bounds("sidebar-frame").unwrap();
    assert_eq!(session_sidebar.size.width, px(width));
    window
        .update(visual, |_, w, cx| {
            manager.update(cx, |manager, cx| manager.open(&bot, w, cx));
        })
        .unwrap();
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    assert_eq!(
        visual.debug_bounds("right-panel-frame").unwrap(),
        session_sidebar
    );
    let toggle = visual.debug_bounds("toggle-right-panel").unwrap();
    visual.simulate_click(toggle.center(), Default::default());
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    assert!(visual.debug_bounds("right-panel-frame").is_none());
    window
        .update(visual, |shell, _, _| {
            assert_eq!(shell.workspace.sidebar_tab(), SidebarTab::Files);
            assert_eq!(shell.workspace.right_panel_width, width);
            let saved = vorkspace_runtime::runtime::locked(&shell.store)
                .unwrap()
                .ui_state()
                .unwrap()
                .unwrap();
            let saved: crate::workspace::Workspace = serde_json::from_str(&saved).unwrap();
            assert_eq!(saved.right_panel_width, width);
            assert!(!saved.right_panel_open);
        })
        .unwrap();
}
#[gpui::test]
fn session_sidebar_peeks_from_the_right_and_never_over_a_bot(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, manager, bot, session) = mixed_window(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1600.), px(1000.)));
    window
        .update(visual, |shell, window, cx| {
            shell.open(session, window, cx);
            shell.set_sidebar_visible(false, cx);
        })
        .unwrap();
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    window
        .update(visual, |shell, _, cx| shell.set_peek(true, cx))
        .unwrap();
    vorkspace_ui::clock::settle_animations(window, visual);
    let peek = visual.debug_bounds("sidebar-peek").unwrap();
    assert_eq!(peek.right(), px(1594.));
    assert_eq!(peek.size.width, px(crate::right_panel::WIDTH_DEFAULT));
    window
        .update(visual, |_, window, cx| {
            manager.update(cx, |manager, cx| manager.open(&bot, window, cx));
        })
        .unwrap();
    visual.run_until_parked();
    assert!(visual.debug_bounds("sidebar-peek").is_none());
}
#[gpui::test]
fn switching_bots_keeps_the_same_details_spring_and_clipped_contents(cx: &mut TestAppContext) {
    let (window, manager, bot, _) = mixed_window(cx);
    let other = window
        .update(cx, |_, window, cx| {
            manager.update(cx, |manager, cx| {
                manager.create_named(Vec::new(), window, cx);
                manager.dismiss_settings(window, cx);
                manager.active.clone().unwrap()
            })
        })
        .unwrap();
    vorkspace_ui::clock::use_test_clock(cx);
    window
        .update(cx, |shell, w, cx| shell.toggle_right_panel(w, cx))
        .unwrap();
    for ms in [35, 70, 110] {
        window
            .update(cx, |shell, w, cx| {
                let slide = shell.right_slide.as_ref().unwrap();
                slide.freeze_elapsed(Duration::from_millis(ms));
                let now = Instant::now();
                let before = slide.at(now);
                let next = if manager.read(cx).active.as_deref() == Some(bot.as_str()) {
                    &other
                } else {
                    &bot
                };
                manager.update(cx, |manager, cx| manager.open(next, w, cx));
                assert_eq!(
                    shell.right_slide.as_ref().unwrap().at(now),
                    before,
                    "tab switches preserve the spring's position and velocity"
                );
                assert!(shell.workspace.right_panel_open);
                cx.notify();
            })
            .unwrap();
        cx.update_window(window.into(), |_, w, cx| {
            w.simulate_next_frame(cx);
            w.draw(cx).clear(cx);
        })
        .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        let frame = visual.debug_bounds("right-panel-frame").unwrap();
        let content = visual.debug_bounds("right-panel-content").unwrap();
        let width = window
            .update(visual, |shell, _, _| shell.workspace.right_panel_width)
            .unwrap();
        assert!(frame.size.width > px(0.) && frame.size.width < px(width));
        assert_eq!(content.size.width, px(width - 1.));
    }
    window
        .update(cx, |shell, w, cx| {
            shell.toggle_right_panel(w, cx);
            assert!(!shell.workspace.right_panel_open);
            assert_eq!(shell.right_slide.as_ref().unwrap().to, 0.);
            shell
                .right_slide
                .as_ref()
                .unwrap()
                .elapsed(Duration::from_millis(45));
            manager.update(cx, |manager, cx| manager.open(&other, w, cx));
            shell.toggle_right_panel(w, cx);
            assert!(shell.workspace.right_panel_open);
            assert_eq!(
                shell.right_slide.as_ref().unwrap().to,
                shell.workspace.right_panel_width
            );
            manager.update(cx, |m, cx| m.open(&bot, w, cx));
            shell
                .right_slide
                .as_ref()
                .unwrap()
                .elapsed(Duration::from_secs(1));
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| assert!(shell.right_slide.is_none()))
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    assert_eq!(
        visual.debug_bounds("right-panel-frame").unwrap().size.width,
        px(window
            .update(visual, |shell, _, _| shell.workspace.right_panel_width)
            .unwrap())
    );
    assert!(visual.debug_bounds("bot-overview-panel").is_some());
}
