//! Release inactive transcript payloads while the composer and provider live on.
use super::*;

pub(super) type HistoryAction = Box<dyn FnOnce(&mut Conversation, &mut Context<Conversation>)>;

pub(super) enum InitialHistory {
    Complete(crate::transcript::PreparedHistory),
    Window(
        vorkspace_core::store::IndexedHistoryPage,
        crate::transcript::PreparedHistory,
    ),
}
pub(super) enum InitialRows {
    Complete(Vec<vorkspace_core::transcript::Row>),
    Window(vorkspace_core::store::IndexedHistoryPage),
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
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use std::sync::Mutex;
    use vorkspace_core::{store::Store, transcript::Row};

    #[gpui::test]
    fn partial_history_restores_before_queued_mutation(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.db");
        let store = Store::open_vorkspace(&path).unwrap();
        let session = store.create("codex", "/project").unwrap();
        let id = session.id;
        let original = store.persisted_session_id(session.id).unwrap();
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(include_str!(
            "../../../vorkspace-core/tests/fixtures/bot-schema.sql"
        ))
        .unwrap();
        conn.execute("INSERT INTO bot_transcripts VALUES(?1)", [&original])
            .unwrap();
        for i in 0..50 {
            let role = if i % 2 == 0 { "user" } else { "assistant" };
            let block = serde_json::json!({"id": format!("block-{i}"), "role": role, "text": format!("message {i}")});
            conn.execute(
                "INSERT INTO bot_blocks VALUES(?1,?2,?3,?4,'',?5,0)",
                rusqlite::params![
                    original,
                    i + 1,
                    format!("block-{i}"),
                    block.to_string(),
                    i % 2 == 0
                ],
            )
            .unwrap();
            if i % 2 == 0 {
                conn.execute(
                    "INSERT INTO bot_turns VALUES(?1,?2)",
                    rusqlite::params![original, i + 1],
                )
                .unwrap();
            }
        }
        let store = Arc::new(Mutex::new(store));
        let chat = cx.new(|cx| Conversation::new(session, store.clone(), cx));
        chat.update(cx, |chat, cx| {
            chat.allow_storage_paging = true;
            chat.configure_bot(
                vorkspace_core::bot::Bot::default(),
                dir.path().to_owned(),
                Default::default(),
                cx,
            );
            chat.load(cx);
        });
        cx.run_until_parked();
        chat.update(cx, |chat, cx| {
            assert!(chat.history_partial);
            assert_eq!(chat.transcript.read(cx).source_rows().len(), 20);
            assert_eq!(chat.bot_row_origin(cx), 30);
            assert_eq!(chat.bot_row_count(cx), 50);
            assert!(!chat.history_ready());
            chat.defer_history_action(
                |chat, cx| {
                    let rows = chat.transcript.read(cx).source_rows();
                    assert_eq!(rows.len(), 50);
                    let row = Row {
                        role: "user".into(),
                        text: "new message".into(),
                        first: true,
                        sent_at: None,
                    };
                    locked(&chat.store)
                        .unwrap()
                        .write_rows(chat.summary.id, rows.len(), &[row])
                        .unwrap();
                },
                cx,
            );
        });
        cx.run_until_parked();
        chat.update(cx, |chat, _| {
            assert!(chat.history_ready());
            assert!(!chat.history_partial);
        });
        let rows = store.lock().unwrap().load(id).unwrap().rows;
        assert_eq!(rows.len(), 51);
        assert_eq!(rows[0].text, "message 0");
        assert_eq!(rows[50].text, "new message");
        chat.update(cx, |chat, cx| {
            chat.transcript.update(cx, |view, cx| {
                view.patch(
                    50,
                    vec![Row {
                        role: "user".into(),
                        text: "unsaved needle".into(),
                        first: true,
                        sent_at: None,
                    }],
                    cx,
                )
            });
            let hits = vorkspace_core::search::messages(
                &chat.summary,
                chat.transcript.read(cx).source_rows(),
                "unsaved needle",
                &std::sync::atomic::AtomicBool::new(false),
            );
            assert!(hits.iter().any(|hit| hit.target
                == vorkspace_core::search::Target::Message {
                    session: id,
                    row: 50
                }));
        });
        let reopened = store
            .lock()
            .unwrap()
            .sessions()
            .unwrap()
            .into_iter()
            .find(|session| session.id == id)
            .unwrap();
        let second = cx.new(|cx| Conversation::new(reopened, store.clone(), cx));
        second.update(cx, |chat, cx| {
            chat.allow_storage_paging = true;
            chat.configure_bot(
                vorkspace_core::bot::Bot::default(),
                dir.path().to_owned(),
                Default::default(),
                cx,
            );
            chat.load(cx);
        });
        cx.run_until_parked();
        second.update(cx, |chat, _| assert!(chat.history_partial));
        conn.execute("UPDATE bot_blocks SET block_json=json_set(block_json,'$.text','external needle') WHERE session_id=?1 AND seq=1", [&original]).unwrap();
        second.update(cx, |chat, cx| {
            chat.reveal_search(0, "external needle".into(), cx)
        });
        cx.run_until_parked();
        second.update(cx, |chat, cx| {
            assert!(chat.history_ready());
            assert_eq!(
                chat.transcript.read(cx).source_rows()[0].text,
                "external needle"
            );
            assert!(chat.pending_search.is_none());
        });
    }

    #[gpui::test]
    fn history_eviction_keeps_live_state_and_pins_drafts_running_and_failed_saves(
        cx: &mut TestAppContext,
    ) {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let session = store.create("codex", "/project").unwrap();
        let chat = cx.new(|cx| Conversation::new(session, Arc::new(Mutex::new(store)), cx));
        chat.update(cx, |chat, cx| {
            chat.loaded = true;
            chat.status = "Ready".into();
            chat.settings.revision = 7;
            chat.settings.model = Some("chosen-model".into());
            chat.transcript.update(cx, |view, cx| {
                view.visible(false, cx);
                view.patch(
                    0,
                    vec![Row {
                        role: "assistant".into(),
                        text: "Saved answer".into(),
                        first: true,
                        sent_at: None,
                    }],
                    cx,
                );
            });
            chat.busy = true;
            assert!(!chat.unload_history(cx));
            chat.busy = false;
            chat.status = "Could not save history".into();
            assert!(!chat.unload_history(cx));
            chat.status = "Ready".into();
            chat.attachments.push("/tmp/unsent-image.png".into());
            assert!(!chat.unload_history(cx));
            chat.attachments.clear();
            chat.input
                .update(cx, |input, cx| input.set_text("Unsent draft".into(), cx));
            assert!(!chat.unload_history(cx));
            assert_eq!(chat.input.read(cx).content, "Unsent draft");
            chat.input
                .update(cx, |input, cx| input.set_text(String::new(), cx));
            chat.follow_ups.state.revision = 2;
            chat.follow_ups.persisted = 1;
            assert!(!chat.unload_history(cx));
            chat.follow_ups.persisted = 2;
            assert!(chat.unload_history(cx));
            assert!(!chat.history_loaded());
            assert!(chat.loaded, "composer/settings lifecycle remains loaded");
            assert_eq!(chat.settings.revision, 7);
            assert_eq!(chat.settings.model.as_deref(), Some("chosen-model"));
            assert_eq!(chat.follow_ups.state.revision, 2);
            assert_eq!(chat.history_weight(cx), 0);
        });
    }

    #[gpui::test]
    fn hidden_mutations_wait_for_history_without_changing_settings_or_receipts(
        cx: &mut TestAppContext,
    ) {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let session = store.create("codex", "/project").unwrap();
        let chat = cx.new(|cx| Conversation::new(session, Arc::new(Mutex::new(store)), cx));
        chat.update(cx, |chat, cx| {
            chat.loaded = true;
            chat.history_unloaded = true;
            // Keep the test's restore under its own control; no provider runs.
            chat.loading = true;
            chat.settings.revision = 17;
            chat.settings.bot_message_id = Some("delivery-receipt".into());
            chat.model = Some("original-model".into());
            chat.input
                .update(cx, |input, cx| input.set_text("Unsent draft".into(), cx));
            let transfer = Settings {
                revision: 99,
                model: Some("new-model".into()),
                ..Default::default()
            };
            chat.quick_submit("Queued API prompt".into(), transfer, cx);
            assert!(!chat.busy);
            assert_eq!(chat.settings.revision, 17);
            assert_eq!(chat.model.as_deref(), Some("original-model"));
            assert_eq!(
                chat.settings.bot_message_id.as_deref(),
                Some("delivery-receipt")
            );
            assert_eq!(chat.input.read(cx).content, "Unsent draft");
            assert_eq!(chat.history_actions.len(), 1);
            assert!(!chat.ready_for_external(cx));
            assert!(chat.bot_post("assistant", "Report".into(), cx).is_err());
            assert!(chat.transcript.read(cx).source_rows().is_empty());
            // Replace the launch intent with observable, provider-free work to
            // verify FIFO actions cannot run against the empty suspended view.
            chat.history_actions.clear();
            chat.defer_history_action(
                |chat, cx| {
                    assert_eq!(
                        chat.transcript.read(cx).source_rows()[0].text,
                        "Restored history"
                    );
                    chat.settings.revision += 1;
                },
                cx,
            );
            chat.defer_history_action(
                |chat, _| {
                    assert_eq!(chat.settings.revision, 18);
                    chat.settings.revision += 1;
                },
                cx,
            );
            chat.run_history_actions(cx);
            assert_eq!(chat.settings.revision, 17);
            assert_eq!(chat.history_actions.len(), 2);
            chat.transcript.update(cx, |view, cx| {
                view.restore_history(
                    vec![Row {
                        role: "user".into(),
                        text: "Restored history".into(),
                        first: true,
                        sent_at: None,
                    }],
                    cx,
                )
            });
            chat.history_unloaded = false;
            chat.loading = false;
            chat.run_history_actions(cx);
            assert_eq!(chat.settings.revision, 19);
            assert!(chat.history_actions.is_empty());
            assert_eq!(
                chat.settings.bot_message_id.as_deref(),
                Some("delivery-receipt")
            );
            chat.follow_ups.state.paused = true;
            chat.attachments.push("/tmp/new-composer-image.png".into());
            assert!(chat.preserve_new_composer(cx));
            chat.enqueue_restored_prompt("Accepted before restore".into(), cx);
            assert_eq!(chat.input.read(cx).content, "Unsent draft");
            assert_eq!(
                chat.attachments,
                vec![std::path::PathBuf::from("/tmp/new-composer-image.png")]
            );
            assert_eq!(
                chat.follow_ups.state.messages[0].text,
                "Accepted before restore"
            );
            assert!(chat.follow_ups.state.messages[0].attachments.is_empty());
        });
    }

    #[gpui::test]
    fn restored_external_prompt_keeps_its_files_separate_from_new_composer_files(
        cx: &mut TestAppContext,
    ) {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let session = store.create("codex", "/project").unwrap();
        let chat = cx.new(|cx| Conversation::new(session, Arc::new(Mutex::new(store)), cx));
        chat.update(cx, |chat, cx| {
            chat.loaded = true;
            chat.follow_ups.state.paused = true;
            chat.input
                .update(cx, |input, cx| input.set_text("New draft".into(), cx));
            chat.attachments.push("/tmp/new-draft.png".into());
            chat.settings
                .attachments
                .push("/tmp/accepted-prompt.png".into());
            chat.enqueue_restored_prompt("Accepted prompt".into(), cx);
            assert_eq!(chat.input.read(cx).content, "New draft");
            assert_eq!(
                chat.attachments,
                vec![std::path::PathBuf::from("/tmp/new-draft.png")]
            );
            assert!(chat.settings.attachments.is_empty());
            let queued = &chat.follow_ups.state.messages[0];
            assert_eq!(queued.text, "Accepted prompt");
            assert_eq!(
                queued.attachments,
                vec![std::path::PathBuf::from("/tmp/accepted-prompt.png")]
            );
            assert!(!chat.busy);
        });
    }
}
