//! Selections that are one thumb springing between items: the workspace tabs,
//! the title-bar tabs and Settings segments.
use super::*;
use gpui::{TestAppContext, VisualTestContext, WindowHandle, point, size};

/// Bounds of every solid `content(0.1)` thumb painted with control corners.
fn thumbs(window: WindowHandle<Shell>, visual: &mut VisualTestContext) -> Vec<Bounds<Pixels>> {
    window
        .update(visual, |_, window, _| {
            let scale = window.scale_factor();
            window
                .painted_quads()
                .into_iter()
                .filter(|quad| {
                    quad.background
                        .as_solid()
                        .is_some_and(|color| (color.a - 0.1).abs() < 0.001)
                        && (quad.corner_radii.top_left.0 - 6. * scale).abs() < 0.5
                })
                .map(|quad| {
                    Bounds::new(
                        point(
                            px(quad.bounds.left().0 / scale),
                            px(quad.bounds.top().0 / scale),
                        ),
                        size(
                            px(quad.bounds.size.width.0 / scale),
                            px(quad.bounds.size.height.0 / scale),
                        ),
                    )
                })
                .collect()
        })
        .unwrap()
}

fn frame(window: WindowHandle<Shell>, visual: &mut VisualTestContext, elapsed_ms: u64) {
    visual
        .executor()
        .advance_clock(std::time::Duration::from_millis(elapsed_ms));
    visual
        .update_window(window.into(), |_, window, cx| {
            window.simulate_next_frame(cx);
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .unwrap();
}

fn near(a: Bounds<Pixels>, b: Bounds<Pixels>) -> bool {
    (a.left() - b.left()).abs() < px(0.5)
        && (a.top() - b.top()).abs() < px(0.5)
        && (a.size.width - b.size.width).abs() < px(0.5)
}

#[gpui::test]
fn the_workspace_tab_thumb_glides_to_the_new_tab_and_lands(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, _, _) = super::tests::window(cx);
    window
        .update(cx, |shell, _, cx| {
            shell.invalidate_chrome(cx);
            cx.notify();
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    let sessions = visual.debug_bounds("classic-tab-explorer").unwrap();
    let explorer = visual.debug_bounds("classic-tab-changes").unwrap();
    // At rest, exactly one thumb sits on the selected tab.
    let resting: Vec<_> = thumbs(window, visual)
        .into_iter()
        .filter(|b| (b.top() - sessions.top()).abs() < px(0.5))
        .collect();
    assert_eq!(resting.len(), 1, "{resting:?}");
    assert!(
        near(resting[0], sessions),
        "{:?} vs {sessions:?}",
        resting[0]
    );

    visual.simulate_click(explorer.center(), Default::default());
    visual.run_until_parked();
    // A frame into the move the thumb is between the tabs: one shape travels,
    // rather than one fill disappearing and another appearing.
    frame(window, visual, 16);
    frame(window, visual, 16);
    let moving: Vec<_> = thumbs(window, visual)
        .into_iter()
        .filter(|b| (b.top() - sessions.top()).abs() < px(0.5))
        .collect();
    assert_eq!(moving.len(), 1, "{moving:?}");
    assert!(
        moving[0].left() > sessions.left() + px(0.5)
            && moving[0].left() < explorer.left() - px(0.5),
        "{:?} travels from {sessions:?} to {explorer:?}",
        moving[0]
    );

    // It lands exactly on the new tab and stops asking for frames.
    for _ in 0..30 {
        frame(window, visual, 16);
    }
    let landed: Vec<_> = thumbs(window, visual)
        .into_iter()
        .filter(|b| (b.top() - explorer.top()).abs() < px(0.5))
        .collect();
    assert_eq!(landed.len(), 1, "{landed:?}");
    assert!(near(landed[0], explorer), "{:?} vs {explorer:?}", landed[0]);
}

#[gpui::test]
fn the_title_tab_thumb_glides_between_tabs_and_lands_on_the_plate(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, a, b) = super::tests::same_project_window(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    let plate = |visual: &mut VisualTestContext, id: i64| {
        visual
            .debug_bounds(Box::leak(format!("title-tab-plate-{id}").into_boxed_str()))
            .unwrap()
    };
    let active = window
        .update(visual, |shell, _, _| shell.workspace.active_tab())
        .unwrap()
        .unwrap();
    let other = if active == a { b } else { a };
    let (from, to) = (plate(visual, active), plate(visual, other));
    let in_strip = |thumbs: Vec<Bounds<Pixels>>| -> Vec<Bounds<Pixels>> {
        thumbs
            .into_iter()
            .filter(|t| (t.top() - from.top()).abs() < px(0.5))
            .collect()
    };
    // At rest the thumb is the selected plate's fill, and only one is drawn.
    let resting = in_strip(thumbs(window, visual));
    assert_eq!(resting.len(), 1, "{resting:?}");
    assert!(near(resting[0], from), "{:?} vs {from:?}", resting[0]);

    let tab = visual
        .debug_bounds(Box::leak(format!("title-tab-{other}").into_boxed_str()))
        .unwrap();
    visual.simulate_click(tab.center(), Default::default());
    visual.run_until_parked();
    frame(window, visual, 16);
    frame(window, visual, 16);
    let moving = in_strip(thumbs(window, visual));
    assert_eq!(moving.len(), 1, "one shape travels: {moving:?}");
    let (low, high) = if from.left() < to.left() {
        (from.left(), to.left())
    } else {
        (to.left(), from.left())
    };
    assert!(
        moving[0].left() > low + px(0.5) && moving[0].left() < high - px(0.5),
        "{:?} travels from {from:?} to {to:?}",
        moving[0]
    );

    for _ in 0..30 {
        frame(window, visual, 16);
    }
    let to = plate(visual, other);
    let landed = in_strip(thumbs(window, visual));
    assert_eq!(landed.len(), 1, "{landed:?}");
    assert!(near(landed[0], to), "{:?} vs {to:?}", landed[0]);
}

#[gpui::test]
fn a_settings_segment_thumb_slides_to_the_chosen_option(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, _, _) = super::tests::window(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.open_settings(crate::settings::Section::Appearance, w, cx)
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    // Segment thumbs carry the frame's inner corners: control less its inset.
    let segments = |visual: &mut VisualTestContext, top: Pixels| -> Vec<Bounds<Pixels>> {
        window
            .update(visual, |_, window, _| {
                let scale = window.scale_factor();
                window
                    .painted_quads()
                    .into_iter()
                    .filter(|quad| {
                        quad.background
                            .as_solid()
                            .is_some_and(|color| (color.a - 0.1).abs() < 0.001)
                            && (quad.corner_radii.top_left.0
                                - (vorkspace_ui::radius::CONTROL - 3.) * scale)
                                .abs()
                                < 0.5
                            && (quad.bounds.top().0 / scale - top.as_f32()).abs() < 0.5
                    })
                    .map(|quad| {
                        Bounds::new(
                            point(
                                px(quad.bounds.left().0 / scale),
                                px(quad.bounds.top().0 / scale),
                            ),
                            size(
                                px(quad.bounds.size.width.0 / scale),
                                px(quad.bounds.size.height.0 / scale),
                            ),
                        )
                    })
                    .collect()
            })
            .unwrap()
    };
    let from = visual.debug_bounds("settings-diff-colors-default").unwrap();
    let to = visual
        .debug_bounds("settings-diff-colors-high-contrast")
        .unwrap();
    let resting = segments(visual, from.top());
    assert_eq!(resting.len(), 1, "{resting:?}");
    assert!(near(resting[0], from), "{:?} vs {from:?}", resting[0]);

    visual.simulate_click(to.center(), Default::default());
    visual.run_until_parked();
    frame(window, visual, 16);
    frame(window, visual, 16);
    let moving = segments(visual, from.top());
    assert_eq!(moving.len(), 1, "one shape travels: {moving:?}");
    assert!(
        moving[0].left() > from.left() + px(0.5) && moving[0].left() < to.left() - px(0.5),
        "{:?} travels from {from:?} to {to:?}",
        moving[0]
    );
    for _ in 0..30 {
        frame(window, visual, 16);
    }
    let landed = segments(visual, to.top());
    assert_eq!(landed.len(), 1, "{landed:?}");
    assert!(near(landed[0], to), "{:?} vs {to:?}", landed[0]);
}
