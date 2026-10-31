//! Fixed chat identity, outside the transcript's scroll and input ownership.
use super::avatar;
use crate::theme::*;
use gpui::{
    AnyElement, App, AppContext, Context, Entity, FontWeight, Render, canvas, div, prelude::*, px,
};
use vorkspace_core::bot::Bot;

const FACE: f32 = 64.;
const BACKDROP: f32 = 256.;
/// Clear of the window's top edge: the Bot page has no title bar.
pub(super) const TOP: f32 = 24.;
pub(super) const HEIGHT: f32 = TOP + FACE + 2. + 22.;

#[derive(Clone)]
struct Identity {
    look: avatar::BotLookConfig,
    name: String,
    status: String,
    animate: bool,
}

/// Decorative ticks belong to this view, never the chat's notification graph.
pub(crate) struct ChatHeader {
    identity: Identity,
}

/// The chat owner supplies only its transcript body, never composer/attention
/// controls. This is the shared clipping and pass-through contract for identity.
pub(crate) fn chat_surface(
    body: AnyElement,
    bot: &Bot,
    status: &str,
    animation: Option<bool>,
    retained: &mut Option<Entity<ChatHeader>>,
    cx: &mut App,
) -> AnyElement {
    chat_surface_with_look(
        body,
        bot,
        avatar::BotLookConfig::from_bot(bot),
        status,
        animation,
        retained,
        cx,
    )
}

pub(crate) fn chat_surface_with_look(
    body: AnyElement,
    bot: &Bot,
    look: avatar::BotLookConfig,
    status: &str,
    animation: Option<bool>,
    retained: &mut Option<Entity<ChatHeader>>,
    cx: &mut App,
) -> AnyElement {
    let header = animation.map(|animate| {
        let identity = Identity {
            look,
            name: bot.label(),
            status: status.into(),
            animate,
        };
        let header = retained.get_or_insert_with(|| {
            cx.new(|_| ChatHeader {
                identity: identity.clone(),
            })
        });
        // This child is rendered in this frame. Apply parent-supplied props
        // immediately; notifying during layout would defer their publication.
        header.update(cx, |header, _| header.identity = identity);
        header.clone()
    });
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
        .when_some(header, |d, header| d.child(header))
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
                    window.paint_quad(gpui::fill(bounds, gpui::black()).border_color(
                        gpui_apple::metal_renderer::radial_cutout_marker(begin, 0.2),
                    ));
                },
            )
            .size_full(),
        )
        .into_any_element()
}

impl Render for ChatHeader {
    fn render(&mut self, _: &mut gpui::Window, _: &mut Context<Self>) -> impl IntoElement {
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
                        let status = self.identity.status.clone();
                        move || format!("bot-chat-header-{status}")
                    })
                    .flex_shrink_0()
                    .child(avatar::header_avatar(
                        self.identity.look.clone(),
                        &self.identity.status,
                        FACE,
                        self.identity.animate,
                    )),
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
                    .child(self.identity.name.clone()),
            )
            .into_any_element()
    }
}
