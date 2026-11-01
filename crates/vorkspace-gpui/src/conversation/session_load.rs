//! Loading a session's history into the view and refreshing it when stale.
use super::*;

impl Conversation {
    pub(crate) fn preview(&self, done: bool) -> AnyElement {
        let kind = Kind::from_id(&self.summary.provider);
        let model = self.resolved_model.as_deref().or(self.model.as_deref());
        let name = self
            .catalog
            .iter()
            .find(|entry| Some(entry.id.as_str()) == model)
            .map(|entry| model_display_name(kind, &entry.id, &entry.name))
            .unwrap_or_else(|| model_name(kind, model));
        let cwd = self
            .settings
            .worktree_cwd
            .as_deref()
            .unwrap_or(&self.summary.cwd);
        let directory = std::env::var_os("HOME")
            .and_then(|home| {
                Path::new(cwd).strip_prefix(home).ok().map(|path| {
                    if path.as_os_str().is_empty() {
                        "~".to_owned()
                    } else {
                        format!("~/{}", path.display())
                    }
                })
            })
            .unwrap_or_else(|| cwd.to_owned());
        let linked =
            self.settings.worktree_cwd.is_some() || self.git.as_ref().is_some_and(|git| git.linked);
        let detail = |label: &'static str, value: String| {
            div()
                .flex()
                .gap_2()
                .child(
                    div()
                        .w(px(72.))
                        .flex_shrink_0()
                        .text_color(content(0.45))
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(content(0.8))
                        .child(value),
                )
        };
        div()
            .px_3()
            .py_2p5()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .text_size(px(11.))
                            .text_color(content(0.55))
                            .child(provider_icon(&self.summary.provider, 14.))
                            .child(div().min_w_0().truncate().child(name)),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(11.))
                            .text_color(if self.needs_approval() {
                                gpui::rgb(0xfbbf24).into()
                            } else if self.responding() {
                                accent_color()
                            } else if done {
                                gpui::rgb(0x34d399).into()
                            } else {
                                content(0.45)
                            })
                            .child(if self.needs_approval() {
                                "Needs approval".to_owned()
                            } else if self.busy {
                                "Working…".to_owned()
                            } else if done {
                                "Done".to_owned()
                            } else if self.loading {
                                "Loading…".to_owned()
                            } else {
                                vorkspace_core::time::relative(
                                    self.summary.updated_at,
                                    vorkspace_core::time::now_ms(),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .mt_1p5()
                    .text_size(px(13.))
                    .line_height(px(20.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(content(1.))
                    .child(self.display_title()),
            )
            .child(
                div()
                    .mt_2()
                    .pt_2()
                    .border_t_1()
                    .border_color(content(0.08))
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .when_some(self.branch.clone(), |d, branch| {
                        d.child(detail("Branch", branch))
                    })
                    .child(detail(
                        if linked { "Worktree" } else { "Directory" },
                        directory,
                    )),
            )
            .into_any_element()
    }

    pub(crate) fn load(&mut self, cx: &mut Context<Self>) {
        if self.is_remote() {
            self.load_remote(cx);
            return;
        }
        if self.history_unloaded {
            self.reload_history(cx);
            return;
        }
        if self.history_partial {
            if let Some(search) = self
                .transcript
                .update(cx, |view, _| view.take_storage_search())
            {
                self.pending_search = Some(search);
            }
            self.history_unloaded = true;
            self.reload_history(cx);
            return;
        }
        if self.loading {
            return;
        }
        if self.loaded {
            self.refresh_if_stale(cx);
            return;
        }
        self.loading = true;
        self.refresh_catalog(cx);
        let store = self.store.clone();
        let id = self.summary.id;
        let cwd = self.summary.cwd.clone();
        let bot = self.settings.bot.is_some();
        let page_bot = bot && self.allow_storage_paging;
        let recovery_epoch = self.submission_recovery_epoch;
        let work = cx.background_executor().spawn(async move {
            let settings = locked(&store).and_then(|s| s.agent_settings(id));
            let execution_cwd = settings
                .as_ref()
                .ok()
                .and_then(|s| s.as_ref())
                .and_then(|s| s.worktree_cwd.as_deref())
                .unwrap_or(&cwd);
            let git = vorkspace_model::git_cache::load(Path::new(execution_cwd)).ok();
            let rev = locked(&store)
                .and_then(|s| s.session_history_revision(id))
                .unwrap_or(0);
            (
                locked(&store)
                    .and_then(|s| {
                        if page_bot
                            && let Ok(Some(page)) = s.indexed_history_page(id, None, 10)
                            && page.before.is_some()
                        {
                            return Ok(history::InitialRows::Window(Box::new(page)));
                        }
                        Ok(history::InitialRows::Complete(s.load(id)?.rows))
                    })
                    .map(|rows| match rows {
                        history::InitialRows::Complete(rows) => history::InitialHistory::Complete(
                            crate::transcript::PreparedHistory::new(rows, bot),
                        ),
                        history::InitialRows::Window(mut page) => {
                            let prepared = crate::transcript::PreparedHistory::new(
                                std::mem::take(&mut page.rows),
                                false,
                            );
                            history::InitialHistory::Window(page, prepared)
                        }
                    }),
                git,
                settings,
                locked(&store).and_then(|s| s.context_usage(id)),
                locked(&store).and_then(|s| s.follow_ups(id)),
                locked(&store).and_then(|s| {
                    s.recover_local_submissions()?;
                    s.latest_unconfirmed_submission(id)
                }),
                rev,
            )
        });
        cx.spawn(async move |this, cx| {
            let (result, git, settings, context, follow_ups, unconfirmed, rev) = work.await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                this.loaded_history_revision = Some(rev);
                this.branch = git.as_ref().and_then(|s| s.current.clone());
                this.git = git;
                this.context = context.ok().flatten();
                if this.submission_recovery_epoch == recovery_epoch {
                    match unconfirmed {
                        Ok(receipt) => this.unconfirmed_submission = receipt,
                        Err(error) => {
                            this.status = error;
                            return;
                        }
                    }
                }
                match follow_ups {
                    Ok(state) => {
                        this.follow_ups.persisted = state.revision;
                        this.follow_ups.state = state;
                        let draft = this.follow_ups.state.draft.clone();
                        if draft.is_some() {
                            this.summary.has_messages = true;
                        }
                        this.transcript
                            .update(cx, |view, cx| view.set_saved_draft(draft, cx));
                        if this.settings.bot.is_some() {
                            this.sync_bot_pending(cx);
                        }
                    }
                    Err(error) => {
                        this.status = error;
                        return;
                    }
                }
                // The composer may have a newer revision while history loads;
                // historical turn provenance still belongs to the loaded rows.
                if let Ok(Some(settings)) = &settings {
                    this.transcript
                        .update(cx, |view, _| view.turns = settings.turns.clone());
                    this.settings.turns = settings.turns.clone();
                }
                if let Ok(Some(mut settings)) = settings
                    && settings.revision >= this.settings.revision
                {
                    if !this.summary.has_messages
                        && settings.revision == 0
                        && settings.account.id == vorkspace_core::accounts::DEFAULT
                    {
                        settings.account = crate::accounts::get(cx)
                            .map(|center| {
                                center.read(cx).selected(
                                    Kind::from_id(&this.summary.provider),
                                    &this.summary.cwd,
                                )
                            })
                            .unwrap_or_default();
                    }
                    this.model = settings.model.clone();
                    let bot = this.settings.bot.take();
                    let environment = std::mem::take(&mut this.settings.environment);
                    this.settings = settings;
                    this.attach_account(cx);
                    this.settings.bot = bot;
                    this.settings.environment = environment;
                }
                match result {
                    Ok(history) => {
                        match history {
                            history::InitialHistory::Complete(history) => this
                                .transcript
                                .update(cx, |v, cx| v.install_prepared_history(history, cx)),
                            history::InitialHistory::Window(page, prepared) => {
                                this.history_partial = true;
                                this.transcript.update(cx, |v, cx| {
                                    v.install_storage_history(
                                        *page,
                                        prepared,
                                        this.store.clone(),
                                        id,
                                        cx,
                                    )
                                });
                            }
                        }
                        this.status = "Ready".into();
                        this.loaded = true;
                        if let Some(message) = this
                            .follow_ups
                            .state
                            .draft
                            .as_ref()
                            .and_then(|draft| this.bot_ui.accepted_drafts.get(&draft.id))
                            .cloned()
                        {
                            this.bot_draft_outbox_saved(&message, cx);
                        }
                        if let Some((row, query)) = this.pending_search.take() {
                            this.reveal_search(row, query, cx);
                        }
                        this.sync_review(cx);
                        this.run_history_actions(cx);
                        this.pump_follow_ups(cx);
                        if this.history_partial
                            && (!this.history_actions.is_empty()
                                || !this.follow_ups.state.messages.is_empty())
                        {
                            this.load(cx);
                        }
                    }
                    Err(e) => this.status = e,
                }
                cx.emit(SessionChanged);
                cx.notify();
            });
        })
        .detach();
    }
    pub(crate) fn refresh_if_stale(&mut self, cx: &mut Context<Self>) {
        if !self.loaded {
            self.load(cx);
            return;
        }
        if self.loading || self.busy || self.stopping {
            return;
        }
        // A partial transcript is refreshed when a cursor or mutation guard
        // detects a change; replacing it here would lose its reading anchor.
        if self.history_partial {
            return;
        }
        if self.history_unloaded {
            self.reload_history(cx);
            return;
        }
        if self.is_remote() {
            self.refresh_remote_snapshot(cx);
            return;
        }
        let id = self.summary.id;
        let store = self.store.clone();
        let current_rev = self.loaded_history_revision;
        let submission = self.submission_generation;
        let bot = self.settings.bot.is_some();
        let work = cx.background_executor().spawn(async move {
            let store = locked(&store)?;
            let rev = store.session_history_revision(id).unwrap_or(0);
            if current_rev == Some(rev) {
                return Ok::<_, String>(None);
            }
            let history = store.load(id)?;
            let turns = store.agent_settings(id)?.unwrap_or_default().turns;
            Ok(Some((
                crate::transcript::PreparedHistory::new(history.rows, bot),
                turns,
                rev,
            )))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                // Focus can start this read just before another turn, history
                // restore or refresh takes ownership of the visible transcript.
                if this.submission_generation != submission
                    || this.loaded_history_revision != current_rev
                    || this.loading
                    || this.history_unloaded
                    || this.history_partial
                    || this.busy
                    || this.stopping
                {
                    return;
                }
                if let Ok(Some((history, turns, rev))) = result {
                    this.settings.turns = turns.clone();
                    this.transcript.update(cx, |view, cx| {
                        view.turns = turns;
                        // Keep the existing rows, reading anchor and opened
                        // Bot pages while applying the authoritative snapshot.
                        view.install_prepared_history(history, cx);
                    });
                    this.loaded_history_revision = Some(rev);
                    this.status = "Ready".into();
                    if this.follow_ups.state.paused
                        && this.follow_ups.state.messages.iter().any(|m| {
                            m.error
                                .as_deref()
                                .is_some_and(|e| e.contains("another window"))
                        })
                    {
                        this.follow_ups.state.paused = false;
                        for message in &mut this.follow_ups.state.messages {
                            if message
                                .error
                                .as_deref()
                                .is_some_and(|e| e.contains("another window"))
                            {
                                message.error = None;
                            }
                        }
                        this.save_follow_ups(cx);
                    }
                    this.sync_review(cx);
                    cx.emit(SessionChanged);
                    cx.notify();
                }
            });
        })
        .detach();
    }
}
