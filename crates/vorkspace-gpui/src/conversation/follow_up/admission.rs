//! A composer remains editable while files are staged. A queue row or draft
//! card is visible only after it and the offered editor revision commit together.
use super::*;
use vorkspace_core::{follow_up::StagedMessage, store::ComposerOrigin};

#[derive(Clone, Copy)]
enum ShelfDestination {
    Queue { continuation: bool },
    Draft,
}

pub(super) struct ShelfOffer {
    origin: ComposerOrigin,
    destination: ShelfDestination,
    cancel: Option<Arc<AtomicBool>>,
}

impl ShelfDestination {
    fn failure(self) -> &'static str {
        match self {
            Self::Queue { .. } => "Could not queue the message",
            Self::Draft => "Could not save the draft",
        }
    }
}

impl FollowUps {
    pub(in crate::conversation) fn admission_label(&self) -> Option<&'static str> {
        match self.offering.as_ref()?.destination {
            ShelfDestination::Queue { .. } => Some("Queueing message"),
            ShelfDestination::Draft => Some("Saving draft"),
        }
    }
}

impl Conversation {
    pub(in crate::conversation) fn queue_composer(
        &mut self,
        prompt: String,
        mode: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let destination = ShelfDestination::Queue {
            continuation: self.is_stopping() || self.settings.pending_provider_switch.is_some(),
        };
        if self.follow_ups.state.messages.len() >= 128 {
            self.shelf_admission_failed(
                destination,
                "The session’s message queue is full.".into(),
                cx,
            );
            return;
        }
        let message = self.queued_message(prompt, self.attachments.clone(), mode, cx);
        self.offer_shelf_message(message, destination, cx);
    }

    fn offer_shelf_message(
        &mut self,
        message: Message,
        destination: ShelfDestination,
        cx: &mut Context<Self>,
    ) {
        let origin = match self.prepare_composer_submission(cx) {
            Ok(origin) => origin,
            Err(error) => {
                self.shelf_admission_failed(destination, error, cx);
                return;
            }
        };
        self.follow_ups.offering = Some(ShelfOffer {
            origin,
            destination,
            cancel: self.turn_in_flight().then(|| self.cancel.clone()),
        });
        self.follow_ups.admission_error = None;
        let session = self.summary.id;
        if message.attachments.is_empty() {
            let staged = StagedMessage::prepare(message, session, None);
            self.finish_shelf_admission(staged, cx);
            return;
        }
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            let directory = locked(&store)?.data_dir().map(Path::to_path_buf);
            StagedMessage::prepare(message, session, directory.as_deref())
        });
        cx.spawn(async move |this, cx| {
            let staged = work.await;
            let _ = this.update(cx, |this, cx| this.finish_shelf_admission(staged, cx));
        })
        .detach();
        cx.notify();
    }

    fn finish_shelf_admission(
        &mut self,
        staged: Result<StagedMessage, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(ShelfOffer {
            origin,
            destination,
            cancel,
        }) = self.follow_ups.offering.take()
        else {
            return;
        };
        let accepted = staged.and_then(|staged| {
            if !self.history_ready()
                || self.user_read_only()
                || self.editing_last_turn.pending()
                || self.settings.bot.is_some()
                || self.is_remote()
            {
                return Err("Wait for the conversation to finish changing and retry.".into());
            }
            let message = &staged.message;
            let mut next = self.follow_ups.state.clone();
            next.revision += 1;
            match destination {
                ShelfDestination::Queue { .. } => {
                    if next.messages.len() >= 128 {
                        return Err("The session’s message queue is full.".into());
                    }
                    // A Stop during attachment preparation wins over auto-send.
                    next.paused |= self.is_stopping()
                        || cancel
                            .as_ref()
                            .is_some_and(|flag| flag.load(Ordering::Acquire));
                    next.messages.push(message.clone());
                }
                ShelfDestination::Draft => {
                    if next.draft.is_some() {
                        return Err("This session already has a saved draft.".into());
                    }
                    next.draft = Some(message.clone());
                }
            }
            locked(&self.store)?.accept_composer_message(
                self.summary.id,
                &next,
                &origin,
                &message.id,
            )?;
            Ok((next, staged.publish()))
        });
        let (next, message) = match accepted {
            Ok(accepted) => accepted,
            Err(error) => {
                self.finish_composer_submission(cx);
                self.shelf_admission_failed(destination, error, cx);
                return;
            }
        };
        let was_empty = self.follow_ups.state.messages.is_empty();
        self.follow_ups.persisted = next.revision;
        self.follow_ups.state = next;
        self.accept_composer_submission(&origin, cx);
        match destination {
            ShelfDestination::Queue { continuation } => {
                self.follow_ups.motion.insert(&message.id, was_empty, true);
                if continuation {
                    self.continue_after_stop(message.id);
                }
                self.pump_follow_ups(cx);
                cx.emit(SessionChanged::Draft);
            }
            ShelfDestination::Draft => self.show_saved_draft(message, cx),
        }
        cx.notify();
    }

    fn shelf_admission_failed(
        &mut self,
        destination: ShelfDestination,
        error: String,
        cx: &mut Context<Self>,
    ) {
        self.follow_ups.admission_error = Some(format!(
            "{}. {error} Your composer was preserved.",
            destination.failure()
        ));
        if error.contains("another window") {
            self.refresh_if_stale(cx);
        }
        cx.notify();
    }

    pub(in crate::conversation) fn save_message_draft(&mut self, cx: &mut Context<Self>) {
        if self.reject_user_entry(cx) {
            return;
        }

        if self.is_remote() {
            self.remote_save_draft(cx);
            return;
        }
        if self.needs_history_restore() {
            self.defer_history_action(|this, cx| this.save_message_draft(cx), cx);
            return;
        }
        if self.follow_ups.state.draft.is_some()
            || !self.loaded
            || self.attaching
            || self.input.read(cx).is_composing()
        {
            return;
        }
        let raw = self.input.read(cx).content.to_string();
        let text = vorkspace_core::composer::consume_command(&raw, "draft")
            .unwrap_or(&raw)
            .to_owned();
        if text.trim().is_empty() && self.attachments.is_empty() {
            return;
        }
        let mut message = self.message(text, self.attachments.clone(), None, cx);
        message.at = self.transcript.read(cx).source_rows().len();
        if self.settings.bot.is_none() {
            self.offer_shelf_message(message, ShelfDestination::Draft, cx);
            return;
        }
        // Bot drafts are accepted before clearing the composer. An attachment
        // copy or storage failure must leave the user's input untouched.
        let origin_revision = match self.bot_draft_origin_revision(cx) {
            Ok(revision) => revision,
            Err(error) => {
                self.status =
                    format!("Could not save draft: {error}. Your composer was preserved.");
                self.bot_ui.draft_error = Some(self.status.clone());
                cx.notify();
                return;
            }
        };
        let mut next = self.follow_ups.state.clone();
        next.revision += 1;
        next.draft = Some(message.clone());
        let result = (|| {
            let store = locked(&self.store)?;
            next.retain_attachments(self.summary.id, store.data_dir())?;
            store.accept_bot_draft(self.summary.id, &next, origin_revision, &message.id)
        })();
        if let Err(error) = result {
            // A shared-store root publication can fail after its database
            // commit. Adopt any matching durable card before permitting a
            // retry, while retaining the original composer as recovery.
            if let Ok(saved) =
                locked(&self.store).and_then(|store| store.follow_ups(self.summary.id))
                && saved
                    .draft
                    .as_ref()
                    .is_some_and(|draft| draft.id == message.id)
            {
                self.follow_ups.persisted = saved.revision;
                self.follow_ups.state = saved;
                self.summary.has_messages = true;
                let draft = self.follow_ups.state.draft.clone();
                self.transcript
                    .update(cx, |view, cx| view.set_saved_draft(draft, cx));
            }
            self.status = format!("Could not save draft: {error}. Your composer was preserved.");
            self.bot_ui.draft_error = Some(self.status.clone());
            cx.notify();
            return;
        }
        self.bot_ui.draft_error = None;
        message = next.draft.clone().unwrap();
        self.follow_ups.persisted = next.revision;
        self.follow_ups.state = next;
        self.input.update(cx, |i, cx| i.set_text(String::new(), cx));
        self.attachments.clear();
        self.follow_ups.draft_selected = false;
        self.clear_submitted_mode(cx);
        self.show_saved_draft(message, cx);
    }

    fn show_saved_draft(&mut self, message: Message, cx: &mut Context<Self>) {
        self.seed_title(&message.label(), cx);
        if !self.summary.has_messages {
            self.composer_motion.capture(self.composer_anchor.get());
        }
        self.summary.has_messages = true;
        self.transcript
            .update(cx, |view, cx| view.introduce_saved_draft(message, cx));
        self.completion
            .update(cx, |completion, cx| completion.dismiss(cx));
        cx.emit(SessionChanged::Draft);
        cx.notify();
    }
}
