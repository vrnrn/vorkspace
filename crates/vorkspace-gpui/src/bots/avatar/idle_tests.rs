use super::*;
use gpui::{AppContext, Context, Entity, Render, TestAppContext, div};
use std::{cell::Cell, rc::Rc, time::Duration};

struct Avatars {
    status: &'static str,
    renders: Rc<Cell<usize>>,
}

impl Render for Avatars {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        let bot = Bot::default();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(sidebar_cursor_tracker())
            .child(header_avatar(
                BotLookConfig::from_bot(&bot),
                self.status,
                64.,
                true,
            ))
            .child(bot_avatar(&bot, self.status, 24., false))
            .child(settings_preview(&bot, 128., Default::default(), window, cx))
    }
}

fn open(
    cx: &mut TestAppContext,
) -> (
    gpui::WindowHandle<Avatars>,
    Entity<Avatars>,
    Rc<Cell<usize>>,
) {
    let renders = Rc::new(Cell::new(0));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |window, cx| {
            window.activate_window();
            cx.new(|_| Avatars {
                status: "idle",
                renders: renders.clone(),
            })
        })
        .unwrap()
    });
    cx.run_until_parked();
    let view = window.root(cx).unwrap();
    cx.refresh().unwrap();
    window
        .update(cx, |_, window, _| assert!(window.is_window_active()))
        .unwrap();
    (window, view, renders)
}

#[gpui::test]
fn all_idle_avatar_surfaces_leave_no_timer_or_display_frame_loop(cx: &mut TestAppContext) {
    let (window, _, renders) = open(cx);
    let painted = renders.get();
    PENDING_FRAMES.with(|pending| assert!(pending.borrow().is_empty()));
    for _ in 0..120 {
        cx.background_executor
            .advance_clock(Duration::from_millis(33));
        cx.run_until_parked();
        let callbacks = window
            .update(cx, |_, window, cx| window.simulate_next_frame(cx))
            .unwrap();
        assert_eq!(
            callbacks, 0,
            "Idle previews must not schedule display callbacks"
        );
    }
    assert_eq!(
        renders.get(),
        painted,
        "Clock passage must not redraw idle avatars"
    );
    PENDING_FRAMES.with(|pending| assert!(pending.borrow().is_empty()));
}

#[gpui::test]
fn working_and_attention_ticks_stop_after_returning_to_idle(cx: &mut TestAppContext) {
    let (window, view, _) = open(cx);
    for status in ["working", "needs-you"] {
        view.update(cx, |view, cx| {
            view.status = status;
            cx.notify();
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        PENDING_FRAMES.with(|pending| assert!(!pending.borrow().is_empty()));
        view.update(cx, |view, cx| {
            view.status = "idle";
            cx.notify();
        });
        cx.refresh().unwrap();
        // One already admitted tick may drain; it must not re-arm itself.
        cx.background_executor.advance_clock(AVATAR_FRAME);
        cx.run_until_parked();
        cx.refresh().unwrap();
        PENDING_FRAMES.with(|pending| assert!(pending.borrow().is_empty()));
        assert_eq!(
            window
                .update(cx, |_, w, cx| w.simulate_next_frame(cx))
                .unwrap(),
            0
        );
    }
}

#[test]
fn idle_gaze_changes_only_with_input_and_rail_tracking_invalidates_enter_move_exit() {
    let look = BotLookConfig::from_legacy("robot", 0x3b82f6);
    let key = |anim| frame_cache_key(true, &look, false, 128, anim);
    let (rest, moving) = header_anim_at(look.seed, "idle", true, 0.);
    assert!(!moving);
    for now in [1., 6., 900.] {
        let (later, moving) = header_anim_at(look.seed, "idle", true, now);
        assert!(!moving);
        assert!(key(rest) == key(later));
    }
    let (gaze, moving) = bot_anim_at(look.seed, "idle", true, 6., Some((0.6, -0.4)));
    assert!(!moving);
    assert!(key(rest) != key(gaze));
    assert_eq!((gaze.yaw, gaze.pitch), (0.6, -0.4));
    let mut cursor = SidebarCursorState {
        rail_bounds: Some(Bounds::new(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(100.), px(200.)),
        )),
        mouse_pos: None,
    };
    let outside = gpui::point(px(300.), px(10.));
    let inside = gpui::point(px(10.), px(10.));
    assert!(!cursor.track(outside));
    assert!(cursor.track(inside));
    assert!(!cursor.track(inside));
    assert!(cursor.track(gpui::point(px(20.), px(10.))));
    assert!(cursor.track(outside));
    assert!(!cursor.track(gpui::point(px(400.), px(10.))));
}
