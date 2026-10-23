use super::*;
impl Conversation {
    pub(super) fn refresh_submission_recovery(&mut self, cx: &mut Context<Self>) {
        if self.is_remote() || self.reviewing_submission {
            return;
        }
        self.submission_recovery_epoch = self.submission_recovery_epoch.wrapping_add(1);
        let epoch = self.submission_recovery_epoch;
        let store = self.store.clone();
        let id = self.summary.id;
        let work = cx.background_executor().spawn(async move {
            let store = locked(&store)?;
            store.recover_local_submissions()?;
            store.latest_unconfirmed_submission(id)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.install_submission_recovery(epoch, result, cx);
                cx.notify();
            });
        })
        .detach();
    }
    fn install_submission_recovery(
        &mut self,
        epoch: u64,
        result: Result<Option<vorkspace_core::store::SubmissionReceipt>, String>,
        cx: &mut Context<Self>,
    ) {
        if self.submission_recovery_epoch != epoch {
            return;
        }
        match result {
            Ok(receipt) => self.unconfirmed_submission = receipt,
            Err(error) => self.status = format!("Cannot read local send recovery: {error}"),
        }
        cx.notify();
    }
    pub(super) fn local_submission_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let receipt = self.unconfirmed_submission.as_ref()?;
        let snapshot = receipt.user_turn.as_ref();
        let preview: String = snapshot
            .and_then(|s| s["text"].as_str())
            .unwrap_or("Saved send")
            .chars()
            .take(160)
            .collect();
        let files = snapshot
            .and_then(|s| s["attachments"].as_array())
            .map_or(0, Vec::len);
        let files_pending = self.attaching || !self.pending_attachments.is_empty();
        let occupied =
            self.input.read(cx).has_message_text() || !self.attachments.is_empty() || files_pending;
        let disabled = self.reviewing_submission || self.busy;
        let draft_ready = self.composer_draft.ready_for_replacement();
        Some(div().px_3().pt_3().flex().flex_col().gap_2().text_size(px(12.)).text_color(content(0.8))
            .child("A local send ended without confirmation. The provider may have received it; it was not replayed.")
            .child(format!("{preview} ({files} attachments)"))
            .when(occupied, |d|d.child(if files_pending { "Wait for selected files to finish attaching." } else { "Clear the composer to restore a copy." }))
            .child(div().flex().gap_2()
                .child(crate::settings::widgets::secondary_button("submission-restore","Restore a copy",true,disabled || !draft_ready || occupied || snapshot.is_none())
                    .child("Restore a copy").on_click(cx.listener(|this,_,_,cx|this.review_local_submission(true,cx))))
                .child(crate::settings::widgets::secondary_button("submission-dismiss","Dismiss notice",false,disabled)
                    .child("Dismiss notice").on_click(cx.listener(|this,_,_,cx|this.review_local_submission(false,cx)))))
            .into_any_element())
    }
    fn review_local_submission(&mut self, restore: bool, cx: &mut Context<Self>) {
        if self.reviewing_submission || self.busy {
            return;
        }
        let Some(receipt) = self.unconfirmed_submission.as_ref() else {
            return;
        };
        let attempt = receipt.attempt_id.clone();
        if restore {
            if !self.composer_draft.ready_for_replacement()
                || self.input.read(cx).has_message_text()
                || !self.attachments.is_empty()
                || self.attaching
                || !self.pending_attachments.is_empty()
            {
                return;
            }
            let Some(snapshot) = receipt.user_turn.as_ref() else {
                return;
            };
            let Some(text) = snapshot["text"].as_str() else {
                self.status = "Saved send text is unreadable; its receipt was preserved".into();
                return;
            };
            self.attachments = snapshot["attachments"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|a| a["path"].as_str().map(std::path::PathBuf::from))
                .collect();
            self.input
                .update(cx, |input, cx| input.set_text(text.into(), cx));
            self.persist_composer(cx);
        }
        let writer = self.composer_draft.writer.clone();
        let store = self.store.clone();
        let id = self.summary.id;
        self.submission_recovery_epoch = self.submission_recovery_epoch.wrapping_add(1);
        let epoch = self.submission_recovery_epoch;
        self.reviewing_submission = true;
        let work = cx.background_executor().spawn(async move {
            // Publish the explicitly requested copy before hiding its recovery notice.
            if restore {
                composer_draft::flush(&writer)?;
            }
            let store = locked(&store)?;
            store.acknowledge_local_submission(id, &attempt)?;
            store.latest_unconfirmed_submission(id)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.reviewing_submission = false;
                if this.submission_recovery_epoch != epoch {
                    return;
                }
                match result {
                    Ok(receipt) => this.unconfirmed_submission = receipt,
                    Err(error) => this.status = format!("Could not review local send: {error}"),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use std::sync::Mutex;
    use vorkspace_core::store::{Store, SubmissionInput};
    fn fixture(
        cx: &mut TestAppContext,
    ) -> (tempfile::TempDir, Database, Entity<Conversation>, i64) {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_vorkspace(&directory.path().join("sessions.db")).unwrap();
        let session = store.create("codex", "/project").unwrap();
        let id = session.id;
        store.reserve_local_submission(id,SubmissionInput{attempt_id:"unconfirmed".into(),user_turn_id:"saved-message".into(),session_id:String::new(),payload_json:"{}".into(),user_turn:Some(serde_json::json!({"id":"saved-message","role":"user","text":"Original request","attachments":[{"id":"image","name":"private.png","kind":"image","mimeType":"image/png","size":7,"path":"/private/cached.png"}]}))}).unwrap();
        store.dispatch_local_submission("unconfirmed").unwrap();
        store
            .settle_local_submission("unconfirmed", "uncertain", Some("Disconnected".into()))
            .unwrap();
        let db = Arc::new(Mutex::new(store));
        let chat = cx.new(|cx| Conversation::new(session, db.clone(), cx));
        cx.run_until_parked();
        chat.update(cx, |chat, cx| chat.refresh_submission_recovery(cx));
        cx.run_until_parked();
        (directory, db, chat, id)
    }
    #[gpui::test]
    fn recovery_copy_is_explicit_durable_and_preserves_typing_after_the_click(
        cx: &mut TestAppContext,
    ) {
        let (_directory, db, chat, id) = fixture(cx);
        chat.update(cx, |chat, cx| {
            assert!(chat.local_submission_banner(cx).is_some());
            chat.input
                .update(cx, |input, cx| input.set_text("New user input".into(), cx));
            chat.review_local_submission(true, cx);
            assert_eq!(chat.input.read(cx).content, "New user input");
            assert!(!chat.reviewing_submission);
            chat.input
                .update(cx, |input, cx| input.set_text(String::new(), cx));
            chat.review_local_submission(true, cx);
            assert_eq!(chat.input.read(cx).content, "Original request");
            assert_eq!(
                chat.attachments,
                [std::path::PathBuf::from("/private/cached.png")]
            );
            chat.input.update(cx, |input, cx| {
                input.set_text("Original request with more detail".into(), cx);
                input.select_utf16(0..8, cx);
            });
        });
        cx.run_until_parked();
        chat.update(cx, |chat, cx| {
            assert_eq!(
                chat.input.read(cx).content,
                "Original request with more detail"
            );
            assert_eq!(chat.input.read(cx).selection_utf16(), 0..8);
            chat.capture_composer(cx);
            chat.composer_draft.flush().unwrap();
            assert!(chat.local_submission_banner(cx).is_none());
        });
        let store = locked(&db).unwrap();
        assert_eq!(
            store.composer_draft(id).unwrap().unwrap().payload["text"],
            "Original request with more detail"
        );
        let receipt = &store.local_submissions(id).unwrap()[0];
        assert_eq!(receipt.phase, "uncertain");
        assert_eq!(receipt.outcome.as_deref(), Some("reviewed"));
        assert_eq!(
            receipt.user_turn.as_ref().unwrap()["text"],
            "Original request"
        );
    }
    #[gpui::test]
    fn delayed_refresh_cannot_republish_a_dismissed_notice(cx: &mut TestAppContext) {
        let (_directory, _db, chat, _id) = fixture(cx);
        let (epoch, receipt) = chat.update(cx, |chat, cx| {
            let epoch = chat.submission_recovery_epoch;
            let receipt = chat.unconfirmed_submission.clone();
            chat.review_local_submission(false, cx);
            (epoch, receipt)
        });
        cx.run_until_parked();
        chat.update(cx, |chat, cx| {
            assert!(chat.unconfirmed_submission.is_none());
            chat.install_submission_recovery(epoch, Ok(receipt), cx);
            assert!(chat.unconfirmed_submission.is_none());
        });
    }

    #[gpui::test]
    fn recovery_cannot_replace_a_saved_draft_before_its_initial_restoration(
        cx: &mut TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_vorkspace(&directory.path().join("sessions.db")).unwrap();
        let session = store.create("codex", "/project").unwrap();
        let id = session.id;
        let saved = serde_json::json!({
            "cwd":"/project", "text":"Unseen saved draft", "selectionStart":3,
            "selectionEnd":9, "attachments":[{"path":"/private/saved.png"}], "mcpTags":[]
        });
        store.save_composer_draft(id, 0, &saved).unwrap();
        store.reserve_local_submission(id,SubmissionInput{attempt_id:"pending-read".into(),user_turn_id:"receipt-copy".into(),session_id:String::new(),payload_json:"{}".into(),user_turn:Some(serde_json::json!({"id":"receipt-copy","role":"user","text":"Unconfirmed request"}))}).unwrap();
        store.dispatch_local_submission("pending-read").unwrap();
        store
            .settle_local_submission("pending-read", "uncertain", None)
            .unwrap();
        let recovery = store.latest_unconfirmed_submission(id).unwrap();
        let db = Arc::new(Mutex::new(store));
        let chat = cx.new(|cx| Conversation::new(session, db.clone(), cx));
        // Hold the scheduled initial draft read/publication. The independent
        // receipt read is allowed to finish first, while visible input is empty.
        chat.update(cx, |chat, cx| {
            chat.unconfirmed_submission = recovery;
            assert!(chat.input.read(cx).content.is_empty());
            assert!(!chat.composer_draft.ready_for_replacement());
            assert!(chat.local_submission_banner(cx).is_some());
            chat.review_local_submission(true, cx);
            assert!(!chat.reviewing_submission);
            assert!(chat.input.read(cx).content.is_empty());
        });
        assert_eq!(
            locked(&db)
                .unwrap()
                .composer_draft(id)
                .unwrap()
                .unwrap()
                .payload,
            saved
        );
        cx.run_until_parked();
        chat.update(cx, |chat, cx| {
            assert!(chat.composer_draft.ready_for_replacement());
            assert_eq!(chat.input.read(cx).content, "Unseen saved draft");
            assert_eq!(chat.input.read(cx).selection_utf16(), 3..9);
            assert_eq!(
                chat.attachments,
                [std::path::PathBuf::from("/private/saved.png")]
            );
            chat.review_local_submission(true, cx);
            assert!(!chat.reviewing_submission);
            assert_eq!(chat.input.read(cx).content, "Unseen saved draft");
        });
        let store = locked(&db).unwrap();
        assert_eq!(store.composer_draft(id).unwrap().unwrap().revision, 1);
        assert_eq!(store.composer_draft(id).unwrap().unwrap().payload, saved);
        assert!(store.latest_unconfirmed_submission(id).unwrap().is_some());
    }

    #[gpui::test]
    fn recovery_cannot_replace_an_in_flight_attachment_selection(cx: &mut TestAppContext) {
        let (_directory, db, chat, id) = fixture(cx);
        chat.update(cx, |chat, cx| {
            assert!(chat.composer_draft.ready_for_replacement());
            chat.attaching = true;
            chat.review_local_submission(true, cx);
            assert!(!chat.reviewing_submission);
            assert!(chat.input.read(cx).content.is_empty());
            assert!(chat.attachments.is_empty());
            chat.attaching = false;
            chat.pending_attachments
                .push("/private/selected.png".into());
            chat.review_local_submission(true, cx);
            assert!(!chat.reviewing_submission);
            assert!(chat.input.read(cx).content.is_empty());
            assert_eq!(
                chat.pending_attachments,
                [std::path::PathBuf::from("/private/selected.png")]
            );
            assert!(chat.attachments.is_empty());
        });
        cx.run_until_parked();
        assert!(
            locked(&db)
                .unwrap()
                .latest_unconfirmed_submission(id)
                .unwrap()
                .is_some()
        );
    }
}
