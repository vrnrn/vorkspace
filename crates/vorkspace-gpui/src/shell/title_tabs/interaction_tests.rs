use super::*;
use crate::shell::tests::window;
use gpui::{Modifiers, TestAppContext, point};

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
    vorkspace_ui::clock::use_test_clock(cx);
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
    let TitleTab::Workspace(bot_pane) = &bot else {
        panic!("expected a Bot");
    };
    let id = window
        .update(cx, |shell, _, _| {
            let crate::workspace::AppPane::Bot(id) = &shell.workspace.app_panes[bot_pane].content
            else {
                panic!("expected a Bot pane");
            };
            id.clone()
        })
        .unwrap();
    let bot_row = format!("bot-rail-{id}");
    let session_row = format!("rail-session-{session}");
    assert!(sidebar_highlight(window, session_row.clone(), cx));
    assert!(!sidebar_highlight(window, bot_row.clone(), cx));
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
    assert!(sidebar_highlight(window, bot_row.clone(), cx));
    // The sidebar is a persistent toggle: it stays open over a Bot, and the
    // parked session's row is no longer the highlighted one.
    assert!(!sidebar_highlight(window, session_row.clone(), cx));
    window
        .update(cx, |shell, _, _| {
            assert_eq!(shell.workspace.active, bot.session());
            assert_eq!(shell.workspace.project, "/work/vorkspace");
            assert_eq!(
                shell.workspace.scope_focus.get(&shell.tab_scope()),
                Some(&bot),
                "the selected Bot tab is remembered in this workspace"
            );
        })
        .unwrap();
    // The sidebar header's project picker opens a project explicitly.
    window
        .update(cx, |shell, w, cx| {
            shell.open_project_from_user("/work/vorkspace".into(), w, cx)
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert!(
                !shell.bot_active(cx),
                "a project click opens the project, never the Bot it left"
            );
            assert_eq!(shell.workspace.project, "/work/vorkspace");
            assert_eq!(shell.workspace.active, Some(session));
        })
        .unwrap();
    // The Bot remains one click away on its own row.
    click(&bot_row, cx);
    window
        .update(cx, |shell, _, cx| assert!(shell.bot_active(cx)))
        .unwrap();
    let session_tab = format!("title-tab-{session}");
    click(&session_tab, cx);
    window
        .update(cx, |shell, _, cx| {
            assert!(!shell.bot_active(cx));
            assert_eq!(shell.workspace.active, Some(session));
            assert_eq!(
                shell.workspace.project_focus.get("/work/vorkspace"),
                Some(&session)
            );
            assert_eq!(
                shell.active_title_tab(cx),
                Some(TitleTab::Workspace(session))
            );
        })
        .unwrap();
    cx.run_until_parked();
    cx.refresh().unwrap();
    assert!(sidebar_highlight(window, session_row.clone(), cx));
    assert!(!sidebar_highlight(window, bot_row.clone(), cx));
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
        })
        .unwrap();
    cx.run_until_parked();
    cx.refresh().unwrap();
    click(&format!("title-tab-{bot_pane}"), cx);
    window
        .update(cx, |shell, _, _| {
            assert!(shell.selected_sessions.is_empty());
            assert!(shell.selection_anchor.is_none());
            assert!(shell.sidebar_target.is_none());
        })
        .unwrap();
    assert!(sidebar_highlight(window, bot_row.clone(), cx));
    for selector in [&session_row, &format!("rail-session-{other}")] {
        assert!(
            !sidebar_highlight(window, selector.clone(), cx),
            "over a Bot, session row {selector} stays listed but unhighlighted"
        );
    }
    click(&session_tab, cx);
    assert!(sidebar_highlight(window, session_row, cx));
    assert!(!sidebar_highlight(window, bot_row.clone(), cx));
    assert!(!sidebar_highlight(
        window,
        format!("rail-session-{other}"),
        cx
    ));
}

#[gpui::test]
fn bot_tabs_share_drag_keyboard_and_close_commands_with_other_tabs(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, other, session) = window(cx);
    let manager = window
        .update(cx, |shell, w, cx| {
            let manager = shell.bot_manager();
            manager.update(cx, |m, cx| {
                m.create_named(vec!["/work/vorkspace".into()], w, cx);
                m.dismiss_settings(w, cx);
            });
            manager
        })
        .unwrap();
    cx.run_until_parked();
    let (bot, terminal, chat) = window
        .update(cx, |shell, w, cx| {
            let bot = shell.active_title_tab(cx).unwrap();
            let chat = manager.read(cx).chat().unwrap();
            shell.open_terminal_tab(w, cx);
            let terminal = shell.active_title_tab(cx).unwrap();
            shell.open(session, w, cx);
            assert_eq!(
                shell.visible_tabs(cx),
                [TitleTab::Workspace(session), bot.clone(), terminal.clone()]
            );
            (bot, terminal, chat)
        })
        .unwrap();
    let TitleTab::Workspace(bot_pane) = &bot else {
        unreachable!()
    };
    let selector = Box::leak(format!("title-tab-{bot_pane}").into_boxed_str());
    let session_selector = Box::leak(format!("title-tab-{session}").into_boxed_str());
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1600.), px(1000.)));
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    let from = visual.debug_bounds(selector).unwrap();
    let to = visual.debug_bounds(session_selector).unwrap();
    visual.simulate_mouse_down(from.center(), MouseButton::Left, Default::default());
    visual.simulate_mouse_move(to.center(), MouseButton::Left, Default::default());
    visual.simulate_mouse_up(to.center(), MouseButton::Left, Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, w, cx| {
            assert_eq!(
                shell.visible_tabs(cx),
                [bot.clone(), TitleTab::Workspace(session), terminal.clone()]
            );
            shell.activate_tab(0, w, cx);
        })
        .unwrap();
    visual.run_until_parked();
    window
        .update(visual, |shell, w, cx| {
            assert_eq!(shell.active_title_tab(cx), Some(bot.clone()));
            assert_eq!(manager.read(cx).chat(), Some(chat.clone()));
            shell.cycle(1, w, cx);
            assert_eq!(
                shell.active_title_tab(cx),
                Some(TitleTab::Workspace(session))
            );
            shell.cycle(1, w, cx);
            assert_eq!(shell.active_title_tab(cx), Some(terminal));
        })
        .unwrap();
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    // A Bot's menu closes the same ordered set, even while another tab is active.
    let bounds = visual.debug_bounds(selector).unwrap();
    visual.simulate_mouse_down(bounds.center(), MouseButton::Right, Default::default());
    visual.run_until_parked();
    let others = visual.debug_bounds("menu-action-others").unwrap();
    visual.simulate_click(others.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, _, cx| {
            assert_eq!(
                shell.visible_tabs(cx).as_slice(),
                std::slice::from_ref(&bot)
            );
            assert_eq!(shell.active_title_tab(cx), Some(bot.clone()));
            assert!(
                shell.workspace.tabs.contains(&other),
                "another project's tabs stay open"
            );
            assert_eq!(manager.read(cx).chat(), Some(chat.clone()));
        })
        .unwrap();
    vorkspace_ui::clock::settle_animations(window, visual);
    let close_selector = Box::leak(format!("close-title-tab-{bot_pane}").into_boxed_str());
    let close = visual.debug_bounds(close_selector).unwrap();
    visual.simulate_mouse_move(close.center(), None, Default::default());
    visual.simulate_click(close.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, _, cx| {
            assert!(!shell.workspace.title_order.contains(&bot));
            assert!(!shell.bot_active(cx));
            assert!(matches!(
                shell.active_title_tab(cx),
                Some(TitleTab::Workspace(_))
            ));
            assert_eq!(
                shell.visible_tabs(cx).len(),
                1,
                "closing the last tab restores a session"
            );
        })
        .unwrap();
}
