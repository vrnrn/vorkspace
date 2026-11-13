//! Shared parts of the project tool panels: tabs, headers, and diff statistics.
use gpui::{
    AnyElement, FontFeatures, FontWeight, Hsla, Role, SharedString, Stateful, div, prelude::*, px,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use vorkspace_ui::radius;
use vorkspace_ui::theme::*;

pub(crate) fn stroke() -> Hsla {
    content(0.04)
}
pub(crate) fn selection() -> Hsla {
    content(0.1)
}

/// The rail's right edge follows the adjacent pane's rounded corners.
/// `cap_top` is the pane's top offset, or the title rule beside a compact rail.
pub(crate) fn rail_divider(cap_top: Option<f32>) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let edge = bounds.right().as_f32();
            let top = bounds.top().as_f32();
            let bottom = bounds.bottom().as_f32();
            let start = top + cap_top.map_or(TITLE_BAR_HEIGHT, |cap| cap + radius::SURFACE);
            let end = bottom - radius::SURFACE;
            let divider = content(0.08);
            if end > start {
                window.paint_quad(gpui::fill(
                    gpui::Bounds::new(
                        gpui::point(px(edge - 1.), px(start)),
                        gpui::size(px(1.), px(end - start)),
                    ),
                    divider,
                ));
            }
            let x = edge - 0.5;
            let at = |x, y| gpui::point(px(x), px(y));
            let mut corners = gpui::PathBuilder::stroke(px(1.));
            if let Some(cap) = cap_top {
                let rule = top + cap - 0.5;
                corners.move_to(at(x, top + cap + radius::SURFACE));
                corners.curve_to(at(edge + radius::SURFACE, rule), at(x, rule));
            }
            corners.move_to(at(x, end));
            corners.curve_to(
                at(edge + radius::SURFACE, bottom + 0.5),
                at(x, bottom + 0.5),
            );
            if let Ok(path) = corners.build() {
                window.paint_path(path, divider);
            }
        },
    )
    .absolute()
    .inset_0()
}
// Tailwind v4.
/// `--color-background-base` in the default dark theme, hsl(240 0% 9%).
pub(crate) const BACKGROUND_BASE: u32 = 0x171717;

pub(crate) fn tabular() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RightTab {
    #[default]
    Files,
    Changes,
    Graph,
}

/// Tool headers share the open-file tab bar's height and bottom rule.
pub(crate) fn pane_header(id: &'static str, label: &'static str) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .role(Role::Toolbar)
        .aria_label(label)
        .h(px(PANE_HEADER_HEIGHT))
        .flex_shrink_0()
        .min_w_0()
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .border_b_1()
        .border_color(stroke())
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(content(0.85))
}

/// GitChangesPanel's `IconAction`: a 20px square, ink at 55%.
pub(crate) fn icon_action(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    path: &'static str,
    enabled: bool,
) -> Stateful<gpui::Div> {
    let id = id.into();
    let group: SharedString = format!("icon-action-{id:?}").into();
    let label = label.into();
    crate::controls::button(id, label.clone(), enabled)
        .tooltip(move |_, cx| cx.new(|_| crate::controls::Tooltip(label.clone())).into())
        .group(group.clone())
        .size(px(20.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(radius::SMALL))
        .when(!enabled, |d| d.opacity(0.4))
        .when(enabled, |d| {
            d.hover(|s| s.bg(vorkspace_ui::fluid::hover(0.1)))
        })
        .child(
            icon(path, 14.)
                .text_color(content(0.55))
                .when(enabled, |i| {
                    i.group_hover(group, |s| s.text_color(content(1.)))
                }),
        )
}

/// The busy indicator: fluidither's Loader, three printed cells counting up,
/// sized to the icon slot it replaces.
pub(crate) fn spinner(id: impl Into<gpui::ElementId>, side: f32) -> AnyElement {
    Loader {
        id: id.into(),
        side,
    }
    .into_any_element()
}

/// How long each count of the loader holds.
const LOADER_STEP: std::time::Duration = std::time::Duration::from_millis(300);

/// The loader's count, advanced by its own timer. A repeating animation would
/// redraw its view on every display refresh for an image that changes three
/// times a cycle; this wakes the view only when a cell turns on. Dropped with
/// the element, which cancels the timer.
struct LoaderCount {
    shown: usize,
    _tick: gpui::Task<()>,
}

#[derive(IntoElement)]
struct Loader {
    id: gpui::ElementId,
    side: f32,
}

impl RenderOnce for Loader {
    fn render(self, window: &mut gpui::Window, cx: &mut gpui::App) -> impl IntoElement {
        // Reduce Motion: a still loader, all cells lit, and no timer.
        if cx.reduce_motion() {
            return loader_cells(self.side, 3);
        }
        let count = window.use_keyed_state(self.id, cx, |_, cx| LoaderCount {
            shown: 1,
            _tick: cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(LOADER_STEP).await;
                    if this
                        .update(cx, |count: &mut LoaderCount, cx| {
                            count.shown = count.shown % 3 + 1;
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }),
        });
        loader_cells(self.side, count.read(cx).shown)
    }
}

/// The loader's three cells with `shown` of them lit.
fn loader_cells(side: f32, shown: usize) -> gpui::Div {
    // Three cells and two gaps of the same size fill the slot's width.
    let cell = (side / 5.).floor().max(2.);
    div()
        .size(px(side))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(cell))
        .children((0..3).map(|index| {
            div()
                .size(px(cell))
                .when(index < shown, |d| d.bg(content(0.6)))
                .when(index >= shown, |d| d.bg(vorkspace_ui::fluid::track(0.1)))
        }))
}

pub(crate) fn format_integer(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if value < 0 { format!("-{out}") } else { out }
}

/// A path as people read it: under the home folder it starts with `~`.
pub(crate) fn home_relative(path: &std::path::Path) -> String {
    std::env::var_os("HOME")
        .and_then(|home| {
            path.strip_prefix(home).ok().map(|rest| {
                if rest.as_os_str().is_empty() {
                    "~".to_owned()
                } else {
                    format!("~/{}", rest.display())
                }
            })
        })
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod loader_tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};

    struct Host;
    impl gpui::Render for Host {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div().size_full().child(spinner("loader-under-test", 14.))
        }
    }

    /// Cells lit, read from the painted quads: lit cells are solid ink.
    fn lit(visual: &mut VisualTestContext) -> usize {
        visual.update(|window, _| {
            window
                .painted_quads()
                .iter()
                .filter(|q| q.background == gpui::Background::from(content(0.6)))
                .count()
        })
    }

    #[gpui::test]
    fn the_loader_counts_on_a_timer_and_never_asks_for_frames(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| cx.new(|_| Host))
                .unwrap()
        });
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert_eq!(lit(visual), 1);
        // Nothing waits on the next display refresh: the view sleeps between
        // counts instead of redrawing every frame.
        let waiting = visual.update(|window, cx| window.simulate_next_frame(cx));
        assert_eq!(waiting, 0);
        visual.executor().advance_clock(LOADER_STEP);
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert_eq!(lit(visual), 2);
        visual.executor().advance_clock(LOADER_STEP * 2);
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert_eq!(lit(visual), 1, "after three it starts over");
    }

    #[gpui::test]
    fn under_reduce_motion_the_loader_holds_still(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| cx.new(|_| Host))
                .unwrap()
        });
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert_eq!(lit(visual), 3);
        visual.executor().advance_clock(LOADER_STEP * 4);
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert_eq!(lit(visual), 3);
    }
}
