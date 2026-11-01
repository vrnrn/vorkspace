//! Peer provenance is derived from saved internal prompts, independently of
//! delivery cursors and the provider's conversation state.
use super::*;
use crate::avatar::BotLookConfig;
use std::collections::HashMap;
use vorkspace_core::bot::{
    Bot,
    peers::{Origin, presentation},
};

/// Name -> look for resolving a saved sender. A name that is shared by several
/// roster Bots is dropped: the saved envelope cannot say which one sent it.
pub(crate) fn peer_looks(bots: &[Bot]) -> HashMap<String, BotLookConfig> {
    let mut looks = HashMap::new();
    let mut ambiguous = std::collections::HashSet::new();
    for bot in bots {
        let name = bot.label().trim().to_owned();
        if looks
            .insert(name.clone(), BotLookConfig::from_bot(bot))
            .is_some()
        {
            ambiguous.insert(name);
        }
    }
    looks.retain(|name, _| !ambiguous.contains(name));
    looks
}

pub(super) fn prompt_origin(rows: &[Row]) -> Option<Origin> {
    if !rows.first().is_some_and(|row| {
        row.role == "bot-system" && row.text.starts_with("<bot_peer_message>\nFrom: ")
    }) {
        return None;
    }
    let text: String = rows.iter().map(|row| row.text.as_str()).collect();
    presentation(&text).map(|(origin, _)| origin)
}

fn peer_route(recipient: &str, queued: bool) -> String {
    format!("→ {recipient}{}", if queued { " · Queued" } else { "" })
}

fn replying_to(sender: &str) -> String {
    format!("replying to {sender}")
}

impl TranscriptView {
    pub(super) fn peer_at(&self, start: usize) -> Option<&Origin> {
        let message = self
            .messages
            .get(self.messages.partition_point(|m| m.start < start))?;
        message.peer.as_ref().filter(|_| message.start == start)
    }

    /// Replace the roster snapshot used to give a peer sender its avatar.
    pub(crate) fn set_peer_looks(
        &mut self,
        looks: HashMap<String, BotLookConfig>,
        cx: &mut Context<Self>,
    ) {
        if self.peer_looks != looks {
            self.peer_looks = looks;
            cx.notify();
        }
    }

    /// The sender's name in a rounded pill. Saved envelopes carry only the
    /// sender's name, so the avatar appears only when the current roster has
    /// exactly one Bot of that name; otherwise the chip is name-only.
    fn peer_chip(&self, name: &str) -> gpui::Div {
        let look = self.peer_looks.get(name.trim()).cloned();
        div()
            .debug_selector(|| "bot-peer-chip".into())
            .min_w_0()
            .flex()
            .items_center()
            .gap_1p5()
            .flex_shrink_0()
            .max_w(px(320.))
            .rounded_full()
            .bg(content(0.07))
            .py(px(2.))
            .pr_3()
            .when_some(look, |d, look| {
                d.pl_2().child(
                    div()
                        .debug_selector(|| "bot-peer-chip-avatar".into())
                        .size(px(14.))
                        .flex_shrink_0()
                        .child(crate::avatar::avatar_with_look(look, "idle", 14., true)),
                )
            })
            .when(!self.peer_looks.contains_key(name.trim()), |d| d.pl_3())
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(content(0.8))
                    .child(name.to_owned()),
            )
    }

    pub(super) fn peer_message_label(&self, peer: &Origin, queued: bool) -> gpui::AnyElement {
        let recipient = self
            .bot_look
            .as_ref()
            .map_or("Bot", |(_, name)| name.as_ref());
        div()
            .id("bot-peer-message-label")
            .debug_selector(|| "bot-peer-message-label".into())
            .role(Role::Label)
            .aria_label(format!(
                "{}{} to {recipient}",
                if queued { "Queued: " } else { "" },
                peer.name,
            ))
            .min_w_0()
            .flex()
            .items_center()
            .gap_1p5()
            .pb_1()
            .text_size(px(12.))
            .line_height(px(18.))
            .text_color(content(0.6))
            .child(self.peer_chip(&peer.name))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .child(peer_route(recipient, queued)),
            )
            .into_any_element()
    }

    pub(super) fn peer_message_body(
        &self,
        start: usize,
        peer: &Origin,
        first: bool,
        last: bool,
        body: gpui::AnyElement,
    ) -> gpui::AnyElement {
        div()
            .id(("bot-peer-message", start))
            .w_full()
            .max_w(px(576.))
            .min_w_0()
            .flex()
            .flex_col()
            .items_start()
            .pt(px(if first { 10. } else { 0. }))
            .pb(px(if last { 10. } else { 0. }))
            .when(first, |d| d.child(self.peer_message_label(peer, false)))
            .child(
                div()
                    .debug_selector(|| "bot-peer-bubble".into())
                    .max_w_full()
                    .min_w_0()
                    .when(!(first && last), |d| d.w_full())
                    .bg(user_bubble_fill())
                    .px_3()
                    .pt(px(if first { 8. } else { 0. }))
                    .pb(px(if last { 8. } else { 0. }))
                    .when(first, |d| d.rounded_t_xl())
                    .when(last, |d| d.rounded_b_xl())
                    .text_size(px(14.))
                    .line_height(px(20.))
                    .child(body),
            )
            .into_any_element()
    }

    pub(super) fn peer_response_label(&self, turn: usize) -> Option<gpui::AnyElement> {
        let prompt = self.prompt_start(turn);
        let peer = self.peer_at(prompt)?;
        let label = replying_to(&peer.name);
        let bot = self
            .bot_look
            .as_ref()
            .map_or("Bot", |(_, name)| name.as_ref());
        Some(
            div()
                .id("bot-peer-response-label")
                .debug_selector(|| "bot-peer-response-label".into())
                .role(Role::Label)
                .aria_label(format!("{bot}, {label}"))
                .min_w_0()
                .truncate()
                .text_size(px(12.))
                .text_color(content(0.6))
                .child(label)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests;
