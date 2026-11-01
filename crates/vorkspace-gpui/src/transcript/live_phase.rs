//! ActivityPhaseGroup's bounded live window and useLivePhaseScroll.
use gpui::{
    AnyElement, Bounds, Context, Pixels, ScrollHandle, Window, canvas, div, prelude::*, px,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use vorkspace_ui::clock::Instant;

#[derive(Default)]
pub(super) struct LiveScroll {
    pub handle: ScrollHandle,
    following: Rc<Cell<bool>>,
    enabled: bool,
    edges: Rc<RefCell<Edges>>,
}
impl LiveScroll {
    pub fn enable(&mut self, enabled: bool) {
        if enabled && !self.enabled {
            self.following.set(true);
        }
        self.enabled = enabled;
        if enabled && self.following.get() {
            // Applied after layout, so appends, reveals and resizes pin to the
            // newly measured bottom rather than last frame's content height.
            self.handle.scroll_to_bottom();
        }
    }
}

pub(super) fn max_height(window: &Window) -> Pixels {
    px(280.).min(window.viewport_size().height * 0.45)
}

#[derive(Default)]
struct Edges {
    from: (f32, f32),
    to: (f32, f32),
    changed: Option<Instant>,
    frame: (f32, f32),
}
impl Edges {
    fn value(&self, now: Instant) -> (f32, f32) {
        let progress = self.changed.map_or(1., |at| {
            vorkspace_ui::reveal::ease_out((now.duration_since(at).as_secs_f32() / 0.16).min(1.))
        });
        (
            self.from.0 + (self.to.0 - self.from.0) * progress,
            self.from.1 + (self.to.1 - self.from.1) * progress,
        )
    }
    fn update(&mut self, target: (f32, f32), now: Instant) -> (f32, f32) {
        if self.to != target {
            self.from = self.value(now);
            self.to = target;
            self.changed = Some(now);
        }
        self.value(now)
    }
}

/// GPUI deltas are negative toward the bottom. Consume only a direction with
/// room, as nestedScrollAbsorbsWheel does; reaching an edge releases the next
/// wheel to the transcript. Upward input pauses follow until the reader returns.
fn wheel(scroll: &ScrollHandle, following: &Cell<bool>, delta: Pixels) -> bool {
    let max = scroll.max_offset().y;
    let offset = scroll.offset().y;
    if max <= px(1.)
        || !((delta > px(0.) && offset < px(0.)) || (delta < px(0.) && offset > -max + px(1.)))
    {
        return false;
    }
    let y = (offset + delta).clamp(-max, px(0.));
    following.set(delta < px(0.) && -max + px(1.) >= y);
    scroll.set_offset(gpui::point(scroll.offset().x, y));
    true
}

pub(super) fn live_phase(
    body: AnyElement,
    scroll: &mut LiveScroll,
    max_height: Pixels,
    enabled: bool,
    cx: &mut Context<super::TranscriptView>,
) -> AnyElement {
    scroll.enable(enabled);
    let handle = scroll.handle.clone();
    let following = scroll.following.clone();
    let view = cx.entity_id();
    div()
        .relative()
        .w_full()
        .min_w_0()
        .max_h(max_height)
        .child(edge_mask(scroll, true, enabled))
        .child(
            div()
                .id("live-phase-scroll")
                .w_full()
                .min_w_0()
                .max_h(max_height)
                .overflow_y_scroll()
                .track_scroll(&scroll.handle)
                // The inner listener runs before GPUI's scroller. Move and consume
                // the wheel here so the outer list never receives a handled event.
                .child(
                    div()
                        .id("live-phase-content")
                        .w_full()
                        .min_w_0()
                        .on_scroll_wheel(move |event, window, cx| {
                            if enabled
                                && wheel(
                                    &handle,
                                    &following,
                                    event.delta.pixel_delta(window.line_height()).y,
                                )
                            {
                                cx.notify(view);
                                cx.stop_propagation();
                            }
                        })
                        .child(body),
                ),
        )
        .child(edge_mask(scroll, false, enabled))
        .into_any_element()
}

fn edge_mask(scroll: &LiveScroll, begin: bool, enabled: bool) -> AnyElement {
    let handle = scroll.handle.clone();
    let edges = scroll.edges.clone();
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            // Read during paint, after the scroller has clamped its offset and
            // measured this frame's content. A live append can change both.
            let offset = handle.offset().y;
            let maximum = handle.max_offset().y;
            let mut edges = edges.borrow_mut();
            // Both markers must use the very same ramp for this frame.
            if begin {
                let target = if enabled {
                    (
                        if -offset > px(1.) { 24. } else { 0. },
                        if maximum + offset > px(1.) { 32. } else { 0. },
                    )
                } else {
                    (0., 0.)
                };
                let now = Instant::now();
                edges.frame = edges.update(target, now);
                if edges
                    .changed
                    .is_some_and(|at| now.duration_since(at).as_secs_f32() < 0.16)
                {
                    window.on_next_frame(|window, _| window.refresh());
                }
            }
            let (top, bottom) = edges.frame;
            if top > 0. || bottom > 0. {
                paint_mask(bounds, begin, top, bottom, window);
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

fn paint_mask(bounds: Bounds<Pixels>, begin: bool, top: f32, bottom: f32, window: &mut Window) {
    window.paint_layer(bounds, |window| {
        window.paint_quad(gpui::fill(bounds, gpui::black()).border_color(
            gpui_apple::metal_renderer::edge_fade_marker(
                begin,
                top * window.scale_factor(),
                bottom * window.scale_factor(),
            ),
        ));
    });
}

#[cfg(test)]
mod tests;
