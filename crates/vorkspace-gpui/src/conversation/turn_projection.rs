//! Apply an admitted turn's updates to a conversation without owning execution.
use super::*;
use vorkspace_core::store::ComposerOrigin;
use vorkspace_runtime::runtime::{
    HistoryRecovery, ProviderBinding, TranscriptPatch, TurnOutcome, TurnSettlement, TurnTransition,
    Update,
};

pub(super) struct TurnProjection {
    pub(super) generation: u64,
    source_provider: String,
    source_account: String,
    source_provider_id: Option<String>,
    turn_model_id: Option<String>,
    pub(super) composer_origin: Option<ComposerOrigin>,
    pub(super) title_seed: Option<String>,
}

impl TurnProjection {
    pub(super) fn new(
        generation: u64,
        source_provider: String,
        source_account: String,
        source_provider_id: Option<String>,
        turn_model_id: Option<String>,
    ) -> Self {
        Self {
            generation,
            source_provider,
            source_account,
            source_provider_id,
            turn_model_id,
            composer_origin: None,
            title_seed: None,
        }
    }

    pub(super) fn apply(
        &mut self,
        this: &mut Conversation,
        mut update: Update,
        cx: &mut Context<Conversation>,
    ) {
        if this.submission_generation != self.generation {
            return;
        }
        let (accepted, settlement) = match std::mem::take(&mut update.transition) {
            TurnTransition::Progress => (false, None),
            TurnTransition::AcceptedComposer(origin) => {
                this.accept_composer_submission(&origin, cx);
                (true, None)
            }
            TurnTransition::Settled(settlement) => (false, Some(settlement)),
        };
        let outcome = settlement.as_ref().map(|settlement| settlement.outcome);
        let finished = settlement.is_some();
        if finished && let Some(origin) = self.composer_origin.as_ref() {
            this.settle_composer_submission(origin, cx);
        }
        let stopping = this.is_stopping();
        // The final update settles accepted rows and storage.
        // Ignore streaming notifications after the user stops.
        if stopping && !finished && !accepted {
            return;
        }
        let source_selected = this.summary.provider == self.source_provider
            && this.settings.account.id == self.source_account;
        if !stopping
            && source_selected
            && (accepted || update.turn_ready || outcome == Some(TurnOutcome::Completed))
            && let Some(seed) = self.title_seed.take()
        {
            this.generate_title(&seed, cx);
        }
        let mut checkout_changed = false;
        if let Some(cwd) = update.worktree_cwd.take()
            && (this.settings.worktree_cwd.as_ref() != Some(&cwd) || this.settings.new_worktree)
        {
            this.preparing_worktree = false;
            this.settings.worktree_cwd = Some(cwd);
            this.settings.new_worktree = false;
            this.save_settings(cx);
            this.refresh_git(cx);
            checkout_changed = true;
        }
        if let Some(commands) = update.commands.take().filter(|_| source_selected) {
            this.completion.update(cx, |completion, cx| {
                completion.set_native_commands(commands, cx)
            });
        }
        let history_recovery = settlement
            .as_ref()
            .and_then(|settlement| settlement.history_recovery);
        let activity_changed = this.activity != update.activity;
        this.activity = update.activity.take();
        let inputs_changed = (!stopping
            && (!update.approvals.is_empty() || !update.questions.is_empty()))
            || !update.approvals_resolved.is_empty()
            || !update.questions_resolved.is_empty();
        let patch = std::mem::take(&mut update.transcript);
        Self::apply_transcript(this, patch, stopping, cx);
        self.apply_provider(this, &mut update, source_selected, cx);
        Self::apply_inputs(this, update, stopping, finished, cx);
        if let Some(settlement) = settlement {
            self.apply_settlement(this, settlement, stopping, history_recovery, cx);
        } else {
            if checkout_changed {
                cx.emit(SessionChanged::Checkout);
            }
            if inputs_changed {
                cx.emit(SessionChanged::Attention);
            }
        }
        if activity_changed {
            cx.emit(SessionActivityChanged);
        }
        this.pump_follow_ups(cx);
        cx.notify();
    }

    fn apply_transcript(
        this: &mut Conversation,
        patch: TranscriptPatch,
        stopping: bool,
        cx: &mut Context<Conversation>,
    ) {
        let stop_stream = stopping && !matches!(&patch, TranscriptPatch::BotSnapshot { .. });
        if stop_stream || !matches!(&patch, TranscriptPatch::Unchanged) {
            this.transcript.update(cx, |view, cx| {
                match patch {
                    TranscriptPatch::Unchanged => {}
                    TranscriptPatch::Tail { start, rows } => view.patch(start, rows, cx),
                    TranscriptPatch::Snapshot { start, rows } => {
                        if start == 0 {
                            view.patch_full_snapshot(rows, cx);
                        } else {
                            let end = start + rows.len();
                            view.patch_snapshot(start, rows, end, cx);
                        }
                    }
                    TranscriptPatch::BotSnapshot { turn_start, rows } => {
                        view.patch_bot_turn_snapshot(turn_start, rows, cx);
                    }
                }
                if stop_stream {
                    view.stop_streaming(cx);
                }
            });
        }
    }

    fn apply_provider(
        &mut self,
        this: &mut Conversation,
        update: &mut Update,
        source_selected: bool,
        cx: &mut Context<Conversation>,
    ) {
        let source_provider = self.source_provider.as_str();
        let source_account = self.source_account.as_str();
        if let Some(id) = update.provider_turn_id.take() {
            this.transcript
                .update(cx, |view, _| view.record_provider_turn(id));
        }
        if let ProviderBinding::Set(id) = std::mem::take(&mut update.binding) {
            if let Some(pending) = this.settings.pending_provider_switch.as_mut()
                && pending.from == source_provider
                && pending.account.id == source_account
                && pending.provider_id == self.source_provider_id
            {
                pending.provider_id = id.clone();
            } else if source_selected && this.summary.provider_id == self.source_provider_id {
                this.summary.provider_id = id.clone();
            }
            self.source_provider_id = id;
        }
        let source_bound = source_selected && this.summary.provider_id == self.source_provider_id;
        if let Some(handoff) = update.handoff.take().filter(|_| source_bound) {
            this.settings.pending_provider_switch = None;
            this.settings.handoff = handoff;
        }
        if let Some(configuration) = update.configuration.take() {
            if let Some(pending) = this.settings.pending_provider_switch.as_mut()
                && pending.from == source_provider
                && pending.account.id == source_account
                && pending.provider_id == configuration.provider_id
            {
                configuration.patch.apply_to_switch(pending);
            } else if source_selected
                && this.settings.revision == configuration.revision
                && this.summary.provider_id == configuration.provider_id
            {
                configuration.patch.apply(&mut this.settings);
                this.model = this.settings.model.clone();
            }
        }
        if let Some(model) = update.model.take() {
            self.turn_model_id = Some(model.clone());
            let name: SharedString = this
                .catalog
                .iter()
                .find(|entry| source_selected && entry.id == model)
                .map(|entry| {
                    let name =
                        model_display_name(Kind::from_id(source_provider), &entry.id, &entry.name);
                    name.strip_prefix("Claude ")
                        .unwrap_or(&name)
                        .to_owned()
                        .into()
                })
                .unwrap_or_else(|| model_name(Kind::from_id(source_provider), Some(&model)));
            if source_selected {
                this.resolved_model = Some(model);
            }
            this.transcript
                .update(cx, |v, _| v.identity(source_provider, &name));
        }
        if let Some(context) = update.context.take().filter(|_| source_bound) {
            this.context = Some(context);
        }
        if let Some(metrics) = update.metrics.take() {
            this.transcript.update(cx, |v, _| v.metrics(metrics));
        }
    }

    fn apply_inputs(
        this: &mut Conversation,
        update: Update,
        stopping: bool,
        finished: bool,
        cx: &mut Context<Conversation>,
    ) {
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
            Self::append_question(this, request, cx);
        }
        if !finished && !this.approvals.is_empty() {
            this.status = "Waiting for approval".into();
        } else if !finished && !this.questions.is_empty() {
            this.status = "Waiting for your answer".into();
        }
    }

    fn append_question(
        this: &mut Conversation,
        request: vorkspace_core::agent::Question,
        cx: &mut Context<Conversation>,
    ) {
        let card = cx.new(|cx| crate::composer::questions::QuestionCard::new(request, cx));
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
                    this.status = "The provider is no longer waiting for this answer".into();
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
                cx.emit(SessionChanged::Attention);
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
                cx.emit(SessionChanged::Attention);
                cx.notify();
            },
        );
        this.questions.push_back((card, vec![events, keep_open]));
        this.status = "Waiting for your answer".into();
    }

    fn apply_settlement(
        &self,
        this: &mut Conversation,
        settlement: TurnSettlement,
        stopping: bool,
        history_recovery: Option<HistoryRecovery>,
        cx: &mut Context<Conversation>,
    ) {
        let outcome = settlement.outcome;

        this.summary.updated_at = vorkspace_core::time::now_ms();
        this.turn_state = TurnState::Settled(outcome);

        this.preparing_worktree = false;
        if !stopping && let Some(limit) = settlement.usage_limit {
            this.set_usage_limit(Some(limit), cx);
        }
        this.detect_usage_limit(cx);
        this.review
            .update(cx, |review, cx| review.set_busy(false, cx));
        this.follow_ups.ready = false;
        if this.follow_ups.resuming {
            this.follow_ups.resuming = false;
            if !stopping && outcome == TurnOutcome::Completed {
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
        if outcome == TurnOutcome::Failed && !this.follow_ups.state.messages.is_empty() {
            this.follow_ups.state.paused = true;
            if history_recovery.is_none() {
                this.save_follow_ups(cx);
            }
        }
        this.approvals.clear();
        this.questions.clear();
        this.controls = None;
        this.refresh_submission_recovery(cx);
        this.transcript.update(cx, |v, cx| v.streaming(false, cx));
        let model_id = self.turn_model_id.clone();
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
        } else if history_recovery.is_none() {
            this.finish_orchestration_plan(cx);
        }
        if history_recovery.is_some() {
            this.recover_history_in_place(cx);
        } else {
            this.settings.turns = this.transcript.read(cx).turns.clone();
            this.save_settings(cx);
            this.run_history_actions(cx);
        }
        this.refresh_git(cx);
        cx.emit(SessionChanged::Settled);
    }
}
