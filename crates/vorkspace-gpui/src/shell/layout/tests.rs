use super::*;
use crate::{bots::Bots, shell::tests::window};
use gpui::{Focusable, TestAppContext, WindowHandle};

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
fn the_sidebar_stays_left_even_with_a_legacy_right_side_preference(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, _, session) = window(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1400.), px(900.)));
    window
        .update(visual, |shell, w, cx| {
            shell.open(session, w, cx);
            shell.set_sidebar_visible(true, cx);
            // The sidebar sits beside the rail by default.
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
    // A legacy preference cannot move the sidebar to the right.
    window
        .update(visual, |shell, _, cx| {
            let mut saved = serde_json::to_value(&shell.workspace.preferences).unwrap();
            saved["sidebar_right"] = serde_json::json!(true);
            shell.workspace.preferences = serde_json::from_value(saved).unwrap();
            shell.invalidate_chrome(cx);
            cx.notify();
        })
        .unwrap();
    visual.run_until_parked();
    let pane = visual.debug_bounds("main-pane-frame").unwrap();
    let sidebar = visual.debug_bounds("sidebar-frame").unwrap();
    assert_eq!(sidebar.left(), rail.right());
    assert_eq!(pane.left(), sidebar.right());
    assert!(pane_has_left_border(window, visual));
    // The rail owns the divider again once the left sidebar closes.
    window
        .update(visual, |shell, _, cx| {
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
    for mode in ["expanded", "compact", "hidden", "expanded"] {
        window
            .update(visual, |shell, _, cx| {
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
                "{mode}: {header:?} overlaps {navigation:?}"
            );
        } else {
            assert_eq!(header.top(), sidebar.top(), "{mode}");
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
                    "{mode}: the project switcher remains clickable"
                );
                shell.close_menu(cx);
            })
            .unwrap();
        visual.run_until_parked();
    }
}

#[gpui::test]
fn the_active_tab_has_a_separate_rounded_highlight(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, _, session) = window(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |shell, w, cx| shell.open(session, w, cx))
        .unwrap();
    visual.run_until_parked();
    let selector = format!("title-tab-plate-{session}");
    assert!(crate::shell::tests::rounded_highlight(
        window, &selector, 0.1, visual
    ));
    let plate = visual
        .debug_bounds(Box::leak(selector.into_boxed_str()))
        .unwrap();
    let strip = visual.debug_bounds("title-tab-strip").unwrap();
    assert!(plate.top() > strip.top());
    assert!(
        plate.bottom() < strip.bottom(),
        "the plate stays above the divider"
    );
}

#[gpui::test]
fn connector_pages_keep_title_tabs_and_settings_keeps_its_header_over_them(
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
    assert!(visual.debug_bounds("title-tab-strip").is_some());
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
        // Search is a workspace tab, so its input stays beneath the same
        // title row in every rail mode, clear of the navigation controls.
        assert_eq!(header.top(), pane.top() + px(TITLE_BAR_HEIGHT), "{mode}");
        assert!(header.top() >= navigation.bottom(), "{mode}: {header:?}");
        assert!(visual.debug_bounds("title-tab-strip").is_some(), "{mode}");
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
            assert_eq!(shell.workspace.right_panel_width, width + 20.)
        })
        .unwrap();

    for surface in ["GitHub", "Notes", "Search", "Bot", "Settings"] {
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
                    _ => unreachable!(),
                }
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        assert!(visual.debug_bounds("sidebar-frame").is_none(), "{surface}");
        assert!(visual.debug_bounds("sidebar-resize").is_none(), "{surface}");
        let (old_sash, before) = window
            .update(visual, |shell, _, _| {
                (
                    gpui::point(
                        px(shell.classic_rail_width() + shell.workspace.right_panel_width - 0.5),
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
fn collapsed_sidebar_stays_closed_when_hovering_the_rail(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, manager, bot, session) = mixed_window(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1600.), px(1000.)));
    window
        .update(visual, |shell, window, cx| {
            shell.open(session, window, cx);
            shell.workspace.project_rail_open = false;
            shell.workspace.preferences.collapsed_project_rail = "compact".into();
            shell.set_sidebar_visible(false, cx);
            shell.invalidate_chrome(cx);
        })
        .unwrap();
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    let rail = visual.debug_bounds("classic-project-rail").unwrap();
    let positions = [
        visual
            .debug_bounds("classic-compact-tab-sessions")
            .unwrap()
            .center(),
        visual.debug_bounds("toggle-sidebar").unwrap().center(),
        gpui::point(rail.right() + px(2.), px(300.)),
    ];
    for position in positions {
        visual.simulate_mouse_move(position, None, Default::default());
        visual.run_until_parked();
        vorkspace_ui::clock::settle_animations(window, visual);
        assert!(visual.debug_bounds("sidebar-peek").is_none());
        assert!(visual.debug_bounds("sidebar-frame").is_none());
    }
    window
        .update(visual, |_, window, cx| {
            manager.update(cx, |manager, cx| manager.open(&bot, window, cx));
        })
        .unwrap();
    visual.run_until_parked();
    for position in positions {
        visual.simulate_mouse_move(position, None, Default::default());
        visual.run_until_parked();
        vorkspace_ui::clock::settle_animations(window, visual);
        assert!(visual.debug_bounds("sidebar-peek").is_none());
        assert!(visual.debug_bounds("sidebar-frame").is_none());
    }
}
