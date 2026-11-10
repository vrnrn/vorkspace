//! Composer commands are handled before taking a draft or entering the turn queue.
use super::*;
use vorkspace_core::provider;
use vorkspace_runtime::runtime::manual_compaction::{self, Started};

impl Conversation {
    /// Returns true when this input belongs to a local command, including failures.
    /// A rejected command keeps its draft and attachments intact.
    pub(super) fn handle_local_command(&mut self, cx: &mut Context<Self>) -> bool {
        let text = self.input.read(cx).content.trim();
        if text.eq_ignore_ascii_case("/mcp") {
            if self.input.read(cx).is_composing() {
                return true;
            }
            self.completion
                .update(cx, |completion, cx| completion.open_mcp(cx));
            return true;
        }
        if let Some(question) = vorkspace_core::composer::consume_command(text, "btw") {
            let question = question.trim().to_owned();
            self.open_btw(question, false, cx);
            return true;
        }
        if !text.eq_ignore_ascii_case("/compact") {
            return false;
        }
        self.compact_context(cx);
        true
    }

    pub(super) fn compact_context(&mut self, cx: &mut Context<Self>) {
        if self.reject_user_entry(cx) {
            return;
        }

        if !self.history_ready() {
            self.defer_history_action(|this, cx| this.compact_context(cx), cx);
            return;
        }
        if self.editing_last_turn.pending()
            || self.stopping
            || self.attaching
            || self.input.read(cx).is_composing()
            || self.account_switching
            || self.controls.is_some()
            || self.needs_approval()
            || !self.questions.is_empty()
        {
            return;
        }
        if self.busy || !self.follow_ups.state.messages.is_empty() {
            self.status =
                "Wait for the current turn and queued messages before compacting context.".into();
            cx.notify();
            return;
        }
        let kind = Kind::from_id(&self.summary.provider);
        if !provider::supports_compact(kind) {
            self.status = format!(
                "Context compaction is not supported by the native {} transport.",
                kind.label()
            );
            cx.notify();
            return;
        }
        if self.summary.provider_id.is_none() {
            self.status = "Send a message before compacting this chat.".into();
            cx.notify();
            return;
        }
        if self.settings.worktree_removed || self.preparing_worktree || !self.loaded {
            return;
        }
        self.attach_account(cx);
        let mut settings = self.settings.clone();
        settings.model = self.model.clone();
        settings.environment.extend(self.app_environment.clone());
        settings.disable_claude_hooks = cx
            .try_global::<vorkspace_ui::preferences::Preferences>()
            .is_some_and(|preferences| !preferences.claude_hooks);
        let source = self.summary.clone();
        let summary = source.clone();
        let account = settings.account.id.clone();
        let cwd = self.execution_cwd().to_owned();
        let view = cx.weak_entity();
        let Some(admission) = turn_driver::admit(&self.store, self.summary.id, view, cx) else {
            self.status = turn_driver::ALREADY_ACTIVE.into();
            cx.notify();
            return;
        };
        self.cancel = admission.cancel_handle();
        let cancel = self.cancel.clone();
        self.busy = true;
        self.submission_generation = self.submission_generation.wrapping_add(1);
        let generation = self.submission_generation;
        self.status = "Compacting context…".into();
        let compact_draft = self.input.read(cx).content.clone();
        let clear_command = compact_draft.trim().eq_ignore_ascii_case("/compact");
        let store = self.store.clone();
        self.completion
            .update(cx, |completion, cx| completion.dismiss(cx));
        let (publish, progress) = async_channel::bounded(1);
        let work = cx.background_executor().spawn(async move {
            manual_compaction::run(summary, settings, store, cancel, move |history| {
                let _ = publish.try_send(history);
            })
        });
        cx.spawn(async move |this, cx| {
            let source_selected = |chat: &Self| {
                chat.summary.provider == source.provider
                    && chat.summary.provider_id == source.provider_id
                    && chat.summary.cwd == source.cwd
                    && chat.settings.account.id == account
                    && !chat.settings.worktree_removed
                    && chat.execution_cwd() == cwd
            };
            let mut started = progress.recv().await.ok();
            let saw_start = started.is_some();
            let mut visible_row = None;
            let _ = this.update(cx, |this, cx| {
                if this.submission_generation == generation
                    && this.summary.id == source.id
                    && source_selected(this)
                    && !this.stopping
                    && let Some(history) = started.take()
                {
                    visible_row = history.rows.last().cloned();
                    this.install_compaction_history(history, cx);
                }
            });
            let finished = work.await;
            if !cx.update(|cx| admission.release(cx)) {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                if this.submission_generation != generation || this.summary.id != source.id {
                    return;
                }
                this.busy = false;
                this.stopping = false;
                let source_selected = source_selected(this);
                let mut refresh_history = finished.row.is_none() || !source_selected;
                if let Some((start, row)) = finished.row.filter(|_| source_selected) {
                    if let Some(mut history) = started {
                        // Stop may beat the first UI update. Install only the
                        // settled snapshot, including any imported prefix.
                        history.rows[start] = row;
                        this.install_compaction_history(history, cx);
                    } else if this.transcript.read(cx).source_rows().get(start)
                        == visible_row.as_ref()
                    {
                        this.transcript
                            .update(cx, |view, cx| view.patch(start, vec![row], cx));
                    } else {
                        refresh_history = true;
                    }
                }
                if let Some(commands) = finished.commands.filter(|_| source_selected) {
                    this.completion.update(cx, |completion, cx| {
                        completion.set_native_commands(commands, cx)
                    });
                }
                if let Some(context) = finished.context.filter(|_| source_selected) {
                    this.context = Some(context);
                }
                this.status = match finished.result {
                    Ok(()) => {
                        if source_selected
                            && clear_command
                            && this.input.read(cx).content == compact_draft
                        {
                            this.input.update(cx, |input, cx| {
                                input.take(cx);
                            });
                        }
                        "Ready".into()
                    }
                    Err(error) if error == "Stopped" => "Ready".into(),
                    Err(error) => error,
                };
                if refresh_history {
                    if saw_start || this.settings.bot.is_none() {
                        this.refresh_if_stale(cx);
                    } else {
                        this.sync_native_history(cx);
                    }
                }
                this.refresh_submission_recovery(cx);
                if this.status == "Ready" {
                    this.run_history_actions(cx);
                    this.pump_follow_ups(cx);
                }
                cx.emit(SessionChanged::Loaded);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn install_compaction_history(&mut self, history: Started, cx: &mut Context<Self>) {
        self.settings.turns = history.turns.clone();
        let len = history.rows.len();
        self.transcript.update(cx, |view, cx| {
            view.turns = history.turns;
            view.patch_snapshot(0, history.rows, len, cx);
        });
    }
}
