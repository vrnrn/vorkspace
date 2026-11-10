//! Submitting prompts and receiving a turn's updates.
use super::*;
use std::path::PathBuf;
use vorkspace_core::{agent::RemoteAction, store::ComposerOrigin};

enum PromptSource<'a> {
    Composer {
        origin: ComposerOrigin,
        save_remote_draft: bool,
    },
    Direct,
    Coordinator,
    Queued {
        id: String,
        attachments: Vec<PathBuf>,
        prepared: Option<String>,
        mode: Option<String>,
    },
    Remote(RemoteAction),
    Bot {
        message: &'a vorkspace_core::bot::Message,
        mode: Option<&'a str>,
    },
}

impl Conversation {
    pub(crate) fn quick_submit(
        &mut self,
        prompt: String,
        settings: Settings,
        cx: &mut Context<Self>,
    ) {
        if self.reject_user_entry(cx) {
            return;
        }
        if self.editing_last_turn.pending() {
            self.defer_edit_transfer(settings, Some(prompt), cx);
            return;
        }
        if self.needs_history_restore() || self.stopping {
            self.defer_history_action(
                move |this, cx| {
                    if this.preserve_new_composer(cx) {
                        this.install_transfer(settings, None, cx);
                        this.enqueue_restored_prompt(prompt, cx);
                    } else {
                        this.quick_submit(prompt, settings, cx);
                    }
                },
                cx,
            );
            return;
        }
        self.loaded = true;
        self.model = settings.model.clone();
        self.settings = settings;
        self.input
            .update(cx, |input, cx| input.set_text(prompt, cx));
        self.send(cx);
    }
    pub(super) fn execution_provider(&self) -> Result<Kind, String> {
        Kind::parse(&self.summary.provider).ok_or_else(|| {
            format!(
                "Provider {} is no longer supported. Saved history and drafts are retained.",
                self.summary.provider
            )
        })
    }
    pub(super) fn send(&mut self, cx: &mut Context<Self>) {
        if self.reject_user_entry(cx) || self.composer_submission_pending(cx) {
            return;
        }
        let kind = match self.execution_provider() {
            Ok(kind) => kind,
            Err(error) => {
                self.status = error;
                cx.notify();
                return;
            }
        };
        if let Some(error) = &self.composer_draft.error {
            self.status = error.clone();
            cx.notify();
            return;
        }
        if self.is_remote() && self.remote_send_guard(cx) {
            return;
        }
        if self.needs_history_restore() {
            self.defer_history_action(|this, cx| this.send(cx), cx);
            return;
        }
        if self.submit_edited(cx) {
            return;
        }
        if self.handle_local_command(cx) {
            return;
        }
        if self.settings.bot.is_some() && self.draft_active(cx) {
            self.save_message_draft(cx);
            return;
        }
        if self.settings.bot.is_some() {
            if self.attaching
                || self.input.read(cx).is_composing()
                || self
                    .bot_ui
                    .direct_sending
                    .as_ref()
                    .is_some_and(|pending| !pending.4)
            {
                return;
            }
            let raw = self.input.read(cx).content.to_string();
            let attachments = self.attachments.clone();
            if !raw.trim().is_empty() || !attachments.is_empty() {
                // The outbox acknowledgement clears only this exact input. A
                // failed write or process exit before admission leaves a saved
                // composer instead of silently discarding the submission.
                self.capture_composer(cx);
                if let Err(error) = self.composer_draft.flush() {
                    self.status =
                        format!("Could not save composer: {error}. Your input was preserved.");
                    cx.notify();
                    return;
                }
                let selection = self.input.read(cx).selection_utf16();
                if self
                    .bot_ui
                    .direct_sending
                    .as_ref()
                    .is_some_and(|(_, previous, files, _, _)| {
                        previous != &raw || files != &attachments
                    })
                {
                    self.status = "The previous message was not queued. Restore its original input and send it to retry; your current input is preserved.".into();
                    cx.notify();
                    return;
                }
                let recovery = match self.pending_bot_submission(&raw, &attachments) {
                    Ok(recovery) => recovery,
                    Err(error) => {
                        self.status = format!(
                            "Could not verify the previous submission: {error}. Your input was preserved."
                        );
                        cx.notify();
                        return;
                    }
                };
                let text = self.consume_mode_command(raw.clone(), cx);
                let mode = if self.settings.orchestrator {
                    Some("orchestrator")
                } else if self.settings.plan {
                    Some("plan")
                } else if self.settings.operator {
                    Some("operator")
                } else {
                    None
                };
                let mode = recovery.as_ref().map_or(mode.map(str::to_owned), |record| {
                    record.payload["botSubmission"]["mode"]
                        .as_str()
                        .map(str::to_owned)
                });
                let queue = recovery.as_ref().map_or(self.bot_ui.queue, |record| {
                    record.payload["botSubmission"]["queue"]
                        .as_bool()
                        .unwrap_or(false)
                });
                let id = if let Some((id, _, _, _, _)) = &self.bot_ui.direct_sending {
                    id.clone()
                } else if let Some(record) = &recovery {
                    record.owner.clone()
                } else {
                    let id = vorkspace_core::bot::new_id();
                    let result = locked(&self.store).and_then(|store| {
                        let mut saved = store
                            .composer_draft(self.summary.id)?
                            .ok_or("Composer was not saved")?;
                        saved.payload["botSubmission"] =
                            serde_json::json!({"mode": mode, "queue": queue});
                        store.save_composer_recovery(
                            self.summary.id,
                            vorkspace_core::store::ComposerRecovery {
                                owner: id.clone(),
                                revision: saved.revision,
                                payload: saved.payload,
                            },
                        )
                    });
                    if let Err(error) = result {
                        self.status = format!(
                            "Could not preserve composer before sending: {error}. Your input was preserved."
                        );
                        cx.notify();
                        return;
                    }
                    id
                };
                self.bot_ui.queue = false;
                self.bot_ui.direct_sending =
                    Some((id.clone(), raw, attachments.clone(), selection, false));
                cx.emit(bot::Submitted {
                    draft_id: Some(id),
                    text,
                    prepared_prompt: None,
                    attachments,
                    queue,
                    mode: mode.clone(),
                });
                if mode.is_some() {
                    self.clear_submitted_mode(cx);
                }
                cx.notify();
            }
            return;
        }
        if self.settings.worktree_removed {
            self.status = "Select a branch or worktree to continue this conversation.".into();
            cx.notify();
            return;
        }
        if !self.loaded || (self.preparing_worktree && !self.busy && !self.stopping) {
            return;
        }
        if self.draft_active(cx) {
            self.save_message_draft(cx);
            return;
        }
        if self.attaching
            || (!self.input.read(cx).has_message_text() && self.attachments.is_empty())
        {
            return;
        }
        if self.input.read(cx).is_composing() {
            return;
        }
        if !self.busy && !self.stopping {
            if !self.launch_account_ready(kind, cx) {
                return;
            }
            if !turn_driver::available(&self.store, self.summary.id, cx) {
                self.status = turn_driver::ALREADY_ACTIVE.into();
                cx.notify();
                return;
            }
        }
        if self.prepare_ticket_context(cx) {
            return;
        }
        let queueing = self.stopping
            || self.busy
            || self.settings.pending_provider_switch.is_some()
            || (!self.follow_ups.state.messages.is_empty() && !self.follow_ups.state.paused);
        let text = self.input.read(cx).content.as_ref();
        let raw = if text.trim().is_empty() {
            String::new()
        } else {
            text.to_owned()
        };
        let prompt = self.consume_mode_command(raw, cx);
        if prompt.trim().is_empty() && self.attachments.is_empty() {
            self.input
                .update(cx, |input, cx| input.set_text(String::new(), cx));
            return;
        }
        self.completion
            .update(cx, |completion, cx| completion.dismiss(cx));
        let mode = if self.settings.orchestrator {
            Some("orchestrator")
        } else if self.settings.plan {
            Some("plan")
        } else if self.settings.operator {
            Some("operator")
        } else {
            None
        };
        if self.settings.pending_provider_switch.is_some() && self.busy {
            // Picking a provider only arms the change. Sending is what stops
            // the outgoing provider and commits the handoff.
            self.stop(cx);
        }
        if queueing {
            self.input.update(cx, |input, cx| input.take(cx));
            self.enqueue_follow_up(prompt, mode.map(str::to_owned), cx);
            if self.stopping || self.settings.pending_provider_switch.is_some() {
                // A fresh Send continues the conversation, while older queued
                // messages remain paused after Stop.
                if let Some(id) = self
                    .follow_ups
                    .state
                    .messages
                    .last()
                    .map(|message| message.id.clone())
                {
                    self.follow_ups.continuations.push_back(id);
                    if self.follow_ups.requested.is_none() {
                        self.follow_ups.requested = self.follow_ups.continuations.front().cloned();
                    }
                }
            }
            if mode.is_some() {
                self.clear_submitted_mode(cx);
            }
            return;
        }
        if self.launch_composer(prompt, false, cx) && mode.is_some() {
            self.clear_submitted_mode(cx);
        }
    }

    pub(super) fn launch_composer(
        &mut self,
        prompt: String,
        save_remote_draft: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let origin = match self.prepare_composer_submission(cx) {
            Ok(origin) => origin,
            Err(error) => {
                self.status = error;
                cx.notify();
                return false;
            }
        };
        let launched = self.launch_turn(
            prompt,
            PromptSource::Composer {
                origin,
                save_remote_draft,
            },
            cx,
        );
        if !launched {
            self.finish_composer_submission(cx);
        }
        launched
    }
    pub(super) fn launch_prompt(&mut self, prompt: String, cx: &mut Context<Self>) {
        self.launch_turn(prompt, PromptSource::Direct, cx);
    }
    pub(super) fn launch_coordinator_prompt(&mut self, prompt: String, cx: &mut Context<Self>) {
        self.launch_turn(prompt, PromptSource::Coordinator, cx);
    }
    pub(super) fn launch_remote(
        &mut self,
        prompt: String,
        action: RemoteAction,
        cx: &mut Context<Self>,
    ) -> bool {
        self.launch_turn(prompt, PromptSource::Remote(action), cx)
    }
    pub(super) fn launch_follow_up(
        &mut self,
        message: vorkspace_core::follow_up::Message,
        cx: &mut Context<Self>,
    ) -> bool {
        self.launch_turn(
            message.text,
            PromptSource::Queued {
                id: message.id,
                attachments: message.attachments,
                prepared: message.prepared_prompt,
                mode: message.mode,
            },
            cx,
        )
    }

    pub(super) fn launch_bot_prompt(
        &mut self,
        message: &vorkspace_core::bot::Message,
        mode: Option<&str>,
        cx: &mut Context<Self>,
    ) -> bool {
        self.launch_turn(
            message.text.clone(),
            PromptSource::Bot { message, mode },
            cx,
        )
    }

    fn launch_turn(
        &mut self,
        prompt: String,
        source: PromptSource<'_>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !matches!(source, PromptSource::Bot { .. }) && self.reject_user_entry(cx) {
            return false;
        }
        let kind = match self.execution_provider() {
            Ok(kind) => kind,
            Err(error) => {
                self.status = error;
                cx.notify();
                return false;
            }
        };
        if self.stopping {
            if !matches!(source, PromptSource::Direct) {
                return false;
            }
            let mode = if self.settings.orchestrator {
                Some("orchestrator")
            } else if self.settings.plan {
                Some("plan")
            } else if self.settings.operator {
                Some("operator")
            } else {
                None
            };
            self.enqueue_follow_up(prompt, mode.map(str::to_owned), cx);
            if let Some(id) = self
                .follow_ups
                .state
                .messages
                .last()
                .map(|message| message.id.clone())
            {
                self.follow_ups.continuations.push_back(id);
                if self.follow_ups.requested.is_none() {
                    self.follow_ups.requested = self.follow_ups.continuations.front().cloned();
                }
            }
            return false;
        }
        if !self.history_ready() {
            if matches!(source, PromptSource::Direct) {
                self.defer_history_action(move |this, cx| this.launch_prompt(prompt, cx), cx);
            }
            return false;
        }
        if !self.launch_account_ready(kind, cx) {
            return false;
        }
        let view = cx.weak_entity();
        let Some(admission) = turn_driver::admit(&self.store, self.summary.id, view, cx) else {
            self.status = turn_driver::ALREADY_ACTIVE.into();
            cx.notify();
            return false;
        };
        self.settings.usage_limit = None;
        self.attach_account(cx);
        if !self.is_remote() {
            self.generate_title(&prompt, cx);
        }
        let model_label = self.model_label();
        self.sync_tool_paths(cx);
        self.transcript
            .update(cx, |v, _| v.identity(&self.summary.provider, &model_label));
        self.begin_turn(cx);
        let generation = self.submission_generation;
        let source_provider = self.summary.provider.clone();
        let source_account = self.settings.account.id.clone();
        self.status = if self.settings.pending_provider_switch.is_some() {
            "Preparing handoff…"
        } else {
            "Connecting…"
        }
        .into();
        self.cancel = admission.cancel_handle();
        let summary = self.summary.clone();
        let mut settings = self.settings.clone();
        let queued = matches!(source, PromptSource::Queued { .. });
        let reset_orchestration = matches!(
            source,
            PromptSource::Composer { .. } | PromptSource::Queued { .. } | PromptSource::Bot { .. }
        );
        let one_shot_mode = matches!(
            source,
            PromptSource::Queued { .. } | PromptSource::Bot { .. }
        );
        let mut composer_origin = None;
        match source {
            PromptSource::Composer {
                origin,
                save_remote_draft,
            } => {
                settings.ticket_context = self.ticket_context.ready.take().map(Into::into);
                settings.attachments = self.attachments.clone();
                settings.remote_action = save_remote_draft.then_some(RemoteAction::SaveDraft);
                composer_origin = Some(origin);
            }
            PromptSource::Coordinator => settings.internal_message = true,
            PromptSource::Queued {
                id,
                attachments,
                prepared,
                mode,
            } => {
                settings.bot_message_id = Some(id);
                settings.attachments = attachments;
                settings.prepared_prompt = prepared;
                apply_turn_mode(&mut settings, mode.as_deref());
            }
            PromptSource::Bot { message, mode } => {
                settings.bot_message_id = Some(message.id.clone());
                settings.bot_sent_at = message.sent_at;
                settings.internal_message = message.internal;
                settings.attachments = message.attachments.iter().map(PathBuf::from).collect();
                settings.prepared_prompt = message.prepared_prompt.clone();
                apply_turn_mode(&mut settings, mode);
            }
            PromptSource::Remote(action) => {
                settings.remote_action = Some(action);
            }
            PromptSource::Direct => {}
        }
        if reset_orchestration {
            self.planning_orchestration = settings.orchestrator;
            self.orchestration_plan = None;
            self.orchestration_error = None;
        }
        if let Some(model) = self.model_info() {
            for control in &model.controls {
                settings
                    .model_settings
                    .entry(control.id.clone())
                    .or_insert_with(|| control.current.clone());
            }
        }
        settings.turn_model = Some(vorkspace_core::agent::TurnInfo {
            provider: self.summary.provider.clone(),
            model: model_label.to_string(),
            model_id: self
                .model_info()
                .map(|model| model.id.clone())
                .or(self.model.clone()),
            started_at: Some(vorkspace_core::time::now_ms()),
            ..Default::default()
        });
        if !self.is_remote() && !queued && settings.prepared_prompt.is_none() {
            let prepared = self.completion.read(cx).prepare_prompt(&prompt);
            if prepared != prompt {
                settings.prepared_prompt = Some(prepared);
            }
        }
        settings.disable_claude_hooks = cx
            .try_global::<vorkspace_ui::preferences::Preferences>()
            .is_some_and(|p| !p.claude_hooks);
        settings.model = self.model.clone();
        self.prepare_app_environment(&mut settings);
        self.preparing_worktree = settings.new_worktree && settings.worktree_cwd.is_none();
        if self.preparing_worktree {
            settings.worktree_base = settings.worktree_base.or(self.branch.clone());
            self.status = "Creating worktree…".into();
        }
        if composer_origin.is_none()
            && settings.bot.is_none()
            && matches!(
                settings.remote_action,
                None | Some(
                    RemoteAction::Compact | RemoteAction::SendDraft(_) | RemoteAction::BuildPlan(_)
                )
            )
        {
            self.transcript
                .update(cx, |view, cx| view.submit_prompt(&prompt, &settings, cx));
        }
        if settings.orchestrator {
            settings.plan = true;
            settings.operator = false;
            if !one_shot_mode {
                self.settings.orchestrator = false;
            }
        }
        if self.settings.handoff.take().is_some() {
            self.save_settings(cx);
        }
        self.approvals.clear();
        self.questions.clear();
        let turn_model_id = settings
            .turn_model
            .as_ref()
            .and_then(|turn| turn.model_id.clone())
            .or_else(|| settings.model.clone());
        let remote = self
            .is_remote()
            .then(|| self.remote_state.client.clone())
            .flatten();
        let mut projection = turn_projection::TurnProjection::new(
            generation,
            source_provider,
            source_account,
            turn_model_id,
        );
        projection.composer_origin = composer_origin.clone();
        self.controls = Some(admission.launch(
            vorkspace_runtime::runtime::TurnInput {
                summary,
                prompt,
                settings,
                composer_origin,
            },
            remote,
            projection,
            cx,
        ));
        cx.emit(SessionChanged::Turn);
        cx.notify();
        true
    }

    fn launch_account_ready(&mut self, kind: Kind, cx: &mut Context<Self>) -> bool {
        if self.account_switching {
            self.status = "Wait for the account switch to finish. Your input was preserved.".into();
            cx.notify();
            return false;
        }
        if !self.is_remote()
            && self.settings.account.id != vorkspace_core::accounts::DEFAULT
            && crate::accounts::get(cx)
                .is_some_and(|center| !center.read(cx).book.exists(kind, &self.settings.account.id))
        {
            self.account_unavailable(cx);
            return false;
        }
        true
    }
    #[cfg(test)]
    pub(super) fn receive_turn_updates(
        &mut self,
        rx: async_channel::Receiver<vorkspace_runtime::runtime::Update>,
        generation: u64,
        source_provider: String,
        source_account: String,
        turn_model_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let view = cx.weak_entity();
        turn_driver::project(
            view,
            rx,
            turn_projection::TurnProjection::new(
                generation,
                source_provider,
                source_account,
                turn_model_id,
            ),
            None,
            cx,
        );
    }

    /// The final update repeats the turn's initial receipt beside any steers.
    /// A steer is always accepted after its turn began, so its receipt wins.
    pub(crate) fn apply_receipts(
        &mut self,
        delivered: Option<String>,
        follow_ups: Vec<String>,
        errors: Vec<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = delivered {
            if self.settings.bot.is_some() {
                self.bot_ui.delivered = Some(id);
                self.sync_bot_pending(cx);
            } else {
                self.follow_up_delivered(&id, cx);
            }
        }
        for id in follow_ups {
            self.follow_up_delivered(&id, cx);
        }
        for (id, error) in errors {
            self.fail_follow_up(&id, error, cx);
        }
    }
    pub(super) fn turn_updates_closed(
        &mut self,
        generation: u64,
        composer_origin: Option<&ComposerOrigin>,
        cx: &mut Context<Self>,
    ) {
        // The sender can disappear while unwinding, without a final Update.
        // A superseded worker must never release the replacement turn's UI.
        if self.submission_generation != generation || (!self.busy && !self.stopping) {
            return;
        }
        if let Some(origin) = composer_origin {
            self.settle_composer_submission(origin, cx);
        }
        self.cancel.store(true, Ordering::Relaxed);
        self.busy = false;
        self.stopping = false;
        self.preparing_worktree = false;
        self.planning_orchestration = false;
        self.orchestration_progress = None;
        self.review
            .update(cx, |review, cx| review.set_busy(false, cx));
        self.follow_ups.ready = false;
        self.follow_ups.resuming = false;
        self.follow_ups.requested = None;
        self.follow_ups.continuations.clear();
        if let Some(id) = self.follow_ups.sending.clone()
            && self.settings.bot.is_some()
        {
            self.fail_follow_up(&id, UNCONFIRMED_TURN.into(), cx);
        }
        if let Some(id) = self.follow_ups.sending.take()
            && let Some(message) = self
                .follow_ups
                .state
                .messages
                .iter_mut()
                .find(|m| m.id == id)
        {
            message.error = Some(UNCONFIRMED_TURN.into());
        }
        // Persistence callbacks pump the queue too. Publish the paused state
        // and clear explicit-send markers before scheduling any of those reads.
        self.follow_ups.state.paused = true;
        self.save_follow_ups(cx);
        self.approvals.clear();
        self.questions.clear();
        self.controls = None;
        self.status = UNCONFIRMED_TURN.into();
        self.transcript
            .update(cx, |view, cx| view.streaming(false, cx));
        self.settings.turns = self.transcript.read(cx).turns.clone();
        self.save_settings(cx);
        self.refresh_submission_recovery(cx);
        if self.is_remote() {
            self.remote_state.refreshing = false;
            self.refresh_remote_snapshot(cx);
        }
        cx.emit(SessionChanged::Settled);
        cx.notify();
    }
    pub(super) fn begin_turn(&mut self, cx: &mut Context<Self>) {
        self.submission_generation = self.submission_generation.wrapping_add(1);
        if !self.summary.has_messages {
            self.composer_motion.capture(self.composer_anchor.get());
        }
        self.activity = None;
        self.busy = true;
        self.stopping = false;
        self.notification_cancelled = false;
        self.review
            .update(cx, |review, cx| review.set_busy(true, cx));
        self.follow_ups.ready = false;
        self.bot_turn = self
            .bot_turn
            .wrapping_add(1)
            .max(vorkspace_core::time::now_ms());
        self.summary.updated_at = vorkspace_core::time::now_ms();
        self.transcript.update(cx, |v, cx| v.streaming(true, cx));
        self.summary.has_messages = true;
    }
}

fn apply_turn_mode(settings: &mut Settings, mode: Option<&str>) {
    settings.plan = mode == Some("plan");
    settings.operator = mode == Some("operator");
    settings.orchestrator = mode == Some("orchestrator");
}
