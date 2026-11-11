//! Submitting prompts and receiving a turn's updates.
use super::*;
use composer_draft::ComposerLane;
use std::path::PathBuf;
use vorkspace_core::store::{ComposerOrigin, ComposerRecovery};

enum PromptSource<'a> {
    Composer {
        origin: ComposerOrigin,
    },
    Direct,
    Coordinator,
    Queued {
        id: String,
        attachments: Vec<PathBuf>,
        prepared: Option<String>,
        mode: Option<String>,
    },
    Bot {
        message: &'a vorkspace_core::bot::Message,
        mode: Option<&'a str>,
    },
}

struct TurnSource {
    composer_origin: Option<ComposerOrigin>,
    queued: bool,
    one_shot_mode: bool,
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
        if self.needs_history_restore() || self.is_stopping() {
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
        self.replace_settings(settings, cx);
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
        if self.reject_user_entry(cx)
            || self.composer_draft.lane_pending(ComposerLane::Shelf)
            || self.composer_submission_pending(cx)
        {
            return;
        }
        self.follow_ups.admission_error = None;
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

        if self.needs_history_restore() {
            self.defer_history_action(|this, cx| this.send(cx), cx);
            return;
        }
        if self.defer_send_until_composer_ready(cx) {
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
            self.send_bot(cx);
            return;
        }
        self.send_normal(kind, cx);
    }

    fn send_bot(&mut self, cx: &mut Context<Self>) {
        if self.attaching
            || self.input.read(cx).is_composing()
            || self
                .bot_ui
                .direct_sending
                .as_ref()
                .is_some_and(|pending| !pending.retry)
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
                .is_some_and(|pending| pending.text != raw || pending.attachments != attachments)
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
            self.submit_bot_input(raw, attachments, selection, recovery, cx);
        }
    }

    fn submit_bot_input(
        &mut self,
        raw: String,
        attachments: Vec<PathBuf>,
        selection: std::ops::Range<usize>,
        recovery: Option<ComposerRecovery>,
        cx: &mut Context<Self>,
    ) {
        let text = self.consume_mode_command(raw.clone(), cx);
        let options = recovery.as_ref().map_or(
            bot::SubmissionOptions {
                mode: self.selected_turn_mode().map(str::to_owned),
                queue: self.bot_ui.queue,
            },
            |record| bot::SubmissionOptions::read(&record.payload).unwrap_or_default(),
        );
        let id = match self.bot_submission_id(recovery.as_ref(), &options) {
            Ok(id) => id,
            Err(error) => {
                self.status = format!(
                    "Could not preserve composer before sending: {error}. Your input was preserved."
                );
                cx.notify();
                return;
            }
        };
        self.bot_ui.queue = false;
        self.bot_ui.direct_sending = Some(bot::DirectSubmission {
            id: id.clone(),
            text: raw,
            attachments: attachments.clone(),
            selection,
            retry: false,
        });
        cx.emit(bot::Submitted {
            draft_id: Some(id),
            text,
            prepared_prompt: None,
            attachments,
            queue: options.queue,
            mode: options.mode.clone(),
        });
        if options.mode.is_some() {
            self.clear_submitted_mode(cx);
        }
        cx.notify();
    }

    fn bot_submission_id(
        &self,
        recovery: Option<&ComposerRecovery>,
        options: &bot::SubmissionOptions,
    ) -> Result<String, String> {
        if let Some(pending) = &self.bot_ui.direct_sending {
            Ok(pending.id.clone())
        } else if let Some(record) = recovery {
            Ok(record.owner.clone())
        } else {
            let id = vorkspace_core::bot::new_id();
            locked(&self.store).and_then(|store| {
                let mut saved = store
                    .composer_draft(self.summary.id)?
                    .ok_or("Composer was not saved")?;
                options.write(&mut saved.payload);
                store.save_composer_recovery(
                    self.summary.id,
                    vorkspace_core::store::ComposerRecovery {
                        owner: id.clone(),
                        revision: saved.revision,
                        payload: saved.payload,
                    },
                )
            })?;
            Ok(id)
        }
    }

    fn send_normal(&mut self, kind: Kind, cx: &mut Context<Self>) {
        if self.settings.worktree_removed {
            self.status = "Select a branch or worktree to continue this conversation.".into();
            cx.notify();
            return;
        }
        if !self.loaded || (self.preparing_worktree && !self.turn_in_flight()) {
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
        if !self.turn_in_flight() {
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
        self.submit_turn_input(cx);
    }

    fn submit_turn_input(&mut self, cx: &mut Context<Self>) {
        let queueing = self.turn_in_flight()
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
        let mode = self.selected_turn_mode();
        if queueing {
            self.queue_composer(prompt, mode.map(str::to_owned), cx);
            return;
        }
        self.launch_composer(prompt, cx);
    }

    pub(super) fn launch_composer(&mut self, prompt: String, cx: &mut Context<Self>) -> bool {
        let origin = match self.prepare_composer_submission(ComposerLane::Turn, cx) {
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
                origin: origin.clone(),
            },
            cx,
        );
        if !launched {
            self.finish_composer_submission(&origin, cx);
        }
        launched
    }
    pub(super) fn launch_prompt(&mut self, prompt: String, cx: &mut Context<Self>) {
        self.launch_turn(prompt, PromptSource::Direct, cx);
    }
    pub(super) fn launch_coordinator_prompt(&mut self, prompt: String, cx: &mut Context<Self>) {
        self.launch_turn(prompt, PromptSource::Coordinator, cx);
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
        if self.is_stopping() {
            if !matches!(source, PromptSource::Direct) {
                return false;
            }
            let mode = self.selected_turn_mode();
            if let Some(id) =
                self.enqueue_follow_up(prompt, Vec::new(), mode.map(str::to_owned), cx)
            {
                self.continue_after_stop(id);
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
        self.launch_admitted_turn(prompt, source, admission, cx);
        true
    }

    fn launch_admitted_turn(
        &mut self,
        prompt: String,
        source: PromptSource<'_>,
        admission: turn_driver::Admission,
        cx: &mut Context<Self>,
    ) {
        self.set_usage_limit(None, cx);
        self.attach_account(cx);
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
        let source = self.configure_turn_source(source, &mut settings);
        self.prepare_turn_settings(&mut settings, &model_label, &prompt, source.queued, cx);
        let turn_model_id = self.prepare_turn_view(&prompt, &mut settings, &source, cx);

        let mut projection = turn_projection::TurnProjection::new(
            generation,
            source_provider,
            source_account,
            summary.provider_id.clone(),
            turn_model_id,
        );
        projection.composer_origin = source.composer_origin.clone();
        if self.needs_title() && !settings.internal_message {
            projection.title_seed = Some(vorkspace_core::response::limit(&prompt, 8_000));
        }
        self.controls = Some(admission.launch(
            vorkspace_runtime::runtime::TurnInput {
                summary,
                prompt,
                settings,
                composer_origin: source.composer_origin,
            },
            projection,
            cx,
        ));
        cx.emit(SessionChanged::Turn);
        cx.notify();
    }

    fn configure_turn_source(
        &mut self,
        source: PromptSource<'_>,
        settings: &mut Settings,
    ) -> TurnSource {
        use PromptSource::{Bot, Composer, Queued};
        let queued = matches!(source, PromptSource::Queued { .. });
        let reset_orchestration = matches!(source, Composer { .. } | Queued { .. } | Bot { .. });
        let one_shot_mode = matches!(source, Composer { .. } | Queued { .. } | Bot { .. });
        let mut composer_origin = None;
        match source {
            PromptSource::Composer { origin } => {
                settings.ticket_context = self.ticket_context.ready.take().map(Into::into);
                settings.attachments = self.attachments.clone();
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
                apply_turn_mode(settings, mode.as_deref());
            }
            PromptSource::Bot { message, mode } => {
                settings.bot_message_id = Some(message.id.clone());
                settings.bot_sent_at = message.sent_at;
                settings.internal_message = message.internal;
                settings.attachments = message.attachments.iter().map(PathBuf::from).collect();
                settings.prepared_prompt = message.prepared_prompt.clone();
                apply_turn_mode(settings, mode);
            }
            PromptSource::Direct => {}
        }
        if reset_orchestration {
            self.planning_orchestration = settings.orchestrator;
            self.orchestration_plan = None;
            self.orchestration_error = None;
        }
        TurnSource {
            composer_origin,
            queued,
            one_shot_mode,
        }
    }

    fn prepare_turn_settings(
        &mut self,
        settings: &mut Settings,
        model_label: &str,
        prompt: &str,
        queued: bool,
        cx: &mut Context<Self>,
    ) {
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
        if !queued && settings.prepared_prompt.is_none() {
            let prepared = self.completion.read(cx).prepare_prompt(prompt);
            if prepared != prompt {
                settings.prepared_prompt = Some(prepared);
            }
        }
        settings.disable_claude_hooks = cx
            .try_global::<vorkspace_ui::preferences::Preferences>()
            .is_some_and(|p| !p.claude_hooks);
        settings.model = self.model.clone();
        self.prepare_app_environment(settings);
        self.preparing_worktree = settings.new_worktree && settings.worktree_cwd.is_none();
        if self.preparing_worktree {
            settings.worktree_base = settings.worktree_base.take().or(self.branch.clone());
            self.status = "Creating worktree…".into();
        }
    }

    fn prepare_turn_view(
        &mut self,
        prompt: &str,
        settings: &mut Settings,
        source: &TurnSource,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        if source.composer_origin.is_none() && settings.bot.is_none() {
            self.transcript
                .update(cx, |view, cx| view.submit_prompt(prompt, settings, cx));
        }
        if settings.orchestrator {
            settings.plan = true;
            settings.operator = false;
            if !source.one_shot_mode {
                self.settings.orchestrator = false;
            }
        }
        self.approvals.clear();
        self.questions.clear();
        settings
            .turn_model
            .as_ref()
            .and_then(|turn| turn.model_id.clone())
            .or_else(|| settings.model.clone())
    }

    fn launch_account_ready(&mut self, kind: Kind, cx: &mut Context<Self>) -> bool {
        if self.account_switching {
            self.status = "Wait for the account switch to finish. Your input was preserved.".into();
            cx.notify();
            return false;
        }
        if self.settings.account.id != vorkspace_core::accounts::DEFAULT
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
        projection: turn_projection::TurnProjection,
        cx: &mut Context<Self>,
    ) {
        let view = cx.weak_entity();
        turn_driver::project(view, rx, projection, None, cx);
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
        if self.submission_generation != generation || !self.turn_in_flight() {
            return;
        }
        if let Some(origin) = composer_origin {
            self.settle_composer_submission(origin, cx);
        }
        self.cancel.store(true, Ordering::Relaxed);
        self.turn_state = TurnState::Unconfirmed;
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

        cx.emit(SessionChanged::Settled);
        cx.notify();
    }
    pub(super) fn begin_turn(&mut self, cx: &mut Context<Self>) {
        self.submission_generation = self.submission_generation.wrapping_add(1);
        if !self.summary.has_messages {
            self.composer_motion.capture(self.composer_anchor.get());
        }
        self.activity = None;
        self.turn_state = TurnState::Running;
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

pub(super) fn apply_turn_mode(settings: &mut Settings, mode: Option<&str>) {
    settings.plan = mode == Some("plan");
    settings.operator = mode == Some("operator");
    settings.orchestrator = mode == Some("orchestrator");
}
