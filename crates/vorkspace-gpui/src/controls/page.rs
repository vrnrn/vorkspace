//! Spacing, typography and controls for compact native pages.
use gpui::{
    Div, ElementId, FocusHandle, FontWeight, Hsla, IntoElement, Pixels, Role, SharedString,
    Stateful, Window, div, prelude::*, px,
};
use vorkspace_ui::radius;
use vorkspace_ui::theme::{accent_color, content};

pub(crate) const TOOLBAR_HEIGHT: f32 = 36.;
pub(crate) const CONTROL_HEIGHT: f32 = 28.;
pub(crate) const ICON_SIZE: f32 = 14.;
pub(crate) const LIST_WIDTH: f32 = 280.;
pub(crate) const LIST_MIN: f32 = 240.;
pub(crate) const LIST_MAX: f32 = 420.;
pub(crate) const DETAIL_MAX: f32 = 1024.;
pub(crate) const TITLE_SIZE: f32 = 20.;
pub(crate) const TITLE_LINE: f32 = 26.;
pub(crate) const LABEL_SIZE: f32 = 13.;
pub(crate) const LABEL_LINE: f32 = 20.;
pub(crate) const META_SIZE: f32 = 12.;
pub(crate) const META_LINE: f32 = 18.;
pub(crate) const BODY_SIZE: f32 = 14.;
pub(crate) const BODY_LINE: f32 = 22.;

pub(crate) fn divider() -> Hsla {
    content(0.10)
}

pub(crate) fn secondary_ink() -> Hsla {
    content(0.60)
}

pub(crate) fn narrow(window: &Window) -> bool {
    window.viewport_size().width < px(900.)
}

pub(crate) fn detail_padding(narrow: bool) -> Pixels {
    px(if narrow { 16. } else { 24. })
}

/// The detail owns the curved seam beside its list. In a single-column view,
/// the workspace frame already supplies the pane's outer edge.
pub(crate) fn detail_pane(split: bool) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .flex()
        .flex_col()
        .when(split, |d| {
            d.rounded_l(px(radius::SURFACE))
                .border_l_1()
                .border_color(divider())
        })
}

/// The grab target spans the seam; only the detail pane's curved border lights
/// up. Painting the grab target itself leaves a square bar across both corners.
pub(crate) fn detail_resize_handle(
    id: &'static str,
    label: &'static str,
    dragging: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .group("page-detail-resize")
        .role(Role::Splitter)
        .aria_label(label)
        .absolute()
        .right(px(-3.))
        .top_0()
        .bottom_0()
        .w(px(6.))
        .cursor(gpui::CursorStyle::ResizeLeftRight)
        .child(
            div()
                .absolute()
                .left(px(3.))
                .top_0()
                .h_full()
                .w(px(radius::SURFACE * 2.))
                .rounded_l(px(radius::SURFACE))
                .border_l_1()
                .border_color(content(if dragging { 0.25 } else { 0. }))
                .group_hover("page-detail-resize", |style| {
                    style.border_color(content(0.25))
                }),
        )
}

pub(crate) fn toolbar() -> Div {
    div()
        .h(px(TOOLBAR_HEIGHT))
        .px(px(8.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(4.))
        .border_b_1()
        .border_color(divider())
}

pub(crate) fn field_frame() -> Div {
    div()
        .h(px(CONTROL_HEIGHT))
        .min_w_0()
        .px(px(8.))
        .flex()
        .items_center()
        .gap(px(8.))
        .rounded(px(radius::CONTROL))
        .bg(content(0.05))
        .text_color(secondary_ink())
}

/// Filters blend into their toolbar until the text field owns focus.
pub(crate) fn search_frame(focus: &FocusHandle) -> Div {
    field_frame()
        .bg(content(0.))
        .track_focus(focus)
        .focus(|style| style.bg(content(0.05)))
}

pub(crate) fn row(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
) -> Stateful<Div> {
    super::button(id, label, true)
        .min_w_0()
        .px(px(8.))
        .py(px(8.))
        .rounded(px(radius::CONTROL))
        .border_1()
        .border_color(content(0.))
        .cursor_pointer()
        .when(selected, |d| d.bg(content(0.10)))
        .when(!selected, |d| {
            d.hover(|s| s.bg(vorkspace_ui::fluid::hover(0.05)))
        })
}

pub(crate) fn title() -> Div {
    div()
        .text_size(px(TITLE_SIZE))
        .line_height(px(TITLE_LINE))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(content(1.))
}

pub(crate) fn label() -> Div {
    div()
        .text_size(px(LABEL_SIZE))
        .line_height(px(LABEL_LINE))
        .text_color(content(1.))
}

pub(crate) fn meta() -> Div {
    div()
        .text_size(px(META_SIZE))
        .line_height(px(META_LINE))
        .text_color(secondary_ink())
}

pub(crate) fn body() -> Div {
    div()
        .text_size(px(BODY_SIZE))
        .line_height(px(BODY_LINE))
        .text_color(content(1.))
}

pub(crate) fn empty(
    icon: impl IntoElement,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
) -> Div {
    let detail = detail.into();
    div()
        .size_full()
        .min_w_0()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .p(px(16.))
        .gap(px(4.))
        .text_center()
        .text_color(secondary_ink())
        .child(
            vorkspace_ui::fluid::empty_mark(
                div()
                    .size(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon),
                48.,
            )
            .mb(px(8.)),
        )
        .child(label().font_weight(FontWeight::MEDIUM).child(title.into()))
        .when(!detail.is_empty(), |d| {
            d.child(meta().max_w(px(360.)).child(detail))
        })
}

pub(crate) fn action(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    enabled: bool,
) -> Stateful<Div> {
    super::button(id, label, enabled)
        .aria_disabled(!enabled)
        .h(px(CONTROL_HEIGHT))
        .px(px(8.))
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .gap(px(4.))
        .rounded(px(radius::CONTROL))
        .border_1()
        .border_color(content(0.))
        .text_size(px(LABEL_SIZE))
        .line_height(px(LABEL_LINE))
        .text_color(content(0.8))
        .whitespace_nowrap()
        .when(enabled, |d| {
            d.cursor_pointer().hover(|s| {
                s.bg(vorkspace_ui::fluid::hover(0.05))
                    .text_color(content(1.))
            })
        })
        .when(!enabled, |d| d.opacity(0.4))
}

pub(crate) fn icon_action(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    enabled: bool,
) -> Stateful<Div> {
    let label = label.into();
    action(id, label.clone(), enabled)
        .size(px(CONTROL_HEIGHT))
        .px_0()
        .tooltip(move |_, cx| cx.new(|_| super::Tooltip(label.clone())).into())
}

pub(crate) fn primary(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    disabled: bool,
) -> Stateful<Div> {
    super::primary_button(id, label, disabled)
        .aria_disabled(disabled)
        .h(px(CONTROL_HEIGHT))
        .px(px(12.))
        .py_0()
        .gap(px(4.))
        .rounded(px(radius::CONTROL))
        .text_size(px(LABEL_SIZE))
        .line_height(px(LABEL_LINE))
        .border_1()
        .border_color(content(0.))
        .when(!disabled, |d| {
            d.focus_visible(|s| s.bg(content(0.85)).border_color(accent_color()))
        })
}

pub(crate) fn secondary(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    danger: bool,
    disabled: bool,
) -> Stateful<Div> {
    super::secondary_button(id, label, danger, disabled)
        .aria_disabled(disabled)
        .h(px(CONTROL_HEIGHT))
        .px(px(12.))
        .py_0()
        .gap(px(4.))
        .rounded(px(radius::CONTROL))
        .text_size(px(LABEL_SIZE))
        .line_height(px(LABEL_LINE))
}

pub(crate) fn tab(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
) -> Stateful<Div> {
    super::button(id, label, true)
        .role(Role::Tab)
        .aria_selected(selected)
        .h_full()
        .flex()
        .items_center()
        .mr(px(16.))
        .border_b_2()
        .border_color(if selected { content(0.95) } else { content(0.) })
        .text_size(px(LABEL_SIZE))
        .line_height(px(LABEL_LINE))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if selected {
            content(1.)
        } else {
            secondary_ink()
        })
        .cursor_pointer()
        .hover(|style| style.text_color(content(1.)))
}
