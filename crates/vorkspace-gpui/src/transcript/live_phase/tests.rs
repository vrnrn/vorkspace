use super::super::{Body, TranscriptView};
use super::*;
use gpui::{
    ScrollDelta, ScrollWheelEvent, TestAppContext, TouchPhase, WindowBounds, WindowOptions, point,
    size,
};
use std::time::Duration;
use vorkspace_core::{tool::ToolCall, transcript::Row};

fn row(role: &str, text: &str) -> Row {
    Row {
        role: role.into(),
        text: text.into(),
        first: true,
        sent_at: None,
    }
}
fn tool(i: usize) -> Row {
    let mut call = ToolCall::new(&format!("read-{i}"), "Read");
    call.input = format!(r#"{{"file_path":"src/file-{i}.rs"}}"#);
    row("tool", &serde_json::to_string(&call).unwrap())
}
fn wheel_event(position: gpui::Point<Pixels>, delta: f32) -> ScrollWheelEvent {
    ScrollWheelEvent {
        position,
        delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
        modifiers: Default::default(),
        touch_phase: TouchPhase::Moved,
    }
}

#[test]
fn fade_lengths_ease_for_160ms_and_reverse_from_the_current_value() {
    let mut edges = Edges::default();
    let now = Instant::now();
    assert_eq!(edges.update((24., 32.), now), (0., 0.));
    let halfway = now + Duration::from_millis(80);
    let (top, bottom) = edges.value(halfway);
    assert!(top > 12. && top < 24.);
    assert!((bottom / top - 32. / 24.).abs() < 0.001);
    assert_eq!(edges.update((0., 32.), halfway), (top, bottom));
    assert_eq!(edges.value(halfway + Duration::from_millis(160)), (0., 32.));
}

// Finish the actual staggered queue (not just one disclosure duration),
// then paint the resulting layout and the finite edge fades.
fn settle_phase(window: gpui::WindowHandle<TranscriptView>, visual: &mut gpui::VisualTestContext) {
    let now = Instant::now();
    let through = window
        .read_with(visual, |view, _| {
            view.motions
                .values()
                .map(|motion| motion.start + motion.duration)
                .max()
                .unwrap_or(now)
        })
        .unwrap();
    visual
        .executor()
        .advance_clock(through.saturating_duration_since(now));
    vorkspace_ui::clock::settle_animations(window, visual);
}

#[gpui::test]
fn live_group_caps_height_follows_appends_and_respects_nested_wheels(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let window = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(800.), px(800.)),
                ))),
                ..Default::default()
            },
            |_, cx| cx.new(|_| TranscriptView::new()),
        )
        .unwrap()
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    let outer_wheels = Rc::new(Cell::new(0));
    window
        .update(visual, |view, _, cx| {
            let wheels = outer_wheels.clone();
            view.scroll_bound = true;
            view.list
                .set_scroll_handler(move |_, _, _| wheels.set(wheels.get() + 1));
            view.streaming(true, cx);
            let mut rows = vec![row("user", "Inspect the project")];
            rows.extend((0..24).map(tool));
            view.patch(0, rows, cx);
        })
        .unwrap();
    visual.run_until_parked();
    settle_phase(window, visual);
    let (position, outer) = window
        .update(visual, |view, _, _| {
            let scroll = &view.phase_scrolls[&1];
            assert_eq!(scroll.handle.bounds().size.height, px(280.));
            assert!(scroll.handle.max_offset().y > px(300.));
            assert_eq!(scroll.handle.offset().y, -scroll.handle.max_offset().y);
            assert_eq!(scroll.edges.borrow().frame, (24., 0.));
            let header = view
                .rows
                .iter()
                .position(|r| matches!(r.body, Body::Phase { group: 1, .. }))
                .unwrap();
            assert_eq!(
                view.list.bounds_for_item(header).unwrap().size.height,
                px(308.)
            );
            (
                scroll.handle.bounds().center(),
                view.list.logical_scroll_top(),
            )
        })
        .unwrap();
    visual.simulate_event(wheel_event(position, 80.));
    visual.run_until_parked();
    settle_phase(window, visual);
    let paused = window
        .update(visual, |view, _, _| {
            let scroll = &view.phase_scrolls[&1];
            assert!(!scroll.following.get());
            assert_eq!(
                scroll.handle.offset().y,
                -scroll.handle.max_offset().y + px(80.)
            );
            assert_eq!(scroll.edges.borrow().frame, (24., 32.));
            let after = view.list.logical_scroll_top();
            assert_eq!(
                (after.item_ix, after.offset_in_item),
                (outer.item_ix, outer.offset_in_item)
            );
            scroll.handle.offset().y
        })
        .unwrap();
    window
        .update(visual, |view, _, cx| {
            view.patch(view.source.len(), vec![tool(24)], cx);
        })
        .unwrap();
    visual.run_until_parked();
    settle_phase(window, visual);
    window
        .update(visual, |view, _, _| {
            assert_eq!(
                view.phase_scrolls[&1].handle.offset().y,
                paused,
                "new calls must not move a reader who scrolled up"
            );
        })
        .unwrap();
    visual.simulate_event(wheel_event(position, -20.));
    visual.run_until_parked();
    settle_phase(window, visual);
    window
        .update(visual, |view, _, _| {
            assert!(!view.phase_scrolls[&1].following.get())
        })
        .unwrap();
    visual.simulate_event(wheel_event(position, -10000.));
    visual.run_until_parked();
    settle_phase(window, visual);
    window
        .update(visual, |view, _, cx| {
            assert!(view.phase_scrolls[&1].following.get());
            view.patch(view.source.len(), vec![tool(25)], cx);
        })
        .unwrap();
    visual.run_until_parked();
    settle_phase(window, visual);
    window
        .update(visual, |view, _, _| {
            let scroll = &view.phase_scrolls[&1];
            assert_eq!(scroll.handle.offset().y, -scroll.handle.max_offset().y);
        })
        .unwrap();
    // At the top, only the bottom edge fades. The next upward wheel
    // escapes the phase and reaches the transcript's scroll handler.
    visual.simulate_event(wheel_event(position, 10000.));
    visual.run_until_parked();
    settle_phase(window, visual);
    window
        .update(visual, |view, _, _| {
            assert_eq!(view.phase_scrolls[&1].handle.offset().y, px(0.));
            assert_eq!(view.phase_scrolls[&1].edges.borrow().frame, (0., 32.));
            assert_eq!(outer_wheels.get(), 0);
        })
        .unwrap();
    visual.simulate_event(wheel_event(position, 80.));
    visual.run_until_parked();
    settle_phase(window, visual);
    assert_eq!(outer_wheels.get(), 1);
    visual.simulate_window_resize(window.into(), size(px(800.), px(400.)));
    visual.run_until_parked();
    settle_phase(window, visual);
    window
        .update(visual, |view, _, cx| {
            assert_eq!(view.phase_scrolls[&1].handle.bounds().size.height, px(180.));
            view.streaming(false, cx);
            view.toggle_phase(1, true, cx);
        })
        .unwrap();
    visual.run_until_parked();
    settle_phase(window, visual);
    window
        .update(visual, |view, _, _| {
            assert!(!view.windowed_phase(1));
            assert!(
                view.list.max_offset_for_scrollbar().y > px(300.),
                "finished groups reopen at natural height"
            );
        })
        .unwrap();
}

#[gpui::test]
fn short_groups_have_no_fades_and_live_groups_collapse_from_the_capped_height(
    cx: &mut TestAppContext,
) {
    vorkspace_ui::clock::use_test_clock(cx);
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|_| TranscriptView::new())
        })
        .unwrap()
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |view, _, cx| {
            view.streaming(true, cx);
            view.patch(0, vec![row("user", "Check"), tool(0), tool(1)], cx);
        })
        .unwrap();
    visual.run_until_parked();
    settle_phase(window, visual);
    window
        .update(visual, |view, _, cx| {
            let scroll = &view.phase_scrolls[&1];
            assert_eq!(scroll.handle.max_offset().y, px(0.));
            assert_eq!(scroll.edges.borrow().frame, (0., 0.));
            view.patch(3, (2..24).map(tool).collect(), cx);
        })
        .unwrap();
    visual.run_until_parked();
    settle_phase(window, visual);
    window
        .update(visual, |view, _, cx| {
            vorkspace_ui::clock::use_test_clock(cx);
            view.streaming(false, cx);
            assert!(view.rows.iter().any(|r| r.phase_group() == Some(1)));
            let now = Instant::now();
            for motion in view.motions.values_mut() {
                assert_eq!(motion.group, Some(1));
                assert!(!motion.opening);
                *motion =
                    vorkspace_ui::reveal::Motion::phase(1, false, now - Duration::from_secs(25));
                motion.duration = Duration::from_secs(100);
            }
            view.visible = false;
            cx.notify();
        })
        .unwrap();
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            let progress = view
                .motions
                .values()
                .next()
                .unwrap()
                .value(view.frame_now)
                .0;
            let header = view
                .rows
                .iter()
                .position(|r| matches!(r.body, Body::Phase { group: 1, .. }))
                .unwrap();
            let height = view.list.bounds_for_item(header).unwrap().size.height;
            assert!(
                (height - (px(28.) + view.phase_max_height * progress)).abs() < px(1.),
                "collapse starts from the window, with no flash of the full tool list: {height:?}"
            );
        })
        .unwrap();
}
