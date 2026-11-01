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
mod tests;
