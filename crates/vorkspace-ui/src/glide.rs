//! One selection thumb that springs between the items of a list or strip, as
//! fluidither's Nav does, instead of each item painting its own fill.
//!
//! Items record their bounds during prepaint; the thumb, painted after every
//! prepaint, springs toward the selected item's bounds from that same frame,
//! so it follows tabs that slide or resize without trailing a frame behind.
//! Until the selection has been measured once the thumb is not ready and the
//! item paints its own fill, so the first frame never loses the selection.
use crate::clock::Instant;
use crate::fluid::{GLIDE, LONGEST_STEP, Spring};
use gpui::{
    AnyElement, App, Bounds, ElementId, Hsla, IntoElement, Pixels, Window, canvas, prelude::*, px,
};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

#[derive(Default)]
struct Measured {
    container: Option<Bounds<Pixels>>,
    items: HashMap<usize, Bounds<Pixels>>,
}

struct Springs {
    x: Spring,
    y: Spring,
    width: Spring,
    height: Spring,
}

impl Springs {
    fn at(target: Bounds<Pixels>) -> Self {
        let spring = |value: Pixels| Spring::new(GLIDE, f64::from(value.as_f32()));
        Self {
            x: spring(target.origin.x),
            y: spring(target.origin.y),
            width: spring(target.size.width),
            height: spring(target.size.height),
        }
    }
    fn aim(&mut self, target: Bounds<Pixels>) {
        self.x.target = f64::from(target.origin.x.as_f32());
        self.y.target = f64::from(target.origin.y.as_f32());
        self.width.target = f64::from(target.size.width.as_f32());
        self.height.target = f64::from(target.size.height.as_f32());
    }
    /// Steps every axis; true once all have landed.
    fn step(&mut self, dt: f64) -> bool {
        let x = self.x.step(dt);
        let y = self.y.step(dt);
        let width = self.width.step(dt);
        let height = self.height.step(dt);
        x && y && width && height
    }
    fn bounds(&self) -> Bounds<Pixels> {
        Bounds::new(
            gpui::point(px(self.x.position as f32), px(self.y.position as f32)),
            gpui::size(
                px(self.width.position as f32),
                px(self.height.position as f32),
            ),
        )
    }
}

/// Seconds without a frame after which a glide in flight lands at once.
const STALL: f64 = 0.25;

struct State {
    measured: Rc<RefCell<Measured>>,
    springs: Option<Springs>,
    selected: Option<usize>,
    last: Instant,
}

impl State {
    /// Springs the thumb toward this frame's measured target. Runs while
    /// painting, after every probe has recorded this frame's layout, so the
    /// thumb never trails a tab that slides, opens or is dragged. Returns the
    /// thumb's window bounds and whether it needs another frame.
    fn advance(
        &mut self,
        selected: Option<usize>,
        reduce_motion: bool,
    ) -> (Option<Bounds<Pixels>>, bool) {
        let now = Instant::now();
        let gap = now.saturating_duration_since(self.last).as_secs_f64();
        // A gap longer than a whole glide (a stalled or hidden window) lands
        // the thumb rather than replaying the move late.
        let stalled = gap > STALL;
        let dt = gap.min(LONGEST_STEP);
        self.last = now;
        let (target, origin) = {
            let measured = self.measured.borrow();
            let Some(container) = measured.container else {
                self.springs = None;
                return (None, false);
            };
            let target = selected.and_then(|index| {
                let item = measured.items.get(&index)?;
                Some(Bounds::new(item.origin - container.origin, item.size))
            });
            (target, container.origin)
        };
        let changed = self.selected != selected;
        self.selected = selected;
        let Some(target) = target else {
            self.springs = None;
            return (None, false);
        };
        let Some(springs) = self.springs.as_mut() else {
            // First sight lands in place: a thumb never flies in from 0,0.
            // The frame that measured it still has the item's own fill, so
            // ask for one more frame to hand the selection to the thumb.
            self.springs = Some(Springs::at(target));
            return (None, true);
        };
        let settled = springs.x.landed()
            && springs.y.landed()
            && springs.width.landed()
            && springs.height.landed();
        // Follow the geometry of a settled selection immediately on relayout.
        if reduce_motion || (!changed && (settled || stalled)) {
            *springs = Springs::at(target);
        } else {
            springs.aim(target);
        }
        let moving = !springs.step(dt);
        let bounds = springs.bounds();
        (
            Some(Bounds::new(origin + bounds.origin, bounds.size)),
            moving,
        )
    }
}

/// A list's glide for one render. Build it before the items, give each item
/// `probe(index)` and the list `container()`, and put `thumb(..)` first among
/// the list's children so it paints behind them.
pub struct Glide {
    state: gpui::Entity<State>,
    measured: Rc<RefCell<Measured>>,
    selected: Option<usize>,
    ready: bool,
}

impl Glide {
    pub fn new(
        id: impl Into<ElementId>,
        selected: Option<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let state = window.use_keyed_state(id.into(), cx, |_, _| State {
            measured: Rc::default(),
            springs: None,
            selected,
            last: Instant::now(),
        });
        let (measured, ready) = {
            let state = state.read(cx);
            (
                state.measured.clone(),
                selected.is_some() && state.springs.is_some(),
            )
        };
        Self {
            state,
            measured,
            selected,
            ready,
        }
    }

    /// Whether the thumb draws the selection this frame. When false, the
    /// selected item paints its own fill.
    pub fn ready(&self) -> bool {
        self.ready
    }

    /// Records the list's own bounds and clears stale item probes before child
    /// probes run in this frame's prepaint pass. Place it as an absolute child.
    pub fn container(&self) -> impl IntoElement {
        let measured = self.measured.clone();
        canvas(
            move |bounds, _, _| {
                let mut measured = measured.borrow_mut();
                measured.container = Some(bounds);
                measured.items.clear();
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }

    /// Records an item's bounds. Place it as an absolute child of the item.
    pub fn probe(&self, index: usize) -> impl IntoElement {
        let measured = self.measured.clone();
        canvas(
            move |bounds, _, _| {
                measured.borrow_mut().items.insert(index, bounds);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }

    /// The thumb, behind the items, in the selected item's fill and corners.
    /// It is placed while painting, from the bounds the probes recorded
    /// moments earlier in the same frame.
    pub fn thumb(&self, fill: Hsla, radius: f32) -> impl IntoElement {
        let state = self.state.clone();
        let selected = self.selected;
        let ready = self.ready;
        canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                let reduce_motion = cx.reduce_motion();
                let (bounds, moving) =
                    state.update(cx, |state, _| state.advance(selected, reduce_motion));
                if let Some(bounds) = bounds.filter(|_| ready) {
                    window.paint_quad(gpui::fill(bounds, fill).corner_radii(px(radius)));
                }
                if moving || (bounds.is_some() && !ready) {
                    window.request_animation_frame();
                }
            },
        )
        .absolute()
        .inset_0()
    }
}

type Items = Box<dyn FnOnce(&Glide) -> Vec<AnyElement>>;

/// A list or strip whose selection is one gliding thumb. `items` builds the
/// children with the frame's glide: each adds `glide.probe(index)` and paints
/// its own selected fill only while `!glide.ready()`.
#[derive(IntoElement)]
pub struct GlideList {
    id: ElementId,
    selected: Option<usize>,
    base: gpui::Div,
    fill: Hsla,
    radius: f32,
    items: Items,
}

impl GlideList {
    pub fn new(
        id: impl Into<ElementId>,
        selected: Option<usize>,
        base: gpui::Div,
        fill: Hsla,
        radius: f32,
        items: impl FnOnce(&Glide) -> Vec<AnyElement> + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            selected,
            base,
            fill,
            radius,
            items: Box::new(items),
        }
    }
}

impl gpui::RenderOnce for GlideList {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let glide = Glide::new(self.id, self.selected, window, cx);
        let items = (self.items)(&glide);
        self.base
            .relative()
            .child(glide.container())
            .child(glide.thumb(self.fill, self.radius))
            .children(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settled_thumb_stays_attached_when_its_item_moves() {
        let bounds = |x, y, width, height| {
            Bounds::new(gpui::point(px(x), px(y)), gpui::size(px(width), px(height)))
        };
        let original = bounds(20., 30., 80., 24.);
        let shifted = bounds(60., 58., 72., 30.);
        let mut state = State {
            measured: Rc::new(RefCell::new(Measured {
                container: Some(bounds(0., 0., 180., 160.)),
                items: HashMap::from([(1, shifted)]),
            })),
            springs: Some(Springs::at(original)),
            selected: Some(1),
            last: Instant::now(),
        };
        assert_eq!(state.advance(Some(1), false), (Some(shifted), false));
    }
}
