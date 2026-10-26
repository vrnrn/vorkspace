//! Fixed chat identity, outside the transcript's scroll and input ownership.
use super::avatar;
use crate::theme::*;
use gpui::{AnyElement, FontWeight, canvas, div, prelude::*, px};
use vorkspace_core::bot::Bot;

const FACE: f32 = 64.;
const BACKDROP: f32 = 256.;
pub(super) const TOP: f32 = 8.;
pub(super) const HEIGHT: f32 = TOP + FACE + 2. + 22.;

/// The chat owner supplies only its transcript body, never composer/attention
/// controls. This is the shared clipping and pass-through contract for identity.
pub(crate) fn chat_surface(
    body: AnyElement,
    bot: &Bot,
    status: &str,
    animation: Option<bool>,
) -> AnyElement {
    div()
        .debug_selector(|| "bot-chat-transcript".into())
        .flex_1()
        .min_h_0()
        .relative()
        .flex()
        .flex_col()
        .overflow_hidden()
        .when(animation.is_some(), |d| d.child(backdrop_mask(true)))
        .child(body)
        .when(animation.is_some(), |d| d.child(backdrop_mask(false)))
        .when_some(animation, |d, animate| {
            d.child(header(bot, status, animate))
        })
        .into_any_element()
}

fn backdrop_mask(begin: bool) -> AnyElement {
    div()
        .debug_selector(move || {
            if begin {
                "bot-chat-mask-begin"
            } else {
                "bot-chat-mask-end"
            }
            .into()
        })
        .absolute()
        .top(px(TOP + (FACE - BACKDROP) / 2.))
        .left(gpui::relative(0.5))
        .ml(px(-BACKDROP / 2.))
        .size(px(BACKDROP))
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    #[cfg(target_os = "macos")]
                    window.paint_quad(gpui::fill(bounds, gpui::black()).border_color(
                        gpui_apple::metal_renderer::radial_cutout_marker(begin, 0.2),
                    ));
                    #[cfg(not(target_os = "macos"))]
                    let _ = (bounds, window, begin);
                },
            )
            .size_full(),
        )
        .into_any_element()
}

fn header(bot: &Bot, status: &str, visible: bool) -> AnyElement {
    // The transcript host clips this decoration before the composer. Absolute
    // layout reserves no chat space; this decoration inserts no hitboxes.
    div()
        .debug_selector(|| "bot-chat-header".into())
        .absolute()
        .top_0()
        .left_0()
        .w_full()
        .h(px(HEIGHT))
        .flex()
        .flex_col()
        .items_center()
        .pt(px(TOP))
        .gap(px(2.))
        .text_color(content(1.))
        .child(
            div()
                .debug_selector({
                    let status = status.to_owned();
                    move || format!("bot-chat-header-{status}")
                })
                .flex_shrink_0()
                .child(avatar::header_avatar(bot, status, FACE, visible)),
        )
        .child(
            div()
                .debug_selector(|| "bot-chat-header-name".into())
                .max_w(gpui::relative(0.7))
                .h(px(22.))
                .text_size(px(13.))
                .line_height(px(22.))
                .font_weight(FontWeight::MEDIUM)
                .truncate()
                .child(bot.label()),
        )
        .into_any_element()
}
