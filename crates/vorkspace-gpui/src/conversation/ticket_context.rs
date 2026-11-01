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
mod tests;
