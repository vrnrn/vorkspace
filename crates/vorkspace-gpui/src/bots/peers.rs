use super::*;
use serde_json::{Value, json};
use vorkspace_core::bot::peers;

impl Bots {
    /// A saved queue owns work independently of which Bot is visible. Loading
    /// through ensure_chat checks submission receipts before any dispatch.
    pub(super) fn resume_pending_work(&mut self, cx: &mut Context<Self>) {
        let shared = self.shared_roster.read(cx);
        let recipients = self
            .roster
            .bots
            .iter()
            .filter(|bot| {
                self.roster.is_active(&bot.id)
                    && (!bot.is_minion() || (self.prefs.bots && self.prefs.minions))
                    && !self.chats.contains_key(&bot.id)
                    && (!bot.pending_reports.is_empty()
                        || bot.outbox.first().is_some_and(|first| {
                            first.error.is_none()
                                && self.persisted_messages.contains(&first.id)
                                && ((shared.live_message(&first.id)
                                    && shared.chat_owner(&bot.id).is_none())
                                    || bot.outbox.iter().any(|message| {
                                        message.peer.is_some()
                                            && message.error.is_none()
                                            && self.persisted_messages.contains(&message.id)
                                    }))
                        }))
            })
            .map(|bot| bot.id.clone())
            .collect::<Vec<_>>();
        for id in recipients {
            self.ensure_chat(&id, cx);
        }
    }

    pub(crate) fn calling_view(&self, session: i64, cx: &App) -> Option<Entity<Conversation>> {
        self.chats
            .values()
            .chain(self.running.values().map(|r| &r.view))
            .find(|view| view.read(cx).summary.id == session)
            .cloned()
    }
    /// Peer-triggered turns keep their autonomous budget; they never become user turns.
    pub(super) fn collaboration_hop(&self, session: i64, cx: &App) -> u8 {
        self.inflight
            .iter()
            .filter(|(bot, _)| {
                self.chats
                    .get(*bot)
                    .is_some_and(|v| v.read(cx).summary.id == session)
            })
            .filter_map(|(_, m)| m.peer.as_ref().map(|p| p.hop))
            .max()
            .unwrap_or(0)
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
            "bots.configure" => {
                let mode: bot::CompactionMode =
                    serde_json::from_value(call.input["compactionMode"].clone())
                        .map_err(|e| e.to_string())?;
                let owner = self
                    .roster
                    .get_mut(author)
                    .filter(|b| !b.is_minion())
                    .ok_or("Only a Bot can change its own context policy")?;
                owner.compaction_mode = mode;
                self.configure_chat(author, cx);
                self.changed(cx);
                // finish_call serializes roster persistence with prior writes
                // and acknowledges only after the durable receipt is saved.
                Ok(json!({"bot":author,"compactionMode":mode}))
            }
            "bots.list" => Ok(
                json!({"bots":self.roster.bots.iter().filter(|b| b.id != author && self.roster.can_communicate(author,&b.id)).map(|b| json!({"id":b.id,"name":b.label(),"mascot":b.mascot,"minion":b.minion,"status":self.chats.get(&b.id).map(|v|v.read(cx).bot_status()).unwrap_or("idle")})).collect::<Vec<_>>()}),
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
        if !self.roster.is_active(bot) {
            return Ok(());
        }
        let Some(peer) = sent
            .peer
            .as_ref()
            .filter(|p| p.expect_reply && p.hop < peers::MAX_HOPS)
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
