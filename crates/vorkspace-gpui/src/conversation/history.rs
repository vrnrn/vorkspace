//! Release inactive transcript payloads while the composer and provider live on.
use super::*;

pub(super) type HistoryAction = Box<dyn FnOnce(&mut Conversation, &mut Context<Conversation>)>;

pub(super) enum InitialHistory {
    Complete(crate::transcript::PreparedHistory),
    Window(
        Box<vorkspace_core::store::IndexedHistoryPage>,
        crate::transcript::PreparedHistory,
    ),
}
pub(super) enum InitialRows {
    Complete(Vec<vorkspace_core::transcript::Row>),
    Window(Box<vorkspace_core::store::IndexedHistoryPage>),
}

impl Conversation {
    pub(super) fn history_ready(&self) -> bool {
        self.loaded && !self.history_unloaded && !self.history_partial && !self.loading
    }
    pub(crate) fn needs_history_restore(&self) -> bool {
        self.history_unloaded
            || self.history_partial
            || self.loading
            || (!self.loaded && self.summary.has_messages)
    }
    pub(crate) fn defer_history_action(
        &mut self,
        action: impl FnOnce(&mut Conversation, &mut Context<Conversation>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.history_actions.push_back(Box::new(action));
        self.load(cx);
    }
    pub(super) fn run_history_actions(&mut self, cx: &mut Context<Self>) {
        while self.history_ready() && !self.stopping && !self.editing_last_turn.pending() {
            let Some(action) = self.history_actions.pop_front() else {
                break;
            };
            action(self, cx);
        }
    }

    pub(super) fn preserve_new_composer(&self, cx: &gpui::App) -> bool {
        self.settings.bot.is_none()
            && (!self.input.read(cx).content.is_empty() || !self.attachments.is_empty())
    }
    pub(super) fn enqueue_restored_prompt(&mut self, prompt: String, cx: &mut Context<Self>) {
        // The accepted prompt predates this composer draft. Queue it without
        // taking any files the user attached while history was restoring.
        let attachments = std::mem::take(&mut self.attachments);
        self.attachments = std::mem::take(&mut self.settings.attachments);
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
        self.attachments = attachments;
    }
    pub(crate) fn history_loaded(&self) -> bool {
        self.loaded && !self.history_unloaded && !self.history_partial
    }
    pub(crate) fn history_weight(&self, cx: &gpui::App) -> usize {
        self.transcript.read(cx).history_weight()
    }
    /// Only a settled, saved conversation can lose its reconstructible payload.
    /// Keep drafts, queued messages and their edits in the live entity, and pin
    /// their history so row positions remain valid during background saves.
    pub(crate) fn unload_history(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.history_loaded()
            || self.loading
            || self.busy
            || self.stopping
            || self.editing_last_turn.pending()
            || self
                .btw
                .as_ref()
                .is_some_and(|sheet| sheet.read(cx).is_open() || sheet.read(cx).is_running())
            || self.status != "Ready"
            || self.settings.bot.is_some()
            || self.account_switching
            || self.preparing_worktree
            || self.planning_orchestration
            || self.orchestration_plan.is_some()
            || self.orchestration_progress.is_some()
            || self.settings.usage_limit.is_some()
            || !self.history_actions.is_empty()
            || self.controls.is_some()
            || self.needs_approval()
            || self.attaching
            || !self.attachments.is_empty()
            || !self.input.read(cx).content.is_empty()
            || self.input.read(cx).is_composing()
            || self.follow_ups.state.revision != self.follow_ups.persisted
            || !self.follow_ups.state.messages.is_empty()
            || self.follow_ups.state.draft.is_some()
            || self.follow_ups.edit.is_some()
            || self.follow_ups.sending.is_some()
            || self.follow_ups.requested.is_some()
            || self.pending_search.is_some()
            || self.response_request.is_some()
            || self.response_menu.is_some()
        {
            return false;
        }
        let suspended = self
            .transcript
            .update(cx, |view, cx| view.suspend_history(cx));
        self.history_unloaded = suspended;
        if suspended && self.is_remote() {
            self.release_remote_snapshot();
        }
        suspended
    }
    /// Restore only the payload. A history cache miss must not replace live
    /// settings, queued state, composer text, attachments or provider controls.
    pub(super) fn reload_history(&mut self, cx: &mut Context<Self>) {
        if self.is_remote() {
            self.refresh_remote_snapshot(cx);
            return;
        }
        if self.loading || !self.history_unloaded {
            return;
        }
        self.loading = true;
        let id = self.summary.id;
        let store = self.store.clone();
        let bot = self.settings.bot.is_some();
        let work = cx.background_executor().spawn(async move {
            let (history, turns, rev) = {
                let store = locked(&store)?;
                (
                    store.load(id)?,
                    store.agent_settings(id)?.unwrap_or_default().turns,
                    store.session_history_revision(id).unwrap_or(0),
                )
            };
            Ok::<_, String>((
                crate::transcript::PreparedHistory::new(history.rows, bot),
                turns,
                rev,
            ))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok((history, turns, rev)) => {
                        this.settings.turns = turns.clone();
                        let partial = this.history_partial;
                        this.transcript.update(cx, |view, cx| {
                            view.turns = turns;
                            if partial {
                                view.complete_storage_history(history, cx);
                            } else {
                                view.restore_prepared_history(history, cx);
                            }
                        });
                        this.loaded_history_revision = Some(rev);
                        this.history_unloaded = false;
                        this.history_partial = false;
                        this.status = "Ready".into();
                        if let Some((row, query)) = this.pending_search.take() {
                            this.reveal_search(row, query, cx);
                        }
                        this.run_history_actions(cx);
                        this.pump_follow_ups(cx);
                    }
                    Err(error) => this.status = error,
                }
                cx.emit(SessionChanged);
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests;
