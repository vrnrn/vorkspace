
use super::*;
use crate::shell::tests::window;
use gpui::{Modifiers, TestAppContext, point};

/// The selected rail row is painted by `crate::liquid`, not as a quad.
fn pill_at(window: gpui::WindowHandle<Shell>, key: &str, cx: &mut TestAppContext) -> bool {
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            shell.liquid.selected().as_deref() == Some(key)
        })
        .unwrap()
}
fn sidebar_highlight(
    window: gpui::WindowHandle<Shell>,
    selector: String,
    cx: &mut TestAppContext,
) -> bool {
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    let selector = Box::leak(selector.into_boxed_str());
    let bounds = visual
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("the sidebar row {selector} is visible"));
    window
        .update(visual, |_, window, _| {
            let scale = window.scale_factor();
            window.painted_quads().into_iter().any(|quad| {
                (quad.bounds.left().0 - f32::from(bounds.left()) * scale).abs() < 1.
                    && (quad.bounds.top().0 - f32::from(bounds.top()) * scale).abs() < 1.
                    && (quad.bounds.size.width.0 - f32::from(bounds.size.width) * scale).abs() < 1.
                    && (quad.bounds.size.height.0 - f32::from(bounds.size.height) * scale).abs()
                        < 1.
                    && quad
                        .background
                        .as_solid()
                        .is_some_and(|color| (color.a - 0.1).abs() < 0.001)
            })
        })
        .unwrap()
}

#[gpui::test]
fn sidebar_highlights_only_the_visible_bot_or_session(cx: &mut TestAppContext) {
    let (window, other, session) = window(cx);
    window
        .update(cx, |shell, w, cx| {
            for id in [other, session] {
                shell.sessions[&id].update(cx, |view, _| {
                    view.summary.cwd = "/work/vorkspace".into();
                    view.summary.has_messages = true;
                });
            }
            shell.bot_manager().update(cx, |m, cx| {
                m.create_named(vec!["/work/vorkspace".into()], w, cx)
            });
        })
        .unwrap();
    cx.run_until_parked();
    let bot = window
        .update(cx, |shell, w, cx| {
            let bot = shell.active_title_tab(cx).unwrap();
            shell.open(session, w, cx);
            bot
        })
        .unwrap();
    cx.run_until_parked();
    cx.refresh().unwrap();
    let TitleTab::Bot { id, .. } = &bot else {
        panic!("expected a Bot");
    };
    let bot_row = format!("bot-rail-{id}");
    let bot_key = format!("bot:{id}");
    let session_row = format!("sidebar-session-{session}");
    let project_row = window
        .update(cx, |shell, _, _| {
            let project = shell
                .workspace
                .projects
                .iter()
                .position(|project| project == "/work/vorkspace")
                .unwrap();
            format!("classic-project-{project}")
        })
        .unwrap();
    assert!(sidebar_highlight(window, session_row.clone(), cx));
    assert!(!pill_at(window, &bot_key, cx));
    let click = |selector: &str, cx: &mut TestAppContext| {
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        let bounds = visual
            .debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
            .unwrap();
        visual.simulate_click(bounds.center(), Default::default());
        visual.simulate_mouse_move(point(px(5.), px(5.)), None, Default::default());
        visual.run_until_parked();
        visual.refresh().unwrap();
    };
    click(&bot_row, cx);
    assert!(pill_at(window, &bot_key, cx));
    assert!(
        gpui::VisualTestContext::from_window(window.into(), cx)
            .debug_bounds(Box::leak(session_row.clone().into_boxed_str()))
            .is_none(),
        "the original Bot view hides the parked session pane"
    );
    window
        .update(cx, |shell, _, _| {
            assert_eq!(shell.workspace.active, Some(session));
            assert_eq!(shell.workspace.project, "/work/vorkspace");
            assert_ne!(
                shell.workspace.scope_focus.get(&shell.tab_scope()),
                Some(&bot),
                "a Bot page is never remembered as a tab"
            );
        })
        .unwrap();
    click(&project_row, cx);
    window
        .update(cx, |shell, _, cx| {
            assert!(!shell.bot_active(cx));
            assert_eq!(shell.workspace.project, "/work/vorkspace");
            assert_eq!(shell.workspace.active, Some(session));
            assert_eq!(
                shell.workspace.project_focus.get("/work/vorkspace"),
                Some(&session)
            );
            assert_eq!(shell.active_title_tab(cx), Some(TitleTab::Session(session)));
        })
        .unwrap();
    assert!(sidebar_highlight(window, session_row.clone(), cx));
    assert!(!pill_at(window, &bot_key, cx));
    // Opening from the title strip must also clear an old bulk selection.
    window
        .update(cx, |shell, w, cx| {
            for id in [other, session] {
                shell.select_sidebar_session(
                    id,
                    Modifiers {
                        platform: true,
                        ..Default::default()
                    },
                    w,
                    cx,
                );
            }
            assert_eq!(shell.selected_sessions.len(), 2);
            shell.open_title_tab(&bot, w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    cx.refresh().unwrap();
    window
        .update(cx, |shell, _, _| {
            assert!(shell.selected_sessions.is_empty());
            assert!(shell.selection_anchor.is_none());
            assert!(shell.sidebar_target.is_none());
        })
        .unwrap();
    assert!(pill_at(window, &bot_key, cx));
    for selector in [&session_row, &format!("sidebar-session-{other}")] {
        assert!(
            gpui::VisualTestContext::from_window(window.into(), cx)
                .debug_bounds(Box::leak(selector.clone().into_boxed_str()))
                .is_none(),
            "the Bot surface hides session row {selector}"
        );
    }
    click(&project_row, cx);
    assert!(sidebar_highlight(window, session_row, cx));
    assert!(!pill_at(window, &bot_key, cx));
    assert!(!sidebar_highlight(
        window,
        format!("sidebar-session-{other}"),
        cx
    ));
}
