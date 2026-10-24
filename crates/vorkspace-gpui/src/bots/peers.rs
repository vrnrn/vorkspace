use super::*;
use serde_json::{Value, json};
use vorkspace_core::bot::{channels, peers};

impl Bots {
    /// A saved queue owns work independently of which Bot is visible. Loading
    /// through ensure_chat checks submission receipts before any dispatch.
    pub(super) fn resume_peer_queues(&mut self, cx: &mut Context<Self>) {
        let recipients = self
            .roster
            .bots
            .iter()
            .filter(|bot| {
                !self.chats.contains_key(&bot.id)
                    && bot.outbox.first().is_some_and(|message| {
                        message.error.is_none() && self.persisted_messages.contains(&message.id)
                    })
                    && bot.outbox.iter().any(|message| {
                        message.peer.is_some()
                            && message.error.is_none()
                            && self.persisted_messages.contains(&message.id)
                    })
            })
            .map(|bot| bot.id.clone())
            .collect::<Vec<_>>();
        for id in recipients {
            self.ensure_chat(&id, cx);
        }
    }

    pub(super) fn calling_view(&self, session: i64, cx: &App) -> Option<Entity<Conversation>> {
        self.chats
            .values()
            .chain(self.running.values().map(|r| &r.view))
            .chain(self.channels.views.values())
            .find(|view| view.read(cx).summary.id == session)
            .cloned()
    }
    /// Both routes consume the same autonomous budget. Creating/reusing a
    /// channel or changing routes cannot turn a peer-triggered turn into a user turn.
    pub(super) fn collaboration_hop(&self, session: i64, cx: &App) -> u8 {
        let peer = self
            .inflight
            .iter()
            .filter(|(bot, _)| {
                self.chats
                    .get(*bot)
                    .is_some_and(|v| v.read(cx).summary.id == session)
            })
            .filter_map(|(_, m)| m.peer.as_ref().map(|p| p.hop))
            .max()
            .unwrap_or(0);
        let channel = self
            .channels
            .views
            .iter()
            .filter(|(_, v)| v.read(cx).summary.id == session)
            .flat_map(|((channel, bot), _)| {
                self.channels
                    .state
                    .get(channel)
                    .into_iter()
                    .flat_map(move |c| {
                        c.deliveries
                            .iter()
                            .filter(move |d| d.bot == *bot && d.state == "running")
                            .map(|d| d.hop)
                    })
            })
            .max()
            .unwrap_or(0);
        peer.max(channel)
    }
    #[allow(clippy::too_many_arguments)]
    fn queue_peer(
        &mut self,
        from: &str,
        to: &str,
        text: &str,
        receipt: &str,
        hop: u8,
        expect_reply: bool,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let mut next = self.roster.clone();
        let accepted = peers::enqueue(&mut next, from, to, text, receipt, hop, expect_reply)?;
        self.roster = next;
        // Use the roster's ordered writer. Dispatch remains gated by its durable
        // acknowledgement; an older snapshot must never overwrite this intent.
        self.save(cx);
        self.ensure_chat(&accepted.bot, cx);
        self.configure_chat(&accepted.bot, cx);
        self.dispatch(&accepted.bot, cx);
        self.changed(cx);
        Ok(
            json!({"to":{"id":accepted.bot,"name":accepted.name},"queued":true,"alreadyQueued":accepted.already_queued,"hop":accepted.hop,"expectReply":accepted.expect_reply}),
        )
    }
    pub(super) fn peer_action(
        &mut self,
        call: &bot::bridge::Call,
        author: &str,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        match call.action.as_str() {
            "bots.list" => Ok(
                json!({"bots":self.roster.bots.iter().filter(|b| b.id != author).map(|b| json!({"id":b.id,"name":b.label(),"mascot":b.mascot,"status":self.chats.get(&b.id).map(|v|v.read(cx).bot_status()).unwrap_or("idle")})).collect::<Vec<_>>()}),
            ),
            "bots.send" => self.queue_peer(
                author,
                call.input["to"].as_str().ok_or("Provide to")?,
                call.input["message"].as_str().ok_or("Provide message")?,
                &format!("{}:{}", call.session, call.request_id),
                self.collaboration_hop(call.session, cx),
                call.input["expectReply"].as_bool().unwrap_or(true),
                cx,
            ),
            _ => Err("Unknown Bot action".into()),
        }
    }
    pub(super) fn reply_to_peer(
        &mut self,
        bot: &str,
        sent: &Message,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let Some(peer) = sent
            .peer
            .as_ref()
            .filter(|p| p.expect_reply && p.hop < channels::MAX_HOPS)
        else {
            return Ok(());
        };
        let Some(view) = self.chats.get(bot) else {
            return Err("Peer conversation is unavailable".into());
        };
        let rows = view.read(cx).bot_rows(cx);
        let start = rows
            .iter()
            .rposition(vorkspace_core::transcript::Row::is_prompt)
            .unwrap_or(rows.len());
        let reply = peers::final_reply(&rows[start..])?;
        if reply.trim().is_empty() {
            return Ok(());
        }
        // Preserve UTF-8 and the reply in its source conversation if it exceeds admission.
        self.queue_peer(
            bot,
            &peer.from,
            &reply,
            &format!("reply:{}", sent.id),
            peer.hop,
            false,
            cx,
        )
        .map(|_| ())
    }
}
