//! Shared native control behavior; callers retain their surface-specific styling.
use crate::theme::{accent_color, content, shell_fill};
use gpui::{
    Context, Div, ElementId, IntoElement, Render, Role, SharedString, Stateful, Window, div,
    prelude::*, px,
};

/// GPUI routes Return/Space releases and accessibility clicks through `on_click`.
/// Do not add a second key handler: it would activate the same command twice.
/// `enabled` admits keyboard focus; callers own disabled styling and callback guards.
pub(crate) fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    enabled: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.into())
        .when(enabled, |d| {
            d.focusable()
                .tab_stop(true)
                // Adding keyboard access must not steal an input's focus on click.
                .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                    window.prevent_default()
                })
                .focus_visible(|s| s.bg(content(0.15)).border_color(accent_color()))
        })
}

/// The common 24px icon action frame. Ink, cursor and hover remain caller-owned.
pub(crate) fn icon_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    enabled: bool,
) -> Stateful<Div> {
    let label = label.into();
    button(id, label.clone(), enabled)
        .size(px(24.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .tooltip(move |_, cx| cx.new(|_| Tooltip(label.clone())).into())
}

/// A 36×20 settings switch. Disabled presentation belongs to its containing row.
pub(crate) fn switch(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on: bool,
    enabled: bool,
) -> Stateful<Div> {
    switch_frame(id, label, on, enabled, 36., 20., 16.)
}

/// The existing 28×16 automation variant; it is deliberately smaller.
pub(crate) fn compact_switch(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on: bool,
) -> Stateful<Div> {
    switch_frame(id, label, on, true, 28., 16., 12.)
}

fn switch_frame(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on: bool,
    enabled: bool,
    width: f32,
    height: f32,
    knob: f32,
) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Switch)
        .aria_label(label.into())
        .aria_toggled(if on {
            gpui::Toggled::True
        } else {
            gpui::Toggled::False
        })
        .relative()
        .w(px(width))
        .h(px(height))
        .flex_shrink_0()
        .rounded_full()
        .bg(if on { accent_color() } else { content(0.2) })
        .when(enabled, |d| {
            d.focusable()
                .tab_stop(true)
                .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                    window.prevent_default()
                })
                .focus_visible(move |s| {
                    // A focus fill does not change the track or knob geometry.
                    s.bg(if on {
                        accent_color().opacity(0.65)
                    } else {
                        content(0.4)
                    })
                })
        })
        .child(
            div()
                .absolute()
                .top(px(2.))
                .left(px(if on { width - knob - 2. } else { 2. }))
                .size(px(knob))
                .rounded_full()
                .bg(gpui::white()),
        )
}

/// The existing visual-artifact tooltip, shared by compact icon actions.
pub(crate) struct Tooltip(pub(crate) SharedString);
impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut background = shell_fill();
        background.a = 1.;
        div()
            .debug_selector(|| "control-tooltip".into())
            .px_2()
            .py_1()
            .rounded_md()
            .bg(background)
            .border_1()
            .border_color(content(0.12))
            .text_size(px(11.))
            .line_height(px(16.))
            .text_color(content(0.85))
            .child(self.0.clone())
    }
}

#[cfg(test)]
pub(crate) mod tests;
