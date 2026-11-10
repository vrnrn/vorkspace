use super::*;
#[cfg(test)]
mod tests;

#[derive(Clone)]
pub(super) struct Recovery {
    token: String,
    session: i64,
    provider: String,
    native_id: Option<String>,
    account: String,
}

impl Conversation {
    fn current_native_recovery(&self) -> Option<&Recovery> {
        self.native_recovery.as_ref().filter(|recovery| {
            recovery.session == self.summary.id
                && recovery.provider == self.summary.provider
                && recovery.native_id == self.summary.provider_id
                && recovery.account == self.settings.account.id
        })
    }
    pub(crate) fn sync_native_history(&mut self, cx: &mut Context<Self>) {
        if self.native_syncing || self.loading || self.turn_in_flight() || self.account_switching {
            return;
        }
        let Some(bot) = &self.settings.bot else {
            return;
        };
        if self.summary.provider_id.is_none() {
            return;
        }
        self.native_syncing = true;
        let session = self.summary.clone();
        let account = self.settings.account.id.clone();
        let generation = self.submission_generation;
        let root = bot.root.clone();
        let store = self.store.clone();
        let id = session.id;
        let expected_provider_id = session.provider_id.clone();
        let work = cx.background_executor().spawn(async move {
            let result = vorkspace_runtime::runtime::native_history::sync(&store, id, &root)?;
            let store = locked(&store)?;
            let revision = store.session_history_revision(id)?;
            let usage = if store
                .bot_session(id)?
                .is_some_and(|s| s.provider_id == expected_provider_id)
            {
                Some(store.context_usage(id)?)
            } else {
                None
            };
            Ok::<_, String>((result, revision, usage))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.native_syncing = false;
                if this.submission_generation != generation
                    || this.summary.id != id
                    || this.summary.provider != session.provider
                    || this.summary.provider_id != session.provider_id
                    || this.settings.account.id != account
                    || this.turn_in_flight()
                {
                    return;
                }
                match result {
                    Ok((result, revision, usage)) if !result.superseded => {
                        if let Some(usage) = usage {
                            this.context = usage;
                        }
                        if this.loaded_history_revision != Some(revision) {
                            // Reuse the history owner, which preserves drafts,
                            // queue receipts, selections and reading anchors.
                            if this.history_partial {
                                this.history_unloaded = true;
                                this.reload_history(cx);
                            } else {
                                this.refresh_if_stale(cx);
                            }
                        }
                        if result.active {
                            this.status = "External turn unfinished".into();
                        } else if this.status == "External turn unfinished" {
                            this.status = "Ready".into();
                        }
                        this.native_recovery = result.recovery.map(|token| Recovery {
                            token,
                            session: id,
                            provider: session.provider.clone(),
                            native_id: session.provider_id.clone(),
                            account: account.clone(),
                        });
                        if result.more {
                            this.sync_native_history(cx);
                        }
                    }
                    Err(error) => this.status = format!("Native history sync: {error}"),
                    _ => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn native_history_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.current_native_recovery().is_none() || self.turn_in_flight() {
            return None;
        }
        Some(
            div()
                .mb_1()
                .px_3()
                .py_2()
                .rounded(px(radius::CONTROL))
                .bg(content(0.06))
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .text_size(px(12.))
                .text_color(content(0.75))
                .child("External turn unfinished")
                .child(
                    tool_pill("recover-native-history", "Resume here")
                        .child("Resume here")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.confirm_native_recovery(window, cx);
                        })),
                )
                .into_any_element(),
        )
    }

    fn confirm_native_recovery(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(recovery) = self.current_native_recovery().cloned() else {
            return;
        };
        let expected = recovery.token;
        let Some(bot) = self.settings.bot.as_ref() else {
            return;
        };
        let root = bot.root.clone();
        let session = self.summary.clone();
        let account = self.settings.account.id.clone();
        let generation = self.submission_generation;
        let prompt = window.prompt(
            gpui::PromptLevel::Warning,
            "Resume this conversation here?",
            Some("Stop or close the external CLI first. Vorkspace will keep the same conversation and will not resend the unfinished message."),
            &["Cancel", "Resume here"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if prompt.await.ok() != Some(1) {
                return;
            }
            let work = this
                .update(cx, |this, cx| {
                    if this.turn_in_flight()
                        || this.account_switching
                        || this.submission_generation != generation
                        || this.summary.id != session.id
                        || this.summary.provider != session.provider
                        || this.summary.provider_id != session.provider_id
                        || this.settings.account.id != account
                        || this
                            .current_native_recovery()
                            .is_none_or(|current| current.token != expected)
                    {
                        return None;
                    }
                    this.native_syncing = true;
                    let store = this.store.clone();
                    Some(cx.background_executor().spawn(async move {
                        vorkspace_runtime::runtime::native_history::recover(
                            &store, session.id, &root, &expected,
                        )
                    }))
                })
                .ok()
                .flatten();
            let Some(work) = work else { return };
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.native_syncing = false;
                if this.submission_generation != generation
                    || this.summary.id != session.id
                    || this.summary.provider != session.provider
                    || this.summary.provider_id != session.provider_id
                    || this.settings.account.id != account
                    || this.turn_in_flight()
                {
                    return;
                }
                match result {
                    Ok(()) => {
                        this.native_recovery = None;
                        this.status = "Ready".into();
                    }
                    Err(error) => this.status = error,
                }
                this.sync_native_history(cx);
                cx.notify();
            });
        })
        .detach();
    }
}
