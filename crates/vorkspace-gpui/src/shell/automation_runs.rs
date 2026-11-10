//! Runs use the same conversation lifecycle as an ordinary native submission.
use super::*;
use vorkspace_core::automations::{self, DueAutomationRun, Service};

pub(super) type ServiceHandle = Arc<Mutex<Service>>;

pub(super) struct Running {
    run_id: String,
    submitting: bool,
    generation_before: u64,
}

#[derive(Default)]
pub(super) struct State {
    pub service: Option<ServiceHandle>,
    pub open_requested: Option<u64>,
    opening: bool,
    polling: bool,
    pending: HashSet<String>,
    running: HashMap<i64, Running>,
    reserved: HashSet<i64>,
    writes: Option<gpui::Task<()>>,
    events_polling: bool,
    pub session_automations: HashMap<i64, String>,
}

fn now() -> i64 {
    vorkspace_core::time::now_ms().min(i64::MAX as u64) as i64
}

fn service_lock(service: &ServiceHandle) -> Result<std::sync::MutexGuard<'_, Service>, String> {
    service
        .lock()
        .map_err(|_| "Automations are unavailable".into())
}

impl Shell {
    pub(super) fn automation_work_in_progress(&self) -> bool {
        let state = &self.automation_state;
        state.polling
            || state.events_polling
            || !state.running.is_empty()
            || !state.reserved.is_empty()
            || !state.pending.is_empty()
            || state.writes.as_ref().is_some_and(|task| !task.is_ready())
    }

    pub(super) fn initialize_automations(&mut self, cx: &mut Context<Self>) {
        if self.automation_state.opening || self.automation_state.service.is_some() {
            return;
        }
        self.automation_state.opening = true;
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            let path = locked(&store)?.database_path().to_path_buf();
            let service = Service::open(&path).map(|service| Arc::new(Mutex::new(service)))?;
            let sessions = locked(&store)?.automation_session_ids()?;
            Ok::<_, String>((service, sessions))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.automation_state.opening = false;
                match result {
                    Ok((service, sessions)) => {
                        this.automation_state.session_automations = sessions;
                        this.automation_state.service = Some(service);
                        if !cfg!(test) && this.holds_scheduler(cx) {
                            this.ensure_background_inbox(window, cx);
                        }
                        this.poll_automations(cx);
                        if this
                            .automation_state
                            .open_requested
                            .take()
                            .is_some_and(|revision| revision == this.navigation_revision)
                        {
                            this.open_automations(window, cx);
                        } else if this.navigation_revision == 0
                            && this.active_app_view.is_none()
                            && this.settings.is_none()
                        {
                            let saved = this.workspace.scope_focus.get(&this.tab_scope()).cloned();
                            if let Some(tab @ TitleTab::App(_)) = saved {
                                this.open_title_tab(&tab, window, cx);
                            }
                        }
                    }
                    Err(error) => {
                        this.error = Some(format!("Could not open automations: {error}"));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    pub(super) fn poll_automations(&mut self, cx: &mut Context<Self>) {
        if !self.holds_scheduler(cx) {
            return;
        }
        // A rejected or deferred submission still needs a terminal receipt.
        let sessions: Vec<_> = self.automation_state.running.keys().copied().collect();
        for id in sessions {
            self.automation_session_changed(id, cx);
        }
        let Some(service) = self.automation_state.service.clone() else {
            self.initialize_automations(cx);
            return;
        };
        if self.automation_state.polling {
            return;
        }
        self.automation_state.polling = true;
        let core = self.app_core.clone().expect("registered workspace window");
        let recover = !core.read(cx).automation_recovered;
        let cutoff = core.read(cx).automation_cutoff;
        let work = cx.background_executor().spawn(async move {
            let mut service = service_lock(&service)?;
            let has_events = service.list()?.iter().any(|automation| {
                automation.enabled
                    && automation.triggers.as_ref().is_some_and(|triggers| {
                        triggers.iter().any(|trigger| trigger.kind != "time")
                    })
            });
            let mut runs = if recover {
                service.recover(cutoff, now())?
            } else {
                Vec::new()
            };
            runs.extend(service.claim_due(now())?);
            Ok::<_, String>((runs, has_events))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.automation_state.polling = false;
                match result {
                    Ok((runs, has_events)) => {
                        core.update(cx, |core, _| core.automation_recovered = true);
                        for run in runs {
                            this.launch_automation(run, false, cx);
                        }
                        if has_events {
                            this.ensure_background_inbox(window, cx);
                        }
                        this.drain_inbox_automation_events(cx);
                    }
                    Err(error) => {
                        this.error = Some(format!("Could not evaluate automations: {error}"));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    pub(super) fn drain_inbox_automation_events(&mut self, cx: &mut Context<Self>) {
        let core = self.app_core.clone().expect("registered workspace window");
        if !self.holds_scheduler(cx) {
            return;
        }
        if self.automation_state.events_polling {
            return;
        }
        let items = core.read(cx).automation_events();
        if items.is_empty() {
            return;
        }
        let Some(service) = self.automation_state.service.clone() else {
            self.initialize_automations(cx);
            return;
        };
        self.automation_state.events_polling = true;
        let attempted = items.clone();
        let flush = core.read(cx).flush_pending(cx);
        let work = cx.background_executor().spawn(async move {
            // The core owns the pending snapshot through scheduler handoffs.
            flush.await?;
            let mut service = service_lock(&service)?;
            automations::claim_inbox_events_retry(&mut service, &items, now())
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.automation_state.events_polling = false;
                match result {
                    Ok((runs, retry)) => {
                        core.update(cx, |core, cx| {
                            core.settle_automation_events(attempted, retry, cx)
                        });
                        for run in runs {
                            this.launch_automation(run, false, cx);
                        }
                    }
                    Err(error) => {
                        this.error =
                            Some(format!("Could not evaluate connector automations: {error}"));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    pub(super) fn launch_automation(
        &mut self,
        due: DueAutomationRun,
        reveal: bool,
        cx: &mut Context<Self>,
    ) {
        if self.automation_state.pending.contains(&due.run.id)
            || self
                .automation_state
                .running
                .values()
                .any(|run| run.run_id == due.run.id)
        {
            return;
        }
        self.automation_state.pending.insert(due.run.id.clone());
        let automation = &due.automation;
        let kind = Kind::from_id(&automation.harness);
        let reuse = automation
            .reuse_session
            .then_some(automation.last_session_id.as_deref())
            .flatten()
            .and_then(|id| locked(&self.store).ok()?.resolve_existing_session_id(id))
            .filter(|id| {
                !self.automation_state.running.contains_key(id)
                    && !self.automation_state.reserved.contains(id)
            })
            .filter(|id| {
                self.session_record(*id, cx).is_some_and(|record| {
                    let summary = &record.summary;
                    summary.provider == automation.harness
                        && !summary.archived
                        && !record.draft
                        && self
                            .sessions
                            .get(id)
                            .is_none_or(|view| view.read(cx).available_for_managed_run(cx))
                        && match automation.workspace_mode.as_str() {
                            "current" => {
                                record.worktree().is_none()
                                    && vorkspace_core::files::path_key(&summary.cwd)
                                        == vorkspace_core::files::path_key(&automation.cwd)
                            }
                            "existing" => record.worktree().is_some_and(|cwd| {
                                vorkspace_core::files::path_key(cwd)
                                    == vorkspace_core::files::path_key(&automation.worktree_cwd)
                            }),
                            _ => false,
                        }
                })
            })
            .filter(|id| {
                self.ensure_session(*id, cx)
                    .is_some_and(|view| view.read(cx).available_for_managed_run(cx))
            });
        let account = crate::accounts::get(cx)
            .map(|accounts| accounts.read(cx).selected(kind, &automation.cwd))
            .unwrap_or_default();
        if let Some(id) = reuse {
            self.automation_state.reserved.insert(id);
        }
        let mut settings = reuse
            .and_then(|id| {
                self.sessions
                    .get(&id)
                    .map(|view| view.read(cx).settings.clone())
            })
            .unwrap_or_default();
        settings.account = account;
        settings.model = vorkspace_core::store::native_model(&automation.model, kind.id());
        settings.model_settings = automation.model_settings.clone();
        settings.effort = [
            "effort",
            "reasoning",
            "reasoningEffort",
            "reasoning_effort",
            "variant",
        ]
        .into_iter()
        .find_map(|key| automation.model_settings.get(key).cloned());
        settings.fast = automation
            .model_settings
            .get("fast")
            .is_some_and(|value| value == "true");
        settings.service_tier = automation
            .model_settings
            .get("serviceTier")
            .or_else(|| automation.model_settings.get("service_tier"))
            .cloned();
        settings.thinking = automation
            .model_settings
            .get("thinking")
            .and_then(|value| value.parse().ok());
        settings.access = vorkspace_core::agent::AccessMode::from_id(&automation.runtime_mode);
        settings.new_worktree = automation.workspace_mode == "worktree";
        settings.worktree_base = (automation.workspace_mode == "worktree").then(|| "HEAD".into());
        settings.worktree_cwd =
            (automation.workspace_mode == "existing").then(|| automation.worktree_cwd.clone());
        settings.plan = false;
        settings.operator = false;
        settings.orchestrator = false;
        settings.prepared_prompt = None;
        let store = self.store.clone();
        let launch = due.clone();
        let revision = self.navigation_revision;
        let mut install = self.session_install(cx);
        let work = cx.background_executor().spawn(async move {
            if Kind::parse(&launch.automation.harness).is_none() {
                return Err("The automation's agent is unavailable".into());
            }
            if !std::path::Path::new(&launch.automation.cwd).is_dir() {
                return Err("The automation's project folder is unavailable".into());
            }
            if launch.automation.workspace_mode == "existing"
                && !std::path::Path::new(&launch.automation.worktree_cwd).is_dir()
            {
                return Err("The automation's worktree is unavailable".into());
            }
            let store = locked(&store)?;
            let mut settings = settings;
            let summary = if let Some(id) = reuse {
                if let Some(previous) = store.agent_settings(id)? {
                    settings.turns = previous.turns;
                    settings.revision = previous.revision;
                }
                store
                    .sessions()?
                    .into_iter()
                    .find(|session| session.id == id)
                    .ok_or("The reusable session no longer exists")?
            } else {
                let mut summary =
                    store.create(&launch.automation.harness, &launch.automation.cwd)?;
                install.track(summary.id);
                if launch.run.trigger != "event" {
                    summary.title = launch.automation.name.clone();
                    store.title(summary.id, &summary.title)?;
                }
                summary
            };
            if reuse.is_none() {
                store.save_agent_settings(summary.id, &settings)?;
            }
            let persisted = store.persisted_session_id(summary.id)?;
            Ok::<_, String>((summary, settings, persisted, install))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.automation_state.pending.remove(&due.run.id);
                if let Some(id) = reuse {
                    this.automation_state.reserved.remove(&id);
                }
                match result {
                    Ok((summary, settings, persisted, mut install)) => {
                        let id = summary.id;
                        if reuse.is_some()
                            && this
                                .sessions
                                .get(&id)
                                .is_none_or(|view| !view.read(cx).available_for_managed_run(cx))
                        {
                            let mut retry = due.clone();
                            retry.automation.reuse_session = false;
                            this.launch_automation(retry, reveal, cx);
                            return;
                        }
                        let linked = automation_work_item(&due.run);
                        if let Err(error) =
                            this.verify_prepared_checkout(id, settings.worktree_cwd.as_deref(), cx)
                        {
                            if reuse.is_none() {
                                let _ = locked(&this.store)
                                    .and_then(|mut store| install.discard(&mut store, id));
                            }
                            this.update_automation_run(
                                due.run.id.clone(),
                                "failed",
                                None,
                                Some(error.clone()),
                                cx,
                            );
                            if reveal {
                                this.error = Some(error);
                                cx.notify();
                            }
                            return;
                        }
                        // The local lease and composer were both revalidated.
                        // A run that falls back must not relink the old session.
                        let linked_result = locked(&this.store).and_then(|store| {
                            store.set_automation_id(id, &due.automation.id)?;
                            if let Some(item) = &linked {
                                store.set_work_item(id, Some(item))?;
                            }
                            Ok(())
                        });
                        if let Err(error) = linked_result {
                            this.update_automation_run(
                                due.run.id.clone(),
                                "failed",
                                None,
                                Some(error.clone()),
                                cx,
                            );
                            if reveal {
                                this.error = Some(error);
                                cx.notify();
                            }
                            return;
                        }
                        let installed = if reuse.is_some() {
                            this.sessions.get(&id).cloned()
                        } else {
                            this.install_prepared(summary, &install, cx)
                        };
                        let Some(view) = installed else {
                            this.update_automation_run(
                                due.run.id.clone(),
                                "failed",
                                None,
                                Some("Could not install the automation session".into()),
                                cx,
                            );
                            return;
                        };
                        if let Some(linked) = linked {
                            view.update(cx, |view, _| view.summary.work_item = Some(linked));
                        }
                        this.automation_state
                            .session_automations
                            .insert(id, due.automation.id.clone());
                        this.automation_state.running.insert(
                            id,
                            Running {
                                run_id: due.run.id.clone(),
                                submitting: true,
                                generation_before: view.read(cx).submission_generation,
                            },
                        );
                        this.update_automation_run(
                            due.run.id.clone(),
                            "running",
                            Some(persisted),
                            None,
                            cx,
                        );
                        if !due.automation.session_folder_id.is_empty() {
                            this.persist_automation_folder(
                                &due.automation.cwd,
                                due.automation.session_folder_id.clone(),
                                id,
                                cx,
                            );
                        }
                        // Background runs append a tab without changing the focused project.
                        if !this.workspace.tabs.contains(&id) {
                            this.workspace.tabs.push(id);
                            this.workspace.title_order.push(TitleTab::Workspace(id));
                        }
                        let prompt = due
                            .run
                            .prompt
                            .clone()
                            .unwrap_or_else(|| due.automation.prompt.clone());
                        view.update(cx, |view, cx| view.quick_submit(prompt, settings, cx));
                        this.publish_prepared(&view, Some(&mut install), cx);
                        if let Some(run) = this.automation_state.running.get_mut(&id) {
                            run.submitting = false;
                        }
                        this.automation_session_changed(id, cx);
                        if reveal && this.navigation_revision == revision {
                            this.activate_session(id, window, cx);
                        }
                        this.persist(cx);
                    }
                    Err(error) => {
                        this.update_automation_run(
                            due.run.id.clone(),
                            "failed",
                            None,
                            Some(error.clone()),
                            cx,
                        );
                        if reveal {
                            this.error = Some(error);
                            cx.notify();
                        }
                    }
                }
            });
        })
        .detach();
    }

    pub(super) fn automation_session_changed(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(run) = self.automation_state.running.get(&id) else {
            return;
        };
        if run.submitting {
            return;
        }
        let Some(view) = self.sessions.get(&id) else {
            return;
        };
        let view = view.read(cx);
        if view.external_submission_pending() {
            return;
        }
        let cancelled = view.notification_was_cancelled();
        let (mut ok, mut message) = view.external_outcome(cx);
        if view.submission_generation == run.generation_before {
            ok = false;
            message = "The selected agent session could not start this run.".into();
        }
        let status = if cancelled {
            "cancelled"
        } else if ok {
            "succeeded"
        } else {
            "failed"
        };
        let run = self.automation_state.running.remove(&id).unwrap();
        self.update_automation_run(
            run.run_id,
            status,
            None,
            (!ok && !cancelled).then_some(message),
            cx,
        );
    }

    fn update_automation_run(
        &mut self,
        run_id: String,
        status: &str,
        session: Option<String>,
        error: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(service) = self.automation_state.service.clone() else {
            return;
        };
        let previous = self.automation_state.writes.take();
        let status = status.to_string();
        self.automation_state.writes = Some(cx.spawn(async move |this, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            let work = cx.background_executor().spawn(async move {
                service_lock(&service)?.update_run(&run_id, &status, session, error, now())
            });
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = result {
                    this.error = Some(format!("Could not save automation result: {error}"));
                }
                if let Some(view) = &this.automations {
                    view.update(cx, |view, cx| view.refresh(cx));
                }
                cx.notify();
            });
        }));
    }

    fn persist_automation_folder(
        &mut self,
        cwd: &str,
        folder: String,
        id: i64,
        cx: &mut Context<Self>,
    ) {
        let cwd = cwd.to_string();
        let store = self.store.clone();
        let previous = self.group_writes.take();
        self.group_writes = Some(cx.spawn(async move |this, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            let project = cwd.clone();
            let work = cx.background_executor().spawn(async move {
                locked(&store)?.edit_groups(&project, |groups| {
                    groups.add_to_folder(&folder, id);
                    Ok(())
                })
            });
            let result = work.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => this.publish_groups(cwd, cx),
                Err(error) => {
                    this.error = Some(error);
                    cx.notify();
                }
            });
        }));
    }
}

fn automation_work_item(
    run: &vorkspace_core::automations::AutomationRun,
) -> Option<vorkspace_core::session_groups::WorkItem> {
    if run.trigger != "event" || run.event_kind.as_deref() != Some("github") {
        return None;
    }
    let mut parts = run.event_key.as_deref()?.split(':');
    if parts.next()? != "github" {
        return None;
    }
    let kind = match parts.next()? {
        "pr" => "pull",
        "issue" => "issues",
        _ => return None,
    };
    let repo = parts.next()?;
    let number = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    vorkspace_core::session_groups::parse_github_work_item(&format!(
        "https://github.com/{repo}/{kind}/{number}"
    ))
}
