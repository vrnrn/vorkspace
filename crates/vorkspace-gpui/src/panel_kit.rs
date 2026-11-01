//! Shared parts of the project tool panels: tabs, headers, and diff statistics.
use gpui::{
    AnyElement, FontFeatures, FontWeight, Hsla, Role, SharedString, Stateful, div, prelude::*, px,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use vorkspace_ui::theme::*;

pub(crate) fn stroke() -> Hsla {
    content(0.04)
}
pub(crate) fn selection() -> Hsla {
    content(0.1)
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
        .rounded(px(4.))
        .when(!enabled, |d| d.opacity(0.4))
        .when(enabled, |d| d.hover(|s| s.bg(content(0.1))))
        .child(
            icon(path, 14.)
                .text_color(content(0.55))
                .when(enabled, |i| {
                    i.group_hover(group, |s| s.text_color(content(1.)))
                }),
        )
}

/// The `animate-spin` Loader.
pub(crate) fn spinner(id: impl Into<gpui::ElementId>, path: &'static str, side: f32) -> AnyElement {
    use gpui::{Animation, AnimationExt, Transformation, percentage};
    icon(path, side)
        .with_animation(
            id,
            Animation::new(std::time::Duration::from_secs(1)).repeat(),
            |icon, delta| icon.with_transformation(Transformation::rotate(percentage(delta))),
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
        .rounded_md()
        .whitespace_nowrap()
        .text_size(px(10. * scale))
        .font_weight(FontWeight::MEDIUM)
        .font_features(tabular())
        .child(
            div()
                .text_color(gpui::rgb(DIFF_ADD))
                .child(format!("+{added}")),
        )
        .child(
            div()
                .text_color(gpui::rgb(DIFF_DEL))
                .child(format!("-{deleted}")),
        )
}
