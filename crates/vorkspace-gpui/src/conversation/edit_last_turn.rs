//! Last-turn editing recalls first, and rewinds only when the user resubmits.
use super::*;
use std::path::PathBuf;
use vorkspace_core::{
    provider,
    store::EditSnapshot,
    transcript::{Row, Transcript},
};

#[derive(Default)]
pub(super) struct State {
    active: bool,
    prepared: Option<EditSnapshot>,
    /// A confirmed provider mutation followed by a storage conflict requires a
    /// history reload, rather than silently appending to inconsistent context.
    blocked: bool,
}
impl State {
    pub(super) fn pending(&self) -> bool {
        self.active || self.prepared.is_some() || self.blocked
    }
}

enum EditOutcome {
    Rewound(Box<EditSnapshot>, String),
    Recovered,
}

fn latest_user(rows: &[Row]) -> Option<usize> {
    rows.iter().rposition(|row| row.first && row.role == "user")
}
impl Conversation {
    pub(super) fn cancel_edit_last_turn(&mut self, cx: &mut Context<Self>) -> bool {
        if self.editing_last_turn.active || self.editing_last_turn.blocked {
            return false;
        }
        if self.editing_last_turn.prepared.take().is_none() {
            return false;
        }
        self.status = "Ready".into();
        self.run_history_actions(cx);
        cx.notify();
        true
    }
    pub(super) fn defer_edit_transfer(
        &mut self,
        settings: Settings,
        prompt: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.history_actions.push_back(Box::new(move |this, cx| {
            if let Some(prompt) = prompt {
                if this.preserve_new_composer(cx) {
                    this.install_transfer(settings, None, cx);
                    this.enqueue_restored_prompt(prompt, cx);
                } else {
                    this.install_transfer(settings, Some(prompt), cx);
                }
            } else {
                this.install_transfer(settings, None, cx);
            }
        }));
        self.status = "Finish or cancel the current message edit to continue".into();
        cx.notify();
    }
    fn can_prepare_edit(&self, cx: &gpui::App) -> bool {
        self.history_ready()
            && !self.busy
            && !self.stopping
            && !self.editing_last_turn.pending()
            && self.settings.bot.is_none()
            && !self.settings.worktree_removed
            && provider::supports_rewind(Kind::from_id(&self.summary.provider))
            && !self.account_switching
            && !self.preparing_worktree
            && !self.attaching
            && !self.planning_orchestration
            && self.orchestration_progress.is_none()
            && !self.needs_approval()
            && self.questions.is_empty()
            && self.controls.is_none()
            && self.follow_ups.state.messages.is_empty()
            && self.follow_ups.state.draft.is_none()
            && self.follow_ups.edit.is_none()
            && self.follow_ups.sending.is_none()
            && self.follow_ups.requested.is_none()
            && self.follow_ups.state.revision == self.follow_ups.persisted
            && self.input.read(cx).content.is_empty()
            && !self.input.read(cx).is_composing()
            && self.attachments.is_empty()
    }
    pub(super) fn begin_edit_last_turn(
        &mut self,
        turn: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.reject_user_entry(cx) {
            return;
        }

        if self.is_remote() {
            self.status = "Editing the last message is not supported by this remote host.".into();
            cx.notify();
            return;
        }
        // The action is tied to a painted source row. A history restoration may
        // replace that row, so never replay this intent against an empty cache.
        if !self.can_prepare_edit(cx) {
            return;
        }
        let rows = self.transcript.read(cx).source_rows().to_vec();
        if latest_user(&rows) != Some(turn) {
            return;
        }
        let exact = self
            .settings
            .turns
            .iter()
            .max_by_key(|(row, _)| **row)
            .and_then(|(_, info)| info.provider_turn_id.clone());
        self.editing_last_turn.active = true;
        self.status = "Preparing edit…".into();
        self.response_menu = None;
        let store = self.store.clone();
        let id = self.summary.id;
        let binding = self.summary.provider_id.clone();
        let work = cx.background_executor().spawn(async move {
            locked(&store)?.prepare_edit_last_turn(id, &rows, exact.as_deref())
        });
        window.focus(&self.input.focus_handle(cx), cx);
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.editing_last_turn.active = false;
                match result {
                    Ok(edit)
                        if this.summary.provider_id == binding
                            && edit.matches(this.transcript.read(cx).source_rows())
                            && this.input.read(cx).content.is_empty()
                            && this.attachments.is_empty()
                            && !this.busy
                            && !this.input.read(cx).is_composing() =>
                    {
                        this.input
                            .update(cx, |input, cx| input.set_text(edit.prompt.clone(), cx));
                        this.attachments = edit.attachments.clone();
                        this.editing_last_turn.prepared = Some(edit);
                        this.status = "Editing last message — send to replace its turn".into();
                    }
                    Ok(_) => {
                        this.status = "Composer or conversation changed; edit cancelled".into()
                    }
                    Err(error) => {
                        this.status = format!("Could not edit last message: {error}");
                        this.refresh_if_stale(cx);
                    }
                }
                if !this.editing_last_turn.pending() {
                    this.run_history_actions(cx);
                }
                cx.emit(SessionChanged);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Called before ordinary send consumes composer text or attachments.
    /// Returns true while this edit owns the submission (including rejection).
    pub(super) fn submit_edited(&mut self, cx: &mut Context<Self>) -> bool {
        if self.reject_user_entry(cx) {
            return true;
        }

        if self.editing_last_turn.active || self.editing_last_turn.blocked {
            return true;
        }
        let recovery =
            locked(&self.store).and_then(|store| store.edit_recovery_reason(self.summary.id));
        match recovery {
            Ok(Some(reason)) => {
                self.editing_last_turn.blocked = true;
                self.status = reason;
                cx.notify();
                return true;
            }
            Err(error) => {
                self.status = format!(
                    "Could not verify conversation history: {error}. Your draft is preserved."
                );
                cx.notify();
                return true;
            }
            Ok(None) => {}
        }
        let Some(edit) = self.editing_last_turn.prepared.clone() else {
            return false;
        };
        if self.input.read(cx).content.is_empty() && self.attachments.is_empty() {
            // Clearing the recalled prompt cancels the edit without touching
            // either provider or history. The next new prompt sends normally.
            self.editing_last_turn.prepared = None;
            self.status = "Ready".into();
            self.run_history_actions(cx);
            cx.notify();
            return true;
        }
        let text = self.input.read(cx).content.trim();
        if text.eq_ignore_ascii_case("/compact")
            || text.eq_ignore_ascii_case("/mcp")
            || ["btw", "draft", "add-to-folder"]
                .iter()
                .any(|command| vorkspace_core::composer::consume_command(text, command).is_some())
            || self.draft_active(cx)
        {
            self.status = "Cancel message editing before running a local command".into();
            cx.notify();
            return true;
        }
        if !self.input.read(cx).has_message_text() && self.attachments.is_empty() {
            return true;
        }
        if self.busy
            || self.stopping
            || self.attaching
            || self.account_switching
            || self.preparing_worktree
            || self.settings.worktree_removed
            || self.input.read(cx).is_composing()
            || !self.follow_ups.state.messages.is_empty()
            || self.follow_ups.state.draft.is_some()
            || self.follow_ups.edit.is_some()
            || self.follow_ups.sending.is_some()
            || self.follow_ups.state.revision != self.follow_ups.persisted
        {
            self.status = "Finish the current action before submitting the edit".into();
            cx.notify();
            return true;
        }
        if !edit.matches(self.transcript.read(cx).source_rows())
            || self.summary.provider_id.as_deref() != Some(edit.provider_id.as_str())
            || self.summary.provider != edit.provider()
            || self.settings.account.id != edit.account_id
            || self.execution_cwd() != edit.cwd
        {
            self.editing_last_turn.prepared = None;
            self.persist_composer(cx);
            self.status =
                "The saved turn changed. Your edited text remains in the composer.".into();
            self.recover_history_in_place(cx);
            return true;
        }
        let kind = Kind::from_id(&self.summary.provider);
        self.attach_account(cx);
        let mut settings = self.settings.clone();
        settings.environment.extend(self.app_environment.clone());
        let cwd = PathBuf::from(self.execution_cwd());
        let model = self.model.clone();
        let prompt = self.input.read(cx).content.to_string();
        let attachments = self.attachments.clone();
        let checked_attachments = attachments.clone();
        let mut competing = Transcript::default();
        competing.begin("user");
        competing.append(&prompt);
        competing.attach_files(&attachments);
        let attempt = vorkspace_core::session::new_id();
        self.editing_last_turn.active = true;
        // Keep the visible draft in place until confirmation, including on a
        // rejected RPC. IME composition was checked before making it read-only.
        self.input.update(cx, |input, _| input.set_read_only(true));
        self.status = "Rewinding last turn…".into();
        let store = self.store.clone();
        let id = self.summary.id;
        let work = cx.background_executor().spawn(async move {
            vorkspace_core::composer::validate_attachments(&checked_attachments)?;
            {
                let store = locked(&store)?;
                if let Err(error) = store.validate_edit_last_turn(id, &edit) {
                    if !store.uses_vorkspace_schema() {
                        return Err(error);
                    }
                    store.preserve_turn_overlap(id, &attempt, &competing.rows, None)?;
                    return Ok(EditOutcome::Recovered);
                }
            }
            let binary = provider::resolve(kind).ok_or_else(|| {
                format!("{} executable was not found", kind.label())
            })?;
            let request = provider::Request {
                kind,
                binary,
                cwd,
                prompt: String::new(),
                provider_id: Some(edit.provider_id.clone()),
                model,
                settings,
            };
            let binding = match provider::rewind_last_turn(request, edit.provider_turn_id.as_deref()) {
                Ok(binding) => binding,
                Err(error) => {
                    if error.provider_may_have_changed {
                        locked(&store)?.mark_edit_recovery_required(id)?;
                        return Err(format!("Provider recovery required: {error}"));
                    }
                    return Err(error.to_string());
                }
            };
            let mut store = locked(&store)?;
            match store.finish_edit_last_turn(id, &edit, &binding) {
                Ok(()) => Ok(EditOutcome::Rewound(Box::new(edit), binding)),
                Err(error) if !store.uses_vorkspace_schema() => Err(format!(
                    "Provider recovery required: Provider rewound, but local history changed: {error}"
                )),
                Err(_) => {
                    store.preserve_turn_overlap(id, &attempt, &competing.rows, None)
                        .and_then(|()| store.recover_confirmed_rewind(id, &edit, &binding))
                        .map_err(|error| format!("Provider recovery required: {error}"))?;
                    Ok(EditOutcome::Recovered)
                }
            }
        });
        cx.spawn(async move |this,cx| {
            let result: Result<EditOutcome,String> = work.await;
            let _ = this.update(cx,|this,cx| {
                this.editing_last_turn.active = false;
                this.input.update(cx,|input,_| input.set_read_only(false));
                match result {
                    Ok(EditOutcome::Rewound(edit,binding)) => {
                        this.editing_last_turn.prepared = None;
                        this.summary.provider_id = Some(binding);
                        this.settings.turns.retain(|&row,_| row < edit.start);
                        this.transcript.update(cx,|view,cx| view.replace_history(edit.rows[..edit.start].to_vec(),cx));
                        this.summary.has_messages = edit.start > 0;
                        this.context = None;
                        if this.input.read(cx).content.as_str() == prompt && this.attachments == attachments {
                            // Normal submit still owns mode commands, attachments,
                            // account checks, durable acceptance and failure recovery.
                            this.send(cx);
                        } else {
                            // Programmatic composer updates can bypass read-only.
                            // Preserve both the new composer and the accepted edit.
                            this.preserve_interrupted_edit(prompt,attachments,edit.start,cx);
                        }
                        this.run_history_actions(cx);
                    }
                    Ok(EditOutcome::Recovered) => {
                        this.editing_last_turn.prepared = None;
                        this.persist_composer(cx);
                        this.status = "Saved both versions. Review your draft before sending.".into();
                        this.recover_history_in_place(cx);
                    }
                    Err(error) => {
                        if error.starts_with("Provider recovery required:") {
                            this.editing_last_turn.blocked = true;
                            this.editing_last_turn.prepared = None;
                            this.status = format!("{error}. Start a new conversation; your edited draft is preserved here.");
                        } else {
                            this.status = format!("Could not rewind last turn: {error}. Your edited prompt is preserved.");
                        }
                    }
                }
                cx.emit(SessionChanged); cx.notify();
            });
        }).detach();
        cx.notify();
        true
    }
    fn preserve_interrupted_edit(
        &mut self,
        text: String,
        attachments: Vec<PathBuf>,
        at: usize,
        cx: &mut Context<Self>,
    ) {
        let message = vorkspace_core::follow_up::Message {
            id: format!(
                "edited-{}-{}",
                self.summary.id,
                vorkspace_core::time::now_ms()
            ),
            text,
            attachments,
            at,
            ..Default::default()
        };
        if self.follow_ups.state.draft.is_none() {
            self.follow_ups.state.draft = Some(message);
        } else {
            self.follow_ups.state.paused = true;
            self.follow_ups.state.messages.push(message);
        }
        self.save_follow_ups(cx);
        self.status = "Last turn rewound; the edited prompt was saved as a draft because the composer changed".into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use std::sync::Mutex;
    use vorkspace_core::{store::Store, transcript::Transcript};
    #[gpui::test]
    fn stale_last_turn_edit_is_recovered_before_provider_lookup(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.db");
        let mut store = Store::open_vorkspace(&path).unwrap();
        let mut session = store.create("codex", "/project").unwrap();
        let id = session.id;
        store.provider_id(id, "thread").unwrap();
        session.provider_id = Some("thread".into());
        let mut history = Transcript::default();
        history.begin("user");
        history.append("Original prompt");
        history.begin("assistant");
        history.append("Original answer");
        store.write_rows(id, 0, &history.rows).unwrap();
        let edit = store
            .prepare_edit_last_turn(id, &history.rows, Some("turn"))
            .unwrap();
        let mut other = Store::open_vorkspace(&path).unwrap();
        let mut saved = other.load(id).unwrap();
        saved.rows[1].text = "New saved answer".into();
        other.write_rows(id, 1, &saved.rows[1..]).unwrap();

        let chat = cx.new(|cx| Conversation::new(session, Arc::new(Mutex::new(store)), cx));
        chat.update(cx, |chat, cx| {
            chat.loaded = true;
            chat.transcript
                .update(cx, |view, cx| view.replace_history(history.rows, cx));
            chat.editing_last_turn.prepared = Some(edit);
            chat.input.update(cx, |input, cx| {
                input.set_text("Edited but unsent".into(), cx)
            });
            assert!(chat.submit_edited(cx));
        });
        let executor = cx.foreground_executor().clone();
        executor.block_test(cx.condition(&chat, |chat, cx| {
            !chat.loading
                && !chat.editing_last_turn.pending()
                && chat.transcript.read(cx).source_rows()[1].text == "New saved answer"
        }));
        cx.run_until_parked();
        chat.read_with(cx, |chat, cx| {
            assert_eq!(chat.input.read(cx).content.as_str(), "Edited but unsent");
            assert_eq!(chat.transcript.read(cx).source_rows(), saved.rows);
            assert_eq!(chat.summary.provider_id.as_deref(), Some("thread"));
        });
        let reopened = Store::read_vorkspace(&path).unwrap();
        let versions = reopened.recovered_turns(id).unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(
            reopened
                .read_recovered_turn(id, &versions[0].id)
                .unwrap()
                .unwrap()[0]
                .text,
            "Edited but unsent"
        );
    }

    #[gpui::test]
    fn cancelled_recall_preserves_history_and_native_provider_binding(cx: &mut TestAppContext) {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let mut session = store.create("codex", "/project").unwrap();
        store.provider_id(session.id, "thread").unwrap();
        session.provider_id = Some("thread".into());
        let mut transcript = Transcript::default();
        transcript.begin("user");
        transcript.append("Original");
        transcript.begin("assistant");
        transcript.append("Answer");
        store.write_rows(session.id, 0, &transcript.rows).unwrap();
        let edit = store
            .prepare_edit_last_turn(session.id, &transcript.rows, Some("turn"))
            .unwrap();
        let chat = cx.new(|cx| Conversation::new(session, Arc::new(Mutex::new(store)), cx));
        chat.update(cx, |chat, cx| {
            chat.loaded = true;
            chat.transcript
                .update(cx, |view, cx| view.patch(0, transcript.rows.clone(), cx));
            chat.editing_last_turn.prepared = Some(edit);
            assert!(chat.submit_edited(cx));
            assert!(!chat.editing_last_turn.pending());
            assert_eq!(chat.transcript.read(cx).source_rows(), transcript.rows);
            assert_eq!(chat.summary.provider_id.as_deref(), Some("thread"));
            assert_eq!(
                locked(&chat.store)
                    .unwrap()
                    .load(chat.summary.id)
                    .unwrap()
                    .rows,
                transcript.rows
            );
        });
    }
    #[gpui::test]
    fn newer_composer_and_saved_draft_survive_interrupted_edit_recovery(cx: &mut TestAppContext) {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let session = store.create("codex", "/project").unwrap();
        let chat = cx.new(|cx| Conversation::new(session, Arc::new(Mutex::new(store)), cx));
        chat.update(cx, |chat, cx| {
            chat.loaded = true;
            chat.input
                .update(cx, |input, cx| input.set_text("New draft".into(), cx));
            chat.attachments.push("/tmp/new.png".into());
            chat.follow_ups.state.draft = Some(vorkspace_core::follow_up::Message {
                id: "saved".into(),
                text: "Existing draft".into(),
                ..Default::default()
            });
            chat.preserve_interrupted_edit("Edited prompt".into(), Vec::new(), 0, cx);
            assert_eq!(chat.input.read(cx).content, "New draft");
            assert_eq!(chat.attachments, vec![PathBuf::from("/tmp/new.png")]);
            assert_eq!(
                chat.follow_ups.state.draft.as_ref().unwrap().text,
                "Existing draft"
            );
            assert_eq!(chat.follow_ups.state.messages[0].text, "Edited prompt");
            assert!(chat.follow_ups.state.paused);
        });
    }
    #[gpui::test]
    fn local_commands_and_cancel_never_remove_recalled_history(cx: &mut TestAppContext) {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let mut session = store.create("codex", "/project").unwrap();
        store.provider_id(session.id, "thread").unwrap();
        session.provider_id = Some("thread".into());
        let mut transcript = Transcript::default();
        transcript.begin("user");
        transcript.append("Original");
        transcript.begin("assistant");
        transcript.append("Answer");
        store.write_rows(session.id, 0, &transcript.rows).unwrap();
        let edit = store
            .prepare_edit_last_turn(session.id, &transcript.rows, Some("turn"))
            .unwrap();
        let chat = cx.new(|cx| Conversation::new(session, Arc::new(Mutex::new(store)), cx));
        chat.update(cx, |chat, cx| {
            chat.loaded = true;
            chat.transcript
                .update(cx, |view, cx| view.patch(0, transcript.rows.clone(), cx));
            chat.editing_last_turn.prepared = Some(edit);
            chat.input
                .update(cx, |input, cx| input.set_text("/compact".into(), cx));
            assert!(chat.submit_edited(cx));
            assert!(chat.editing_last_turn.pending());
            assert_eq!(chat.input.read(cx).content, "/compact");
            assert!(chat.cancel_edit_last_turn(cx));
            assert!(!chat.editing_last_turn.pending());
            assert_eq!(chat.input.read(cx).content, "/compact");
            assert_eq!(
                locked(&chat.store)
                    .unwrap()
                    .load(chat.summary.id)
                    .unwrap()
                    .rows,
                transcript.rows
            );
        });
    }
    #[gpui::test]
    fn persisted_recovery_guard_preserves_a_new_composer_before_normal_send(
        cx: &mut TestAppContext,
    ) {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let mut session = store.create("pi", "/project").unwrap();
        store.provider_id(session.id, "original").unwrap();
        session.provider_id = Some("original".into());
        let mut transcript = Transcript::default();
        transcript.begin("user");
        transcript.append("Original");
        store.write_rows(session.id, 0, &transcript.rows).unwrap();
        let edit = store
            .prepare_edit_last_turn(session.id, &transcript.rows, None)
            .unwrap();
        store.provider_id(session.id, "changed").unwrap();
        assert!(
            store
                .finish_edit_last_turn(session.id, &edit, "fork")
                .is_err()
        );
        let chat = cx.new(|cx| Conversation::new(session, Arc::new(Mutex::new(store)), cx));
        chat.update(cx, |chat, cx| {
            chat.loaded = true;
            chat.input.update(cx, |input, cx| {
                input.set_text("Unsent edited prompt".into(), cx)
            });
            assert!(chat.submit_edited(cx));
            assert!(chat.editing_last_turn.blocked);
            assert_eq!(chat.input.read(cx).content, "Unsent edited prompt");
            assert!(!chat.cancel_edit_last_turn(cx));
        });
    }
}
