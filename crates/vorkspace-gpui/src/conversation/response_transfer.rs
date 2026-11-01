//! Sending a response to another session or target.
use super::*;

impl Conversation {
    pub(crate) fn install_transfer(
        &mut self,
        settings: Settings,
        prompt: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.editing_last_turn.pending() {
            self.defer_edit_transfer(settings, prompt, cx);
            return;
        }
        if self.needs_history_restore() || self.stopping {
            self.defer_history_action(
                move |this, cx| {
                    // Bot callers may configure their live context immediately
                    // after installing a transfer. Keep that context at replay.
                    let bot = this.settings.bot.take();
                    let environment = std::mem::take(&mut this.settings.environment);
                    let mut settings = settings;
                    if settings.bot.is_none() {
                        settings.bot = bot;
                    }
                    settings.environment.extend(environment);
                    match prompt {
                        Some(prompt) if this.preserve_new_composer(cx) => {
                            this.install_transfer(settings, None, cx);
                            this.enqueue_restored_prompt(prompt, cx);
                        }
                        prompt => this.install_transfer(settings, prompt, cx),
                    }
                },
                cx,
            );
            return;
        }
        self.model = settings.model.clone();
        self.settings = settings;
        self.attach_account(cx);
        self.loaded = true;
        self.status = "Ready".into();
        self.refresh_catalog(cx);
        self.refresh_git(cx);
        if let Some(prompt) = prompt {
            self.input.update(cx, |i, cx| i.set_text(prompt, cx));
            self.send(cx);
        } else {
            cx.notify();
        }
    }
    pub(super) fn close_response_menu(&mut self, cx: &mut Context<Self>) {
        self.response_menu = None;
        self.response_request = None;
        self.transcript.update(cx, |view, cx| {
            view.target_action = None;
            cx.notify();
        });
        cx.notify();
    }
    pub(super) fn response_request(
        &mut self,
        request: &ResponseRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.user_read_only() && !matches!(request, ResponseRequest::SaveNote { .. }) {
            return;
        }
        if self.history_partial && !matches!(request, ResponseRequest::QuoteSelection { .. }) {
            self.status = "Loading complete history; please retry the action".into();
            self.load(cx);
            return;
        }
        match request {
            ResponseRequest::RemotePlan { id, text, build } => {
                self.remote_plan_request(id, text, *build, cx)
            }
            ResponseRequest::QuoteSelection { text } => self.bot_quote(text, window, cx),
            ResponseRequest::EditLastTurn { turn } => self.begin_edit_last_turn(*turn, window, cx),
            ResponseRequest::Draft { send } => self.draft_action(*send, cx),
            ResponseRequest::EditDraft {
                id,
                edit_id,
                text,
                finish,
                cancel,
            } => self.edit_saved_draft(id, edit_id, text, *finish, *cancel, cx),
            ResponseRequest::SaveNote {
                turn,
                text,
                selection,
            } => {
                let turn = *turn;
                let note = vorkspace_core::response::Note {
                    id: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos() as u64,
                    title: {
                        let title = self.display_title();
                        if *selection || title.trim().is_empty() || title == "New session" {
                            vorkspace_core::notes::title(text)
                        } else {
                            title
                        }
                    },
                    body: text.clone(),
                    tags: Vec::new(),
                    source_session: self.summary.id,
                    source_cwd: self.summary.cwd.clone(),
                };
                let store = self.store.clone();
                let work = cx
                    .background_executor()
                    .spawn(async move { locked(&store)?.save_note(&note) });
                cx.spawn(async move |this, cx| {
                    let result = work.await;
                    let _ = this.update(cx, |this, cx| {
                        this.transcript
                            .update(cx, |view, cx| view.note_result(turn, result, cx))
                    });
                })
                .detach();
            }
            ResponseRequest::Target { turn, kind, anchor } => {
                if self
                    .response_request
                    .is_some_and(|(at, k, _)| at == *turn && k == *kind)
                {
                    self.close_response_menu(cx);
                    return;
                }
                let turn = *turn;
                let kind = *kind;
                let token = Instant::now();
                let anchor = anchor.clone();
                self.menu = None;
                self.git_picker = None;
                self.response_request = Some((turn, kind, token));
                self.transcript.update(cx, |view, cx| {
                    view.target_action = Some((turn, kind));
                    cx.notify();
                });
                self.open_response_menu(turn, kind, token, anchor, window, cx);
                cx.notify();
            }
        }
    }
    pub(super) fn finish_response_target(
        &mut self,
        turn: usize,
        kind: vorkspace_core::response::TransferKind,
        id: &str,
        cx: &mut Context<Self>,
    ) {
        let Ok(target) = serde_json::from_str::<vorkspace_core::response::Target>(id) else {
            return;
        };
        let from = self
            .transcript
            .read(cx)
            .turns
            .get(&turn)
            .map(|t| t.provider.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.summary.provider)
            .to_owned();
        let (rows, through) = self.transcript.read(cx).response_rows(turn);
        let transfer = Transfer {
            kind,
            target,
            from,
            turn: rows,
            through,
            cwd: self.summary.cwd.clone(),
            title: self.summary.title.clone(),
            settings: self.settings.clone(),
        };
        self.close_response_menu(cx);
        cx.emit(transfer);
    }
}
