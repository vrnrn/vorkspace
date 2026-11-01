//! Submitting prompts and receiving a turn's updates.
use super::*;

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
        if self.reject_user_entry(cx) {
            return;
        }
        if let Err(error) = self.execution_provider() {
            self.status = error;
            cx.notify();
            return;
        }
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
        if !self.folder_selected
            && vorkspace_core::composer::consume_command(
                &self.input.read(cx).content,
                "add-to-folder",
            )
            .is_some()
        {
            self.request_folder(cx);
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
        if self.prepare_ticket_context(cx) {
            return;
        }
        let raw = self
            .input
            .update(cx, |i, cx| i.take(cx))
            .unwrap_or_default();
        let raw = vorkspace_core::composer::consume_command(&raw, "add-to-folder")
            .map(str::to_owned)
            .unwrap_or(raw);
        self.folder_selected = false;
        let prompt = self.consume_mode_command(raw, cx);
        if prompt.trim().is_empty() && self.attachments.is_empty() {
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
        if self.stopping
            || self.busy
            || self.settings.pending_provider_switch.is_some()
            || (!self.follow_ups.state.messages.is_empty() && !self.follow_ups.state.paused)
        {
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
        self.planning_orchestration = self.settings.orchestrator;
        self.orchestration_plan = None;
        self.orchestration_error = None;
        self.launch_prompt(prompt, cx);
        if self.busy && mode.is_some() {
            self.clear_submitted_mode(cx);
        }
    }
    pub(super) fn launch_prompt(&mut self, prompt: String, cx: &mut Context<Self>) {
        if !self.team_dispatch && self.reject_user_entry(cx) {
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
        if self.stopping {
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
            return;
        }
        if !self.history_ready() {
            self.defer_history_action(move |this, cx| this.launch_prompt(prompt, cx), cx);
            return;
        }
        if self.account_switching {
            return;
        }
        if self.settings.account.id != vorkspace_core::accounts::DEFAULT
            && crate::accounts::get(cx)
                .is_some_and(|center| !center.read(cx).book.exists(kind, &self.settings.account.id))
        {
            self.account_unavailable(cx);
            return;
        }
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
        self.cancel = Arc::new(AtomicBool::new(false));
        let summary = self.summary.clone();
        let store = self.store.clone();
        let cancel = self.cancel.clone();
        let mut settings = self.settings.clone();
        settings.ticket_context = self.ticket_context.ready.take().map(Into::into);
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
        if !self.is_remote() && settings.prepared_prompt.is_none() {
            let prepared = self.completion.read(cx).prepare_prompt(&prompt);
            if prepared != prompt {
                settings.prepared_prompt = Some(prepared);
            }
        }
        settings.disable_claude_hooks = cx
            .try_global::<crate::settings::Preferences>()
            .is_some_and(|p| !p.claude_hooks);
        settings.model = self.model.clone();
        self.prepare_app_environment(&mut settings);
        if settings.bot.is_none()
            && settings.remote_action.as_ref().is_none_or(|action| {
                (action["type"] == "send"
                    && action["draftBlockId"].is_null()
                    && action["planBlockId"].is_null())
                    || action["type"] == "draft"
            })
        {
            settings
                .attachments
                .extend(std::mem::take(&mut self.attachments));
        }
        if settings
            .remote_action
            .as_ref()
            .is_some_and(|action| action["planBlockId"].is_string())
        {
            settings.attachments.clear();
        }
        self.preparing_worktree = settings.new_worktree && settings.worktree_cwd.is_none();
        if self.preparing_worktree {
            settings.worktree_base = settings.worktree_base.or(self.branch.clone());
            self.status = "Creating worktree…".into();
        }
        if settings.bot.is_none()
            && settings
                .remote_action
                .as_ref()
                .is_none_or(|action| action["type"] == "send")
        {
            self.transcript
                .update(cx, |view, cx| view.submit_prompt(&prompt, &settings, cx));
        }
        if settings.orchestrator {
            settings.plan = true;
            settings.operator = false;
            self.settings.orchestrator = false;
        }
        if self.settings.handoff.take().is_some() {
            self.save_settings(cx);
        }
        let (controls, control_rx) = std::sync::mpsc::channel();
        self.controls = Some(controls);
        self.approvals.clear();
        self.questions.clear();
        let (tx, rx) = async_channel::bounded(8);
        let turn_model_id = settings
            .turn_model
            .as_ref()
            .and_then(|turn| turn.model_id.clone())
            .or_else(|| settings.model.clone());
        let remote_client = self
            .is_remote()
            .then(|| self.remote_state.client.clone())
            .flatten();
        cx.background_executor()
            .spawn(async move {
                if let Some(client) = remote_client {
                    vorkspace_runtime::runtime::remote::run_with_client(
                        client, summary, prompt, settings, control_rx, store, cancel, tx,
                    );
                } else {
                    run_turn(summary, prompt, settings, control_rx, store, cancel, tx);
                }
            })
            .detach();
        self.receive_turn_updates(
            rx,
            generation,
            source_provider,
            source_account,
            turn_model_id,
            cx,
        );
        cx.emit(SessionChanged);
        cx.notify();
    }
    pub(super) fn receive_turn_updates(
        &mut self,
        rx: async_channel::Receiver<vorkspace_runtime::runtime::Update>,
        generation: u64,
        source_provider: String,
        source_account: String,
        mut turn_model_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let mut completed = false;
            while let Ok(update) = rx.recv().await {
                let finished = update.finished;
                completed |= finished;
                if this
                    .update(cx, |this, cx| {
                        if this.submission_generation != generation {
                            return;
                        }
                        let stopping = this.stopping;
                        // The final update settles accepted rows and storage.
                        // Ignore streaming notifications after the user stops.
                        if stopping && !finished {
                            return;
                        }
                        let source_selected = this.summary.provider == source_provider
                            && this.settings.account.id == source_account;
                        if let Some(cwd) = update.worktree_cwd
                            && (this.settings.worktree_cwd.as_ref() != Some(&cwd)
                                || this.settings.new_worktree)
                        {
                            this.preparing_worktree = false;
                            this.settings.worktree_cwd = Some(cwd);
                            this.settings.new_worktree = false;
                            this.save_settings(cx);
                            this.refresh_git(cx);
                        }
                        if let Some(commands) = update.commands.filter(|_| source_selected) {
                            this.completion.update(cx, |completion, cx| {
                                completion.set_native_commands(commands, cx)
                            });
                        }
                        let activity_changed = this.activity != update.activity;
                        this.activity = update.activity;
                        let needed_approval = this.needs_approval();
                        let incoming_notification_input = !stopping
                            && (!update.approvals.is_empty() || !update.questions.is_empty());
                        this.transcript.update(cx, |v, cx| {
                            if let Some(len) = update.source_len {
                                v.patch_snapshot(update.start, update.rows, len, cx);
                            } else {
                                v.patch(update.start, update.rows, cx);
                            }
                            if stopping {
                                v.stop_streaming(cx);
                            }
                        });
                        if let Some(id) = update.provider_turn_id {
                            this.transcript
                                .update(cx, |view, _| view.record_provider_turn(id));
                        }
                        if let Some(id) = update.provider_id {
                            if let Some(pending) = this.settings.pending_provider_switch.as_mut()
                                && pending.from == source_provider
                                && pending.account.id == source_account
                            {
                                pending.provider_id = Some(id);
                            } else if source_selected {
                                this.summary.provider_id = Some(id);
                            }
                        }
                        if let Some(handoff) = update.handoff {
                            this.settings.pending_provider_switch = None;
                            this.settings.handoff = handoff;
                            this.save_settings(cx);
                        }
                        if let Some(model) = update.config_model {
                            if let Some(pending) = this.settings.pending_provider_switch.as_mut()
                                && pending.from == source_provider
                                && pending.account.id == source_account
                            {
                                pending.model = Some(model);
                            } else if source_selected {
                                this.model = Some(model.clone());
                                this.settings.model = Some(model);
                            }
                        }
                        if let Some(settings) = update.config_settings {
                            if let Some(pending) = this.settings.pending_provider_switch.as_mut()
                                && pending.from == source_provider
                                && pending.account.id == source_account
                            {
                                pending.model_settings.extend(settings);
                            } else if source_selected {
                                this.settings.model_settings.extend(settings);
                            }
                            this.save_settings(cx);
                        }
                        if let Some(model) = update.model {
                            turn_model_id = Some(model.clone());
                            let name: SharedString = this
                                .catalog
                                .iter()
                                .find(|entry| source_selected && entry.id == model)
                                .map(|entry| {
                                    let name = model_display_name(
                                        Kind::from_id(&source_provider),
                                        &entry.id,
                                        &entry.name,
                                    );
                                    name.strip_prefix("Claude ")
                                        .unwrap_or(&name)
                                        .to_owned()
                                        .into()
                                })
                                .unwrap_or_else(|| {
                                    model_name(Kind::from_id(&source_provider), Some(&model))
                                });
                            if source_selected {
                                this.resolved_model = Some(model);
                            }
                            this.transcript
                                .update(cx, |v, _| v.identity(&source_provider, &name));
                        }
                        if let Some(context) = update.context.filter(|_| source_selected) {
                            this.context = Some(context);
                        }
                        if let Some(metrics) = update.metrics {
                            this.transcript.update(cx, |v, _| v.metrics(metrics));
                        }
                        if update.turn_ready && !stopping {
                            this.follow_ups.ready = true;
                        }
                        this.apply_receipts(
                            update.delivered,
                            update.follow_up_delivered,
                            update.follow_up_errors,
                            cx,
                        );
                        if !stopping || finished {
                            this.status = update.status;
                        }
                        if !stopping {
                            this.approvals.extend(update.approvals);
                        }
                        for id in update.approvals_resolved {
                            this.approvals.retain(|approval| approval.id != id);
                        }
                        for id in update.questions_resolved {
                            this.questions.retain(|(card, _)| card.read(cx).id() != id);
                        }
                        for request in update.questions.into_iter().filter(|_| !stopping) {
                            let card = cx.new(|cx| {
                                crate::composer::questions::QuestionCard::new(request, cx)
                            });
                            let events = cx.subscribe(
                                &card,
                                |this, _, reply: &crate::composer::questions::Answered, cx| {
                                    if this
                                        .questions
                                        .front()
                                        .is_none_or(|(card, _)| card.read(cx).id() != reply.id)
                                    {
                                        return;
                                    }
                                    let before = this.needs_approval();
                                    let decision = reply
                                        .answers
                                        .clone()
                                        .map_or(Decision::Deny, Decision::Answer);
                                    if this.controls.as_ref().is_none_or(|controls| {
                                        controls
                                            .send(Control {
                                                id: reply.id.clone(),
                                                decision,
                                            })
                                            .is_err()
                                    }) {
                                        this.status =
                                            "The provider is no longer waiting for this answer"
                                                .into();
                                        cx.notify();
                                        return;
                                    }
                                    this.questions.pop_front();
                                    this.status = if !this.approvals.is_empty() {
                                        "Waiting for approval"
                                    } else if !this.questions.is_empty() {
                                        "Waiting for your answer"
                                    } else {
                                        "Working…"
                                    }
                                    .into();
                                    if before != this.needs_approval() {
                                        cx.emit(SessionChanged);
                                    }
                                    cx.notify();
                                },
                            );
                            let keep_open = cx.subscribe(
                                &card,
                                |this, _, request: &crate::composer::questions::KeptOpen, cx| {
                                    if let Some(controls) = &this.controls {
                                        let _ = controls.send(Control {
                                            id: request.id.clone(),
                                            decision: Decision::KeepOpen,
                                        });
                                    }
                                    cx.emit(SessionChanged);
                                    cx.notify();
                                },
                            );
                            this.questions.push_back((card, vec![events, keep_open]));
                            this.status = "Waiting for your answer".into();
                        }
                        if !finished && !this.approvals.is_empty() {
                            this.status = "Waiting for approval".into();
                        } else if !finished && !this.questions.is_empty() {
                            this.status = "Waiting for your answer".into();
                        }
                        if finished {
                            if this.is_remote() {
                                this.settings.remote_action = None;
                                this.remote_state.refreshing = false;
                            }
                            this.summary.updated_at = vorkspace_core::time::now_ms();
                            this.busy = false;
                            this.stopping = false;
                            if this.is_remote() {
                                this.refresh_remote_snapshot(cx);
                            }
                            this.preparing_worktree = false;
                            if !stopping && let Some(limit) = update.usage_limit {
                                this.settings.usage_limit = Some(limit);
                            }
                            if !this.is_remote() {
                                this.detect_usage_limit(cx);
                            }
                            this.review
                                .update(cx, |review, cx| review.set_busy(false, cx));
                            this.follow_ups.ready = false;
                            if this.follow_ups.resuming {
                                this.follow_ups.resuming = false;
                                if !stopping && this.status == "Ready" {
                                    this.follow_ups.state.paused = false;
                                    for message in &mut this.follow_ups.state.messages {
                                        message.error = None;
                                    }
                                    this.save_follow_ups(cx);
                                }
                            }
                            if let Some(id) = this.follow_ups.sending.clone() {
                                this.fail_follow_up(
                                    &id,
                                    "The turn ended before this message was accepted".into(),
                                    cx,
                                );
                            }
                            if this.status != "Ready" && !this.follow_ups.state.messages.is_empty()
                            {
                                this.follow_ups.state.paused = true;
                                this.save_follow_ups(cx);
                            }
                            this.approvals.clear();
                            this.questions.clear();
                            this.controls = None;
                            this.refresh_submission_recovery(cx);
                            this.transcript.update(cx, |v, cx| v.streaming(false, cx));
                            let model_id = turn_model_id.clone();
                            this.transcript.update(cx, |view, _| {
                                if let Some(turn) = view.active_turn()
                                    && let Some(info) = view.turns.get_mut(&turn)
                                {
                                    info.model_id = model_id;
                                }
                            });
                            if stopping {
                                this.planning_orchestration = false;
                                this.orchestration_progress = None;
                            } else {
                                this.finish_orchestration_plan(cx);
                            }
                            this.settings.turns = this.transcript.read(cx).turns.clone();
                            this.save_settings(cx);
                            this.refresh_git(cx);
                            this.run_history_actions(cx);
                            cx.emit(SessionChanged);
                        } else if incoming_notification_input
                            || needed_approval != this.needs_approval()
                        {
                            cx.emit(SessionChanged);
                        }
                        if activity_changed {
                            cx.emit(SessionActivityChanged);
                        }
                        this.pump_follow_ups(cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
                // recv().await need not suspend: a producer can refill even a
                // bounded channel while we patch. Return to the native event
                // loop between atomic updates so input and frames can run.
                // A positive timer also avoids immediately requeueing a ready
                // continuation onto the main dispatch queue.
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(1))
                    .await;
            }
            if !completed {
                let _ = this.update(cx, |this, cx| this.turn_updates_closed(generation, cx));
            }
        })
        .detach();
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
    pub(super) fn turn_updates_closed(&mut self, generation: u64, cx: &mut Context<Self>) {
        // The sender can disappear while unwinding, without a final Update.
        // A superseded worker must never release the replacement turn's UI.
        if self.submission_generation != generation || (!self.busy && !self.stopping) {
            return;
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
            self.settings.remote_action = None;
            self.remote_state.refreshing = false;
            self.refresh_remote_snapshot(cx);
        }
        cx.emit(SessionChanged);
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
            .max(vorkspace_core::time::now_ms() & 0xffff_ffff);
        self.summary.updated_at = vorkspace_core::time::now_ms();
        self.transcript.update(cx, |v, cx| v.streaming(true, cx));
        self.summary.has_messages = true;
    }
}
