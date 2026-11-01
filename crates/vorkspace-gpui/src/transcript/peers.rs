//! Peer provenance is derived from saved internal prompts, independently of
//! delivery cursors and the provider's conversation state.
use super::*;
use crate::bots::avatar::BotLookConfig;
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
                        .child(crate::bots::avatar::avatar_with_look(
                            look, "idle", 14., true,
                        )),
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
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use vorkspace_core::{
        bot::{Bot, Roster, peers::enqueue},
        store::Store,
        transcript::Transcript,
    };

    fn prompt(body: &str, reply: bool) -> String {
        let mut roster = Roster::default();
        roster.bots = vec![
            Bot {
                id: "alpha".into(),
                name: "Alpha".into(),
                ..Default::default()
            },
            Bot {
                id: "beta".into(),
                name: "Beta".into(),
                ..Default::default()
            },
        ];
        enqueue(&mut roster, "alpha", "beta", body, "test", 0, reply).unwrap();
        roster.get_mut("beta").unwrap().outbox.remove(0).text
    }

    #[gpui::test]
    fn saved_peer_messages_are_visible_separate_turns_without_exposing_envelopes(
        cx: &mut TestAppContext,
    ) {
        let body = format!(
            "Hello Beta! 👋\n{}End.",
            "A longer message 🦀\n".repeat(150)
        );
        let peer_prompt = prompt(&body, true);
        let mut transcript = Transcript::default();
        transcript.begin("user");
        transcript.append("Hello");
        transcript.begin("assistant");
        transcript.append("Hello user");
        let incoming = transcript.begin("bot-system");
        transcript.append(&peer_prompt);
        let reply = transcript.begin("assistant");
        transcript.append("Hello Alpha!");
        let literal = transcript.begin("user");
        transcript.append(&peer_prompt);
        let human_reply = transcript.begin("assistant");
        transcript.append("That is literal text from the user");
        for row in &mut transcript.rows {
            row.sent_at = row.first.then_some(1_700_000_000_000);
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peer-history.db");
        let mut store = Store::open(&path).unwrap();
        let session = store.create("codex", "/project").unwrap().id;
        store.write_rows(session, 0, &transcript.rows).unwrap();
        drop(store);
        let saved = Store::open(&path).unwrap().load(session).unwrap().rows;
        assert_eq!(saved, transcript.rows);

        let view = cx.new(|_| TranscriptView::new());
        view.update(cx, |view, cx| {
            view.bot_mode("cat", 0xffffff, "Beta");
            view.install_prepared_history(PreparedHistory::new(saved, true), cx);
            assert_eq!(view.peer_at(incoming).unwrap().name, "Alpha");
            assert_eq!(
                view.bot_group_start(reply),
                reply,
                "peer turn must not merge into the user's answer"
            );
            assert!(view.peer_response_label(reply).is_some());
            assert!(view.peer_response_label(human_reply).is_none());
            assert!(
                view.peer_at(literal).is_none(),
                "user text cannot claim peer provenance"
            );
            assert!(
                view.bot_separator(incoming).is_none(),
                "same-hour messages do not need a new date"
            );
            assert!(view.bot_separator(reply).is_none());
            let shown: String = view
                .rows
                .iter()
                .filter(|row| view.messages[row.message].start == incoming)
                .filter_map(|row| match &row.body {
                    Body::Plain(text) => Some(text.as_ref()),
                    _ => None,
                })
                .collect();
            assert_eq!(shown, body);
            assert_eq!(
                view.source, transcript.rows,
                "display must not rewrite provider history"
            );
        });
    }

    #[gpui::test]
    fn queued_failed_and_one_way_messages_keep_their_origin(cx: &mut TestAppContext) {
        vorkspace_ui::clock::use_test_clock(cx);
        let peer_prompt = prompt("Hi Beta!", false);
        let (origin, body) = presentation(&peer_prompt).unwrap();
        assert!(!origin.auto_reply);
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| TranscriptView::new())
            })
            .unwrap()
        });
        window
            .update(cx, |view, _, cx| {
                view.bot_mode("cat", 0xffffff, "Beta");
                view.set_pending(
                    vec![PendingMessage {
                        id: "peer".into(),
                        text: body.to_owned().into(),
                        peer: Some(origin),
                        error: Some("Interrupted".into()),
                        sent_at: None,
                        attachments: Vec::new(),
                    }],
                    cx,
                );
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        assert!(visual.debug_bounds("bot-peer-message-label").is_some());
        assert!(visual.debug_bounds("bot-peer-chip").is_some());
        assert!(
            visual.debug_bounds("bot-peer-chip-avatar").is_none(),
            "an unknown sender keeps the name-only chip"
        );
        let bot = |id: &str| Bot {
            id: id.into(),
            name: "Alpha".into(),
            ..Default::default()
        };
        window
            .update(visual, |view, _, cx| {
                view.set_peer_looks(peer_looks(&[bot("a")]), cx)
            })
            .unwrap();
        visual.run_until_parked();
        assert!(visual.debug_bounds("bot-peer-chip-avatar").is_some());
        window
            .update(visual, |view, _, cx| {
                view.set_peer_looks(peer_looks(&[bot("a"), bot("b")]), cx)
            })
            .unwrap();
        visual.run_until_parked();
        assert!(
            visual.debug_bounds("bot-peer-chip-avatar").is_none(),
            "an ambiguous name must not guess an avatar"
        );
        window
            .update(visual, |view, _, cx| {
                let mut transcript = Transcript::default();
                transcript.begin("bot-system");
                transcript.append(&peer_prompt);
                let turn = transcript.begin("assistant");
                transcript.append("Acknowledged");
                view.set_pending(Vec::new(), cx);
                view.patch(0, transcript.rows, cx);
                assert!(!view.peer_at(0).unwrap().auto_reply);
                assert!(view.peer_response_label(turn).is_some());
                assert!(view.bot_separator(0).is_some());
                assert!(
                    view.bot_separator(turn).is_none(),
                    "date belongs before the visible incoming message"
                );
            })
            .unwrap();
        visual.run_until_parked();
        assert!(visual.debug_bounds("bot-peer-message-label").is_some());
        assert!(visual.debug_bounds("bot-peer-response-label").is_some());
    }

    #[test]
    fn peer_labels_read_like_chat_without_bot_to_bot_wording() {
        assert_eq!(peer_route("x", false), "→ x");
        assert_eq!(peer_route("x", true), "→ x · Queued");
        assert_eq!(replying_to("y"), "replying to y");
        for text in [peer_route("x", true), replying_to("y")] {
            assert!(!text.contains("Bot-to-bot") && !text.contains("Bot reply"));
        }
    }
}
