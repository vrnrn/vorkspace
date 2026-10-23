//! A typed draft is consumed only after its ticket lookup succeeds for the
//! same input owner. Queued/Bot messages resolve at their durable delivery.
use super::*;
use vorkspace_core::ticket_context::{self, Resolved};
#[derive(Default)]
pub(super) struct State {
    pending: Option<(String, Arc<AtomicBool>)>,
    pub(super) ready: Option<Resolved>,
}
impl State {
    pub(super) fn pending(&self) -> bool {
        self.pending.is_some()
    }
    pub(super) fn edited(&mut self, text: &str) -> bool {
        if self.pending.as_ref().is_some_and(|(old, _)| old != text) {
            self.cancel();
            true
        } else {
            false
        }
    }
    pub(super) fn cancel(&mut self) {
        if let Some((_, cancel)) = self.pending.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.ready = None;
    }
}
impl Drop for State {
    fn drop(&mut self) {
        self.cancel();
    }
}
impl Conversation {
    /// Return true while admission is pending or failed. The composer, files,
    /// selection, queue and transcript remain owned by their existing views.
    pub(super) fn prepare_ticket_context(&mut self, cx: &mut Context<Self>) -> bool {
        if self.busy
            || self.stopping
            || self.settings.bot.is_some()
            || self.settings.pending_provider_switch.is_some()
            || (!self.follow_ups.state.messages.is_empty() && !self.follow_ups.state.paused)
        {
            return false;
        }
        let text = self.input.read(cx).content.to_string();
        if !ticket_context::has_links(&text) {
            self.ticket_context.cancel();
            return false;
        }
        if self.is_remote()
            && !self
                .remote_state
                .capabilities
                .contains("sessions.preparedText")
        {
            self.ticket_context.cancel();
            self.status =
                "Update the remote host before sending ticket context. Your draft was preserved."
                    .into();
            cx.notify();
            return true;
        }
        if self
            .ticket_context
            .ready
            .as_ref()
            .is_some_and(|ready| ready.matches(&text))
        {
            return false;
        }
        if self
            .ticket_context
            .pending
            .as_ref()
            .is_some_and(|(old, _)| old == &text)
        {
            return true;
        }
        self.ticket_context.cancel();
        let cancel = Arc::new(AtomicBool::new(false));
        self.ticket_context.pending = Some((text.clone(), cancel.clone()));
        let attachments = self.attachments.clone();
        let account = self.settings.account.id.clone();
        let provider = self.summary.provider.clone();
        let generation = self.submission_generation;
        let cwd = self
            .settings
            .worktree_cwd
            .clone()
            .unwrap_or_else(|| self.summary.cwd.clone());
        let store = self.store.clone();
        self.status = "Loading ticket context…".into();
        let work = cx.background_executor().spawn({
            let text = text.clone();
            let cancel = cancel.clone();
            async move {
                let directory = locked(&store)?.data_dir().map(Path::to_path_buf);
                ticket_context::resolve(&text, directory.as_deref(), Path::new(&cwd), &cancel, None)
            }
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if !this
                    .ticket_context
                    .pending
                    .as_ref()
                    .is_some_and(|(_, current)| Arc::ptr_eq(current, &cancel))
                {
                    return;
                }
                this.ticket_context.pending = None;
                if cancel.load(Ordering::Relaxed)
                    || this.submission_generation != generation
                    || this.input.read(cx).content.as_str() != text
                    || this.attachments != attachments
                    || this.attaching
                    || !this.pending_attachments.is_empty()
                    || this.settings.account.id != account
                    || this.summary.provider != provider
                    || this.busy
                    || this.stopping
                {
                    if this.status == "Loading ticket context…" {
                        this.status = "Ready".into();
                    }
                    cx.notify();
                    return;
                }
                match result {
                    Ok(Some(context)) => {
                        this.ticket_context.ready = Some(context);
                        this.send(cx);
                        // A changed mode or other admission guard may decline
                        // the recursive send. Never cache its lookup for later.
                        this.ticket_context.ready = None;
                    }
                    Ok(None) => {
                        this.status = "Ticket reference changed; send the message again.".into()
                    }
                    Err(error) => this.status = error,
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, WindowHandle};
    fn fixture(cx: &mut TestAppContext) -> WindowHandle<Conversation> {
        let store = vorkspace_core::store::Store::open(Path::new(":memory:")).unwrap();
        let summary = store.create("codex", "/missing/ticket-project").unwrap();
        cx.update(|cx| {
            input::bind(cx);
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| {
                    let mut chat =
                        Conversation::new(summary, Arc::new(std::sync::Mutex::new(store)), cx);
                    chat.loaded = true;
                    chat
                })
            })
            .unwrap()
        })
    }
    const REFERENCE: &str = "Explain vorkspace://context/github?url=https%3A%2F%2Fgithub.com%2Facme%2Fdemo%2Fissues%2F42";
    #[gpui::test]
    fn failed_lookup_preserves_composer_selection_files_and_history(cx: &mut TestAppContext) {
        let window = fixture(cx);
        cx.run_until_parked();
        window
            .update(cx, |chat, _, cx| {
                chat.input.update(cx, |input, cx| {
                    input.set_text(REFERENCE.into(), cx);
                    input.select_utf16(2..9, cx);
                });
                chat.attachments.push("/missing/private.txt".into());
                chat.send(cx);
                assert!(!chat.busy);
                assert_eq!(chat.input.read(cx).content.as_str(), REFERENCE);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |chat, _, cx| {
                assert!(!chat.busy);
                assert!(
                    chat.status
                        .contains("Ticket context storage is unavailable")
                );
                assert_eq!(chat.input.read(cx).content.as_str(), REFERENCE);
                assert_eq!(chat.input.read(cx).selection_utf16(), 2..9);
                assert_eq!(
                    chat.attachments,
                    vec![std::path::PathBuf::from("/missing/private.txt")]
                );
                assert!(chat.follow_ups.state.messages.is_empty());
                assert!(chat.transcript.read(cx).source_rows().is_empty());
                assert!(
                    locked(&chat.store)
                        .unwrap()
                        .local_submissions(chat.summary.id)
                        .unwrap()
                        .is_empty()
                );
            })
            .unwrap();
    }
    #[gpui::test]
    fn newer_typing_and_cancel_own_the_composer_before_lookup_completion(cx: &mut TestAppContext) {
        for cancel in [false, true] {
            let window = fixture(cx);
            cx.run_until_parked();
            window
                .update(cx, |chat, _, cx| {
                    chat.input
                        .update(cx, |input, cx| input.set_text(REFERENCE.into(), cx));
                    chat.send(cx);
                    assert!(chat.ticket_context.pending.is_some());
                    if cancel {
                        chat.stop(cx);
                        assert_eq!(chat.status, "Ready");
                        assert!(chat.ticket_context.pending.is_none());
                        assert_eq!(chat.input.read(cx).content.as_str(), REFERENCE);
                    }
                    chat.input.update(cx, |input, cx| {
                        input.set_text("My newer draft 🙂".into(), cx)
                    });
                    chat.status = "User owns the new draft".into();
                })
                .unwrap();
            cx.run_until_parked();
            window
                .update(cx, |chat, _, cx| {
                    assert_eq!(chat.input.read(cx).content.as_str(), "My newer draft 🙂");
                    assert_eq!(chat.status, "User owns the new draft");
                    assert!(!chat.busy && chat.ticket_context.ready.is_none());
                    assert!(chat.transcript.read(cx).source_rows().is_empty());
                })
                .unwrap();
        }
    }
    #[gpui::test]
    fn an_older_remote_host_cannot_consume_a_typed_ticket_reference(cx: &mut TestAppContext) {
        let window = fixture(cx);
        cx.run_until_parked();
        window
            .update(cx, |chat, _, cx| {
                chat.summary.cwd = "remote://fixture/project".into();
                chat.remote_state.online = true;
                chat.input
                    .update(cx, |input, cx| input.set_text(REFERENCE.into(), cx));
                chat.attachments.push("/missing/private.txt".into());
                assert!(chat.prepare_ticket_context(cx));
                assert!(chat.status.contains("Update the remote host"));
                assert_eq!(chat.input.read(cx).content.as_str(), REFERENCE);
                assert_eq!(chat.attachments.len(), 1);
                assert!(!chat.busy && chat.ticket_context.pending.is_none());
            })
            .unwrap();
    }
    #[gpui::test]
    fn editing_a_pending_lookup_clears_its_loading_indicator(cx: &mut TestAppContext) {
        let window = fixture(cx);
        cx.run_until_parked();
        window
            .update(cx, |chat, _, cx| {
                chat.input
                    .update(cx, |input, cx| input.set_text(REFERENCE.into(), cx));
                chat.send(cx);
                chat.input
                    .update(cx, |input, cx| input.set_text("New draft".into(), cx));
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |chat, _, cx| {
                assert_eq!(chat.input.read(cx).content.as_str(), "New draft");
                assert_eq!(chat.status, "Ready");
                assert!(!chat.busy && !chat.ticket_context.pending());
            })
            .unwrap();
    }
    #[gpui::test]
    fn unknown_remote_capabilities_wait_for_loading_without_consuming_the_draft(
        cx: &mut TestAppContext,
    ) {
        let window = fixture(cx);
        cx.run_until_parked();
        window
            .update(cx, |chat, _, cx| {
                chat.summary.cwd = "remote://fixture/project".into();
                chat.loaded = false;
                chat.remote_state.online = false;
                chat.input
                    .update(cx, |input, cx| input.set_text(REFERENCE.into(), cx));
                chat.send(cx);
                assert!(
                    chat.status
                        .contains("Wait for this remote conversation to load")
                );
                assert!(!chat.status.contains("Update"));
                assert_eq!(chat.input.read(cx).content.as_str(), REFERENCE);
                assert!(!chat.busy && chat.ticket_context.pending.is_none());
            })
            .unwrap();
    }
}
