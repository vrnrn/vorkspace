use super::*;
use crate::accounts::AccountEvent;
impl Shell {
    pub(super) fn observe_accounts(&mut self, cx: &mut Context<Self>) {
        let Some(center) = crate::accounts::get(cx) else {
            return;
        };
        self.observations.push(cx.subscribe(
            &center,
            |this, _, event: &AccountEvent, cx| match event {
                AccountEvent::Select {
                    kind,
                    id,
                    project,
                    session,
                } => this.select_provider_account(*kind, id, project, *session, cx),
                AccountEvent::Removing(kind, id) => {
                    for view in this.sessions.values() {
                        if view.read(cx).summary.provider == kind.id()
                            && view.read(cx).settings.account.id == *id
                        {
                            view.update(cx, |v, cx| v.account_unavailable(cx));
                        }
                    }
                    if let Some(bots) = &this.bots {
                        bots.update(cx, |m, cx| m.account_changed(*kind, id, true, cx));
                    }
                }
                AccountEvent::Authenticated(kind, id) => {
                    for view in this.sessions.values() {
                        if view.read(cx).summary.provider == kind.id()
                            && view.read(cx).settings.account.id == *id
                        {
                            view.update(cx, |v, cx| v.account_authenticated(cx));
                        }
                    }
                    if let Some(bots) = &this.bots {
                        bots.update(cx, |m, cx| m.account_changed(*kind, id, false, cx));
                    }
                    this.update_usage_provider(cx);
                }
                AccountEvent::Updated => this.update_usage_provider(cx),
                AccountEvent::PickerDismiss | AccountEvent::AuthStarted => {}
                AccountEvent::Manage => {
                    cx.spawn(async move |this, cx| {
                        let _ = this.update_in(cx, |this, window, cx| {
                            this.open_settings(crate::settings::Section::Providers, window, cx)
                        });
                    })
                    .detach();
                }
            },
        ));
    }
    fn select_provider_account(
        &mut self,
        kind: Kind,
        id: &str,
        project: &str,
        session: Option<i64>,
        cx: &mut Context<Self>,
    ) {
        let Some(center) = crate::accounts::get(cx) else {
            return;
        };
        if !center.read(cx).book.exists(kind, id) {
            return;
        }
        let profile = center.read(cx).profile(id);
        let view = session
            .and_then(|id| self.sessions.get(&id).cloned())
            .or_else(|| {
                self.bots
                    .as_ref()
                    .and_then(|m| m.read(cx).chat())
                    .filter(|v| Some(v.read(cx).summary.id) == session)
            });
        let Some(view) = view else {
            return;
        };
        let current = view.read(cx);
        if current.summary.provider != kind.id() || current.summary.cwd != project {
            return;
        }
        // The footer can be used while a restored/evicted conversation is
        // loading. Resolve its actual account and terminal error before
        // deciding whether the selection needs a new conversation.
        if current.needs_history_restore() {
            let owner = cx.weak_entity();
            let id = id.to_owned();
            let project = project.to_owned();
            let session = Some(current.summary.id);
            view.update(cx, |view, cx| {
                view.defer_history_action(
                    move |_, cx| {
                        cx.defer(move |cx| {
                            let _ = owner.update(cx, |shell, cx| {
                                shell.select_provider_account(kind, &id, &project, session, cx)
                            });
                        });
                    },
                    cx,
                );
            });
            return;
        }
        if current.settings.account.id == id {
            return;
        }
        if current.account_recovery_limit(cx).is_some() && !current.busy {
            view.update(cx, |v, cx| v.switch_usage_account(profile, cx));
            return;
        }
        if !current.summary.has_messages && !current.busy {
            view.update(cx, |v, cx| v.set_blank_account(profile, cx));
            self.update_usage_provider(cx);
            return;
        }
        let mut settings = current.settings.clone();
        settings.account = profile;
        settings.bot = None;
        settings.usage_limit = None;
        settings.handoff = None;
        settings.turns.clear();
        settings.revision = 0;
        let cwd = current.summary.cwd.clone();
        let store = self.store.clone();
        let revision = self.navigation_revision;
        let work = cx.background_executor().spawn(async move {
            let store = locked(&store)?;
            let summary = store.create(kind.id(), &cwd)?;
            store.save_agent_settings(summary.id, &settings)?;
            Ok::<_, String>((summary, settings))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok((summary, settings)) => {
                    let id = summary.id;
                    this.add(summary, cx);
                    this.sessions[&id].update(cx, |v, cx| v.install_transfer(settings, None, cx));
                    if this.navigation_revision == revision {
                        this.activate_session(id, window, cx);
                    } else {
                        this.invalidate_chrome(cx);
                        cx.notify();
                    }
                }
                Err(error) => {
                    this.error = Some(error);
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests;
