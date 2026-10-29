//! The rail's rows and the main pane read as one surface: a hovered or
//! selected row's pill joins the pane's left edge with concave corners, and
//! the rail's divider runs around it. Hovering pours a pill out of the pane at
//! partial strength; leaving drains it back, so a click lands on a pill that
//! is already out and only deepens it. Pills use the rail's ordinary hover and
//! selection fills.
//!
//! The title bar's tabs use the same surface turned on its side (`below`):
//! their pills pour up out of the line beneath them. Shapes are computed as
//! for the rail, in a frame with the axes swapped.
//!
//! Every hoverable row adds `row`; the rail paints one `surface` beneath them,
//! laid out before them so it can clear the previous frame's rows. Each pill
//! is clipped to its row's scroller. Hover follows the pointer, which already
//! repaints the rail (`bots::avatar::sidebar_cursor_tracker`); the surface asks
//! for frames only while a pill is moving.
use crate::clock::Instant;
use gpui::{
    Bounds, ContentMask, Hsla, IntoElement, PathBuilder, Pixels, Point, SharedString, Styled,
    canvas, fill, point, px, size,
};
use std::{cell::RefCell, rc::Rc, time::Duration};

/// `rounded_md`, for the pill's end and its joins into the pane.
const RADIUS: f32 = 6.;
const POUR: Duration = Duration::from_millis(97);
const DRAIN: Duration = Duration::from_millis(71);
/// Pills sit this far inside their rows, so neighbours never touch.
const INSET: f32 = 1.;
/// The pane's outer corners, which the divider follows.
const PANE_RADIUS: f32 = 12.;
/// A selected row's fill; a hovered one's is `HOVER` of it.
const SELECTED: f32 = 0.1;
const HOVER: f32 = 0.7;
/// The rail's divider, as before the pills.
const DIVIDER: f32 = 0.08;

#[derive(Clone, Default)]
pub(crate) struct Liquid(Rc<RefCell<State>>);

#[derive(Default)]
struct State {
    rows: Vec<Row>,
    notches: Vec<Notch>,
    painted: bool,
    /// Pills rise from the bottom edge instead of the right one, and the
    /// divider is this colour.
    below: Option<Hsla>,
}

/// Swaps the axes for a surface `below` its rows; its own inverse.
fn flip(below: bool, b: Bounds<Pixels>) -> Bounds<Pixels> {
    if below {
        Bounds::new(
            point(b.origin.y, b.origin.x),
            size(b.size.height, b.size.width),
        )
    } else {
        b
    }
}
fn flip_point(below: bool, p: Point<Pixels>) -> Point<Pixels> {
    if below { point(p.y, p.x) } else { p }
}

struct Row {
    key: SharedString,
    bounds: Bounds<Pixels>,
    /// The row's scroller, which also clips its pill.
    clip: Bounds<Pixels>,
    selected: bool,
}

struct Notch {
    key: SharedString,
    bounds: Bounds<Pixels>,
    clip: Bounds<Pixels>,
    /// How far the pill is out of the pane.
    out: Anim,
    /// How far the selected look has grown in.
    lit: Anim,
}

#[derive(Clone, Copy)]
struct Anim {
    from: f32,
    to: f32,
    start: Instant,
}

impl Anim {
    fn at_rest(value: f32, now: Instant) -> Self {
        Self {
            from: value,
            to: value,
            start: now,
        }
    }
    fn value(&self, now: Instant) -> f32 {
        let (duration, ease): (_, fn(f32) -> f32) = if self.to > self.from {
            (POUR, crate::reveal::ease_out)
        } else {
            (DRAIN, |t| t * t)
        };
        let t = (now.saturating_duration_since(self.start).as_secs_f32() / duration.as_secs_f32())
            .min(1.);
        self.from + (self.to - self.from) * ease(t)
    }
    fn moving(&self, now: Instant) -> bool {
        self.value(now) != self.to
    }
    /// Retargets from wherever it is now.
    fn toward(&mut self, to: f32, now: Instant) {
        if self.to != to {
            *self = Self {
                from: self.value(now),
                to,
                start: now,
            };
        }
    }
}

impl Liquid {
    /// A surface whose pills rise from the line along its bottom edge, which
    /// is painted in `divider`.
    pub(crate) fn below(divider: Hsla) -> Self {
        let liquid = Self::default();
        liquid.0.borrow_mut().below = Some(divider);
        liquid
    }

    /// Registers a hoverable row; `key` identifies it across frames.
    pub(crate) fn row(
        &self,
        key: impl Into<SharedString>,
        selected: bool,
    ) -> impl IntoElement + Styled {
        let state = self.0.clone();
        let key = key.into();
        canvas(
            move |bounds, window, _| {
                let mut state = state.borrow_mut();
                let below = state.below.is_some();
                state.rows.push(Row {
                    key,
                    bounds: flip(below, bounds),
                    clip: flip(below, window.content_mask().bounds),
                    selected,
                })
            },
            |_, _, _, _| {},
        )
        // Inset, not sized: a row's padding must not shift its bounds.
        .absolute()
        .inset_0()
    }

    /// Paints the divider down this element's right edge, which must be the
    /// pane's left edge, and every pill running into it. Lay it over the whole
    /// rail. With `cap_bottom` the divider follows the pane's rounded bottom
    /// corner; with `cap_top` the top one, otherwise it starts below the title
    /// bar and its native controls.
    pub(crate) fn surface(&self, cap_top: bool, cap_bottom: bool) -> impl IntoElement {
        let state = self.0.clone();
        let paint = self.0.clone();
        canvas(
            move |_, _, _| state.borrow_mut().rows.clear(),
            move |bounds, _, window, _| {
                let mut state = paint.borrow_mut();
                let below = state.below.is_some();
                let bounds = flip(below, bounds);
                let now = Instant::now();
                let mouse = flip_point(below, window.mouse_position());
                let hovered =
                    (window.is_window_hovered() && bounds.contains(&mouse)).then_some(mouse);
                if state.update(hovered, now) {
                    window.request_animation_frame();
                }
                let edge = bounds.right().as_f32();
                let shapes = state.shapes(edge, now);
                // The divider, except where pills run into the pane. It stops
                // short of the pane's rounded bottom corner.
                let divider = state
                    .below
                    .unwrap_or_else(|| crate::theme::content(DIVIDER));
                let (top, bottom) = (bounds.top().as_f32(), bounds.bottom().as_f32());
                // Beneath tabs, the line simply spans the surface.
                let mut y = top
                    + if below {
                        0.
                    } else if cap_top {
                        PANE_RADIUS
                    } else {
                        crate::theme::TITLE_BAR_HEIGHT
                    };
                let end = bottom - if cap_bottom { PANE_RADIUS } else { 0. };
                // Around the pane's corners, on the divider's column.
                let x = edge - 0.5;
                let at = |x: f32, y: f32| point(px(x), px(y));
                let mut corners = PathBuilder::stroke(px(1.));
                if cap_top {
                    corners.move_to(at(x, top + PANE_RADIUS));
                    corners.curve_to(at(edge + PANE_RADIUS, top - 0.5), at(x, top - 0.5));
                }
                if cap_bottom {
                    corners.move_to(at(x, end));
                    corners.curve_to(at(edge + PANE_RADIUS, bottom + 0.5), at(x, bottom + 0.5));
                }
                if !below && let Ok(path) = corners.build() {
                    window.paint_path(path, divider);
                }
                let mut gaps: Vec<_> = shapes.iter().map(|(_, shape)| shape.span()).collect();
                gaps.sort_by(|a, b| a.0.total_cmp(&b.0));
                for (from, to) in gaps.into_iter().chain([(end, end)]) {
                    if from.min(end) > y {
                        let line = Bounds::new(
                            point(px(edge - 1.), px(y)),
                            size(px(1.), px(from.min(end) - y)),
                        );
                        window.paint_quad(fill(flip(below, line), divider));
                    }
                    y = y.max(to);
                }
                for (notch, shape) in shapes {
                    let clip = Bounds::new(
                        point(bounds.left(), notch.clip.top() - px(RADIUS)),
                        size(bounds.size.width, notch.clip.size.height + px(2. * RADIUS)),
                    );
                    let clip = flip(below, clip);
                    window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                        notch.paint(shape, edge, now, below, divider, window);
                    });
                }
            },
        )
        .absolute()
        .inset_0()
    }

    #[cfg(test)]
    pub(crate) fn pills(&self) -> Vec<(SharedString, bool)> {
        self.0
            .borrow()
            .notches
            .iter()
            .filter(|notch| notch.out.to > 0.)
            .map(|notch| (notch.key.clone(), notch.lit.to == 1.))
            .collect()
    }

    /// The row the selected pill belongs to.
    #[cfg(test)]
    pub(crate) fn selected(&self) -> Option<SharedString> {
        let state = self.0.borrow();
        state
            .notches
            .iter()
            .find(|notch| notch.lit.to == 1.)
            .map(|notch| notch.key.clone())
    }
}

impl State {
    /// Retargets every pill for this frame's rows; true while any moves.
    fn update(&mut self, mouse: Option<gpui::Point<Pixels>>, now: Instant) -> bool {
        let first = !std::mem::replace(&mut self.painted, true);
        let targets: Vec<_> = self
            .rows
            .iter()
            .filter(|row| {
                row.selected
                    || mouse.is_some_and(|m| row.bounds.contains(&m) && row.clip.contains(&m))
            })
            .collect();
        for row in &targets {
            if !self.notches.iter().any(|n| n.key == row.key) {
                // The first frame shows the selection without pouring it.
                let at = if first && row.selected { 1. } else { 0. };
                self.notches.push(Notch {
                    key: row.key.clone(),
                    bounds: row.bounds,
                    clip: row.clip,
                    out: Anim::at_rest(at, now),
                    lit: Anim::at_rest(at, now),
                });
            }
        }
        for notch in &mut self.notches {
            let target = targets.iter().find(|row| row.key == notch.key);
            if let Some(row) = target {
                notch.bounds = row.bounds;
                notch.clip = row.clip;
            }
            notch
                .out
                .toward(if target.is_some() { 1. } else { 0. }, now);
            let selected = target.is_some_and(|row| row.selected);
            notch.lit.toward(if selected { 1. } else { 0. }, now);
        }
        self.notches
            .retain(|n| n.out.moving(now) || n.lit.moving(now) || n.out.to > 0.);
        self.notches
            .iter()
            .any(|n| n.out.moving(now) || n.lit.moving(now))
    }
}

/// A pill's vertical extent and its concave joins, after meeting neighbours.
#[derive(Clone, Copy)]
struct Shape {
    left: f32,
    top: f32,
    bottom: f32,
    /// The concave joins' radius, where they aren't a seam with a neighbour.
    join: f32,
    join_top: bool,
    join_bottom: bool,
}

impl Shape {
    /// Where the pill meets the pane, joins included.
    fn span(&self) -> (f32, f32) {
        (
            self.top - if self.join_top { self.join } else { 0. },
            self.bottom + if self.join_bottom { self.join } else { 0. },
        )
    }
}

impl State {
    /// Each pill's shape. Neighbouring pills, such as a hovered row beside the
    /// selected one, stay apart and drop the joins that would overlap.
    fn shapes(&self, edge: f32, now: Instant) -> Vec<(&Notch, Shape)> {
        let mut shapes: Vec<_> = self
            .notches
            .iter()
            .filter(|n| n.out.value(now) * (edge - n.bounds.left().as_f32()) >= 0.5)
            .map(|n| {
                let out = n.out.value(now);
                let ext = (edge - n.bounds.left().as_f32()) * out;
                // A partial pill thins toward its centre line, like a droplet.
                let height = (n.bounds.size.height.as_f32() - 2. * INSET) * (0.55 + 0.45 * out);
                let mid = n.bounds.center().y.as_f32();
                let shape = Shape {
                    left: edge - ext,
                    top: mid - height / 2.,
                    bottom: mid + height / 2.,
                    join: RADIUS.min(ext / 2.),
                    join_top: true,
                    join_bottom: true,
                };
                (n, shape)
            })
            .collect();
        shapes.sort_by(|a, b| {
            a.0.bounds
                .top()
                .as_f32()
                .total_cmp(&b.0.bounds.top().as_f32())
        });
        // Neighbours too close for both joins meet the pane squarely instead.
        for ix in 1..shapes.len() {
            if shapes[ix].1.top - shapes[ix - 1].1.bottom < 2. * RADIUS {
                shapes[ix - 1].1.join_bottom = false;
                shapes[ix].1.join_top = false;
            }
        }
        shapes
    }
}

impl Notch {
    fn paint(
        &self,
        shape: Shape,
        edge: f32,
        now: Instant,
        below: bool,
        divider: Hsla,
        window: &mut gpui::Window,
    ) {
        let lit = self.lit.value(now);
        let strength = HOVER + (1. - HOVER) * lit;
        if let Some(path) = outline(shape, edge, below, PathBuilder::fill()) {
            window.paint_path(path, crate::theme::content(SELECTED * strength));
        }
        // The divider runs around the pill, centred on the divider's column.
        if let Some(path) = outline(shape, edge - 0.5, below, PathBuilder::stroke(px(1.))) {
            window.paint_path(path, divider);
        }
    }
}

/// The pill's outline from `edge` and back. A fill closes it along `edge`;
/// a stroke leaves that side open, where the pill meets the pane.
fn outline(
    shape: Shape,
    edge: f32,
    below: bool,
    mut path: PathBuilder,
) -> Option<gpui::Path<Pixels>> {
    let Shape {
        left, top, bottom, ..
    } = shape;
    if edge - left < 0.5 {
        return None;
    }
    let (from, to) = shape.span();
    let (tj, bj) = (top - from, to - bottom);
    let end = RADIUS.min((bottom - top) / 2.).min((edge - left) / 2.);
    let at = |x: f32, y: f32| flip_point(below, point(px(x), px(y)));
    // Quadratic corners: indistinguishable from arcs at this radius.
    path.move_to(at(edge, from));
    path.curve_to(at(edge - tj, top), at(edge, top));
    path.line_to(at(left + end, top));
    path.curve_to(at(left, top + end), at(left, top));
    path.line_to(at(left, bottom - end));
    path.curve_to(at(left + end, bottom), at(left, bottom));
    path.line_to(at(edge - bj, bottom));
    path.curve_to(at(edge, to), at(edge, bottom));
    path.build().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(y: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(8.), px(y)), size(px(200.), px(32.)))
    }

    /// Lays out rows "a" at 0 and "b" at 100, with `selected` selected.
    fn frame(state: &mut State, selected: &str, mouse: Option<f32>, now: Instant) -> bool {
        state.rows = ["a", "b"]
            .into_iter()
            .zip([0., 100.])
            .map(|(key, y)| Row {
                key: key.into(),
                bounds: bounds(y),
                clip: Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(1000.))),
                selected: key == selected,
            })
            .collect();
        state.update(mouse.map(|y| point(px(20.), px(y))), now)
    }

    fn out(state: &State, key: &str, now: Instant) -> f32 {
        state
            .notches
            .iter()
            .find(|n| n.key.as_ref() == key)
            .map_or(0., |n| n.out.value(now))
    }

    #[test]
    fn hover_pours_a_pill_that_a_click_keeps_while_the_old_one_drains() {
        let mut state = State::default();
        let start = Instant::now();
        assert!(
            !frame(&mut state, "a", None, start),
            "the first paint never animates"
        );
        assert_eq!(out(&state, "a", start), 1.);

        // Hovering "b" pours its pill out of the pane.
        assert!(frame(&mut state, "a", Some(110.), start));
        let mid = start + POUR / 2;
        assert!(frame(&mut state, "a", Some(110.), mid));
        assert!(out(&state, "b", mid) > 0. && out(&state, "b", mid) < 1.);
        let poured = start + POUR;
        assert!(!frame(&mut state, "a", Some(110.), poured));
        assert_eq!(out(&state, "b", poured), 1.);

        // Clicking it keeps "b" out; only "a" moves, back into the pane.
        assert!(frame(&mut state, "b", Some(110.), poured));
        assert_eq!(out(&state, "b", poured), 1.);
        // The click also grows "b"'s look in, which takes as long as a pour.
        let drained = poured + POUR;
        assert!(!frame(&mut state, "b", Some(110.), drained));
        assert_eq!(out(&state, "a", drained), 0.);
        assert!(state.notches.iter().all(|n| n.key.as_ref() == "b"));
    }

    #[test]
    fn leaving_an_unselected_row_drains_it_from_where_it_was() {
        let mut state = State::default();
        let start = Instant::now();
        frame(&mut state, "a", None, start);
        frame(&mut state, "a", Some(110.), start);
        let mid = start + POUR / 2;
        frame(&mut state, "a", Some(110.), mid);
        let partial = out(&state, "b", mid);
        frame(&mut state, "a", None, mid);
        assert_eq!(out(&state, "b", mid), partial);
        assert!(!frame(&mut state, "a", None, mid + DRAIN));
        assert!(state.notches.iter().all(|n| n.key.as_ref() == "a"));
    }

    #[gpui::test]
    fn a_rows_padding_does_not_shift_its_pill(cx: &mut gpui::TestAppContext) {
        use gpui::prelude::*;
        struct Rail(Liquid);
        impl gpui::Render for Rail {
            fn render(
                &mut self,
                _: &mut gpui::Window,
                _: &mut gpui::Context<Self>,
            ) -> impl IntoElement {
                gpui::div()
                    .size_full()
                    .relative()
                    .child(self.0.surface(false, true))
                    .child(
                        gpui::div()
                            .debug_selector(|| "row".into())
                            .relative()
                            .w(px(200.))
                            .h(px(32.))
                            .px_2()
                            .py(px(6.))
                            .child(self.0.row("row", true)),
                    )
            }
        }
        let liquid = Liquid::default();
        let window = cx.add_window(|_, _| Rail(liquid.clone()));
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let row = visual.debug_bounds("row").unwrap();
        assert_eq!(liquid.0.borrow().notches[0].bounds, row);
    }

    #[test]
    fn neighbouring_pills_stay_apart_without_overlapping_joins() {
        let mut state = State::default();
        let now = Instant::now();
        state.rows = ["a", "b"]
            .into_iter()
            .zip([0., 33.])
            .map(|(key, y)| Row {
                key: key.into(),
                bounds: bounds(y),
                clip: Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(1000.))),
                selected: key == "a",
            })
            .collect();
        state.update(None, now);
        state.update(Some(point(px(20.), px(40.))), now);
        let later = now + POUR;
        state.update(Some(point(px(20.), px(40.))), later);
        let shapes = state.shapes(240., later);
        assert_eq!(shapes.len(), 2);
        let (above, below) = (shapes[0].1, shapes[1].1);
        assert!(!above.join_bottom && !below.join_top);
        assert!(above.bottom < below.top, "neighbours never overlap");
        assert!(above.join_top && below.join_bottom);
    }

    #[test]
    fn a_surface_below_its_rows_works_in_swapped_axes() {
        let b = Bounds::new(point(px(3.), px(5.)), size(px(7.), px(11.)));
        assert_eq!(flip(true, flip(true, b)), b);
        assert_eq!(flip(true, b).origin, point(px(5.), px(3.)));
        assert_eq!(flip(false, b), b);
    }

    #[test]
    fn an_idle_rail_requests_no_frames() {
        let mut state = State::default();
        let now = Instant::now();
        frame(&mut state, "a", None, now);
        for _ in 0..3 {
            assert!(!frame(&mut state, "a", None, now));
        }
        let shape = Shape {
            left: 8.,
            top: 0.,
            bottom: 32.,
            join: RADIUS,
            join_top: true,
            join_bottom: true,
        };
        assert!(outline(shape, 240., false, PathBuilder::fill()).is_some());
        let empty = Shape {
            left: 240.,
            ..shape
        };
        assert!(outline(empty, 240., false, PathBuilder::fill()).is_none());
    }
}
