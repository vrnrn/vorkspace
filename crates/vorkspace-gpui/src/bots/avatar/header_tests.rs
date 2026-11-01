
use super::*;

#[gpui::test]
fn avatar_clock_origin_follows_the_bound_executor(cx: &mut gpui::TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let first_origin = avatar_start();
    cx.background_executor
        .advance_clock(Duration::from_secs(100));
    vorkspace_ui::clock::use_test_clock(cx);
    assert_eq!(
        avatar_start(),
        first_origin,
        "rebinding the same executor must preserve its origin"
    );
    assert_eq!(avatar_start().elapsed(), Duration::from_secs(100));
    let other = gpui::TestAppContext::build(gpui::TestDispatcher::new(42), None);
    vorkspace_ui::clock::use_test_clock(&other);
    assert_eq!(
        avatar_start(),
        Instant::now(),
        "a new executor must get a fresh origin"
    );
    assert_eq!(avatar_start().elapsed(), Duration::ZERO);
}

#[gpui::test]
fn pending_avatar_frame_keeps_the_earliest_wake_and_releases_on_cancellation(
    cx: &mut gpui::TestAppContext,
) {
    let first = cx.new(|_| ());
    let second = cx.new(|_| ());
    let open = |cx: &mut gpui::App| -> gpui::AnyWindowHandle {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|_| AvatarFrameTestView(None))
        })
        .unwrap()
        .into()
    };
    let window: gpui::AnyWindowHandle = cx.update(open);
    let other_window: gpui::AnyWindowHandle = cx.update(open);
    let id = (window, first.entity_id());
    let at = |ms| avatar_start() + Duration::from_millis(ms);
    let pending = PendingAvatarFrame::acquire(id, at(100)).unwrap();
    // An equal or later wake is already covered.
    assert!(PendingAvatarFrame::acquire(id, at(100)).is_none());
    assert!(PendingAvatarFrame::acquire(id, at(4_000)).is_none());
    // A sooner wake supersedes; the superseded owner can no longer
    // notify, and its cancellation must not release its successor.
    let sooner = PendingAvatarFrame::acquire(id, at(33)).unwrap();
    assert!(!pending.is_current() && sooner.is_current());
    drop(pending);
    assert!(PendingAvatarFrame::acquire(id, at(66)).is_none());
    // Other owning views and windows are independently admitted.
    let other = PendingAvatarFrame::acquire((window, second.entity_id()), at(100)).unwrap();
    let another_window =
        PendingAvatarFrame::acquire((other_window, first.entity_id()), at(100)).unwrap();
    let unpolled = async move {
        let _pending = sooner;
        std::future::pending::<()>().await;
    };
    drop(unpolled);
    let replacement = PendingAvatarFrame::acquire(id, at(4_000)).unwrap();
    // Also cancel after a poll reached an await boundary.
    let mut polled = Box::pin(async move {
        let _pending = replacement;
        std::future::pending::<()>().await;
    });
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    assert!(std::future::Future::poll(polled.as_mut(), &mut context).is_pending());
    drop(polled);
    assert!(PendingAvatarFrame::acquire(id, at(4_000)).is_some());
    drop(other);
    drop(another_window);
    PENDING_FRAMES.with(|pending| assert!(pending.borrow().is_empty()));
}

#[gpui::test]
fn avatar_frame_coalesces_supersedes_and_releases_after_timer(cx: &mut gpui::TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let renders = std::rc::Rc::new(std::cell::Cell::new(0));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|_| AvatarFrameTestView(Some(vec![AVATAR_FRAME * 30, AVATAR_FRAME])))
        })
        .unwrap()
    });
    let counter = renders.clone();
    let view = window.root(cx).unwrap();
    let _observer = cx.update(|cx| cx.observe(&view, move |_, _| counter.set(counter.get() + 1)));
    cx.refresh().unwrap();
    cx.run_until_parked();
    PENDING_FRAMES.with(|pending| assert_eq!(pending.borrow().len(), 1));
    // The sooner of the two wakes notifies once; the superseded timer is
    // silent when it later expires.
    cx.background_executor.advance_clock(AVATAR_FRAME * 2);
    cx.run_until_parked();
    assert_eq!(renders.get(), 1);
    PENDING_FRAMES.with(|pending| assert!(pending.borrow().is_empty()));
    cx.background_executor.advance_clock(AVATAR_FRAME * 30);
    cx.run_until_parked();
    assert_eq!(renders.get(), 1);
    window
        .update(cx, |view, _, cx| {
            view.0 = Some(vec![AVATAR_FRAME]);
            cx.notify();
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    PENDING_FRAMES.with(|pending| assert_eq!(pending.borrow().len(), 1));
    window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    cx.background_executor.advance_clock(AVATAR_FRAME * 2);
    cx.run_until_parked();
    PENDING_FRAMES.with(|pending| assert!(pending.borrow().is_empty()));
}

#[gpui::test]
fn due_avatar_wake_is_not_replaced_by_a_later_deadline(cx: &mut gpui::TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|_| AvatarFrameTestView(None))
        })
        .unwrap()
    });
    let view = window.root(cx).unwrap();
    let notifications = std::rc::Rc::new(std::cell::Cell::new(0));
    let counter = notifications.clone();
    let _observer = cx.update(|cx| cx.observe(&view, move |_, _| counter.set(counter.get() + 1)));
    let key = (window.into(), view.entity_id());
    cx.update(|cx| {
        let pending = PendingAvatarFrame::acquire(key, Instant::now() - AVATAR_FRAME).unwrap();
        let generation = pending.generation;
        let view_id = view.entity_id();
        // Model a due timer queued but not yet polled by the executor.
        let task = cx.spawn(async move |cx| {
            let current = pending.is_current();
            drop(pending);
            if current {
                cx.update(|cx| cx.notify(view_id));
            }
        });
        PendingAvatarFrame::attach_to(key, generation, task);
        assert!(PendingAvatarFrame::acquire(key, Instant::now() + AVATAR_FRAME * 5).is_none());
    });
    cx.run_until_parked();
    assert_eq!(notifications.get(), 1);
    PENDING_FRAMES.with(|pending| assert!(pending.borrow().is_empty()));
}

/// Requests each listed delay (relative to now) on its next paint.
struct AvatarFrameTestView(Option<Vec<Duration>>);
impl gpui::Render for AvatarFrameTestView {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        let delays = self.0.take().unwrap_or_default();
        gpui::canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                for delay in &delays {
                    request_avatar_frame(window, cx, avatar_start().elapsed() + *delay);
                }
            },
        )
        .size(px(20.))
    }
}

#[test]
fn header_modes_share_motion_without_pointer_gaze() {
    let look = BotLookConfig::from_legacy("robot", 0x3b82f6);
    let at = Duration::from_secs(6);
    for status in ["idle", "working", "needs-you"] {
        let (header, next) = header_anim_at(look.seed, status, true, at);
        let (autonomous, _) = bot_anim_at(look.seed, status, true, at, None);
        let (tracking, _) = bot_anim_at(look.seed, status, true, at, Some((0.78, 0.68)));
        let key = |anim| frame_cache_key(true, &look, status == "needs-you", 128, anim);
        assert!(next.is_some_and(|next| next > at));
        assert!(key(header) == key(autonomous));
        assert!(key(header) != key(tracking));
        assert_eq!(tracking.yaw, 0.78);
        assert_eq!(tracking.pitch, 0.68);
        assert!(!header.crossed_eyes);
    }
    let (working, next) = header_anim_at(look.seed, "working", true, at);
    let (attention, _) = header_anim_at(look.seed, "needs-you", true, at);
    assert_eq!(working.tex_rot, 84.);
    assert_eq!(next, Some(Duration::from_millis(6_006)));
    assert_eq!(attention.wide, 1.14);
}

#[test]
fn header_hidden_state_is_frozen_without_frame_requests() {
    let look = BotLookConfig::from_legacy("cat", 0xe86b5f);
    for status in ["idle", "working", "needs-you"] {
        let (first, next) = header_anim_at(look.seed, status, false, Duration::ZERO);
        let (later, later_next) =
            header_anim_at(look.seed, status, false, Duration::from_secs(900));
        assert!(next.is_none() && later_next.is_none());
        assert!(
            frame_cache_key(true, &look, status == "needs-you", 128, first)
                == frame_cache_key(true, &look, status == "needs-you", 128, later)
        );
        assert_eq!(first.blink, 1.);
        assert_eq!(first.hop_y, 0.);
        assert_eq!(first.tex_rot, 0.);
    }
}
