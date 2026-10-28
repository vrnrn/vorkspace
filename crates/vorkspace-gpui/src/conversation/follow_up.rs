//! Composer.tsx / MessageQueue.tsx's durable follow-ups and saved user draft.
use super::*;
use vorkspace_core::follow_up::{Message, State};

#[derive(Default)]
pub(super) struct FollowUps {
    pub state: State,
    pub motion: super::composer_shelf::Motion,
    pub persisted: u64,
    pub sending: Option<String>,
    pub requested: Option<String>,
    pub continuations: std::collections::VecDeque<String>,
    pub resuming: bool,
    pub ready: bool,
    pub draft_selected: bool,
    pub edit: Option<QueueEdit>,
}
pub(super) struct QueueEdit {
    id: String,
    input: Entity<input::Input>,
    _submit: Subscription,
}

impl Conversation {
    pub(crate) fn voice_send(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.accept_external_message(text, None, cx)
    }
    pub(crate) fn app_send(&mut self, text: String, cx: &mut Context<Self>) -> Result<(), String> {
        if self.history_partial {
            self.load(cx);
            return Err("Session history is loading; retry the message when ready".into());
        }
        if !self.ready_for_external(cx) {
            return Err(
                "Session is busy, loading, or has an unsent draft; it was preserved".into(),
            );
        }
        self.app_start_message(text, cx)
    }
    /// Only the creator of a fresh session calls this before its initial
    /// composer read. Queue acceptance never takes or replaces composer input.
    pub(crate) fn app_start_message(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.execution_provider()?;
        let text = self.consume_mode_command(text, cx);
        let mode = if self.settings.orchestrator {
            Some("orchestrator")
        } else if self.settings.plan {
            Some("plan")
        } else if self.settings.operator {
            Some("operator")
        } else {
            None
        };
        self.accept_external_message(text, mode.map(str::to_owned), cx)
    }
    fn accept_external_message(
        &mut self,
        text: String,
        mode: Option<String>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.execution_provider()?;
        if self.history_partial {
            self.load(cx);
            return Err("Session history is loading; retry the message when ready".into());
        }
        if !self.history_ready()
            || self.stopping
            || self.account_switching
            || self.attaching
            || self.settings.worktree_removed
        {
            return Err(
                "Session is loading, stopping, or changing accounts; try again when ready".into(),
            );
        }
        if !self.input.read(cx).content.is_empty()
            || !self.attachments.is_empty()
            || self.input.read(cx).is_composing()
        {
            return Err("This session has an unsent draft; it was preserved".into());
        }
        if self.settings.bot.is_some() {
            return Err("Use bots.send to message a Bot".into());
        }
        if self.follow_ups.state.messages.len() >= 128 {
            return Err("The session's message queue is full".into());
        }
        let mut message = self.message(text, Vec::new(), mode, cx);
        message.steer = cx
            .try_global::<crate::settings::Preferences>()
            .is_none_or(|p| p.follow_up == "steer")
            && Kind::from_id(&self.summary.provider).can_steer();
        let id = message.id.clone();
        let mut next = self.follow_ups.state.clone();
        next.revision += 1;
        next.messages.push(message);
        // External callers receive acceptance only after the queue owner has a
        // durable message. Never consume the user's draft or attachments.
        locked(&self.store)?.accept_follow_ups(self.summary.id, &next)?;
        self.follow_ups.state = next;
        self.follow_ups.persisted = self.follow_ups.state.revision;
        if self.follow_ups.state.paused {
            self.follow_ups.requested = Some(id);
        }
        self.pump_follow_ups(cx);
        cx.emit(SessionChanged);
        cx.notify();
        Ok(())
    }
    pub(super) fn draft_active(&self, cx: &gpui::App) -> bool {
        self.follow_ups.draft_selected
            || vorkspace_core::composer::consume_command(&self.input.read(cx).content, "draft")
                .is_some()
    }
    fn message(
        &mut self,
        text: String,
        attachments: Vec<std::path::PathBuf>,
        mode: Option<String>,
        cx: &gpui::App,
    ) -> Message {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let prepared = self.completion.read(cx).prepare_prompt(&text);
        Message {
            prepared_prompt: (prepared != text).then_some(prepared),
            id: format!(
                "composer-{}-{}-{}",
                self.summary.id,
                vorkspace_core::time::now_ms(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ),
            text,
            attachments,
            mode,
            ..Default::default()
        }
    }
    pub(super) fn save_follow_ups(&mut self, cx: &mut Context<Self>) {
        if self.history_partial {
            self.defer_history_action(|this, cx| this.save_follow_ups(cx), cx);
            return;
        }
        self.follow_ups.state.revision += 1;
        let mut snapshot = self.follow_ups.state.clone();
        let revision = snapshot.revision;
        let store = self.store.clone();
        let id = self.summary.id;
        let work = cx.background_executor().spawn(async move {
            let directory = locked(&store)?.data_dir().map(Path::to_path_buf);
            snapshot.retain_attachments(id, directory.as_deref())?;
            locked(&store)?.save_follow_ups(id, &snapshot)?;
            Ok::<_, String>(snapshot)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(snapshot) => {
                        if this.follow_ups.state.revision == revision {
                            this.follow_ups.state = snapshot;
                            let draft = this.follow_ups.state.draft.clone();
                            this.transcript
                                .update(cx, |view, cx| view.set_saved_draft(draft, cx));
                        }
                        this.follow_ups.persisted = this.follow_ups.persisted.max(revision);
                        this.pump_follow_ups(cx);
                    }
                    Err(error) if this.follow_ups.state.revision == revision => {
                        this.follow_ups.state.paused = true;
                        this.status = format!("Could not save messages: {error}");
                        if let Some(head) = this.follow_ups.state.messages.first_mut() {
                            head.error = Some(this.status.clone());
                        }
                        if error.contains("another window") {
                            this.refresh_if_stale(cx);
                        }
                    }
                    Err(_) => {}
                }
                cx.emit(SessionChanged);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn enqueue_follow_up(
        &mut self,
        prompt: String,
        mode: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let attachments = std::mem::take(&mut self.attachments);
        let mut message = self.message(prompt, attachments, mode, cx);
        message.steer = cx
            .try_global::<crate::settings::Preferences>()
            .is_none_or(|prefs| prefs.follow_up == "steer")
            && Kind::from_id(&self.summary.provider).can_steer()
            && message.can_steer();
        self.follow_ups
            .motion
            .insert(&message.id, self.follow_ups.state.messages.is_empty(), true);
        self.follow_ups.state.messages.push(message);
        self.save_follow_ups(cx);
    }
    pub(super) fn save_message_draft(&mut self, cx: &mut Context<Self>) {
        if self.is_remote() {
            self.remote_save_draft(cx);
            return;
        }
        if self.needs_history_restore() {
            self.defer_history_action(|this, cx| this.save_message_draft(cx), cx);
            return;
        }
        if self.follow_ups.state.draft.is_some()
            || !self.loaded
            || self.attaching
            || self.input.read(cx).is_composing()
        {
            return;
        }
        let raw = self.input.read(cx).content.to_string();
        let text = vorkspace_core::composer::consume_command(&raw, "draft")
            .unwrap_or(&raw)
            .to_owned();
        if text.trim().is_empty() && self.attachments.is_empty() {
            return;
        }
        let mut message = self.message(text, self.attachments.clone(), None, cx);
        message.at = self.transcript.read(cx).source_rows().len();
        // Bot drafts are accepted before clearing the composer. An attachment
        // copy or storage failure must leave the user's input untouched.
        if self.settings.bot.is_some() {
            let origin_revision = match self.bot_draft_origin_revision(cx) {
                Ok(revision) => revision,
                Err(error) => {
                    self.status =
                        format!("Could not save draft: {error}. Your composer was preserved.");
                    self.bot_ui.draft_error = Some(self.status.clone());
                    cx.notify();
                    return;
                }
            };
            let mut next = self.follow_ups.state.clone();
            next.revision += 1;
            next.draft = Some(message.clone());
            let result = (|| {
                let store = locked(&self.store)?;
                next.retain_attachments(self.summary.id, store.data_dir())?;
                store.accept_bot_draft(self.summary.id, &next, origin_revision, &message.id)
            })();
            if let Err(error) = result {
                // A shared-store root publication can fail after its database
                // commit. Adopt any matching durable card before permitting a
                // retry, while retaining the original composer as recovery.
                if let Ok(saved) =
                    locked(&self.store).and_then(|store| store.follow_ups(self.summary.id))
                    && saved
                        .draft
                        .as_ref()
                        .is_some_and(|draft| draft.id == message.id)
                {
                    self.follow_ups.persisted = saved.revision;
                    self.follow_ups.state = saved;
                    self.summary.has_messages = true;
                    let draft = self.follow_ups.state.draft.clone();
                    self.transcript
                        .update(cx, |view, cx| view.set_saved_draft(draft, cx));
                }
                self.status =
                    format!("Could not save draft: {error}. Your composer was preserved.");
                self.bot_ui.draft_error = Some(self.status.clone());
                cx.notify();
                return;
            }
            self.bot_ui.draft_error = None;
            message = next.draft.clone().unwrap();
            self.follow_ups.persisted = next.revision;
            self.follow_ups.state = next;
        }
        self.input.update(cx, |i, cx| i.set_text(String::new(), cx));
        self.attachments.clear();
        if self.settings.bot.is_none() && self.summary.title.starts_with("New ") {
            self.summary.title = message
                .label()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(42)
                .collect();
            let (store, id, title) = (
                self.store.clone(),
                self.summary.id,
                self.summary.title.clone(),
            );
            cx.background_executor()
                .spawn(async move {
                    let _ = locked(&store).and_then(|s| s.title(id, &title));
                })
                .detach();
        }
        if !self.summary.has_messages {
            self.composer_motion.capture(self.composer_anchor.get());
        }
        self.summary.has_messages = true;
        self.follow_ups.draft_selected = false;
        self.settings.plan = false;
        self.settings.operator = false;
        self.settings.orchestrator = false;
        self.save_settings(cx);
        self.follow_ups.state.draft = Some(message.clone());
        self.transcript
            .update(cx, |view, cx| view.introduce_saved_draft(message, cx));
        self.completion
            .update(cx, |completion, cx| completion.dismiss(cx));
        if self.settings.bot.is_none() {
            self.save_follow_ups(cx);
        }
        cx.emit(SessionChanged);
    }
    pub(super) fn draft_action(&mut self, send: bool, cx: &mut Context<Self>) {
        if self.is_remote() {
            self.remote_draft_action(send, cx);
            return;
        }
        if self.needs_history_restore() {
            self.defer_history_action(move |this, cx| this.draft_action(send, cx), cx);
            return;
        }
        if send && (!self.loaded || self.preparing_worktree) {
            return;
        }
        if self.settings.bot.is_some() {
            if self.bot_ui.draft_sending.is_some() && (!send || !self.bot_ui.draft_retry) {
                return;
            }
            if send {
                let Some(message) = self.follow_ups.state.draft.as_ref().cloned() else {
                    return;
                };
                if self.execution_provider().is_err()
                    || self.follow_ups.persisted < self.follow_ups.state.revision
                {
                    self.status = "Draft is not ready to send; it was preserved.".into();
                    cx.notify();
                    return;
                }
                if self.bot_ui.draft_sending.is_none() {
                    self.bot_ui.draft_queue = std::mem::take(&mut self.bot_ui.queue);
                }
                self.bot_ui.draft_sending = Some(message.id.clone());
                self.bot_ui.draft_retry = false;
                self.transcript
                    .update(cx, |view, cx| view.set_draft_transfer(Some(false), cx));
                cx.emit(bot::Submitted {
                    draft_id: Some(message.id),
                    prepared_prompt: Some(
                        message
                            .prepared_prompt
                            .unwrap_or_else(|| message.text.clone()),
                    ),
                    text: message.text,
                    attachments: message.attachments,
                    queue: self.bot_ui.draft_queue,
                    mode: message.mode,
                });
                cx.notify();
                return;
            }
            let Some(draft) = self.follow_ups.state.draft.as_ref() else {
                return;
            };
            let mut next = self.follow_ups.state.clone();
            next.draft = None;
            next.revision += 1;
            let result = self
                .mark_bot_draft_discarded(&draft.id)
                .and_then(|()| locked(&self.store)?.accept_follow_ups(self.summary.id, &next));
            if let Err(error) = result {
                self.bot_ui.draft_error = Some(format!(
                    "Could not remove draft: {error}. Your draft was preserved."
                ));
                cx.notify();
                return;
            }
            self.follow_ups.persisted = next.revision;
            self.follow_ups.state = next;
            self.summary.has_messages = !self.transcript.read(cx).source_rows().is_empty();
            self.transcript
                .update(cx, |view, cx| view.set_saved_draft(None, cx));
            self.bot_ui.draft_error = None;
            cx.emit(SessionChanged);
            cx.notify();
            return;
        }
        let Some(mut message) = self.follow_ups.state.draft.take() else {
            return;
        };
        if send {
            message.steer = cx
                .try_global::<crate::settings::Preferences>()
                .is_none_or(|prefs| prefs.follow_up == "steer")
                && Kind::from_id(&self.summary.provider).can_steer();
            if !self.busy {
                self.follow_ups.requested = Some(message.id.clone());
            }
            self.follow_ups.motion.insert(
                &message.id,
                self.follow_ups.state.messages.is_empty(),
                true,
            );
            self.follow_ups.state.messages.push(message);
        }
        self.summary.has_messages = !self.transcript.read(cx).source_rows().is_empty() || send;
        self.transcript
            .update(cx, |view, cx| view.set_saved_draft(None, cx));
        self.save_follow_ups(cx);
        cx.emit(SessionChanged);
    }
    pub(super) fn edit_saved_draft(
        &mut self,
        id: &str,
        edit_id: &str,
        text: &str,
        finish: bool,
        cancel: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.transcript.read(cx).is_saved_draft_edit(id, edit_id) {
            return;
        }
        if !cancel && !self.transcript.read(cx).saved_draft_edit_matches(text, cx) {
            return;
        }
        let original = if cancel {
            let Some((original, prepared)) = self
                .transcript
                .read(cx)
                .saved_draft_edit_original(id, edit_id)
            else {
                return;
            };
            if original != text {
                return;
            }
            Some(prepared)
        } else {
            None
        };
        if self.bot_ui.draft_sending.is_some()
            || self.follow_ups.persisted < self.follow_ups.state.revision
        {
            self.status = "Wait for the draft to finish saving before editing.".into();
            cx.notify();
            return;
        }
        let mut next = self.follow_ups.state.clone();
        let Some(draft) = next.draft.as_mut().filter(|draft| draft.id == id) else {
            return;
        };
        if text.trim().is_empty() && draft.attachments.is_empty() {
            return;
        }
        if draft.text == text
            && original
                .as_ref()
                .is_none_or(|prepared| draft.prepared_prompt.as_ref() == prepared.as_ref())
        {
            if finish {
                let draft = next.draft.clone();
                self.transcript
                    .update(cx, |view, cx| view.finish_saved_draft_edit(draft, cx));
            }
            return;
        }
        if let Some(prepared) = original {
            draft.text = text.to_owned();
            draft.prepared_prompt = prepared;
        } else {
            let prepared = self.completion.read(cx).prepare_prompt(text);
            draft.replace_text(text.to_owned(), prepared);
        }
        // An edited version must never share a delivery receipt with a
        // previous version that another window has already queued.
        if draft.id != edit_id {
            draft.replaces_draft_id = Some(draft.id.clone());
            draft.id = edit_id.to_owned();
        }
        next.revision += 1;
        let result = (|| {
            if self.settings.bot.is_some() && id != edit_id {
                self.mark_bot_draft_edit_origin(id, edit_id)?;
            }
            let store = locked(&self.store)?;
            next.retain_attachments(self.summary.id, store.data_dir())?;
            store.accept_follow_ups(self.summary.id, &next)
        })();
        match result {
            Ok(()) => {
                let _ = locked(&self.store).and_then(|store| {
                    store.clear_composer_recovery(
                        self.summary.id,
                        &format!("saved-draft-edit:{edit_id}"),
                    )
                });
                self.bot_ui.draft_error = None;
                self.follow_ups.persisted = next.revision;
                self.follow_ups.state = next;
                let draft = self.follow_ups.state.draft.clone();
                self.transcript.update(cx, |view, cx| {
                    if finish {
                        view.finish_saved_draft_edit(draft, cx);
                    } else {
                        view.update_saved_draft_edit(draft);
                    }
                });
                cx.emit(SessionChanged);
            }
            Err(error) => {
                let end = text.encode_utf16().count();
                let files = next
                    .draft
                    .as_ref()
                    .map(|draft| {
                        draft
                            .attachments
                            .iter()
                            .map(|path| serde_json::json!({"path":path}))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let recovery = vorkspace_core::store::ComposerRecovery {
                    owner: format!("saved-draft-edit:{edit_id}"),
                    revision: next.revision,
                    payload: serde_json::json!({"cwd":self.summary.cwd, "text":text, "selectionStart":end, "selectionEnd":end, "attachments":files, "mcpTags":[]}),
                };
                let recovery_result = locked(&self.store)
                    .and_then(|store| store.save_composer_recovery(self.summary.id, recovery));
                self.status =
                    format!("Could not save draft edit: {error}. Your edit is still open.");
                if let Err(recovery_error) = recovery_result {
                    self.status = format!(
                        "{}. Could not save a recovery copy: {recovery_error}",
                        self.status
                    );
                }
                if self.settings.bot.is_some() {
                    self.bot_ui.draft_error = Some(self.status.clone());
                }
            }
        }
        cx.notify();
    }
    pub(super) fn follow_up_delivered(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.follow_ups.sending.as_deref() == Some(id) {
            self.follow_ups.sending = None;
            if self.settings.bot.is_some() {
                // The Bot outbox retires its head against this receipt.
                self.bot_ui.delivered = Some(id.to_owned());
                self.sync_bot_pending(cx);
            }
        }
        if self.follow_ups.requested.as_deref() == Some(id) {
            self.follow_ups.requested = None;
        }
        self.follow_ups
            .continuations
            .retain(|message| message != id);
        if self.follow_ups.requested.is_none() {
            self.follow_ups.requested = self.follow_ups.continuations.front().cloned();
        }
        let before = self.follow_ups.state.messages.len();
        self.follow_ups
            .state
            .messages
            .retain(|message| message.id != id);
        if before != self.follow_ups.state.messages.len() {
            self.save_follow_ups(cx);
        }
    }
    pub(super) fn fail_follow_up(&mut self, id: &str, error: String, cx: &mut Context<Self>) {
        if self.settings.bot.is_some() {
            if self.follow_ups.sending.as_deref() == Some(id) {
                self.follow_ups.sending = None;
                cx.emit(super::bot::SteerFailed {
                    id: id.to_owned(),
                    error,
                });
            }
            return;
        }
        if let Some(message) = self
            .follow_ups
            .state
            .messages
            .iter_mut()
            .find(|m| m.id == id)
        {
            message.error = Some(error);
            self.follow_ups.state.paused = true;
            if self
                .follow_ups
                .continuations
                .iter()
                .any(|message| message == id)
            {
                self.follow_ups.continuations.clear();
            }
            if self.follow_ups.sending.as_deref() == Some(id) {
                self.follow_ups.sending = None;
            }
            if self.follow_ups.requested.as_deref() == Some(id) {
                self.follow_ups.requested = None;
            }
            self.save_follow_ups(cx);
        }
    }
    pub(super) fn pump_follow_ups(&mut self, cx: &mut Context<Self>) {
        if self.execution_provider().is_err() || self.editing_last_turn.pending() || self.stopping {
            return;
        }
        if !self.history_ready() {
            // Persistence callbacks can arrive while a hidden history reloads.
            // The load completion pumps the durable queue after restoring rows.
            return;
        }
        if self.settings.usage_limit.is_some()
            || self.settings.worktree_removed
            || self.account_switching
            || self.settings.bot.is_some()
            || !self.loaded
            || self.preparing_worktree
            || (self.follow_ups.state.paused && self.follow_ups.requested.is_none())
            || self.follow_ups.sending.is_some()
            || self.follow_ups.resuming
            || self.follow_ups.persisted < self.follow_ups.state.revision
        {
            return;
        }
        let message = self
            .follow_ups
            .requested
            .as_ref()
            .and_then(|id| self.follow_ups.state.messages.iter().find(|m| &m.id == id))
            .or_else(|| self.follow_ups.state.messages.first());
        let Some(message) = message.cloned() else {
            return;
        };
        if self
            .follow_ups
            .edit
            .as_ref()
            .is_some_and(|edit| edit.id == message.id)
        {
            return;
        }
        if self.busy {
            let requested = self.follow_ups.requested.as_deref() == Some(message.id.as_str());
            if self.settings.pending_provider_switch.is_some() {
                if requested {
                    self.stop(cx);
                    self.follow_ups.requested = Some(message.id.clone());
                    self.follow_ups.continuations.push_back(message.id);
                }
                return;
            }
            if !message.steer
                || !Kind::from_id(&self.summary.provider).can_steer()
                || !message.can_steer()
                || !self.follow_ups.ready
                || (!requested
                    && self
                        .questions
                        .iter()
                        .any(|(card, _)| card.read(cx).blocks_steering()))
            {
                return;
            }
            if let Some(controls) = &self.controls {
                if controls
                    .send(Control {
                        id: message.id.clone(),
                        decision: Decision::Steer(message.clone()),
                    })
                    .is_ok()
                {
                    self.follow_ups.sending = Some(message.id);
                } else {
                    self.fail_follow_up(
                        &message.id,
                        "The provider disconnected before accepting this message".into(),
                        cx,
                    );
                }
            }
        } else {
            if self.settings.new_worktree && self.settings.worktree_cwd.is_none() {
                self.prepare_worktree(cx);
                return;
            }
            self.follow_ups.sending = Some(message.id.clone());
            let message_id = message.id.clone();
            // A queued message carries its own one-shot intent; the user's next
            // composer selection must not change a message already in the queue.
            let previous = (
                self.settings.plan,
                self.settings.operator,
                self.settings.orchestrator,
            );
            self.settings.plan = message.mode.as_deref() == Some("plan");
            self.settings.operator = message.mode.as_deref() == Some("operator");
            self.settings.orchestrator = message.mode.as_deref() == Some("orchestrator");
            self.planning_orchestration = self.settings.orchestrator;
            self.orchestration_plan = None;
            self.orchestration_error = None;
            let composer_attachments = std::mem::take(&mut self.attachments);
            self.settings.attachments = message.attachments;
            self.settings.bot_message_id = Some(message.id);
            let previous_prepared = self.settings.prepared_prompt.take();
            self.settings.prepared_prompt = message.prepared_prompt;
            self.launch_prompt(message.text, cx);
            if !self.busy {
                self.fail_follow_up(&message_id, self.status.clone(), cx);
            }
            self.settings.prepared_prompt = previous_prepared;
            self.attachments = composer_attachments;
            self.settings.attachments.clear();
            self.settings.bot_message_id = None;
            (
                self.settings.plan,
                self.settings.operator,
                self.settings.orchestrator,
            ) = previous;
        }
        cx.notify();
    }
    fn resume_queue(&mut self, cx: &mut Context<Self>) {
        if self.stopping {
            self.defer_history_action(|this, cx| this.resume_queue(cx), cx);
            return;
        }
        if self.busy || !self.follow_ups.state.paused || self.follow_ups.state.messages.is_empty() {
            return;
        }
        // Resume the interrupted provider turn before releasing its outbox,
        // matching App.tsx's queueStatus = "resuming" continuation.
        self.follow_ups.resuming = true;
        self.launch_prompt("Continue from where you left off.".into(), cx);
        if !self.busy {
            self.follow_ups.resuming = false;
        }
        cx.notify();
    }
    fn edit_queued(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.follow_ups.sending.as_ref() == Some(&id) {
            return;
        }
        let Some(message) = self
            .follow_ups
            .state
            .messages
            .iter()
            .find(|message| message.id == id)
        else {
            return;
        };
        let text = message.text.clone();
        let input = cx.new(|cx| {
            let mut input = input::Input::queued_message(cx);
            input.set_text(text, cx);
            input
        });
        let submit = cx.subscribe_in(
            &input,
            window,
            |this, _, _: &input::Submitted, window, cx| this.finish_queue_edit(true, window, cx),
        );
        window.focus(&input.focus_handle(cx), cx);
        self.follow_ups.edit = Some(QueueEdit {
            id,
            input,
            _submit: submit,
        });
        cx.notify();
    }
    fn finish_queue_edit(&mut self, save: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = &self.follow_ups.edit else {
            return;
        };
        if edit.input.read(cx).is_composing() {
            return;
        }
        if save {
            let text = edit.input.read(cx).content.to_string();
            let prepared = self.completion.read(cx).prepare_prompt(&text);
            if let Some(message) = self
                .follow_ups
                .state
                .messages
                .iter_mut()
                .find(|m| m.id == edit.id)
            {
                if text.trim().is_empty() && message.attachments.is_empty() {
                    return;
                }
                message.replace_text(text, prepared);
                message.error = None;
            }
        }
        self.follow_ups.edit = None;
        window.focus(&self.input.focus_handle(cx), cx);
        if save {
            self.save_follow_ups(cx);
        } else {
            self.pump_follow_ups(cx);
        }
        cx.notify();
    }
    pub(super) fn message_queue(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let width = self.composer_anchor.get().map(|bounds| bounds.size.width);
        self.follow_ups.motion.retain(
            self.follow_ups
                .state
                .messages
                .iter()
                .map(|message| message.id.as_str()),
            width.is_some(),
        );
        if self.follow_ups.state.messages.is_empty() {
            return div().into_any_element();
        }
        let card_width = (width.unwrap_or_default() - px(16.)).max(px(0.));
        let row_width = (card_width - px(22.)).max(px(0.));
        let failed = self
            .follow_ups
            .state
            .messages
            .iter()
            .any(|m| m.error.is_some());
        let card = composer_shelf::card("message-queue-card", self.classic_layout)
            .when(self.follow_ups.state.paused && !self.follow_ups.resuming, |d| d.child(
                div().h(px(28.)).flex().items_center().gap_2().border_b_1().border_color(content(0.1))
                    .child(icon("queue-pause", 14.))
                    .child(div().flex_1().min_w_0().truncate().child(if failed { "A message couldn't be sent" } else { "Queue paused because you interrupted" }))
                    .child(queue_button("queue-resume", if failed { "Retry queue" } else { "Resume queue" }, "queue-play").w_auto().px(px(6.)).gap(px(6.)).child(if failed { "Retry" } else { "Resume" }).on_click(cx.listener(|this, _, _, cx| this.resume_queue(cx))))
            ))
            .children(self.follow_ups.state.messages.iter().enumerate().map(|(index, message)| {
                let id = message.id.clone();
                let editing = self.follow_ups.edit.as_ref().filter(|edit| edit.id == id);
                let sending = self.follow_ups.sending.as_ref() == Some(&id);
                div().id(SharedString::from(format!("queued-{id}"))).role(Role::Group).aria_label(format!("Queued message: {}", message.label())).min_h(px(32.)).flex().items_center().gap(px(10.)).when(index > 0, |d| d.border_t_1().border_color(content(0.05)))
                    .child(icon("queue-list", 14.).text_color(content(0.35)))
                    .map(|row| if let Some(edit) = editing {
                        row.child(div().flex_1().min_w_0().px(px(6.)).py(px(2.)).rounded_md().border_1().border_color(content(0.3)).bg(content(0.05)).on_action(cx.listener(|this, _: &crate::shell::DismissMenu, window, cx| { this.finish_queue_edit(false, window, cx); cx.stop_propagation(); })).child(edit.input.clone()))
                            .child(queue_button("queue-save", "Save queued message", "check").on_click(cx.listener(|this, _, window, cx| this.finish_queue_edit(true, window, cx))))
                            .child(queue_button("queue-cancel", "Cancel queued message edit", "close").on_click(cx.listener(|this, _, window, cx| this.finish_queue_edit(false, window, cx))))
                    } else {
                        let (steer, edit, remove) = (id.clone(), id.clone(), id);
                        row.child(div().flex_1().min_w_0().py_1().child(div().id("queued-message-text").role(Role::Label).aria_label(message.label()).truncate().text_color(content(0.75)).child(message.label())).when_some(message.error.clone(), |d, error| d.child(div().id("queue-error").role(Role::Alert).text_color(gpui::rgb(0xf87171)).child(error))))
                            .when(Kind::from_id(&self.summary.provider).can_steer(), |d| d.child(queue_button("queue-steer", "Steer queued message", "queue-steer").w_auto().px_2().gap(px(6.)).tab_stop(!sending).opacity(if sending { 0.4 } else { 1. }).child("Steer").on_click(cx.listener(move |this, _, _, cx| {
                                if !sending && let Some(message) = this.follow_ups.state.messages.iter_mut().find(|m| m.id == steer) {
                                    if !message.can_steer() && this.busy { return; }
                                    message.steer = true;
                                    message.error = None;
                                    this.follow_ups.requested = Some(steer.clone());
                                    this.save_follow_ups(cx);
                                }
                            }))))
                            .child(queue_button("queue-edit", "Edit queued message", "pencil-edit").tab_stop(!sending).opacity(if sending { 0.4 } else { 1. }).on_click(cx.listener(move |this, _, window, cx| { if !sending { this.edit_queued(edit.clone(), window, cx); } })))
                            .child(queue_button("queue-remove", "Remove queued message", "trash").tab_stop(!sending).opacity(if sending { 0.4 } else { 1. }).on_click(cx.listener(move |this, _, _, cx| {
                                if !sending {
                                    this.follow_ups.state.messages.retain(|m| m.id != remove);
                                    this.follow_ups.continuations.retain(|id| id != &remove);
                                    if this.follow_ups.requested.as_ref() == Some(&remove) { this.follow_ups.requested = this.follow_ups.continuations.front().cloned(); }
                                    this.save_follow_ups(cx);
                                }
                            })))
                    })
                    .map(|row| self.follow_ups.motion.row(&message.id, row, row_width, window))
            }));
        composer_shelf::shelf("message-queue")
            .child(self.follow_ups.motion.card(card, card_width, window))
            .into_any_element()
    }
}

fn queue_button(
    id: &'static str,
    label: &'static str,
    glyph: &'static str,
) -> gpui::Stateful<gpui::Div> {
    crate::controls::icon_button(id, label, true)
        .text_color(content(0.5))
        .hover(|s| s.bg(content(0.08)).text_color(content(0.9)))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(icon(glyph, 14.))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext, WindowHandle};
    use std::sync::Mutex;
    use vorkspace_core::store::Store;

    fn conversation(cx: &mut TestAppContext) -> WindowHandle<Conversation> {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let summary = store.create("codex", "/tmp").unwrap();
        cx.update(|cx| {
            input::bind(cx);
            cx.set_global(crate::settings::Preferences {
                follow_up: "queue".into(),
                ..Default::default()
            });
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| {
                    let mut view = Conversation::new(summary, Arc::new(Mutex::new(store)), cx);
                    view.loaded = true;
                    view
                })
            })
            .unwrap()
        })
    }

    #[gpui::test]
    fn queue_entrances_slide_above_a_stationary_composer_for_the_first_and_later_messages(
        cx: &mut TestAppContext,
    ) {
        use std::time::Duration;
        let window = conversation(cx);
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                view.summary.has_messages = true;
                view.busy = true;
                view.follow_ups.state.paused = true;
                cx.notify();
            })
            .unwrap();
        cx.refresh().unwrap();
        let bounds = |window: WindowHandle<Conversation>, cx: &mut TestAppContext| {
            window
                .update(cx, |view, window, _| {
                    let composer = window
                        .painted_quads()
                        .into_iter()
                        .find(|quad| {
                            (quad.corner_radii.top_left.0 / window.scale_factor() - 8.).abs() < 0.1
                                && quad.bounds.size.width.0 / window.scale_factor() > 300.
                        })
                        .expect("the composer surface must be painted");
                    (
                        f32::from(view.composer_anchor.get().unwrap().top()),
                        composer.bounds.top().0 / window.scale_factor(),
                    )
                })
                .unwrap()
        };
        let (mut queue_top, input_top) = bounds(window, cx);
        for (index, text) in ["First queued message", "A second queued message"]
            .into_iter()
            .enumerate()
        {
            window
                .update(cx, |view, _, cx| {
                    view.enqueue_follow_up(text.into(), None, cx);
                    view.follow_ups.motion.elapsed(Duration::ZERO);
                    cx.notify();
                })
                .unwrap();
            cx.refresh().unwrap();
            let (starting, composer) = bounds(window, cx);
            assert!(
                (composer - input_top).abs() < 1.,
                "the input must stay still"
            );
            assert!(
                (starting - queue_top).abs() < 8.,
                "the entrance starts behind the input"
            );

            window
                .update(cx, |view, _, cx| {
                    view.follow_ups.motion.elapsed(Duration::from_millis(40));
                    cx.notify();
                })
                .unwrap();
            cx.refresh().unwrap();
            let (middle, composer) = bounds(window, cx);
            assert!((composer - input_top).abs() < 1.);
            assert!(middle < starting - 5., "the queue must slide upward");

            window
                .update(cx, |view, _, cx| {
                    view.follow_ups.motion.elapsed(Duration::from_secs(1));
                    cx.notify();
                })
                .unwrap();
            cx.refresh().unwrap();
            let (settled, composer) = bounds(window, cx);
            assert!((composer - input_top).abs() < 1.);
            assert!(
                settled < middle - 5.,
                "the spring must settle into the full row height"
            );
            assert!(queue_top - settled >= if index == 0 { 60. } else { 30. });
            queue_top = settled;
        }
    }

    #[gpui::test]
    fn submitted_modes_clear_after_queueing_and_do_not_leak_into_the_next_message(
        cx: &mut TestAppContext,
    ) {
        for mode in ["operator", "orchestrator", "plan"] {
            let window = conversation(cx);
            cx.run_until_parked();
            window
                .update(cx, |view, _, cx| {
                    view.busy = true;
                    view.follow_ups.state.paused = true;
                    view.toggle_mode(mode, cx);
                    let selected = view.input.read(cx).content.clone();
                    view.send(cx);
                    assert_eq!(
                        view.input.read(cx).content,
                        selected,
                        "an empty submission keeps its mode"
                    );
                    view.input
                        .update(cx, |i, cx| i.set_text(format!("/{mode} first message"), cx));
                    view.send(cx);
                    assert!(view.input.read(cx).content.is_empty());
                    assert!(
                        !view.settings.plan
                            && !view.settings.operator
                            && !view.settings.orchestrator
                    );
                    view.input
                        .update(cx, |i, cx| i.set_text("ordinary follow-up".into(), cx));
                    view.send(cx);
                    let messages = &view.follow_ups.state.messages;
                    assert_eq!(messages[0].mode.as_deref(), Some(mode));
                    assert_eq!(messages[0].text, "first message");
                    assert_eq!(messages[1].mode, None);
                    assert_eq!(messages[1].text, "ordinary follow-up");
                })
                .unwrap();
            cx.run_until_parked();
            window
                .update(cx, |view, _, _| {
                    let store = locked(&view.store).unwrap();
                    let saved = store.agent_settings(view.summary.id).unwrap().unwrap();
                    assert!(!saved.plan && !saved.operator && !saved.orchestrator);
                    let queue = store.follow_ups(view.summary.id).unwrap();
                    assert_eq!(queue.messages[0].mode.as_deref(), Some(mode));
                    assert_eq!(queue.messages[1].mode, None);
                })
                .unwrap();
        }
    }

    #[gpui::test]
    fn voice_send_is_durable_before_acceptance_and_never_consumes_drafts(cx: &mut TestAppContext) {
        let window = conversation(cx);
        cx.run_until_parked();
        cx.update(|cx| {
            let prefs = crate::settings::Preferences {
                follow_up: "queue".into(),
                ..Default::default()
            };
            cx.set_global(prefs);
        });
        window
            .update(cx, |view, _, cx| {
                view.busy = true;
                view.input
                    .update(cx, |i, cx| i.set_text("typed draft".into(), cx));
                assert!(view.voice_send("voice message".into(), cx).is_err());
                assert_eq!(view.input.read(cx).content, "typed draft");
                view.input.update(cx, |i, cx| i.set_text(String::new(), cx));
                view.attachments.push("/tmp/unsent-image.png".into());
                assert!(view.voice_send("voice message".into(), cx).is_err());
                assert_eq!(view.attachments.len(), 1);
                view.attachments.clear();
                view.voice_send("voice message".into(), cx).unwrap();
                let saved = locked(&view.store)
                    .unwrap()
                    .follow_ups(view.summary.id)
                    .unwrap();
                assert_eq!(saved.messages.len(), 1);
                assert_eq!(saved.messages[0].text, "voice message");
                assert!(view.input.read(cx).content.is_empty());
            })
            .unwrap();
    }
    #[gpui::test]
    fn busy_sends_keep_their_order_attachments_and_edits_until_accepted(cx: &mut TestAppContext) {
        let window = conversation(cx);
        let (controls, received) = std::sync::mpsc::channel();
        window
            .update(cx, |view, _, cx| {
                view.busy = true;
                view.controls = Some(controls);
                view.input
                    .update(cx, |i, cx| i.set_text("first".into(), cx));
                view.attachments.push("/tmp/image.png".into());
                view.send(cx);
                view.input
                    .update(cx, |i, cx| i.set_text("second".into(), cx));
                view.send(cx);
                assert!(view.input.read(cx).content.is_empty());
                assert_eq!(
                    view.follow_ups
                        .state
                        .messages
                        .iter()
                        .map(|m| m.text.as_str())
                        .collect::<Vec<_>>(),
                    ["first", "second"]
                );
            })
            .unwrap();
        cx.run_until_parked();
        assert!(received.try_recv().is_err());
        window
            .update(cx, |view, w, cx| {
                let id = view.follow_ups.state.messages[0].id.clone();
                view.edit_queued(id, w, cx);
                view.follow_ups
                    .edit
                    .as_ref()
                    .unwrap()
                    .input
                    .update(cx, |i, cx| i.set_text("edited 🙂\nwith detail".into(), cx));
                view.follow_ups.ready = true;
                view.follow_ups.state.messages[0].steer = true;
                view.pump_follow_ups(cx);
                assert!(
                    received.try_recv().is_err(),
                    "editing the head must block delivery"
                );
                view.finish_queue_edit(true, w, cx);
            })
            .unwrap();
        cx.run_until_parked();
        let control = received.try_recv().unwrap();
        let Decision::Steer(message) = control.decision else {
            panic!("expected steering");
        };
        assert_eq!(message.text, "edited 🙂\nwith detail");
        assert_eq!(
            message.attachments,
            [std::path::PathBuf::from("/tmp/image.png")]
        );
        window
            .update(cx, |view, _, cx| {
                assert_eq!(
                    view.follow_ups.state.messages.len(),
                    2,
                    "a write to the control channel is not an acceptance"
                );
                view.follow_up_delivered(&message.id, cx);
                assert_eq!(view.follow_ups.state.messages[0].text, "second");
                assert!(view.follow_ups.sending.is_none());
            })
            .unwrap();
    }

    #[gpui::test]
    fn stop_pauses_the_queue_and_retires_its_controls(cx: &mut TestAppContext) {
        let window = conversation(cx);
        let (controls, received) = std::sync::mpsc::channel();
        window
            .update(cx, |view, _, cx| {
                view.busy = true;
                view.controls = Some(controls);
                view.input
                    .update(cx, |input, cx| input.set_text("keep this".into(), cx));
                view.send(cx);
                view.stop(cx);
                assert!(view.stopping);
                assert!(view.follow_ups.state.paused);
                assert!(!view.follow_ups.ready);
                assert!(view.controls.is_none());
                assert_eq!(view.follow_ups.state.messages[0].text, "keep this");
            })
            .unwrap();
        cx.run_until_parked();
        assert!(matches!(
            received.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ));
    }

    #[gpui::test]
    fn a_rejected_steer_keeps_its_text_for_retry(cx: &mut TestAppContext) {
        let window = conversation(cx);
        cx.update(|cx| {
            cx.update_global::<crate::settings::Preferences, _>(|prefs, _| {
                prefs.follow_up = "steer".into()
            });
        });
        let (controls, received) = std::sync::mpsc::channel();
        window
            .update(cx, |view, _, cx| {
                view.busy = true;
                view.controls = Some(controls);
                view.follow_ups.ready = true;
                view.input
                    .update(cx, |input, cx| input.set_text("keep this".into(), cx));
                view.send(cx);
            })
            .unwrap();
        cx.run_until_parked();
        let id = received.try_recv().unwrap().id;
        window
            .update(cx, |view, _, cx| {
                view.fail_follow_up(&id, "Rejected by provider".into(), cx);
                assert!(view.follow_ups.state.paused);
                assert_eq!(view.follow_ups.state.messages[0].text, "keep this");
                assert_eq!(
                    view.follow_ups.state.messages[0].error.as_deref(),
                    Some("Rejected by provider")
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn working_sessions_and_bots_can_save_a_draft_without_sending_it(cx: &mut TestAppContext) {
        for bot in [false, true] {
            let window = conversation(cx);
            let (controls, received) = std::sync::mpsc::channel();
            window
                .update(cx, |view, _, cx| {
                    if bot {
                        view.configure_bot(
                            vorkspace_core::bot::Bot::default(),
                            "/tmp".into(),
                            Default::default(),
                            cx,
                        );
                    }
                    view.busy = true;
                    view.summary.has_messages = true;
                    view.follow_ups.ready = true;
                    view.controls = Some(controls);
                    cx.notify();
                })
                .unwrap();
            let visual = &mut VisualTestContext::from_window(window.into(), cx);
            visual.run_until_parked();
            let add = visual.debug_bounds("composer-add").unwrap().center();
            visual.simulate_click(add, Default::default());
            visual.run_until_parked();
            let draft = visual.debug_bounds("menu-action-draft").unwrap().center();
            visual.simulate_click(draft, Default::default());
            visual.run_until_parked();
            window
                .update(visual, |view, _, cx| {
                    assert!(
                        view.follow_ups.draft_selected,
                        "Draft is selectable while working"
                    );
                    view.input.update(cx, |input, cx| {
                        input.set_text("/draft Save this for later".into(), cx)
                    });
                    cx.notify();
                })
                .unwrap();
            visual.run_until_parked();
            let save = visual.debug_bounds("send-or-stop").unwrap().center();
            visual.simulate_click(save, Default::default());
            visual.run_until_parked();
            window
                .update(visual, |view, _, cx| {
                    assert!(view.busy, "saving must not stop the running agent");
                    assert_eq!(
                        view.follow_ups.state.draft.as_ref().unwrap().text,
                        "Save this for later"
                    );
                    assert!(view.follow_ups.state.messages.is_empty());
                    assert!(view.input.read(cx).content.is_empty());
                })
                .unwrap();
            assert!(
                received.try_recv().is_err(),
                "saving must not send or steer"
            );
            window
                .update(visual, |view, _, _| {
                    assert_eq!(
                        locked(&view.store)
                            .unwrap()
                            .follow_ups(view.summary.id)
                            .unwrap()
                            .draft
                            .unwrap()
                            .text,
                        "Save this for later"
                    );
                })
                .unwrap();
        }
    }

    #[gpui::test]
    fn draft_commands_save_without_starting_an_agent_and_remove_restores_a_blank(
        cx: &mut TestAppContext,
    ) {
        let window = conversation(cx);
        window
            .update(cx, |view, _, cx| {
                view.input.update(cx, |i, cx| {
                    i.set_text(" /DRAFT Explore this later".into(), cx)
                });
                view.attachments.push("/tmp/note.txt".into());
                view.send(cx);
                assert!(!view.busy);
                assert!(view.summary.has_messages);
                assert!(
                    view.transcript.read(cx).source_rows().is_empty(),
                    "saved drafts must never enter provider history"
                );
                assert_eq!(
                    view.follow_ups.state.draft.as_ref().unwrap().text,
                    "Explore this later"
                );
                assert!(view.input.read(cx).content.is_empty());
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                let saved = locked(&view.store)
                    .unwrap()
                    .follow_ups(view.summary.id)
                    .unwrap();
                assert_eq!(
                    saved.draft.unwrap().attachments,
                    [std::path::PathBuf::from("/tmp/note.txt")]
                );
                view.input
                    .update(cx, |i, cx| i.set_text("my next message".into(), cx));
                view.draft_action(false, cx);
                assert!(!view.summary.has_messages);
                assert_eq!(view.input.read(cx).content, "my next message");
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, _, _| {
                assert!(!locked(&view.store).unwrap().sessions().unwrap()[0].has_messages);
            })
            .unwrap();
    }

    #[gpui::test]
    fn sending_a_saved_draft_preserves_the_new_composer(cx: &mut TestAppContext) {
        let window = conversation(cx);
        cx.update(|cx| {
            cx.update_global::<crate::settings::Preferences, _>(|prefs, _| {
                prefs.follow_up = "steer".into()
            })
        });
        let (controls, received) = std::sync::mpsc::channel();
        window
            .update(cx, |view, _, cx| {
                view.input
                    .update(cx, |i, cx| i.set_text("/draft send later".into(), cx));
                view.send(cx);
                view.busy = true;
                view.follow_ups.ready = true;
                view.controls = Some(controls);
                view.input
                    .update(cx, |i, cx| i.set_text("keep typing".into(), cx));
                view.attachments.push("/tmp/new-file.txt".into());
                view.draft_action(true, cx);
            })
            .unwrap();
        cx.run_until_parked();
        let control = received.try_recv().unwrap();
        let Decision::Steer(message) = control.decision else {
            panic!("expected saved draft");
        };
        assert_eq!(message.text, "send later");
        assert!(message.attachments.is_empty());
        window
            .update(cx, |view, _, cx| {
                assert_eq!(view.input.read(cx).content, "keep typing");
                assert_eq!(
                    view.attachments,
                    [std::path::PathBuf::from("/tmp/new-file.txt")]
                );
                assert!(view.follow_ups.state.draft.is_none());
            })
            .unwrap();
    }
}
