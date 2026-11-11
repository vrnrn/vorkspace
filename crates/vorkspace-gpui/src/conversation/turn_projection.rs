//! Apply an admitted turn's updates to a conversation without owning execution.
use super::*;
use vorkspace_core::store::ComposerOrigin;
use vorkspace_runtime::runtime::{TurnOutcome, TurnTransition, Update};

pub(super) struct TurnProjection {
    pub(super) generation: u64,
    source_provider: String,
    source_account: String,
    turn_model_id: Option<String>,
    pub(super) composer_origin: Option<ComposerOrigin>,
    pub(super) title_seed: Option<String>,
}

impl TurnProjection {
    pub(super) fn new(
        generation: u64,
        source_provider: String,
        source_account: String,
        turn_model_id: Option<String>,
    ) -> Self {
        Self {
            generation,
            source_provider,
            source_account,
            turn_model_id,
            composer_origin: None,
            title_seed: None,
        }
    }

    pub(super) fn apply(
        &mut self,
        this: &mut Conversation,
        update: Update,
        cx: &mut Context<Conversation>,
    ) {
        let source_provider = self.source_provider.as_str();
        let source_account = self.source_account.as_str();
        if this.submission_generation != self.generation {
            return;
        }
        let (accepted, settlement) = match update.transition {
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
        let source_selected =
            this.summary.provider == source_provider && this.settings.account.id == source_account;
        if !stopping
            && source_selected
            && (accepted || update.turn_ready || outcome == Some(TurnOutcome::Completed))
            && let Some(seed) = self.title_seed.take()
        {
            this.generate_title(&seed, cx);
        }
        let mut checkout_changed = false;
        if let Some(cwd) = update.worktree_cwd
            && (this.settings.worktree_cwd.as_ref() != Some(&cwd) || this.settings.new_worktree)
        {
            this.preparing_worktree = false;
            this.settings.worktree_cwd = Some(cwd);
            this.settings.new_worktree = false;
            this.save_settings(cx);
            this.refresh_git(cx);
            checkout_changed = true;
        }
        if let Some(commands) = update.commands.filter(|_| source_selected) {
            this.completion.update(cx, |completion, cx| {
                completion.set_native_commands(commands, cx)
            });
        }
        let history_recovery = settlement
            .as_ref()
            .and_then(|settlement| settlement.history_recovery);
        let activity_changed = this.activity != update.activity;
        this.activity = update.activity;
        let needed_approval = this.needs_approval();
        let incoming_notification_input =
            !stopping && (!update.approvals.is_empty() || !update.questions.is_empty());
        use vorkspace_runtime::runtime::TranscriptPatch;
        let patch = update.transcript;
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
        if let Some(id) = update.provider_turn_id {
            this.transcript
                .update(cx, |view, _| view.record_provider_turn(id));
        }
        if update.provider_reset && source_selected {
            this.summary.provider_id = None;
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
        if let Some(handoff) = update.handoff.filter(|_| source_selected) {
            this.settings.pending_provider_switch = None;
            this.settings.handoff = handoff;
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
                    if before != this.needs_approval() {
                        cx.emit(SessionChanged::Attention);
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
                    cx.emit(SessionChanged::Attention);
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
        if let Some(settlement) = settlement {
            let outcome = settlement.outcome;
            if this.is_remote() {
                this.remote_state.refreshing = false;
                this.remote_state.awaiting_successor = outcome == TurnOutcome::Superseded;
            }
            this.summary.updated_at = vorkspace_core::time::now_ms();
            this.turn_state = TurnState::Settled(outcome);
            if this.is_remote() {
                this.refresh_remote_snapshot(cx);
            }
            this.preparing_worktree = false;
            if !stopping && let Some(limit) = settlement.usage_limit {
                this.set_usage_limit(Some(limit), cx);
            }
            if !this.is_remote() {
                this.detect_usage_limit(cx);
            }
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
        } else {
            if checkout_changed {
                cx.emit(SessionChanged::Checkout);
            }
            if incoming_notification_input || needed_approval != this.needs_approval() {
                cx.emit(SessionChanged::Attention);
            }
        }
        if activity_changed {
            cx.emit(SessionActivityChanged);
        }
        this.pump_follow_ups(cx);
        cx.notify();
    }
}
