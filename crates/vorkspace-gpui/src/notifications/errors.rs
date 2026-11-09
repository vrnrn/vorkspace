//! Dismissible app errors use the same glass card as pending approvals.
use gpui::{Div, Hsla, Role, Stateful, div, prelude::*, px};
use vorkspace_ui::theme::*;

pub(crate) fn card(id: &'static str, message: String, dismiss: impl IntoElement) -> Stateful<Div> {
    let rose: Hsla = vorkspace_ui::theme::danger();
    let mut border = rose;
    border.a = 0.22;
    let mut tint = rose;
    tint.a = 0.1;
    super::surface::toast_frame()
        .id(id)
        .debug_selector(move || id.into())
        .role(Role::Alert)
        .border_color(border)
        .bg(tint)
        .relative()
        .overflow_hidden()
        .child(vorkspace_ui::fluid::strip(
            rose,
            vorkspace_ui::radius::SURFACE,
        ))
        // The danger strip and icon already say "error"; the message itself
        // is the headline, so it leads the card beside the dismiss button.
        .child(
            div()
                .pl(px(vorkspace_ui::fluid::STRIP_WIDTH + 10.))
                .pr(px(10.))
                .py(px(10.))
                .flex()
                .items_start()
                .gap_2()
                .child(
                    div()
                        .h(px(19.5))
                        .flex()
                        .items_center()
                        .child(icon("circle-alert", 16.).text_color(rose)),
                )
                .child(
                    div()
                        .id((id, 0usize))
                        .flex_1()
                        .min_w_0()
                        .max_h(px(220.))
                        .overflow_y_scroll()
                        .text_size(px(12.5))
                        .line_height(px(19.5))
                        .text_color(content(0.9))
                        .child(message),
                )
                .child(dismiss),
        )
}
