use super::*;
use gpui::App;
use std::path::PathBuf;
use vorkspace_core::bot::{Bot, Message};
use vorkspace_ui::radius;
#[cfg(test)]
#[path = "bot_tests.rs"]
mod tests;
pub(super) struct DirectSubmission {
    pub(super) id: String,
    pub(super) text: String,
    pub(super) attachments: Vec<PathBuf>,
    pub(super) retry: bool,
}

#[derive(Default, serde::Serialize)]
pub(super) struct SubmissionOptions {
    pub(super) mode: Option<String>,
    pub(super) queue: bool,
}
impl SubmissionOptions {
    pub(super) fn read(payload: &serde_json::Value) -> Option<Self> {
        let fields = &payload["botSubmission"];
        fields.is_object().then(|| Self {
            mode: fields["mode"].as_str().map(str::to_owned),
            queue: fields["queue"].as_bool().unwrap_or(false),
        })
    }
    pub(super) fn write(&self, payload: &mut serde_json::Value) {
        payload["botSubmission"] = serde_json::json!(self);
    }
}
#[derive(Default)]
pub(super) struct ChatState {
    /// Standalone Bot chats opt into identity decoration; others keep None.
    /// Some(false) retains the header but freezes it beneath a host modal.
    pub(super) header_animation: Option<bool>,
    pub(super) header: Option<Entity<crate::bot_header::ChatHeader>>,
    pub(super) header_action: Option<crate::bot_header::Activate>,
    pub(super) pending: Vec<Message>,
    pub(super) card_versions: std::collections::HashMap<String, u64>,
    /// The next message waits for the current reply instead of steering it.
    pub(super) queue: bool,
    /// IDs whose outbox publication succeeded, including across a cold load.
    pub(super) accepted_drafts: std::collections::HashMap<String, Message>,
    pub(super) draft_sending: Option<String>,
    /// Composer input remains durable until its outbox publication succeeds.
    pub(super) direct_sending: Option<DirectSubmission>,
    pub(super) draft_retry: bool,
    pub(super) draft_queue: bool,
    pub(super) draft_error: Option<String>,
    pub(super) delivered: Option<String>,
    /// ModelPicker with `hideSettings`: the model list opens on its own.
    pub(super) models: Option<(Entity<ModelList>, Vec<Subscription>)>,
    /// The setting pills' anchors, by `ComposerMenu::Setting` index.
    pub(super) setting_anchors: Vec<AnchorSlot>,
}
pub(crate) struct Submitted {
    pub draft_id: Option<String>,
    pub text: String,
    pub prepared_prompt: Option<String>,
    pub attachments: Vec<PathBuf>,
    pub queue: bool,
    pub mode: Option<String>,
}
pub(crate) struct QueueChanged {
    pub id: String,
    pub remove: bool,
}
/// The provider did not confirm a steered outbox message. The Bot outbox owns
/// the message, so it records the error and offers Retry/Discard.
pub(crate) struct SteerFailed {
    pub id: String,
    pub error: String,
}
impl EventEmitter<QueueChanged> for Conversation {}
impl EventEmitter<Submitted> for Conversation {}
impl EventEmitter<SteerFailed> for Conversation {}
fn card_at(
    rows: &[vorkspace_core::transcript::Row],
    hint: usize,
    id: &str,
) -> Option<(usize, serde_json::Value)> {
    let parse = |row: &vorkspace_core::transcript::Row| {
        (row.role == "bot-card")
            .then(|| serde_json::from_str::<serde_json::Value>(&row.text).ok())
            .flatten()
            .filter(|card| card["id"].as_str() == Some(id))
    };
    if let Some(card) = rows.get(hint).and_then(parse) {
        return Some((hint, card));
    }
    rows.iter()
        .enumerate()
        .find_map(|(index, row)| parse(row).map(|card| (index, card)))
}

impl Conversation {
    pub(crate) fn bot_draft_outbox_saved(&mut self, message: &Message, cx: &mut Context<Self>) {
        let id = &message.id;
        if self
            .bot_ui
            .direct_sending
            .as_ref()
            .is_some_and(|pending| &pending.id == id)
        {
            let pending = self.bot_ui.direct_sending.take().unwrap();
            let unchanged = self.input.read(cx).content.as_ref() == pending.text
                && self.attachments == pending.attachments;
            if unchanged {
                self.input
                    .update(cx, |input, cx| input.set_text(String::new(), cx));
                self.attachments.clear();
            }
            // Publish the cleared or newer composer before deleting the only
            // durable link between the old input and its outbox identity.
            self.capture_composer(cx);
            let result = self.composer_draft.flush().and_then(|()| {
                locked(&self.store)?.clear_composer_recovery(self.summary.id, &pending.id)
            });
            if let Err(error) = result {
                self.status = format!(
                    "Message queued, but its composer recovery could not be cleared: {error}"
                );
                self.composer_draft.error = Some(self.status.clone());
            } else {
                self.composer_draft.forget_recovery(&pending.id);
            }
            self.bot_ui.draft_error = None;
            cx.notify();
        }
        self.bot_ui
            .accepted_drafts
            .insert(id.clone(), message.clone());
        if self
            .follow_ups
            .state
            .draft
            .as_ref()
            .is_none_or(|draft| &draft.id != id)
            && self.bot_ui.draft_sending.as_deref() != Some(id.as_str())
        {
            return;
        }
        if self.transcript.read(cx).has_saved_draft_edit(id) {
            self.bot_ui.draft_error = Some("An earlier version was queued while this draft was being edited. Your edit is preserved.".into());
            cx.notify();
            return;
        }
        if self
            .follow_ups
            .state
            .draft
            .as_ref()
            .is_some_and(|draft| &draft.id == id)
            && self.needs_history_restore()
        {
            let message = message.clone();
            self.defer_history_action(
                move |this, cx| this.bot_draft_outbox_saved(&message, cx),
                cx,
            );
            return;
        }
        if let Some(draft) = self
            .follow_ups
            .state
            .draft
            .as_ref()
            .filter(|draft| &draft.id == id)
        {
            let same = draft.text == message.text
                && draft
                    .attachments
                    .iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .eq(message.attachments.iter().cloned());
            if !same {
                self.bot_ui.draft_error = Some("This draft changed after an earlier version was queued. Reopen the conversation to review both messages.".into());
                self.transcript
                    .update(cx, |view, cx| view.set_draft_transfer(Some(false), cx));
                cx.notify();
                return;
            }
            let mut next = self.follow_ups.state.clone();
            next.revision += 1;
            next.draft = None;
            let result = locked(&self.store)
                .and_then(|store| store.accept_follow_ups(self.summary.id, &next));
            if let Err(error) = result {
                self.bot_ui.draft_error = Some(format!(
                    "The message is queued, but its draft could not be cleared: {error}. Reopen the conversation to recover it."
                ));
                self.transcript
                    .update(cx, |view, cx| view.set_draft_transfer(Some(false), cx));
                cx.notify();
                return;
            }
            self.follow_ups.persisted = next.revision;
            self.follow_ups.state = next;
            self.transcript
                .update(cx, |view, cx| view.set_saved_draft(None, cx));
            self.sync_bot_pending(cx);
            cx.emit(SessionChanged::Draft);
        }
        if self.bot_ui.draft_sending.as_deref() == Some(id.as_str()) {
            self.bot_ui.draft_sending = None;
            self.bot_ui.draft_retry = false;
            self.bot_ui.draft_error = None;
            self.transcript
                .update(cx, |view, cx| view.set_draft_transfer(None, cx));
        }
        cx.notify();
    }
    pub(crate) fn bot_draft_outbox_failed(
        &mut self,
        id: &str,
        error: &str,
        cx: &mut Context<Self>,
    ) {
        if self
            .bot_ui
            .direct_sending
            .as_ref()
            .is_some_and(|pending| pending.id == id)
        {
            if let Some(pending) = self.bot_ui.direct_sending.as_mut() {
                pending.retry = true;
            }
            self.status = format!(
                "Could not queue message: {error}. Your composer was preserved; send the same input to retry."
            );
            self.bot_ui.draft_error = Some(self.status.clone());
            cx.notify();
        }
        if self.bot_ui.draft_sending.as_deref() == Some(id) {
            self.bot_ui.draft_retry = true;
            self.bot_ui.draft_error = Some(format!(
                "Could not queue saved draft: {error}. Your draft is preserved; choose Send to retry."
            ));
            self.transcript
                .update(cx, |view, cx| view.set_draft_transfer(Some(true), cx));
            cx.notify();
        }
    }
    pub(crate) fn show_bot_header(&mut self, animate: bool, cx: &mut Context<Self>) {
        if self.bot_ui.header_animation != Some(animate) {
            self.bot_ui.header_animation = Some(animate);
            cx.notify();
        }
    }
    pub(crate) fn set_bot_header_action(&mut self, action: Option<crate::bot_header::Activate>) {
        self.bot_ui.header_action = action;
    }
    pub(crate) fn bot_quote(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.reject_user_entry(cx) {
            return;
        }
        if self.input.read(cx).is_composing() {
            return;
        }
        self.input.update(cx, |i, cx| {
            i.set_text(
                vorkspace_core::composer::append_selection_quote(&i.content, text),
                cx,
            );
        });
        window.focus(&self.input.focus_handle(cx), cx);
    }
    pub(crate) fn bot_attention(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        div()
            .child(self.approval_shelf(window, cx))
            .when(!self.user_read_only(), |d| {
                d.when_some(self.questions.front(), |d, (card, _)| d.child(card.clone()))
            })
            .into_any_element()
    }
    pub(crate) fn configure_bot(
        &mut self,
        bot: Bot,
        root: PathBuf,
        environment: std::collections::HashMap<String, String>,
        cx: &mut Context<Self>,
    ) {
        self.input
            .update(cx, |input, cx| input.bot_composer(&bot.label(), cx));
        self.completion
            .update(cx, |completion, cx| completion.set_bot_chat(cx));
        let name = bot.label();
        let look = self.actor_look(&bot);
        if bot.is_minion() || self.user_read_only() {
            self.input.update(cx, |input, _| input.set_read_only(true));
        }
        self.transcript
            .update(cx, |t, _| t.bot_mode_with_look(look, &name));
        self.settings.environment = environment;
        self.bot_ui
            .accepted_drafts
            .retain(|id, _| bot.outbox.iter().any(|message| &message.id == id));
        self.bot_ui.pending = bot.outbox.clone();
        self.sync_bot_pending(cx);
        self.settings.bot = Some(vorkspace_core::agent::BotContext {
            bot,
            root,
            executable: std::env::current_exe().unwrap_or_default(),
        });
        cx.notify();
    }
    /// The user's queued messages show in the transcript as bubbles; the one
    /// just delivered is already a transcript row.
    pub(super) fn sync_bot_pending(&mut self, cx: &mut Context<Self>) {
        let delivered = self.bot_ui.delivered.clone();
        let pending = self
            .bot_ui
            .pending
            .iter()
            .filter(|m| {
                (!m.internal || m.peer.is_some())
                    && (m.error.is_some() || Some(&m.id) != delivered.as_ref())
                    && self
                        .follow_ups
                        .state
                        .draft
                        .as_ref()
                        .is_none_or(|draft| draft.id != m.id)
            })
            .map(|m| {
                let peer = m
                    .peer
                    .as_ref()
                    .and_then(|_| vorkspace_core::bot::peers::presentation(&m.text));
                crate::transcript::PendingMessage {
                    id: m.id.clone(),
                    text: peer
                        .as_ref()
                        .map(|(_, body)| *body)
                        .unwrap_or(&m.text)
                        .to_owned()
                        .into(),
                    error: m.error.clone(),
                    sent_at: m.sent_at,
                    attachments: m.attachments.iter().map(PathBuf::from).collect(),
                    peer: peer.map(|(origin, _)| origin),
                }
            })
            .collect();
        self.transcript
            .update(cx, |t, cx| t.set_pending(pending, cx));
    }
    pub(crate) fn voice_composer_ready(&self) -> bool {
        !self.user_read_only() && self.history_ready() && !self.account_switching
    }
    pub(crate) fn bot_ready(&self, cx: &App) -> bool {
        self.history_ready()
            && !self.turn_in_flight()
            && self.loaded
            && self.settings.usage_limit.is_none()
            && !self.account_switching
            && Kind::parse(&self.summary.provider).is_some()
            && turn_driver::available(&self.store, self.summary.id, cx)
    }
    pub(crate) fn bot_settled(&self) -> bool {
        !self.turn_in_flight()
    }
    /// Model selection belongs to the conversation, while Settings also holds
    /// the last loaded model. Derived execution lanes need the live selection.
    pub(crate) fn bot_execution_settings(&self) -> Settings {
        let mut settings = self.settings.clone();
        settings.model = self.model.clone();
        settings
    }
    pub(crate) fn bot_dispatch(&mut self, message: &Message, cx: &mut Context<Self>) -> bool {
        self.bot_dispatch_with_mode(message, None, cx)
    }
    pub(crate) fn bot_dispatch_with_mode(
        &mut self,
        message: &Message,
        mode: Option<&str>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.user_read_only() && self.settings.bot.is_none() {
            return false;
        }
        if !self.history_ready() {
            self.load_complete(cx);
            return false;
        }
        if self.settings.pending_provider_switch.is_some() && self.responding() {
            self.stop(cx);
        }
        if self.turn_in_flight()
            || !self.loaded
            || self.settings.usage_limit.is_some()
            || self.account_switching
        {
            return false;
        }
        self.launch_bot_prompt(message, mode, cx)
    }
    /// The user's own message joins the active turn through the provider's
    /// follow-up control, as in a session. Peer and automatic messages keep
    /// whole-turn delivery because their replies belong to a complete turn.
    /// Acceptance writes the same delivery receipt as a new turn would.
    pub(crate) fn bot_steer(&mut self, message: &Message, cx: &mut Context<Self>) -> bool {
        if message.peer.is_some()
            || message.internal
            || message.error.is_some()
            || !self.responding()
            || self.is_stopping()
            || !self.follow_ups.ready
            || self.follow_ups.sending.is_some()
            || self.settings.pending_provider_switch.is_some()
            || !cx
                .try_global::<vorkspace_ui::preferences::Preferences>()
                .is_none_or(|p| p.follow_up == "steer")
            || !Kind::from_id(&self.summary.provider).can_steer()
            || self
                .questions
                .iter()
                .any(|(card, _)| card.read(cx).blocks_steering())
        {
            return false;
        }
        let Some(controls) = &self.controls else {
            return false;
        };
        let prepared = message
            .prepared_prompt
            .clone()
            .unwrap_or_else(|| self.completion.read(cx).prepare_prompt(&message.text));
        let steer = vorkspace_core::follow_up::Message {
            id: message.id.clone(),
            text: message.text.clone(),
            prepared_prompt: message
                .prepared_prompt
                .clone()
                .or_else(|| (prepared != message.text).then_some(prepared)),
            attachments: message.attachments.iter().map(PathBuf::from).collect(),
            steer: true,
            ..Default::default()
        };
        if controls
            .send(Control {
                id: message.id.clone(),
                decision: Decision::Steer(steer),
            })
            .is_err()
        {
            return false;
        }
        self.follow_ups.sending = Some(message.id.clone());
        true
    }
    pub(crate) fn bot_rows<'a>(&self, cx: &'a App) -> &'a [vorkspace_core::transcript::Row] {
        self.transcript.read(cx).source_rows()
    }
    pub(crate) fn bot_row_origin(&self, cx: &App) -> usize {
        self.transcript.read(cx).storage_origin().unwrap_or(0)
    }
    pub(crate) fn bot_row_count(&self, cx: &App) -> usize {
        self.bot_row_origin(cx)
            .saturating_add(self.bot_rows(cx).len())
    }
    pub(crate) fn bot_patch_card(
        &mut self,
        index: usize,
        id: &str,
        generation: u64,
        expected: serde_json::Value,
        value: serde_json::Value,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.submission_generation != generation {
            return Ok(());
        }
        if !self.history_ready() || self.is_stopping() {
            return Err("Conversation history is loading; retry the update when ready".into());
        }
        // Row offsets are merely hints. Card IDs and the captured value own
        // refresh identity, including PR metadata fetched before a later edit.
        let current = card_at(self.transcript.read(cx).source_rows(), index, id);
        if current.as_ref().map(|(_, card)| card) != Some(&expected)
            || value["id"].as_str() != Some(id)
        {
            return Ok(());
        }
        if value == expected && !self.bot_ui.card_versions.contains_key(id) {
            return Ok(());
        }
        let session = self.summary.id;
        let Some(saved) = crate::card_saves::writer(cx)?.admit(
            self.store.clone(),
            session,
            expected.clone(),
            value,
        )?
        else {
            return Ok(());
        };
        let version = saved.version;
        self.bot_ui.card_versions.insert(id.to_owned(), version);
        let id = id.to_owned();
        // Only the weak UI observer is detached. The application owns the save
        // worker and drains accepted writes at quit, independently of this view.
        cx.spawn(async move |this, cx| {
            let Ok(result) = saved.recv().await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if this.bot_ui.card_versions.get(&id) != Some(&version) {
                    return;
                }
                this.bot_ui.card_versions.remove(&id);
                // Generation fences fetched metadata before admission. Once
                // committed, the row belongs to the session across new turns;
                // version and expected-value checks still protect newer cards.
                if this.summary.id != session {
                    return;
                }
                match result {
                    Ok(Some(row)) if this.history_ready() => {
                        if let Some((index, _)) =
                            card_at(this.transcript.read(cx).source_rows(), index, &id)
                                .filter(|(_, current)| *current == expected)
                        {
                            this.transcript
                                .update(cx, |t, cx| t.patch(index, vec![row], cx));
                            cx.notify();
                        }
                    }
                    Err(_) => {
                        // Refreshes are repeatable. Leave the old durable/UI
                        // value and release admission so the next tick can retry.
                        // Never log payloads or provider error strings.
                        eprintln!("Bot card refresh could not be saved");
                    }
                    _ => {}
                }
            });
        })
        .detach();
        Ok(())
    }
    pub(crate) fn bot_status(&self) -> &'static str {
        if self.needs_approval() || self.settings.usage_limit.is_some() {
            "needs-you"
        } else if self.responding() {
            "working"
        } else {
            "idle"
        }
    }
    /// A compact description of the conversation owner's current lifecycle.
    pub(crate) fn bot_status_label(&self) -> &'static str {
        if self.needs_approval() {
            "Needs your input"
        } else if self.settings.usage_limit.is_some() {
            "Usage limit reached"
        } else if self.is_stopping() {
            "Stopping"
        } else if self.responding() {
            if self.status == "Thinking…" {
                "Thinking"
            } else {
                "Working"
            }
        } else if self.history_loading() {
            "Loading"
        } else if self.status != "Ready" {
            "Needs attention"
        } else if self.summary.has_messages {
            "Done"
        } else {
            "Ready"
        }
    }
    pub(crate) fn bot_model(&self) -> Option<String> {
        self.model.clone()
    }
    pub(crate) fn bot_model_label(&self) -> SharedString {
        self.model_label()
    }
    #[cfg(test)]
    pub(crate) fn app_hold_account_switch(&mut self) {
        self.account_switching = true;
    }
    #[cfg(test)]
    pub(crate) fn app_steerable_turn(&mut self, controls: std::sync::mpsc::Sender<Control>) {
        self.turn_state = TurnState::Running;
        self.controls = Some(controls);
        self.follow_ups.ready = true;
    }
    #[cfg(test)]
    pub(crate) fn app_steer_delivered(&mut self, id: &str, cx: &mut Context<Self>) {
        self.follow_up_delivered(id, cx);
        cx.notify();
    }
    #[cfg(test)]
    pub(crate) fn app_settle_stopped_turn(&mut self, cx: &mut Context<Self>) {
        // Reproduce a late terminal update that leaves status Ready. The
        // cancellation receipt, not the provider's status text, must win.
        assert!(self.is_stopping());
        self.turn_state = TurnState::Settled(vorkspace_runtime::runtime::TurnOutcome::Cancelled);
        cx.emit(SessionChanged::Settled);
        cx.notify();
    }
    pub(crate) fn app_flush_draft(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        self.capture_composer(cx);
        self.composer_draft.flush()
    }
    pub(crate) fn discard_unaccepted_app_start(&mut self) {
        self.settings_write_cancel.store(true, Ordering::Release);
        self.catalog_epoch = self.catalog_epoch.wrapping_add(1);
        self.composer_draft.discard();
    }
    /// App management pauses even an idle queue. Publish the higher revision
    /// before replying so older queued writes cannot restore automatic sending.
    pub(crate) fn app_stop(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        if !self.loaded || self.history_loading() {
            return Err("Session is loading; its queue and draft were preserved. Try again after loading completes.".into());
        }
        self.stop_if_busy(cx);
        self.follow_ups.resuming = false;
        self.follow_ups.requested = None;
        self.follow_ups.continuations.clear();
        self.follow_ups.state.paused = true;
        self.follow_ups.state.revision += 1;
        let mut snapshot = self.follow_ups.state.clone();
        let directory = locked(&self.store)?.data_dir().map(PathBuf::from);
        snapshot.retain_attachments(self.summary.id, directory.as_deref())?;
        locked(&self.store)?.accept_follow_ups(self.summary.id, &snapshot)?;
        self.follow_ups.state = snapshot;
        self.follow_ups.persisted = self.follow_ups.state.revision;
        self.composer_draft.flush()?;
        cx.emit(SessionChanged::Draft);
        cx.notify();
        Ok(())
    }
    pub(crate) fn bot_stop(&mut self, cx: &mut Context<Self>) {
        self.stop_if_busy(cx);
    }
    pub(crate) fn bot_post(
        &mut self,
        role: &str,
        text: String,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !self.history_ready() {
            self.load_complete(cx);
            return Err("Conversation history is loading; retry the report when ready".into());
        }
        if self.turn_in_flight() {
            return Err("Wait for the current turn before posting a report".into());
        }
        let row = vorkspace_core::transcript::Row {
            role: role.into(),
            text,
            first: true,
            sent_at: Some(vorkspace_core::time::now_ms()),
        };
        let at = self.transcript.read(cx).source_rows().len();
        locked(&self.store)?.write_rows(self.summary.id, at, std::slice::from_ref(&row))?;
        self.transcript
            .update(cx, |t, cx| t.patch(at, vec![row], cx));
        self.summary.has_messages = true;
        cx.emit(SessionChanged::History);
        cx.notify();
        Ok(())
    }
    pub(crate) fn bot_access_options(&self) -> Vec<MenuItem> {
        super::bot_settings::access_options(
            Kind::from_id(&self.summary.provider),
            self.settings.access,
            self.responding(),
        )
    }
    pub(crate) fn bot_access(
        &mut self,
        mode: vorkspace_core::agent::AccessMode,
        cx: &mut Context<Self>,
    ) {
        self.settings.access = mode;
        super::remember_access(mode, cx);
        self.save_settings(cx);
        cx.notify();
    }
    pub(crate) fn bot_delivered(&self) -> Option<&str> {
        self.bot_ui.delivered.as_deref()
    }
    pub(crate) fn bot_model_options(&self) -> Vec<MenuItem> {
        super::bot_settings::model_options(
            Kind::from_id(&self.summary.provider),
            self.model_info(),
            &self.settings,
        )
    }
    pub(crate) fn bot_pick_model_option(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.turn_in_flight() {
            return Err("Wait for the current reply before changing model settings".into());
        }
        self.menu_choice(id, cx);
        cx.emit(SessionChanged::Configuration);
        Ok(())
    }
    pub(crate) fn bot_config(
        &mut self,
        provider: Kind,
        model: Option<String>,
        values: std::collections::HashMap<String, String>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.turn_in_flight() {
            return Err("Wait for the current reply before changing models".into());
        }
        if self.summary.provider != provider.id() {
            self.summary.provider = provider.id().into();
            self.summary.provider_id = None;
            self.catalog = Default::default();
        }
        self.model = model;
        self.settings.effort = super::preferred_effort(provider.id(), cx);
        self.settings.fast = false;
        self.settings.thinking = None;
        self.settings.service_tier = None;
        if let Some(context) = &mut self.context {
            context.window = None;
        }
        self.settings.model_settings = values;
        self.resolved_model = None;
        self.refresh_catalog(cx);
        self.save_settings(cx);
        cx.emit(SessionChanged::Configuration);
        cx.notify();
        Ok(())
    }
    fn can_fill_starter(&self, cx: &App) -> bool {
        let input = self.input.read(cx);
        input.content.is_empty() && !input.is_composing()
    }

    /// First messages for an empty conversation: a click fills the composer
    /// for editing rather than sending.
    pub(super) fn starters(
        &self,
        id: &'static str,
        starters: Vec<String>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let can_fill = self.can_fill_starter(cx);
        div()
            .flex()
            .flex_wrap()
            .justify_center()
            .gap_2()
            .max_w(px(520.))
            .children(starters.into_iter().enumerate().map(|(ix, text)| {
                let fill = text.clone();
                vorkspace_ui::theme::keyboard_reachable(div().id((id, ix)), can_fill)
                    .debug_selector(move || format!("{id}-{ix}"))
                    .role(gpui::Role::Button)
                    .aria_label(text.clone())
                    .px_3()
                    .py(px(6.))
                    .rounded_full()
                    .border_1()
                    .border_color(content(0.12))
                    .text_size(px(12.))
                    .text_color(content(0.75))
                    .when(can_fill, |d| {
                        d.cursor_pointer().hover(|s| {
                            s.bg(vorkspace_ui::fluid::hover(0.06))
                                .text_color(content(1.))
                        })
                    })
                    .when(!can_fill, |d| d.opacity(0.45))
                    .child(text)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if !this.can_fill_starter(cx) {
                            return;
                        }
                        this.input
                            .update(cx, |input, cx| input.set_text(fill.clone(), cx));
                        window.focus(&this.input.focus_handle(cx), cx);
                        cx.notify();
                    }))
            }))
    }
    pub(super) fn render_bot(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let command = if self.follow_ups.draft_selected {
            Some(input::Command::Draft)
        } else if self.settings.orchestrator {
            Some(input::Command::Orchestrator)
        } else if self.settings.plan {
            Some(input::Command::Plan)
        } else if self.settings.operator {
            Some(input::Command::Operator)
        } else {
            None
        };
        self.input
            .update(cx, |input, cx| input.selected_command(command, cx));
        let bot = self.settings.bot.as_ref().unwrap().bot.clone();
        // The fixed Bot header owns identity; the empty state only offers help.
        let body = if !self.summary.has_messages {
            let project = bot
                .projects
                .first()
                .map(|p| {
                    std::path::Path::new(p)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                })
                .unwrap_or_default();
            let line = if bot.projects.is_empty() {
                "Add the projects I should work on in my settings, or just ask me something."
                    .to_owned()
            } else {
                format!(
                    "I can see every session, note and folder in {}. Ask what’s going on, or hand me work.",
                    bot.projects
                        .iter()
                        .map(|p| std::path::Path::new(p)
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            // A first message is the hardest; offer three that show what a
            // Bot is for. A click fills the composer, so it can be edited.
            let starters: Vec<String> = if project.is_empty() {
                vec![
                    "What can you help me with?".into(),
                    "Set up a morning digest habit".into(),
                ]
            } else {
                vec![
                    format!("What changed in {project} today?"),
                    "Summarize the sessions still running".into(),
                    "Plan my next task".into(),
                ]
            };
            let greeting = div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .px_6()
                .py_12()
                .child(
                    div()
                        .mt_3()
                        .max_w(px(384.))
                        .text_center()
                        .text_size(px(13.))
                        .line_height(px(21.))
                        .text_color(content(0.5))
                        .child(line),
                )
                .child(self.starters("bot-starter", starters, cx).mt_5());
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(greeting)
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .child(
                    self.transcript
                        .clone()
                        .cached(gpui::StyleRefinement::default().size_full()),
                )
                .into_any_element()
        };
        self.attachment_drop_target("bot-chat", cx)
            .capture_action(cx.listener(|this, _: &input::Paste, _, cx| {
                if this.paste_attachments(cx) {
                    cx.stop_propagation();
                }
            }))
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .when_some(self.menu.as_ref(), |d, menu| {
                d.child(
                    menu_popover(&menu.menu, Anchor::Slot(self.anchor(menu.kind).clone()), cx)
                        .side(Side::Top)
                        .when(menu.kind == ComposerMenu::Account, |p| {
                            p.width(250.).max_height(260.)
                        }),
                )
            })
            .children(self.model_list_popover(cx))
            .child({
                // Conversation owns the transcript/composer boundary. Keep the
                // floating identity inside that viewport, including when a
                // draft or attention control makes the composer grow.
                let surface = crate::bot_header::chat_surface(
                    body,
                    &bot,
                    self.bot_status(),
                    self.bot_ui.header_animation,
                    &mut self.bot_ui.header,
                    cx,
                );
                if let Some(header) = &self.bot_ui.header {
                    header.update(cx, |header, _| {
                        header.activate = self.bot_ui.header_action.clone()
                    });
                }
                surface
            })
            .child(
                composer_column()
                    .children(self.native_history_banner(cx))
                    .when_some(self.bot_ui.draft_error.clone(), |d, error| {
                        d.child(
                            div()
                                .mb_1()
                                .px_3()
                                .py_2()
                                .rounded(px(radius::CONTROL))
                                .bg(content(0.06))
                                .text_size(px(12.))
                                .text_color(content(0.75))
                                .child(error),
                        )
                    })
                    .children(
                        self.bot_ui
                            .pending
                            .iter()
                            // Peer messages and their recovery controls already live
                            // in the transcript. Other automatic updates stay quiet
                            // unless delivery needs the user's attention.
                            .filter(|m| m.internal && m.peer.is_none() && m.error.is_some())
                            .map(|message| {
                                let retry = message.id.clone();
                                let remove = retry.clone();
                                div()
                                    .id(SharedString::from(format!("bot-queued-{}", message.id)))
                                    .debug_selector(|| "bot-background-update-error".into())
                                    .mb_1()
                                    .flex()
                                    .justify_end()
                                    .child(
                                        div()
                                            .max_w(px(440.))
                                            .px_3()
                                            .py_2()
                                            .rounded(px(radius::SURFACE))
                                            .border_1()
                                            .border_color(content(0.15))
                                            .bg(content(0.04))
                                            .text_size(px(13.))
                                            .text_color(content(0.6))
                                            .child("Background update needs attention")
                                            .child(
                                                div()
                                                    .text_size(px(11.))
                                                    .text_color(content(0.4))
                                                    .child(
                                                        message.error.clone().unwrap_or_default(),
                                                    ),
                                            )
                                            .when(message.error.is_some(), |d| {
                                                d.child(
                                                    div()
                                                        .flex()
                                                        .gap_2()
                                                        .child(
                                                            icon_button(
                                                                "bot-queue-retry",
                                                                "Retry queued message",
                                                                "refresh-cw",
                                                            )
                                                            .debug_selector(|| {
                                                                "bot-queue-retry".into()
                                                            })
                                                            .on_click(cx.listener(
                                                                move |_, _, _, cx| {
                                                                    cx.emit(QueueChanged {
                                                                        id: retry.clone(),
                                                                        remove: false,
                                                                    })
                                                                },
                                                            )),
                                                        )
                                                        .child(
                                                            icon_button(
                                                                "bot-queue-discard",
                                                                "Discard queued message",
                                                                "close",
                                                            )
                                                            .debug_selector(|| {
                                                                "bot-queue-discard".into()
                                                            })
                                                            .on_click(cx.listener(
                                                                move |_, _, _, cx| {
                                                                    cx.emit(QueueChanged {
                                                                        id: remove.clone(),
                                                                        remove: true,
                                                                    })
                                                                },
                                                            )),
                                                        ),
                                                )
                                            }),
                                    )
                            }),
                    )
                    .when_some(self.questions.front(), |d, (card, _)| d.child(card.clone()))
                    .children(self.usage_notice(true, cx))
                    .children(self.auth_notice(cx))
                    .child(self.approval_shelf(window, cx))
                    .child(self.composer_box(window, cx))
                    .child(self.composer_tools(cx)),
            )
            .into_any_element()
    }

    /// Whether the model list and a composer menu are open.
    #[cfg(test)]
    pub(crate) fn bot_menus_open(&self) -> (bool, bool) {
        (self.bot_ui.models.is_some(), self.menu.is_some())
    }
    /// `ModelPicker` with `hideSettings`: its trigger opens the model list.
    pub(super) fn toggle_bot_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.bot_ui.models.is_some() {
            self.close_bot_models(window, cx);
            return;
        }
        self.menu = None;
        self.completion
            .update(cx, |completion, cx| completion.dismiss(cx));
        self.git_picker = None;
        self.git_events = None;
        let provider = Kind::from_id(&self.summary.provider);
        let list = cx.new(|cx| {
            let hidden = cx
                .try_global::<vorkspace_ui::preferences::Preferences>()
                .map(|p| p.hidden_providers.clone())
                .unwrap_or_default();
            ModelList::new_for(
                provider,
                self.model.clone(),
                &self.summary.cwd,
                self.settings.account.clone(),
                cx,
            )
            .hide_providers(hidden)
            .allow_provider_switch(true)
        });
        let events = vec![cx.subscribe_in(
            &list,
            window,
            |this, _, picked: &ModelPicked, window, cx| {
                if this.apply_model_pick(picked, cx) {
                    this.close_bot_models(window, cx);
                }
            },
        )];
        window.focus(&list.read(cx).search_focus(cx), cx);
        self.bot_ui.models = Some((list, events));
        cx.notify();
    }
    pub(super) fn model_list_popover(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (list, _) = self.bot_ui.models.as_ref()?;
        let this = cx.entity().downgrade();
        Some(
            popover("composer-models", Anchor::Slot(self.model_anchor.clone()))
                .side(Side::Top)
                .align(Align::End)
                .width(310.)
                .height(442.)
                .on_dismiss(move |window, cx| {
                    let _ = this.update(cx, |this, cx| this.close_bot_models(window, cx));
                })
                .child(list.clone())
                .into_any_element(),
        )
    }
    fn close_bot_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.bot_ui.models = None;
        window.focus(&self.input.focus_handle(cx), cx);
        cx.notify();
    }
    /// ModelControlPills' select menus: effort (with fast and service tier
    /// grouped under it), then each provider control.
    pub(super) fn bot_setting_menu(
        &self,
        ix: usize,
    ) -> (&'static str, Vec<MenuItem>, f32, MenuStyle) {
        let Some(model) = self.model_info() else {
            return ("Model setting", Vec::new(), 210., MenuStyle::Explorer);
        };
        let mut items = Vec::new();
        if ix == 0 {
            let group = |items: &mut Vec<MenuItem>, label: &str, rows: Vec<MenuAction>| {
                if !items.is_empty() {
                    items.push(MenuItem::Separator);
                }
                items.push(MenuItem::Header(label.to_owned().into()));
                items.extend(rows.into_iter().map(MenuItem::from));
            };
            if !model.efforts.is_empty() {
                group(&mut items, self.effort_setting_label(), self.effort_items());
            }
            if model.fast {
                group(
                    &mut items,
                    "Fast",
                    vec![
                        MenuAction::new("fast:on", "On").checked(self.settings.fast),
                        MenuAction::new("fast:off", "Off").checked(!self.settings.fast),
                    ],
                );
            }
            if !model.tiers.is_empty() {
                let mut tiers = vec![
                    MenuAction::new("tier:", "Standard").checked(self.selected_tier().is_none()),
                ];
                tiers.extend(model.tiers.iter().map(|(id, name)| {
                    MenuAction::new(format!("tier:{id}"), name.clone())
                        .checked(self.selected_tier() == Some(id.as_str()))
                }));
                group(&mut items, "Service tier", tiers);
            }
            if items
                .iter()
                .filter(|i| matches!(i, MenuItem::Header(_)))
                .count()
                == 1
            {
                items.retain(|i| !matches!(i, MenuItem::Header(_)));
            }
            return ("Effort", items, 210., MenuStyle::Explorer);
        }
        if let Some(control) = model.controls.get(ix - 1) {
            let selected = self
                .settings
                .model_settings
                .get(&control.id)
                .unwrap_or(&control.current);
            items.extend(control.choices.iter().map(|(value, name)| {
                let key = serde_json::to_string(&(control.id.clone(), value.clone())).unwrap();
                MenuItem::from(
                    MenuAction::new(format!("model-setting:{key}"), name.clone())
                        .checked(selected == value),
                )
            }));
        }
        ("Model setting", items, 210., MenuStyle::Explorer)
    }
    pub(super) fn model_controls(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let model = self.model_info().cloned();
        let mut pills: Vec<AnyElement> = Vec::new();
        if let Some(model) = &model {
            let anchors = 1 + model.controls.len();
            if self.bot_ui.setting_anchors.len() < anchors {
                self.bot_ui
                    .setting_anchors
                    .resize_with(anchors, AnchorSlot::default);
            }
            let grouped = !model.efforts.is_empty() || !model.tiers.is_empty();
            if grouped {
                let open = self.menu_is(ComposerMenu::Setting(0));
                let (glyph, label, value) = if model.efforts.is_empty() {
                    (
                        "zap",
                        "Service tier",
                        self.selected_tier()
                            .and_then(|id| model.tiers.iter().find(|(t, _)| t == id))
                            .map_or("Standard".into(), |(_, name)| name.clone()),
                    )
                } else {
                    (
                        "gauge",
                        self.effort_setting_label(),
                        self.effort_label().to_string(),
                    )
                };
                pills.push(
                    quiet_control("model-effort", open)
                        .debug_selector(|| "model-effort".into())
                        .max_w(px(112.))
                        .aria_label(format!("{label}: {value}"))
                        .aria_expanded(open)
                        .child(self.bot_ui.setting_anchors[0].probe())
                        .child(icon(glyph, 14.).text_color(content(if open { 1. } else { 0.55 })))
                        .child(div().min_w_0().truncate().child(value))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_menu(ComposerMenu::Setting(0), window, cx)
                        }))
                        .into_any_element(),
                );
            }
            if model.fast && !grouped {
                let on = self.settings.fast;
                pills.push(
                    quiet_control("model-fast", false)
                        .aria_label(format!("Fast mode: {}", if on { "On" } else { "Off" }))
                        .child(div().when(!on, |d| d.opacity(0.5)).child("Fast"))
                        .on_click(cx.listener(|this, _, _, cx| this.menu_choice("fast", cx)))
                        .into_any_element(),
                );
            }
            if model.thinking {
                let on = self.settings.thinking.unwrap_or(false);
                pills.push(
                    quiet_control("model-thinking", false)
                        .aria_label(format!("Thinking: {}", if on { "On" } else { "Off" }))
                        .child(div().when(!on, |d| d.opacity(0.5)).child("Thinking"))
                        .on_click(cx.listener(|this, _, _, cx| this.menu_choice("thinking", cx)))
                        .into_any_element(),
                );
            }
            for (ix, control) in model.controls.iter().enumerate() {
                let kind = ComposerMenu::Setting(ix + 1);
                let toggle = model_control_toggle(control);
                let open = !toggle && self.menu_is(kind);
                let selected = self
                    .settings
                    .model_settings
                    .get(&control.id)
                    .unwrap_or(&control.current);
                let value = control
                    .choices
                    .iter()
                    .find(|(value, _)| value == selected)
                    .map_or(selected.clone(), |(_, name)| name.clone());
                let toggle_id = format!("model-toggle:{}", control.id);
                let on = selected == "true";
                let title = if toggle {
                    format!("{}: {}", control.name, if on { "On" } else { "Off" })
                } else {
                    format!("{}: {value}", control.name)
                };
                pills.push(
                    quiet_control(SharedString::from(format!("model-control-{ix}")), open)
                        .max_w(px(112.))
                        .aria_label(title)
                        .when(!toggle, |d| {
                            d.aria_expanded(open)
                                .child(self.bot_ui.setting_anchors[ix + 1].probe())
                        })
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .when(toggle && !on, |d| d.opacity(0.5))
                                .child(if toggle { control.name.clone() } else { value }),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if toggle {
                                this.menu_choice(&toggle_id, cx);
                            } else {
                                this.toggle_menu(kind, window, cx);
                            }
                        }))
                        .into_any_element(),
                );
            }
        }
        pills
    }
}
