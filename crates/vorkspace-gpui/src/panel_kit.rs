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
pub(crate) const DIFF_ADD: u32 = 0x6ee7b7;
pub(crate) const DIFF_DEL: u32 = 0xfda4af;
// Tailwind v4.
pub(crate) const AMBER_400: u32 = 0xffb900;
pub(crate) const SKY_400: u32 = 0x00bcff;
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
impl RightTab {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Files => "Explorer",
            Self::Changes => "Changes",
            Self::Graph => "History",
        }
    }
    /// loadRightPanelTabOrder: unique known tabs, the missing ones appended.
    pub(crate) fn normalize(order: &[RightTab]) -> Vec<RightTab> {
        let mut out = Vec::new();
        for tab in order
            .iter()
            .chain(&[Self::Files, Self::Changes, Self::Graph])
        {
            if !out.contains(tab) {
                out.push(*tab);
            }
        }
        out
    }
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
/// sized to the icon slot it replaces. `_path` names the icon the slot used
/// to spin, so callers keep describing what is loading.
pub(crate) fn spinner(
    id: impl Into<gpui::ElementId>,
    _path: &'static str,
    side: f32,
) -> AnyElement {
    use gpui::{Animation, AnimationExt};
    // Three cells and two gaps of the same size fill the slot's width.
    let cell = (side / 5.).floor().max(2.);
    div()
        .size(px(side))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(cell))
        .with_animation(
            id,
            Animation::new(std::time::Duration::from_millis(900)).repeat(),
            move |row, delta| {
                let shown = ((delta * 3.).floor() as usize).min(2) + 1;
                row.children((0..3).map(|index| {
                    div()
                        .size(px(cell))
                        .when(index < shown, |d| d.bg(content(0.6)))
                        .when(index >= shown, |d| d.bg(vorkspace_ui::fluid::track(0.1)))
                }))
            },
        )
        .into_any_element()
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

pub(crate) fn diff_stat_width(stats: Option<&vorkspace_core::scm::DiffStats>) -> f32 {
    let added = format_integer(stats.map_or(0, |s| s.additions.max(0)));
    let deleted = format_integer(stats.map_or(0, |s| s.deletions.max(0)));
    // Tabular digits at 10px, the signs, a 6px gap and 6px padding per side.
    (added.len() + deleted.len() + 2) as f32 * 6.5 + 18.
}

pub(crate) fn diff_stat(
    stats: Option<&vorkspace_core::scm::DiffStats>,
    width: f32,
) -> Stateful<gpui::Div> {
    let added = format_integer(stats.map_or(0, |s| s.additions.max(0)));
    let deleted = format_integer(stats.map_or(0, |s| s.deletions.max(0)));
    let scale = ((width - 18.) / (diff_stat_width(stats) - 18.)).clamp(0.1, 1.);
    // A clean tree reads as quiet zeros, not as green and red changes.
    let clean = stats.is_none_or(|s| s.additions <= 0 && s.deletions <= 0);
    let (add, del) = if clean {
        (content(0.35), content(0.35))
    } else {
        (gpui::rgb(DIFF_ADD).into(), gpui::rgb(DIFF_DEL).into())
    };
    div()
        .id("project-diff-stat")
        .debug_selector(|| "project-diff-stat".into())
        .role(Role::Button)
        .aria_label(format!(
            "Open Changes: {added} lines added, {deleted} lines deleted"
        ))
        .w(px(width))
        .h(px(24.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_end()
        .gap(px(6.))
        .px_1p5()
        .rounded(px(radius::CONTROL))
        .whitespace_nowrap()
        .text_size(px(10. * scale))
        .font_weight(FontWeight::MEDIUM)
        .font_features(tabular())
        .font_family(vorkspace_ui::fonts::MONO)
        // The counts roll to each new total rather than jumping.
        .child(div().text_color(add).child(vorkspace_ui::fluid::Count::new(
            "project-diff-added",
            stats.map_or(0, |s| s.additions.max(0)),
            |n| format!("+{}", format_integer(n)),
        )))
        .child(div().text_color(del).child(vorkspace_ui::fluid::Count::new(
            "project-diff-deleted",
            stats.map_or(0, |s| s.deletions.max(0)),
            |n| format!("-{}", format_integer(n)),
        )))
}
