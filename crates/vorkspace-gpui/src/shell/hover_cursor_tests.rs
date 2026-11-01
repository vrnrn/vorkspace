//! Cursor dispatch through the complete application shell, including cached panes.
use super::*;
use gpui::{CursorStyle, TestAppContext, VisualTestContext, point, size};

#[gpui::test]
fn hover_cursor_composer_through_session_and_bot_shell(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let (window, _, session) = tests::window(cx);
    window
        .update(cx, |shell, window, cx| shell.open(session, window, cx))
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1400.), px(1000.)));
    visual.run_until_parked();
    for bot in [false, true] {
        if bot {
            window
                .update(visual, |shell, window, cx| {
                    let manager = shell.bot_manager();
                    let bot = manager.update(cx, |manager, cx| {
                        manager.create_named(Vec::new(), window, cx);
                        manager.dismiss_settings(window, cx);
                        manager.active.clone().unwrap()
                    });
                    shell.click_bot_tab(&bot, window, cx);
                    shell.invalidate_chrome(cx);
                    cx.notify();
                })
                .unwrap();
        }
        for percent in [100, 150, 100] {
            window
                .update(visual, |shell, _, cx| {
                    shell.workspace.preferences.ui_scale = percent;
                    cx.notify();
                })
                .unwrap();
            visual.run_until_parked();
            visual.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
            visual.run_until_parked();
            let bounds = visual
                .debug_bounds("composer-box")
                .unwrap_or_else(|| panic!("composer Bot={bot}, zoom={percent}"));
            let scale = percent as f32 / 100.;
            let position = point(bounds.center().x, bounds.top() + px(16.));
            visual.simulate_mouse_move(
                point(position.x * scale, position.y * scale),
                None,
                Default::default(),
            );
            visual.run_until_parked();
            assert_eq!(
                visual.cursor_style(),
                CursorStyle::IBeam,
                "Bot={bot}, zoom={percent}"
            );
            visual.refresh().unwrap();
            assert_eq!(visual.cursor_style(), CursorStyle::IBeam, "cached repaint");
        }
    }
}
