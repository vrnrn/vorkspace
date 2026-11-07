use super::*;

impl Conversation {
    pub(crate) fn sync_native_history(&mut self, cx: &mut Context<Self>) {
        if self.native_syncing
            || self.loading
            || self.busy
            || self.stopping
            || self.account_switching
        {
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
                    || this.busy
                    || this.stopping
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
                            this.status = "Working in external CLI…".into();
                        } else if this.status == "Working in external CLI…" {
                            this.status = "Ready".into();
                        }
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
}
