//! Runs use the same conversation lifecycle as an ordinary native submission.
use super::*;
use crate::conversation::composer_features::ManagedAvailability;
use vorkspace_core::{
    accounts::Profile,
    agent::Settings,
    automations::{self, Automation, DueAutomationRun, Service},
    files::path_key,
};

pub(super) type ServiceHandle = Arc<Mutex<Service>>;

pub(super) struct Running {
    run_id: String,
    submitting: bool,
    generation_before: u64,
}

struct Launch {
    due: DueAutomationRun,
    reveal: bool,
    navigation: u64,
}

#[derive(Clone, Copy)]
struct Candidate {
    id: i64,
    entity: gpui::EntityId,
}

struct WaitingForReuse {
    launch: Launch,
    candidate: Candidate,
    _changes: gpui::Subscription,
}

struct NewSession {
    summary: Session,
    settings: Settings,
    install: crate::session_install::Install,
}

#[derive(Default)]
pub(super) struct State {
    pub service: Option<ServiceHandle>,
    pub open_requested: Option<u64>,
    opening: bool,
    polling: bool,
    pending: HashSet<String>,
    running: HashMap<i64, Running>,
    pub(super) reserved: HashSet<i64>,
    waiting_for_reuse: HashMap<i64, WaitingForReuse>,
    writes: Option<gpui::Task<()>>,
    events_polling: bool,
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
        let core = self.app_core.clone().expect("registered workspace window");
        if let Some(service) = core.update(cx, |core, cx| core.request_automation_service(cx)) {
            self.automation_service_ready(Ok(service), cx);
        }
    }

    pub(super) fn automation_service_ready(
        &mut self,
        result: Result<ServiceHandle, String>,
        cx: &mut Context<Self>,
    ) {
        if self.automation_state.service.is_some() {
            return;
        }
        let requested = std::mem::take(&mut self.automation_state.opening);
        match result {
            Ok(service) => self.automation_state.service = Some(service),
            Err(error) => {
                if requested {
                    self.error = Some(format!("Could not open automations: {error}"));
                    cx.notify();
                }
                return;
            }
        }
        cx.spawn(async move |this, cx| {
            let _ = this.update_in(cx, |this, window, cx| {
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
            let mut runs = if recover {
                service.recover(cutoff, now())?
            } else {
                Vec::new()
            };
            runs.extend(service.claim_due(now())?);
            Ok::<_, String>(runs)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.automation_state.polling = false;
                match result {
                    Ok(runs) => {
                        core.update(cx, |core, _| core.automation_recovered = true);
                        for run in runs {
                            this.launch_automation(run, false, cx);
                        }
                        if !cfg!(test) && this.inbox.is_none() {
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
        if self
            .automation_state
            .running
            .values()
            .any(|run| run.run_id == due.run.id)
            || !self.automation_state.pending.insert(due.run.id.clone())
        {
            return;
        }
        let launch = Launch {
            due,
            reveal,
            navigation: self.navigation_revision,
        };
        let Some(view) = self.automation_reuse_view(&launch.due.automation, cx) else {
            self.prepare_automation(launch, None, cx);
            return;
        };
        let candidate = Candidate {
            id: view.read(cx).summary.id,
            entity: view.entity_id(),
        };
        match view.read(cx).managed_availability(cx) {
            ManagedAvailability::Available => {
                self.automation_state.reserved.insert(candidate.id);
                self.prepare_automation(launch, Some(candidate), cx);
            }
            ManagedAvailability::Pending => {
                self.automation_state.reserved.insert(candidate.id);
                let changes = cx.observe(&view, move |this, _, cx| {
                    this.resume_automation_reuse(candidate.id, cx);
                });
                self.automation_state.waiting_for_reuse.insert(
                    candidate.id,
                    WaitingForReuse {
                        launch,
                        candidate,
                        _changes: changes,
                    },
                );
                view.update(cx, |view, cx| view.load_complete(cx));
            }
            ManagedAvailability::Unavailable => self.prepare_automation(launch, None, cx),
        }
    }

    fn automation_reuse_view(
        &mut self,
        automation: &Automation,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Conversation>> {
        if !automation.reuse_session
            || !matches!(automation.workspace_mode.as_str(), "current" | "existing")
        {
            return None;
        }
        let original = automation.last_session_id.as_deref()?;
        let id = locked(&self.store)
            .ok()?
            .resolve_existing_session_id(original)?;
        if self.automation_state.running.contains_key(&id)
            || self.automation_state.reserved.contains(&id)
            || !self.session_available_here(id, cx)
        {
            return None;
        }
        let record = self.session_record(id, cx)?;
        let summary = &record.summary;
        if summary.provider != automation.harness
            || summary.archived
            || record.draft
            || path_key(&summary.cwd) != path_key(&automation.cwd)
            || record.checkout.as_ref().is_some_and(|worktree| {
                worktree.as_deref().map(path_key) != requested_worktree(automation).map(path_key)
            })
        {
            return None;
        }
        self.ensure_session(id, cx)
    }

    pub(super) fn resume_automation_reuse(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(waiting) = self.automation_state.waiting_for_reuse.get(&id) else {
            return;
        };
        let candidate = waiting.candidate;
        let readiness = self
            .automation_candidate(candidate, cx)
            .map_or(ManagedAvailability::Unavailable, |view| {
                view.read(cx).managed_availability(cx)
            });
        if readiness == ManagedAvailability::Pending {
            return;
        }
        let waiting = self.automation_state.waiting_for_reuse.remove(&id).unwrap();
        if readiness == ManagedAvailability::Available {
            self.prepare_automation(waiting.launch, Some(candidate), cx);
        } else {
            self.automation_state.reserved.remove(&id);
            self.prepare_automation(waiting.launch, None, cx);
        }
    }

    fn automation_candidate(
        &self,
        candidate: Candidate,
        cx: &Context<Self>,
    ) -> Option<Entity<Conversation>> {
        if !self.session_available_here(candidate.id, cx) {
            return None;
        }
        self.sessions
            .get(&candidate.id)
            .filter(|view| view.entity_id() == candidate.entity)
            .cloned()
    }

    fn prepare_automation(
        &mut self,
        launch: Launch,
        reuse: Option<Candidate>,
        cx: &mut Context<Self>,
    ) {
        let automation = &launch.due.automation;
        let fresh = reuse.is_none().then(|| {
            let account = automation_account(automation, cx);
            (
                automation_settings(automation, account, Settings::default()),
                self.session_install(cx),
            )
        });
        let store = self.store.clone();
        let title = (launch.due.run.trigger != "event").then(|| automation.name.clone());
        let project = automation.cwd.clone();
        let checkout = requested_worktree(automation).map(str::to_owned);
        let provider = automation.harness.clone();
        let work = cx.background_executor().spawn(async move {
            if Kind::parse(&provider).is_none() {
                return Err("The automation's agent is unavailable".into());
            }
            if !std::path::Path::new(&project).is_dir() {
                return Err("The automation's project folder is unavailable".into());
            }
            if checkout
                .as_deref()
                .is_some_and(|path| !std::path::Path::new(path).is_dir())
            {
                return Err("The automation's worktree is unavailable".into());
            }
            let Some((settings, mut install)) = fresh else {
                return Ok(None);
            };
            let mut store = locked(&store)?;
            let mut summary = store.create(&provider, &project)?;
            install.track(summary.id);
            let prepared = (|| {
                if let Some(title) = title {
                    store.title(summary.id, &title)?;
                    summary.title = title;
                }
                store.save_agent_settings(summary.id, &settings)
            })();
            if let Err(error) = prepared {
                return Err(discard_failed_session(
                    &mut store,
                    &mut install,
                    summary.id,
                    error,
                ));
            }
            Ok::<_, String>(Some(NewSession {
                summary,
                settings,
                install,
            }))
        });
        cx.spawn(async move |this, cx| {
            let prepared = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_automation_preparation(launch, reuse, prepared, window, cx);
            });
        })
        .detach();
    }

    fn finish_automation_preparation(
        &mut self,
        launch: Launch,
        reuse: Option<Candidate>,
        prepared: Result<Option<NewSession>, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(candidate) = reuse {
            self.automation_state.reserved.remove(&candidate.id);
        }
        let created = match prepared {
            Ok(created) => created,
            Err(error) => {
                self.automation_launch_failed(launch, error, cx);
                return;
            }
        };
        let automation = &launch.due.automation;
        let checkout = requested_worktree(automation);
        let (summary, settings, mut install, view) = if let Some(created) = created {
            (
                created.summary,
                created.settings,
                Some(created.install),
                None,
            )
        } else {
            let account = automation_account(automation, cx);
            let reusable = reuse.and_then(|candidate| self.automation_candidate(candidate, cx));
            let Some(view) = reusable.filter(|view| {
                let chat = view.read(cx);
                chat.managed_availability(cx) == ManagedAvailability::Available
                    && chat.settings.account == account
                    && chat.summary.provider == automation.harness
                    && path_key(&chat.summary.cwd) == path_key(&automation.cwd)
                    && chat.settings.worktree_cwd.as_deref().map(path_key) == checkout.map(path_key)
            }) else {
                self.prepare_automation(launch, None, cx);
                return;
            };
            let chat = view.read(cx);
            let settings = automation_settings(automation, account, chat.settings.clone());
            (chat.summary.clone(), settings, None, Some(view))
        };
        let id = summary.id;
        let linked = automation_work_item(&launch.due.run);
        let verified = if self.project_is_deleting(&automation.cwd, cx) {
            Err("The automation's project is being removed.".into())
        } else {
            self.verify_prepared_checkout(id, checkout, cx)
        };
        if reuse.is_some() && verified.is_err() {
            self.prepare_automation(launch, None, cx);
            return;
        }
        let linked_result = verified.and_then(|_| {
            let store = locked(&self.store)?;
            store
                .link_automation_session(
                    id,
                    &automation.id,
                    &automation.harness,
                    &automation.cwd,
                    checkout,
                    linked.as_ref(),
                )?
                .then(|| store.persisted_session_id(id))
                .transpose()
        });
        let persisted = match linked_result {
            Ok(Some(persisted)) => persisted,
            Ok(None) if reuse.is_some() => {
                self.prepare_automation(launch, None, cx);
                return;
            }
            Ok(None) => {
                let error = self.discard_uninstalled_automation(
                    id,
                    install.as_mut(),
                    "The automation session changed before it could start".into(),
                );
                self.automation_launch_failed(launch, error, cx);
                return;
            }
            Err(error) => {
                let error = self.discard_uninstalled_automation(id, install.as_mut(), error);
                self.automation_launch_failed(launch, error, cx);
                return;
            }
        };
        let installed = if let Some(install) = &install {
            self.install_prepared(summary, install, cx)
        } else {
            view
        };
        let Some(view) = installed else {
            let error = self.discard_uninstalled_automation(
                id,
                install.as_mut(),
                "Could not install the automation session".into(),
            );
            self.automation_launch_failed(launch, error, cx);
            return;
        };
        if let Some(linked) = linked {
            view.update(cx, |view, _| view.summary.work_item = Some(linked));
        }
        let due = launch.due;
        self.automation_state.pending.remove(&due.run.id);
        self.automation_state.running.insert(
            id,
            Running {
                run_id: due.run.id.clone(),
                submitting: true,
                generation_before: view.read(cx).submission_generation,
            },
        );
        self.update_automation_run(due.run.id, "running", Some(persisted), None, cx);
        if !due.automation.session_folder_id.is_empty() {
            self.persist_automation_folder(
                &due.automation.cwd,
                due.automation.session_folder_id,
                id,
                cx,
            );
        }
        // Background runs append a tab without changing the focused project.
        if !self.workspace.tabs.contains(&id) {
            self.workspace.tabs.push(id);
            self.workspace.title_order.push(TitleTab::Workspace(id));
        }
        let prompt = due.run.prompt.unwrap_or(due.automation.prompt);
        view.update(cx, |view, cx| view.quick_submit(prompt, settings, cx));
        self.publish_prepared(&view, install.as_mut(), cx);
        if let Some(run) = self.automation_state.running.get_mut(&id) {
            run.submitting = false;
        }
        self.automation_session_changed(id, cx);
        if launch.reveal && self.navigation_revision == launch.navigation {
            self.activate_session(id, window, cx);
        }
        self.persist(cx);
    }

    fn automation_launch_failed(&mut self, launch: Launch, error: String, cx: &mut Context<Self>) {
        self.automation_state.pending.remove(&launch.due.run.id);
        if launch.reveal {
            self.error = Some(error.clone());
            cx.notify();
        }
        self.update_automation_run(launch.due.run.id, "failed", None, Some(error), cx);
    }

    fn discard_uninstalled_automation(
        &self,
        id: i64,
        install: Option<&mut crate::session_install::Install>,
        error: String,
    ) -> String {
        let Some(install) = install else {
            return error;
        };
        match locked(&self.store) {
            Ok(mut store) => discard_failed_session(&mut store, install, id, error),
            Err(cleanup) => {
                format!("{error}. Could not remove prepared session: {cleanup}")
            }
        }
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

fn discard_failed_session(
    store: &mut vorkspace_core::store::Store,
    install: &mut crate::session_install::Install,
    id: i64,
    error: String,
) -> String {
    match install.discard(store, id) {
        Ok(()) => error,
        Err(cleanup) => format!("{error}. Could not remove prepared session: {cleanup}"),
    }
}

fn requested_worktree(automation: &Automation) -> Option<&str> {
    (automation.workspace_mode == "existing").then_some(automation.worktree_cwd.as_str())
}

fn automation_account(automation: &Automation, cx: &App) -> Profile {
    crate::accounts::get(cx)
        .map(|accounts| {
            accounts
                .read(cx)
                .selected(Kind::from_id(&automation.harness), &automation.cwd)
        })
        .unwrap_or_default()
}

fn automation_settings(
    automation: &Automation,
    account: Profile,
    mut settings: Settings,
) -> Settings {
    settings.account = account;
    settings.model = vorkspace_core::store::native_model(&automation.model, &automation.harness);
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
    settings.worktree_base = settings.new_worktree.then(|| "HEAD".into());
    settings.worktree_cwd = requested_worktree(automation).map(str::to_owned);
    settings.plan = false;
    settings.operator = false;
    settings.orchestrator = false;
    settings.prepared_prompt = None;
    settings
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
