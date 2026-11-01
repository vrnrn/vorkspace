
use super::*;
use crate::shell::tests::window;
use gpui::TestAppContext;

#[gpui::test]
fn the_open_rail_holds_navigation_at_low_zoom(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, _, _) = window(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1100.), px(800.)));
    for scale in [60, 80, 100, 160] {
        window
            .update(visual, |shell, w, cx| {
                shell.workspace.project_rail_open = true;
                shell.workspace.project_rail_width = 180.;
                shell.workspace.preferences.ui_scale = scale;
                cx.set_global(shell.workspace.preferences.clone());
                w.set_ui_scale(scale as f32 / 100., cx);
                shell.open_app_search(w, cx);
                shell.invalidate_chrome(cx);
                cx.notify();
            })
            .unwrap();
        vorkspace_ui::clock::settle_animations(window, visual);
        let rail = visual.debug_bounds("classic-project-rail").unwrap();
        let forward = visual.debug_bounds("visit-forward").unwrap();
        let pane = visual.debug_bounds("main-pane-frame").unwrap();
        let header = visual.debug_bounds("app-search-header").unwrap();
        assert!(
            forward.right() <= rail.right(),
            "{scale}%: {forward:?} {rail:?}"
        );
        // No second title row pushes the page down.
        assert_eq!(header.top(), pane.top(), "{scale}%");
        // The saved width is untouched for when zoom rises again.
        window
            .update(visual, |shell, _, _| {
                assert_eq!(shell.workspace.project_rail_width, 180., "{scale}%")
            })
            .unwrap();
    }
}

#[gpui::test]
fn the_icon_rail_divider_curves_into_the_title_rule(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, _, session) = window(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1000.), px(700.)));
    let lead_rule_left = |visual: &mut gpui::VisualTestContext| {
        window
            .update(visual, |_, w, _| {
                let scale = w.scale_factor();
                w.painted_quads()
                    .iter()
                    .filter(|q| {
                        q.border_widths.bottom.0 > 0.
                            && q.border_color.a > 0.
                            && q.bounds.top().0 == 0.
                            && (q.bounds.size.height.0 / scale - TITLE_BAR_HEIGHT).abs() < 0.5
                    })
                    .map(|q| q.bounds.left().0 / scale)
                    .fold(f32::INFINITY, f32::min)
            })
            .unwrap()
    };
    for sidebar_right in [true, false] {
        window
            .update(visual, |shell, w, cx| {
                shell.workspace.project_rail_open = false;
                shell.workspace.preferences.sidebar_right = sidebar_right;
                shell.open(session, w, cx);
                shell.set_sidebar_visible(false, cx);
                shell.invalidate_chrome(cx);
                cx.notify();
            })
            .unwrap();
        vorkspace_ui::clock::settle_animations(window, visual);
        let rail = visual.debug_bounds("classic-project-rail").unwrap();
        window
            .update(visual, |shell, _, _| assert!(shell.rail_meets_title_rule()))
            .unwrap();
        // The rule leaves the corner to the divider's curve.
        assert_eq!(
            lead_rule_left(visual),
            rail.right().as_f32() + crate::liquid::PANE_RADIUS,
            "sidebar_right={sidebar_right} rail={rail:?} pane={:?}",
            visual.debug_bounds("main-pane-frame")
        );
    }
    // A left sidebar takes the overhang, and the rule, in its own row.
    window
        .update(visual, |shell, _, cx| {
            shell.set_sidebar_visible(true, cx);
            shell.invalidate_chrome(cx);
            cx.notify();
        })
        .unwrap();
    vorkspace_ui::clock::settle_animations(window, visual);
    let rail = visual.debug_bounds("classic-project-rail").unwrap();
    let row = visual.debug_bounds("sidebar-navigation-row").unwrap();
    let sidebar = visual.debug_bounds("sidebar-frame").unwrap();
    let pane = visual.debug_bounds("main-pane-frame").unwrap();
    assert_eq!(row.left(), rail.right());
    assert_eq!(row.right(), sidebar.right());
    assert_eq!(
        lead_rule_left(visual),
        rail.right().as_f32() + crate::liquid::PANE_RADIUS
    );
    // The pane's tabs start at its edge; navigation does not reach it.
    // Inside the pane's own 1px border beside the sidebar.
    let strip = visual.debug_bounds("title-tab-strip").unwrap();
    assert_eq!(strip.left(), pane.left() + px(1.));
}

#[gpui::test]
fn the_sidebar_tabs_rise_from_their_line(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, _, session) = window(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1200.), px(800.)));
    window
        .update(visual, |shell, w, cx| {
            shell.open(session, w, cx);
            shell.set_sidebar_visible(true, cx);
            shell.invalidate_chrome(cx);
            cx.notify();
        })
        .unwrap();
    vorkspace_ui::clock::settle_animations(window, visual);
    let selected = |visual: &mut gpui::VisualTestContext| {
        window
            .update(visual, |shell, _, _| shell.sidebar_tab_liquid.selected())
            .unwrap()
    };
    assert_eq!(selected(visual).as_deref(), Some("sidebar-tab:Sessions"));
    let changes = visual.debug_bounds("classic-tab-changes").unwrap();
    visual.simulate_click(changes.center(), Default::default());
    vorkspace_ui::clock::settle_animations(window, visual);
    assert_eq!(selected(visual).as_deref(), Some("sidebar-tab:Changes"));
}

#[gpui::test]
fn to_dos_open_from_the_sidebar_and_the_project_switcher(cx: &mut TestAppContext) {
    use vorkspace_core::todos::TodoBoard;
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, _, session) = window(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1200.), px(800.)));
    window
        .update(visual, |shell, w, cx| {
            shell.workspace.project_rail_open = true;
            shell.open(session, w, cx);
            shell.set_sidebar_visible(true, cx);
            shell.invalidate_chrome(cx);
            cx.notify();
        })
        .unwrap();
    vorkspace_ui::clock::settle_animations(window, visual);
    // The project's switcher has a To-dos tab for the current project.
    let tab = visual.debug_bounds("classic-tab-to-dos").unwrap();
    visual.simulate_click(tab.center(), Default::default());
    vorkspace_ui::clock::settle_animations(window, visual);
    assert!(visual.debug_bounds("todo-board").is_some());
    window
        .update(visual, |shell, _, cx| {
            let board = shell
                .project_todos
                .as_ref()
                .unwrap()
                .read(cx)
                .board()
                .cloned();
            assert_eq!(
                board,
                Some(TodoBoard::Project(shell.workspace.project.clone()))
            );
        })
        .unwrap();
    // The left sidebar's To-dos page starts on the personal board.
    let entry = visual.debug_bounds("sidebar-navigation-To-dos").unwrap();
    visual.simulate_click(entry.center(), Default::default());
    vorkspace_ui::clock::settle_animations(window, visual);
    assert!(visual.debug_bounds("todos-page").is_some());
    window
        .update(visual, |shell, _, cx| {
            assert_eq!(
                shell.active_app_view,
                Some(crate::workspace::AppView::Todos)
            );
            let page = shell.todos_page.as_ref().unwrap().read(cx);
            assert_eq!(page.selected(cx), Some(TodoBoard::Personal));
        })
        .unwrap();
    // Its picker lists the project.
    let picker = visual.debug_bounds("todo-board-picker-button").unwrap();
    visual.simulate_click(picker.center(), Default::default());
    vorkspace_ui::clock::settle_animations(window, visual);
    let project = visual.debug_bounds("todo-board-choice-1").unwrap();
    visual.simulate_click(project.center(), Default::default());
    vorkspace_ui::clock::settle_animations(window, visual);
    window
        .update(visual, |shell, _, cx| {
            let page = shell.todos_page.as_ref().unwrap().read(cx);
            assert!(matches!(page.selected(cx), Some(TodoBoard::Project(_))));
        })
        .unwrap();
}
