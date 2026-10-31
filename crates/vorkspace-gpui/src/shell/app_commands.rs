//! Operator uses the existing authenticated, per-session local app bridge.
use super::*;
use serde_json::{Value, json};
use vorkspace_core::{
    agent::Settings,
    bot::bridge::{Call, Server},
};

pub(crate) struct AppStart {
    pub summary: Session,
    pub settings: Settings,
    pub prompt: String,
    pub draft: bool,
    pub reveal: bool,
    pub placement: Option<(i64, Edge)>,
    pub cleanup: Option<vorkspace_core::bot::launch::CreatedWorktree>,
}
enum Effect {
    None,
    /// A board changed; open views reload.
    Todos,
    Start(Box<AppStart>),
    Groups(String, vorkspace_core::session_groups::Groups),
}
impl Shell {
    fn app_caller_allowed(&self, caller: i64, project: &str, cx: &App) -> bool {
        self.sessions.get(&caller).is_some_and(|view| {
            let view = view.read(cx);
            view.settings.operator
                && !view.settings.plan
                && view.settings.access != vorkspace_core::agent::AccessMode::ReadOnly
                && vorkspace_core::files::path_key(&view.summary.cwd)
                    == vorkspace_core::files::path_key(project)
        }) || self
            .bots
            .as_ref()
            .is_some_and(|bots| bots.read(cx).app_project_allowed(caller, project, cx))
    }
    fn check_app_placement(
        &self,
        placement: Option<(i64, Edge)>,
        project: &str,
        cx: &App,
    ) -> Result<(), String> {
        if let Some((target, edge)) = placement {
            if self.workspace.tab_of(target).is_none()
                || !self.session_summaries.contains_key(&target)
            {
                return Err("Split target must be an open session pane".into());
            }
            if vorkspace_core::files::path_key(&self.session_metadata(target, cx).summary.cwd)
                != vorkspace_core::files::path_key(project)
            {
                return Err("Split target is outside the selected project".into());
            }
            if !self.placement_allowed(DragSource::Session(i64::MAX), target, edge) {
                return Err(PANE_LIMIT_MESSAGE.into());
            }
        }
        Ok(())
    }
    pub(crate) fn app_placement(
        &self,
        caller: i64,
        project: &str,
        input: &Value,
        cx: &App,
    ) -> Result<Option<(i64, Edge)>, String> {
        let edge = match input
            .get("placement")
            .and_then(Value::as_str)
            .unwrap_or("tab")
        {
            "tab" => return Ok(None),
            "right" => Edge::Right,
            "down" => Edge::Bottom,
            _ => return Err("placement must be tab, right or down".into()),
        };
        let target = if let Some(target) = input.get("besideSessionId") {
            session_id(&json!({"sessionId":target}), &self.store)?
        } else {
            caller
        };
        let placement = Some((target, edge));
        self.check_app_placement(placement, project, cx)?;
        Ok(placement)
    }
    /// Layout and launch share this owner. A target closed during preparation
    /// fails before any provider is started; it never falls back to another tab.
    pub(crate) fn app_install_start(
        &mut self,
        caller: i64,
        start: AppStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<i64, String> {
        let AppStart {
            summary,
            settings,
            prompt,
            draft,
            reveal,
            placement,
            cleanup,
        } = start;
        let id = summary.id;
        // Preparation ran in the background. A view opened since then belongs
        // to its newer owner, including any input it has already collected.
        if self.sessions.contains_key(&id) || self.workspace.tab_of(id).is_some() {
            return Err(format!(
                "New session {id} was already opened; its state and working copy were preserved"
            ));
        }
        let valid = if !self.app_caller_allowed(caller, &summary.cwd, cx) {
            Err("The caller no longer owns this project's app capability".into())
        } else {
            self.check_app_placement(placement, &summary.cwd, cx)
        };
        if let Err(error) = valid {
            return Err(self.rollback_app_start(id, cleanup, error, cx));
        }
        // Pane, rail and caller links publish only after the fresh view owns a
        // durable draft or queued prompt. No provider starts on a failed write.
        let observations_before = self.observations.len();
        self.add_loaded(summary.clone(), cx);
        let accepted = self
            .sessions
            .get(&id)
            .cloned()
            .ok_or_else(|| "New session view is unavailable".to_owned())
            .and_then(|view| {
                view.update(cx, |view, cx| {
                    view.install_transfer(settings, None, cx);
                    if draft {
                        view.input
                            .update(cx, |input, cx| input.set_text(prompt, cx));
                        view.app_flush_draft(cx)?;
                    } else {
                        view.app_start_message(prompt, cx)?;
                    }
                    Ok(())
                })
            });
        if let Err(error) = accepted {
            // Shell owns these subscriptions independently of the discarded
            // view. Nothing else can register one during this synchronous call.
            self.observations.truncate(observations_before);
            return Err(self.rollback_app_start(id, cleanup, error, cx));
        }
        self.workspace.restore_project(&summary.cwd);
        self.started_by.insert(id, caller);
        if let Some((target, edge)) = placement {
            let before = self.active_leaves();
            self.workspace.place(id, target, edge);
            self.fit_active_pane_width();
            self.begin_pane_motion(before);
            self.fit_sidebar(cx);
            self.activate_session(id, window, cx);
        } else if reveal {
            self.open(id, window, cx);
        }
        self.invalidate_chrome(cx);
        cx.notify();
        Ok(id)
    }
    fn rollback_app_start(
        &mut self,
        id: i64,
        cleanup: Option<vorkspace_core::bot::launch::CreatedWorktree>,
        error: String,
        cx: &mut Context<Self>,
    ) -> String {
        if let Some(view) = self.sessions.get(&id) {
            view.update(cx, |view, _| view.discard_unaccepted_app_start());
        }
        self.forget_session(id, cx);
        self.started_by.remove(&id);
        self.session_worktrees.remove(&id);
        let removed = locked(&self.store).and_then(|mut store| store.delete(id));
        let cleanup_error = match removed {
            Ok(()) => cleanup
                .map(vorkspace_core::bot::launch::CreatedWorktree::rollback)
                .transpose()
                .err(),
            // Retain the checkout if its session could not be removed, so the
            // persisted settings still identify the preserved resource.
            Err(error) => Some(format!(
                "unused session {id} could not be removed; its working copy was preserved: {error}"
            )),
        };
        self.invalidate_chrome(cx);
        cx.notify();
        format!(
            "{error}{}",
            cleanup_error
                .map(|error| format!("; {error}"))
                .unwrap_or_default()
        )
    }
    fn check_live_app_target(&self, id: i64, cx: &App) -> Result<(), String> {
        if self.orchestration_worker(id)
            || self
                .bots
                .as_ref()
                .is_some_and(|bots| bots.read(cx).protected_app_target(id, cx))
        {
            return Err("Bot, habit, channel, hidden and orchestration sessions are protected from app commands".into());
        }
        Ok(())
    }
    /// Both command dispatchers enter here after deferral. Validate the live
    /// owner and the persisted classification immediately before any effect.
    pub(crate) fn app_session_action(
        &mut self,
        caller: i64,
        id: i64,
        project: &str,
        action: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let authorized = self.app_caller_allowed(caller, project, cx);
        if !authorized {
            return Err("The caller no longer owns this project's app capability".into());
        }
        locked(&self.store)?.check_app_target(caller, id, project)?;
        self.check_live_app_target(id, cx)?;
        if self.archiving.contains(&id) || self.deleting.contains(&id) {
            return Err("Session management is already in progress".into());
        }
        if action == "sessions.delete"
            && self.workspace.review_owners.iter().any(|(pane, owner)| {
                *owner == id
                    && (self.workspace.tab_of(*pane).is_some()
                        || self
                            .reviews
                            .get(pane)
                            .is_some_and(|review| review.read(cx).unsaved_files(cx)))
            })
        {
            return Err("Close this session's detached file panes before deleting it".into());
        }
        if let Some(view) = self.sessions.get(&id) {
            view.update(cx, |view, cx| view.app_stop(cx))?;
        } else {
            let store = locked(&self.store)?;
            let mut queue = store.follow_ups(id)?;
            queue.paused = true;
            queue.revision += 1;
            store.accept_follow_ups(id, &queue)?;
        }
        if action == "sessions.stop" {
            return Ok(json!({"stopping":true}));
        }
        if !matches!(action, "sessions.archive" | "sessions.delete") {
            return Err("Unsupported session management action".into());
        }
        // There is no await between the live-owner check and this transaction.
        // Store repeats the durable check under the mutation's write lock.
        if action == "sessions.delete" {
            locked(&self.store)?.delete_app_session(caller, id, project)?;
        } else {
            locked(&self.store)?.archive_app_session(caller, id, project)?;
        }
        if let Some(summary) = self.session_summaries.get_mut(&id) {
            summary.archived = true;
        }
        if let Some(view) = self.sessions.get(&id) {
            view.update(cx, |view, _| view.summary.archived = true);
        }
        if self.workspace.tab_of(id).is_some() {
            self.close_pane(id, window, cx);
        }
        if action == "sessions.delete" {
            self.forget_session(id, cx);
        } else {
            self.refresh_session_mark(id, cx);
        }
        self.invalidate_chrome(cx);
        cx.notify();
        Ok(json!({"accepted":true}))
    }
    pub(super) fn init_app_bridge(&mut self, cx: &mut Context<Self>) {
        // GPUI's deterministic executor cannot be woken by an OS socket thread.
        // Command tests exercise the same dispatcher directly without a listener.
        if cx.is_test() {
            return;
        }
        match Server::start() {
            Ok((server, calls)) => {
                self.app_bridge = Some(server);
                cx.spawn(async move |this, cx| {
                    while let Ok(call) = calls.recv().await {
                        if this
                            .update_in(cx, |this, window, cx| {
                                this.handle_app_call(call, window, cx)
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                })
                .detach();
            }
            Err(error) => self.error = Some(format!("Operator app access: {error}")),
        }
    }
    fn finish_app_call(&mut self, call: Call, result: Result<Value, String>) {
        let saved = if vorkspace_core::bot::api::is_read_only(&call.action) {
            Ok(())
        } else {
            locked(&self.store).and_then(|store| {
                store.finish_app_command(
                    call.session,
                    &call.request_id,
                    &call.action,
                    &call.input,
                    &result,
                )
            })
        };
        let _ = call.reply.send(saved.and(result));
    }
    fn app_background<F>(&mut self, call: Call, work: F, cx: &mut Context<Self>)
    where
        F: FnOnce(Database) -> Result<(Value, Effect), String> + Send + 'static,
    {
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move { work(store) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let caller = call.session;
                let result = result.and_then(|(result, effect)| {
                    match effect {
                        Effect::None => (),
                        Effect::Todos => crate::todos::changed(cx),
                        Effect::Start(start) => {
                            this.app_install_start(caller, *start, window, cx)?;
                        }
                        Effect::Groups(project, groups) => {
                            this.groups.insert(project, groups);
                            this.invalidate_chrome(cx);
                        }
                    }
                    cx.notify();
                    Ok(result)
                });
                this.finish_app_call(call, result);
            });
        })
        .detach();
    }
    fn handle_app_call(&mut self, call: Call, window: &mut Window, cx: &mut Context<Self>) {
        let caller = self.sessions.get(&call.session).map(|v| v.read(cx));
        let Some(caller) = caller.filter(|v| v.settings.operator) else {
            let _ = call
                .reply
                .send(Err("Operator is not enabled for this thread.".into()));
            return;
        };
        if let Err(error) = vorkspace_core::bot::api::validate(&call.action, &call.input) {
            let _ = call.reply.send(Err(error));
            return;
        }
        let project = caller.summary.cwd.clone();
        if (caller.settings.plan
            || caller.settings.access == vorkspace_core::agent::AccessMode::ReadOnly)
            && !matches!(
                call.action.as_str(),
                "models.list"
                    | "sessions.list"
                    | "sessions.read"
                    | "folders.list"
                    | "notes.list"
                    | "notes.read"
                    | "todos.list"
                    | "worktrees.list"
            )
        {
            let _ = call.reply.send(Err(
                "This thread's access policy allows read-only app actions.".into(),
            ));
            return;
        }
        if call
            .input
            .get("project")
            .and_then(Value::as_str)
            .is_some_and(|p| {
                vorkspace_core::files::path_key(p) != vorkspace_core::files::path_key(&project)
            })
        {
            let _ = call
                .reply
                .send(Err("Operator can only access this thread's project.".into()));
            return;
        }
        let mut inherited = caller.settings.clone();
        inherited.operator = false;
        inherited.orchestrator = false;
        inherited.environment.clear();
        inherited.attachments.clear();
        inherited.handoff = None;
        inherited.turns.clear();
        let provider = caller.summary.provider.clone();
        let needs_admission = locked(&self.store)
            .and_then(|store| store.app_command_known(call.session, &call.request_id))
            .map(|known| known || !vorkspace_core::bot::api::is_read_only(&call.action));
        let needs_admission = match needs_admission {
            Ok(value) => value,
            Err(error) => {
                let _ = call.reply.send(Err(error));
                return;
            }
        };
        if needs_admission {
            let admission = locked(&self.store).and_then(|store| {
                store.admit_app_command(call.session, &call.request_id, &call.action, &call.input)
            });
            match admission {
                Ok(vorkspace_core::store::AppCommandAdmission::Admitted) => (),
                Ok(vorkspace_core::store::AppCommandAdmission::Completed(result)) => {
                    let _ = call.reply.send(result);
                    return;
                }
                Err(error) => {
                    let _ = call.reply.send(Err(error));
                    return;
                }
            }
        }
        let input = call.input.clone();
        let action = call.action.clone();
        let result = match action.as_str() {
            "sessions.list" => Ok(json!(self.order.iter().map(|&id| self.session_metadata(id, cx).summary).filter(|s| s.cwd == project).map(|s| json!({"sessionId":s.id,"title":s.title,"harness":s.provider,"project":s.cwd,"archived":s.archived})).collect::<Vec<_>>())),
            "models.list" => Ok(json!(Kind::ALL.into_iter().map(|kind| json!({"harness":kind.id(),"name":kind.label(),"models":self.sessions.values().find(|v| { let v=v.read(cx); let profile=if kind.id()==provider { inherited.account.clone() } else { crate::accounts::get(cx).map(|center|center.read(cx).selected(kind,&project)).unwrap_or_default() }; v.summary.provider == kind.id() && v.settings.account.id==profile.id }).map(|v| v.read(cx).settings_models()).unwrap_or_default()})).collect::<Vec<_>>())),
            "folders.list" => Ok(json!(self.groups.get(&project).cloned().unwrap_or_default().folders)),
            "sessions.stop" | "sessions.archive" | "sessions.delete" => {
                session_id(&input,&self.store).and_then(|id|self.app_session_action(call.session,id,&project,&action,window,cx))
            }
            "sessions.send" | "sessions.draft" => {
                session_id(&input,&self.store).and_then(|id|{
                    locked(&self.store)?.check_app_target(call.session,id,&project)?;
                    self.check_live_app_target(id,cx)?;
                    let view=self.ensure_session(id,cx).ok_or("Session is unavailable")?;
                    view.update(cx,|view,cx|view.load(cx));
                    if !view.read(cx).ready_for_external(cx) { return Err("Session is busy, loading, or has an unsent draft. Its draft was preserved.".into()); }
                    let prompt=text(&input,"prompt",65536)?;
                    view.update(cx,|view,cx|{
                        if action=="sessions.draft" { view.input.update(cx,|input,cx|input.set_text(prompt,cx));view.app_flush_draft(cx)?; }
                        else { view.app_send(prompt,cx)?; }
                        Ok::<_,String>(())
                    })?;
                    Ok(json!({"accepted":true}))
                })
            }
            "sessions.start" => {
                let placement=match self.app_placement(call.session,&project,&input,cx){Ok(placement)=>placement,Err(error)=>{self.finish_app_call(call,Err(error));return;}};
                let mut launch=match vorkspace_core::bot::launch::Launch::parse(&input,&provider,&project,&project,inherited,true) {
                    Ok(launch)=>launch,Err(error)=>{self.finish_app_call(call,Err(error));return;}
                };
                if launch.provider.id()!=provider { launch.settings.account=crate::accounts::get(cx).map(|center|center.read(cx).selected(launch.provider,&project)).unwrap_or_default(); }
                let caller=call.session;
                self.app_background(call,move|store|{
                    let mut store=locked(&store)?;
                    let summary=launch.create_started(&mut store,&project,caller)?;
                    Ok((json!({"sessionId":summary.id,"accepted":true}),Effect::Start(Box::new(AppStart{summary,settings:launch.settings,prompt:launch.prompt,draft:launch.draft,reveal:launch.reveal,placement,cleanup:launch.created_worktree}))))
                },cx);return;
            }
            "sessions.read" => {
                let id = match session_id(&input, &self.store) { Ok(id) => id, Err(e) => { self.finish_app_call(call, Err(e)); return; } };
                if !self.session_summaries.contains_key(&id) || self.session_metadata(id, cx).summary.cwd != project { self.finish_app_call(call, Err("Session is outside this project.".into())); return; }
                let limit = input.get("limit").and_then(Value::as_u64).unwrap_or(40).clamp(1,200) as usize;
                self.app_background(call, move |store| {
                    let transcript = locked(&store)?.load(id)?;
                    let end=input.get("before").and_then(Value::as_u64).unwrap_or(transcript.rows.len() as u64).min(transcript.rows.len() as u64) as usize;
                    let start=end.saturating_sub(limit);
                    let max=input.get("maxChars").and_then(Value::as_u64).unwrap_or(24000).clamp(100,200000) as usize;
                    let rows:Vec<_>=transcript.rows[start..end].iter().map(|row|json!({"role":row.role,"text":vorkspace_core::response::limit(&row.text,max/limit.max(1))})).collect();
                    Ok((json!({"sessionId":id,"messages":rows,"before":start,"hasMore":start>0}),Effect::None))
                }, cx); return;
            }
            "notes.list" | "notes.read" | "notes.write" => {
                let caller = call.session;
                self.app_background(call, move |store| {
                    let store = locked(&store)?;
                    let notes = store.notes()?;
                    let result = match action.as_str() {
                        "notes.list" => json!(notes.iter().filter(|n| n.source_cwd == project).map(|n| json!({"id":n.id,"title":n.title})).collect::<Vec<_>>()),
                        "notes.read" => { let id = number(&input, "id")?; json!(notes.iter().find(|n| n.id == id && n.source_cwd == project).ok_or("Note not found in this project.")?) },
                        _ => {
                            let id = input.get("id").and_then(Value::as_u64).unwrap_or_else(vorkspace_core::time::now_ms);
                            if notes.iter().any(|n| n.id == id && n.source_cwd != project) { return Err("Note is outside this project.".into()); }
                            let note = vorkspace_core::response::Note { id, title: text(&input,"title",512)?, body: text(&input,"body",256*1024)?, tags:input.get("tags").and_then(Value::as_array).map(|tags|tags.iter().filter_map(Value::as_str).map(str::to_owned).collect()).or_else(||notes.iter().find(|n|n.id==id).map(|n|n.tags.clone())).unwrap_or_default(),source_session: caller, source_cwd: project.clone() };
                            store.save_note(&note)?; json!(note)
                        }
                    };
                    Ok((result, Effect::None))
                }, cx); return;
            }
            "todos.list" | "todos.add" | "todos.update" | "todos.remove" => {
                let board = vorkspace_core::todos::TodoBoard::Project(project.clone());
                let effect = if action == "todos.list" { Effect::None } else { Effect::Todos };
                self.app_background(call, move |store| Ok((locked(&store)?.todo_action(&board, &action, &input)?, effect)), cx); return;
            }
            "folders.move" => {
                let id = match session_id(&input, &self.store) { Ok(id) => id, Err(e) => { self.finish_app_call(call, Err(e)); return; } };
                if !self.session_summaries.contains_key(&id) || self.session_metadata(id, cx).summary.cwd != project { self.finish_app_call(call, Err("Session is outside this project.".into())); return; }
                let mut groups = self.groups.get(&project).cloned().unwrap_or_default();
                if let Some(name) = input.get("newFolderName").and_then(Value::as_str) {
                    if let Some(folder) = groups.create_folder(&[id], vorkspace_core::bot::new_id()) { groups.rename_folder(&folder, name); }
                } else if let Some(folder) = input.get("folderId").and_then(Value::as_str) {
                    if groups.folder(folder).is_none() { self.finish_app_call(call, Err("Folder not found.".into())); return; }
                    groups.add_to_folder(folder,id);
                } else { groups.remove_sessions(&[id]); }
                self.app_background(call, move |store| { locked(&store)?.save_groups(&project, &groups)?; Ok((json!({"moved":true}), Effect::Groups(project, groups))) }, cx); return;
            }
            "worktrees.list" => {
                self.app_background(call, move |_| { let snapshot = vorkspace_core::git::snapshot(std::path::Path::new(&project))?; Ok((json!(snapshot.worktrees.iter().map(|w| json!({"path":w.path,"branch":w.branch,"head":w.head,"main":w.main,"missing":w.missing})).collect::<Vec<_>>()), Effect::None)) }, cx); return;
            }
            "worktrees.create"=>{
                self.app_background(call,move |_|{
                    let branch=text(&input,"branch",240)?;
                    let tree=vorkspace_core::git::create_project_worktree(std::path::Path::new(&project),&branch,input.get("base").and_then(Value::as_str),input.get("existing").and_then(Value::as_bool).unwrap_or(false))?;
                    Ok((json!({"path":tree.path,"branch":tree.branch}),Effect::None))
                },cx);return;
            }
            _ => Err("Unknown Operator action. Supported: models.list, sessions.list/read/start/send/draft/stop/archive/delete, folders.list/move, notes.list/read/write, todos.list/add/update/remove, worktrees.list/create.".into()),
        };
        self.finish_app_call(call, result);
        let _ = window;
    }
}
fn text(input: &Value, key: &str, max: usize) -> Result<String, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty() && s.len() <= max)
        .map(str::to_owned)
        .ok_or_else(|| format!("Provide {key} (up to {max} bytes)."))
}
fn number(input: &Value, key: &str) -> Result<u64, String> {
    input
        .get(key)
        .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
        .ok_or_else(|| format!("Provide {key}."))
}
fn session_id(input: &Value, store: &Database) -> Result<i64, String> {
    if let Some(id) = input.get("sessionId").and_then(Value::as_str) {
        return locked(store)?.resolve_session_id(id);
    }
    i64::try_from(number(input, "sessionId")?).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    fn command_window(
        cx: &mut TestAppContext,
    ) -> (gpui::WindowHandle<Shell>, i64, tempfile::TempDir) {
        let root = tempfile::tempdir().unwrap();
        for args in [
            vec!["init", "-q"],
            vec![
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "initial",
            ],
        ] {
            assert!(
                std::process::Command::new("git")
                    .current_dir(root.path())
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
        let store = vorkspace_core::store::Store::open(&root.path().join("state.db")).unwrap();
        let project = root.path().to_string_lossy().into_owned();
        let summary = store.create("codex", &project).unwrap();
        let caller = summary.id;
        let store = Arc::new(Mutex::new(store));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| Shell::new(store, vec![summary], project, None, cx))
            })
            .unwrap()
        });
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                shell.sessions[&caller].update(cx, |view, cx| {
                    view.enable_mode("operator", cx);
                })
            })
            .unwrap();
        (window, caller, root)
    }
    fn command(
        window: gpui::WindowHandle<Shell>,
        caller: i64,
        request: &str,
        action: &str,
        input: Value,
        cx: &mut TestAppContext,
    ) -> Result<Value, String> {
        let (reply, rx) = std::sync::mpsc::channel();
        window
            .update(cx, |shell, window, cx| {
                shell.handle_app_call(
                    Call {
                        session: caller,
                        request_id: request.into(),
                        action: action.into(),
                        input,
                        reply,
                    },
                    window,
                    cx,
                )
            })
            .unwrap();
        cx.run_until_parked();
        rx.try_recv().expect("command completed")
    }
    fn rollback_window(
        cx: &mut TestAppContext,
        shared: bool,
    ) -> (gpui::WindowHandle<Shell>, i64, tempfile::TempDir) {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        std::fs::create_dir(&project).unwrap();
        for args in [
            vec!["init", "-q"],
            vec![
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "initial",
            ],
        ] {
            assert!(
                std::process::Command::new("git")
                    .current_dir(&project)
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
        let path = root.path().join("state.db");
        let store = if shared {
            vorkspace_core::store::Store::open_vorkspace(&path)
        } else {
            vorkspace_core::store::Store::open(&path)
        }
        .unwrap();
        let project = project.to_string_lossy().into_owned();
        let summary = store.create("codex", &project).unwrap();
        let caller = summary.id;
        let store = Arc::new(Mutex::new(store));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| Shell::new(store, vec![summary], project, None, cx))
            })
            .unwrap()
        });
        cx.run_until_parked();
        window
            .update(cx, |shell, window, cx| {
                shell.open(caller, window, cx);
                shell.sessions[&caller].update(cx, |view, cx| {
                    view.enable_mode("operator", cx);
                    view.input.update(cx, |input, cx| {
                        input.set_text("Keep the caller's newer draft".into(), cx)
                    });
                    view.app_flush_draft(cx).unwrap();
                });
            })
            .unwrap();
        cx.run_until_parked();
        (window, caller, root)
    }
    /// The real admission, installation and receipt settlement run in one UI
    /// callback. No hydration, Drop or provider task can conceal a failed write.
    fn failed_install(
        window: gpui::WindowHandle<Shell>,
        caller: i64,
        root: &std::path::Path,
        draft: bool,
        dirty: bool,
        refuse_delete: bool,
        cx: &mut TestAppContext,
    ) -> (i64, std::path::PathBuf, String) {
        window.update(cx, |shell, window, cx| {
            let project = shell.workspace.project.clone();
            let input = json!({"prompt":"/plan Unaccepted child request","draft":draft,"workspaceMode":"worktree","placement":"right"});
            let mut launch = vorkspace_core::bot::launch::Launch::parse(&input,"codex",&project,&project,Settings::default(),true).unwrap();
            let (summary, shared, original) = {
                let mut store = locked(&shell.store).unwrap();
                let summary = launch.create(&mut store,&project).unwrap();
                store.save_started_link(summary.id,caller).unwrap();
                let original = if store.uses_vorkspace_schema() { store.persisted_session_id(summary.id).unwrap() } else { summary.id.to_string() };
                (summary,store.uses_vorkspace_schema(),original)
            };
            let id = summary.id;
            let tree = std::path::PathBuf::from(launch.settings.worktree_cwd.as_ref().unwrap());
            if dirty { std::fs::write(tree.join("newer-user-work.txt"),"Preserve this new work").unwrap(); }
            let connection = rusqlite::Connection::open(root.join("state.db")).unwrap();
            if shared && draft {
                connection.execute_batch("CREATE TABLE IF NOT EXISTS composer_drafts(session_id TEXT PRIMARY KEY,cwd TEXT NOT NULL,revision INTEGER NOT NULL,payload_json TEXT NOT NULL,deleted INTEGER NOT NULL DEFAULT 0,updated_at INTEGER NOT NULL);").unwrap();
            }
            let trigger = if shared {
                if draft {
                    format!("CREATE TRIGGER reject_app_input BEFORE INSERT ON composer_drafts WHEN NEW.session_id='{original}' BEGIN SELECT RAISE(ABORT,'injected composer failure'); END;")
                } else {
                    format!("CREATE TRIGGER reject_app_input BEFORE UPDATE OF queued_messages_json ON sessions WHEN NEW.id='{original}' BEGIN SELECT RAISE(ABORT,'injected queue failure'); END;")
                }
            } else {
                let key = if draft { "composer" } else { "follow-up" };
                format!("CREATE TRIGGER reject_app_input BEFORE INSERT ON native_settings WHEN NEW.key='{key}:{id}' BEGIN SELECT RAISE(ABORT,'injected input failure'); END;")
            };
            connection.execute_batch(&trigger).unwrap();
            if refuse_delete {
                let (table,target) = if shared { ("sessions",original.clone()) } else { ("native_sessions",id.to_string()) };
                connection.execute_batch(&format!("CREATE TRIGGER reject_app_delete BEFORE DELETE ON {table} WHEN OLD.id='{target}' BEGIN SELECT RAISE(ABORT,'injected cleanup failure'); END;")).unwrap();
            }
            let panes = shell.active_leaves();
            let active = shell.workspace.active;
            let projects = shell.workspace.projects.clone();
            let observations = shell.observations.len();
            locked(&shell.store).unwrap().admit_app_command(caller,"failed-install","sessions.start",&input).unwrap();
            let result = shell.app_install_start(caller,AppStart {
                summary,settings:launch.settings,prompt:launch.prompt,draft:launch.draft,reveal:true,
                placement:Some((caller,Edge::Right)),cleanup:launch.created_worktree,
            },window,cx);
            let error = result.unwrap_err();
            assert!(error.contains("injected"),"{error}");
            assert!(!shell.sessions.contains_key(&id));
            assert!(!shell.session_summaries.contains_key(&id));
            assert!(!shell.session_marks.contains_key(&id));
            assert!(!shell.tab_marks.contains_key(&id));
            assert!(!shell.started_by.contains_key(&id));
            assert!(!shell.session_worktrees.contains_key(&id));
            assert!(!shell.order.contains(&id));
            assert!(shell.workspace.tab_of(id).is_none());
            assert_eq!(shell.active_leaves(),panes);
            assert_eq!(shell.workspace.active,active);
            assert_eq!(shell.workspace.projects,projects);
            assert_eq!(shell.observations.len(),observations);
            assert_eq!(shell.sessions[&caller].read(cx).input.read(cx).content,"Keep the caller's newer draft");
            assert!(!shell.sessions[&caller].read(cx).busy);
            // Remove the write failure before deferred callbacks and Drop run:
            // an undiscarded composer would now be free to recreate an orphan.
            connection.execute_batch("DROP TRIGGER reject_app_input;").unwrap();
            let (reply,rx) = std::sync::mpsc::channel();
            shell.finish_app_call(Call { session:caller,request_id:"failed-install".into(),action:"sessions.start".into(),input:input.clone(),reply },Err(error.clone()));
            assert_eq!(rx.try_recv().unwrap().unwrap_err(),error);
            assert!(matches!(locked(&shell.store).unwrap().admit_app_command(caller,"failed-install","sessions.start",&input).unwrap(),vorkspace_core::store::AppCommandAdmission::Completed(Err(_))));
            (id,tree,error)
        }).unwrap()
    }
    #[gpui::test]
    fn app_install_write_failures_remove_only_the_unaccepted_child(cx: &mut TestAppContext) {
        for shared in [false, true] {
            for draft in [false, true] {
                let (window, caller, root) = rollback_window(cx, shared);
                let (id, tree, _) =
                    failed_install(window, caller, root.path(), draft, false, false, cx);
                cx.run_until_parked();
                window
                    .update(cx, |shell, _, cx| {
                        let store = locked(&shell.store).unwrap();
                        assert!(store.bot_session(id).unwrap().is_none());
                        assert!(store.bot_session(caller).unwrap().is_some());
                        assert!(
                            store
                                .setting(&format!("started-by:{id}"))
                                .unwrap()
                                .is_none()
                        );
                        assert!(store.setting(&format!("agent:{id}")).unwrap().is_none());
                        assert!(store.setting(&format!("follow-up:{id}")).unwrap().is_none());
                        if !shared {
                            assert!(store.composer_draft(id).unwrap().is_none());
                        }
                        assert!(store.composer_recoveries(id).unwrap().is_empty());
                        assert!(!shell.sessions.contains_key(&id));
                        assert_eq!(
                            shell.sessions[&caller].read(cx).input.read(cx).content,
                            "Keep the caller's newer draft"
                        );
                    })
                    .unwrap();
                assert!(!tree.exists());
                let snapshot = vorkspace_core::git::snapshot(&root.path().join("project")).unwrap();
                assert_eq!(snapshot.worktrees.len(), 1);
                assert!(
                    !snapshot
                        .branches
                        .iter()
                        .any(|branch| branch.name.starts_with("mc/native-"))
                );
            }
        }
    }
    #[gpui::test]
    fn operator_preparation_link_failure_rolls_back_before_settling_the_request(
        cx: &mut TestAppContext,
    ) {
        for workspace in ["current", "worktree", "existing"] {
            let (window, caller, root) = rollback_window(cx, false);
            let project = root.path().join("project");
            let mut input = json!({"prompt":"Unaccepted request","draft":true});
            if workspace == "existing" {
                let tree = command(
                    window,
                    caller,
                    "existing-tree",
                    "worktrees.create",
                    json!({"branch":"existing-fixture"}),
                    cx,
                )
                .unwrap();
                input["worktreeCwd"] = tree["path"].clone();
                std::fs::write(
                    std::path::Path::new(tree["path"].as_str().unwrap()).join("user-work.txt"),
                    "Retain existing work",
                )
                .unwrap();
            } else {
                input["workspaceMode"] = json!(workspace);
            }
            let before = vorkspace_core::git::snapshot(&project).unwrap();
            let worktrees_before = window
                .update(cx, |shell, _, _| shell.session_worktrees.clone())
                .unwrap();
            let connection = rusqlite::Connection::open(root.path().join("state.db")).unwrap();
            connection.execute_batch("CREATE TRIGGER reject_started_link BEFORE INSERT ON native_settings WHEN NEW.key LIKE 'started-by:%' BEGIN SELECT RAISE(ABORT,'injected started link failure'); END;").unwrap();
            let error = command(
                window,
                caller,
                "failed-preparation",
                "sessions.start",
                input.clone(),
                cx,
            )
            .unwrap_err();
            assert!(error.contains("injected started link failure"), "{error}");
            window
                .update(cx, |shell, _, cx| {
                    let store = locked(&shell.store).unwrap();
                    assert_eq!(
                        store
                            .sessions()
                            .unwrap()
                            .iter()
                            .map(|session| session.id)
                            .collect::<Vec<_>>(),
                        vec![caller]
                    );
                    assert_eq!(shell.sessions.len(), 1);
                    assert_eq!(shell.order, vec![caller]);
                    assert_eq!(
                        shell
                            .active_leaves()
                            .iter()
                            .map(|leaf| leaf.id)
                            .collect::<Vec<_>>(),
                        vec![caller]
                    );
                    assert!(shell.started_by.is_empty());
                    assert_eq!(shell.session_worktrees, worktrees_before);
                    assert_eq!(
                        shell.sessions[&caller].read(cx).input.read(cx).content,
                        "Keep the caller's newer draft"
                    );
                })
                .unwrap();
            assert_eq!(connection.query_row("SELECT count(*) FROM native_settings WHERE key LIKE 'agent:%' AND key <> ?1",[format!("agent:{caller}")],|row|row.get::<_,i64>(0)).unwrap(),0);
            let after = vorkspace_core::git::snapshot(&project).unwrap();
            assert_eq!(after.worktrees.len(), before.worktrees.len());
            assert_eq!(
                after
                    .branches
                    .iter()
                    .map(|branch| &branch.name)
                    .collect::<Vec<_>>(),
                before
                    .branches
                    .iter()
                    .map(|branch| &branch.name)
                    .collect::<Vec<_>>()
            );
            if let Some(path) = input["worktreeCwd"].as_str() {
                assert_eq!(
                    std::fs::read_to_string(std::path::Path::new(path).join("user-work.txt"))
                        .unwrap(),
                    "Retain existing work"
                );
            }
            connection
                .execute_batch("DROP TRIGGER reject_started_link;")
                .unwrap();
            // A saved failure stays a failure after reopen and after the disk
            // fault is gone: retrying cannot allocate a replacement worker.
            let reopened =
                vorkspace_core::store::Store::open(&root.path().join("state.db")).unwrap();
            assert!(
                matches!(reopened.admit_app_command(caller,"failed-preparation","sessions.start",&input).unwrap(),vorkspace_core::store::AppCommandAdmission::Completed(Err(saved)) if saved==error)
            );
            assert_eq!(
                command(
                    window,
                    caller,
                    "failed-preparation",
                    "sessions.start",
                    input.clone(),
                    cx
                )
                .unwrap_err(),
                error
            );
            let accepted = command(
                window,
                caller,
                "next-preparation",
                "sessions.start",
                input,
                cx,
            )
            .unwrap();
            assert!(accepted["sessionId"].as_i64().is_some());
        }
    }
    #[gpui::test]
    fn operator_preparation_cleanup_failure_keeps_the_session_and_worktree(
        cx: &mut TestAppContext,
    ) {
        let (window, caller, root) = rollback_window(cx, false);
        let connection = rusqlite::Connection::open(root.path().join("state.db")).unwrap();
        connection.execute_batch(&format!("CREATE TRIGGER reject_started_link BEFORE INSERT ON native_settings WHEN NEW.key LIKE 'started-by:%' BEGIN SELECT RAISE(ABORT,'injected started link failure'); END;
            CREATE TRIGGER reject_child_delete BEFORE DELETE ON native_sessions WHEN OLD.id <> {caller} BEGIN SELECT RAISE(ABORT,'injected cleanup failure'); END;")).unwrap();
        let input = json!({"prompt":"Unaccepted request","draft":true,"workspaceMode":"worktree"});
        let error = command(
            window,
            caller,
            "failed-preparation",
            "sessions.start",
            input.clone(),
            cx,
        )
        .unwrap_err();
        assert!(error.contains("injected started link failure"), "{error}");
        assert!(error.contains("injected cleanup failure"), "{error}");
        assert!(error.contains("preserved"), "{error}");
        let tree = window
            .update(cx, |shell, _, cx| {
                let store = locked(&shell.store).unwrap();
                let sessions = store.sessions().unwrap();
                assert_eq!(sessions.len(), 2);
                let child = sessions
                    .iter()
                    .find(|session| session.id != caller)
                    .unwrap();
                assert!(error.contains(&child.id.to_string()), "{error}");
                let path = store
                    .agent_settings(child.id)
                    .unwrap()
                    .unwrap()
                    .worktree_cwd
                    .unwrap();
                assert!(!shell.sessions.contains_key(&child.id));
                assert_eq!(
                    shell
                        .active_leaves()
                        .iter()
                        .map(|leaf| leaf.id)
                        .collect::<Vec<_>>(),
                    vec![caller]
                );
                assert_eq!(
                    shell.sessions[&caller].read(cx).input.read(cx).content,
                    "Keep the caller's newer draft"
                );
                std::path::PathBuf::from(path)
            })
            .unwrap();
        assert!(tree.is_dir());
        assert_eq!(
            vorkspace_core::git::snapshot(&root.path().join("project"))
                .unwrap()
                .worktrees
                .len(),
            2
        );
        connection
            .execute_batch("DROP TRIGGER reject_started_link; DROP TRIGGER reject_child_delete;")
            .unwrap();
        assert_eq!(
            command(
                window,
                caller,
                "failed-preparation",
                "sessions.start",
                input,
                cx
            )
            .unwrap_err(),
            error
        );
        assert!(tree.is_dir());
    }
    #[gpui::test]
    fn app_install_failure_preserves_dirty_created_worktree_and_reports_it(
        cx: &mut TestAppContext,
    ) {
        let (window, caller, root) = rollback_window(cx, true);
        let (id, tree, error) = failed_install(window, caller, root.path(), true, true, false, cx);
        assert!(error.contains("preserved"), "{error}");
        assert!(error.contains(tree.to_str().unwrap()), "{error}");
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert!(
                    locked(&shell.store)
                        .unwrap()
                        .bot_session(id)
                        .unwrap()
                        .is_none()
                )
            })
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(tree.join("newer-user-work.txt")).unwrap(),
            "Preserve this new work"
        );
    }
    #[gpui::test]
    fn app_install_cleanup_failure_preserves_the_stored_session_and_its_worktree(
        cx: &mut TestAppContext,
    ) {
        let (window, caller, root) = rollback_window(cx, true);
        let (id, tree, error) = failed_install(window, caller, root.path(), false, false, true, cx);
        assert!(error.contains("could not be removed"), "{error}");
        assert!(error.contains("injected cleanup failure"), "{error}");
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert!(
                    locked(&shell.store)
                        .unwrap()
                        .bot_session(id)
                        .unwrap()
                        .is_some()
                );
                assert!(!shell.sessions.contains_key(&id));
            })
            .unwrap();
        assert!(tree.is_dir());
    }
    #[gpui::test]
    fn app_install_keeps_a_child_opened_by_a_newer_owner(cx: &mut TestAppContext) {
        let (window, caller, root) = rollback_window(cx, true);
        let (id, tree) = window
            .update(cx, |shell, window, cx| {
                let project = shell.workspace.project.clone();
                let input =
                    json!({"prompt":"Original delegated request","workspaceMode":"worktree"});
                let mut launch = vorkspace_core::bot::launch::Launch::parse(
                    &input,
                    "codex",
                    &project,
                    &project,
                    Settings::default(),
                    true,
                )
                .unwrap();
                let summary = launch
                    .create(&mut locked(&shell.store).unwrap(), &project)
                    .unwrap();
                let id = summary.id;
                let tree = std::path::PathBuf::from(launch.settings.worktree_cwd.as_ref().unwrap());
                shell.add(summary.clone(), cx);
                shell.open(id, window, cx);
                let adopted = shell.sessions[&id].clone();
                adopted.update(cx, |view, cx| {
                    view.input.update(cx, |input, cx| {
                        input.set_text("The user's newer child draft".into(), cx)
                    });
                    view.app_flush_draft(cx).unwrap();
                });
                let active = shell.workspace.active;
                let result = shell.app_install_start(
                    caller,
                    AppStart {
                        summary,
                        settings: launch.settings,
                        prompt: launch.prompt,
                        draft: false,
                        reveal: true,
                        placement: None,
                        cleanup: launch.created_worktree,
                    },
                    window,
                    cx,
                );
                assert!(result.unwrap_err().contains("already opened"));
                assert_eq!(shell.workspace.active, active);
                assert_eq!(shell.sessions[&id], adopted);
                assert_eq!(
                    adopted.read(cx).input.read(cx).content,
                    "The user's newer child draft"
                );
                assert!(
                    locked(&shell.store)
                        .unwrap()
                        .bot_session(id)
                        .unwrap()
                        .is_some()
                );
                (id, tree)
            })
            .unwrap();
        cx.run_until_parked();
        assert!(tree.is_dir());
        window
            .update(cx, |shell, _, cx| {
                assert_eq!(
                    shell.sessions[&id].read(cx).input.read(cx).content,
                    "The user's newer child draft"
                );
                assert_eq!(
                    locked(&shell.store)
                        .unwrap()
                        .composer_draft(id)
                        .unwrap()
                        .unwrap()
                        .payload["text"],
                    "The user's newer child draft"
                );
            })
            .unwrap();
        assert_eq!(
            vorkspace_core::git::snapshot(&root.path().join("project"))
                .unwrap()
                .worktrees
                .len(),
            2
        );
    }
    #[gpui::test]
    fn operator_launch_honors_access_model_and_worktree_and_durable_retries(
        cx: &mut TestAppContext,
    ) {
        let (window, caller, root) = command_window(cx);
        let tree = command(
            window,
            caller,
            "tree",
            "worktrees.create",
            json!({"branch":"review-fixture"}),
            cx,
        )
        .unwrap();
        let path = tree["path"].as_str().unwrap();
        assert!(std::path::Path::new(path).is_dir());
        let input = json!({"prompt":"Review this checkout","draft":true,"reveal":false,"worktreeCwd":path,"runtimeMode":"read-only","model":"codex:gpt-fixture","modelSettings":{"effort":"high","custom":"retained"}});
        let start = command(
            window,
            caller,
            "launch",
            "sessions.start",
            input.clone(),
            cx,
        )
        .unwrap();
        let child = start["sessionId"].as_i64().unwrap();
        window
            .update(cx, |shell, _, cx| {
                let view = shell.sessions[&child].read(cx);
                assert_eq!(
                    view.settings.access,
                    vorkspace_core::agent::AccessMode::ReadOnly
                );
                assert_eq!(view.settings.model.as_deref(), Some("gpt-fixture"));
                assert_eq!(view.settings.effort.as_deref(), Some("high"));
                assert_eq!(view.settings.model_settings["custom"], "retained");
                assert_eq!(view.settings.worktree_cwd.as_deref(), Some(path));
                assert_eq!(view.input.read(cx).content, "Review this checkout");
                assert!(!view.settings.operator && !view.busy);
                let draft = locked(&shell.store)
                    .unwrap()
                    .composer_draft(child)
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    draft.payload["text"], "Review this checkout",
                    "acknowledgement follows durable first-frame draft publication"
                );
            })
            .unwrap();
        assert_eq!(
            start,
            command(window, caller, "launch", "sessions.start", input, cx).unwrap()
        );
        assert!(
            command(
                window,
                caller,
                "launch",
                "sessions.start",
                json!({"prompt":"different","draft":true}),
                cx
            )
            .unwrap_err()
            .contains("different")
        );
        let before = window
            .update(cx, |shell, _, _| {
                locked(&shell.store).unwrap().sessions().unwrap().len()
            })
            .unwrap();
        for (request, input) in [
            ("bad-layout", json!({"prompt":"x","placement":"left"})),
            (
                "bad-settings",
                json!({"prompt":"x","modelSettings":{"effort":3}}),
            ),
            (
                "bad-path",
                json!({"prompt":"x","worktreeCwd":root.path().parent().unwrap().to_string_lossy()}),
            ),
            ("bad-mode", json!({"prompt":"x","runtimeMode":"unsafe"})),
        ] {
            assert!(command(window, caller, request, "sessions.start", input, cx).is_err());
        }
        assert_eq!(
            before,
            window
                .update(cx, |shell, _, _| locked(&shell.store)
                    .unwrap()
                    .sessions()
                    .unwrap()
                    .len())
                .unwrap()
        );
        assert!(
            std::process::Command::new("git")
                .current_dir(root.path())
                .args(["worktree", "remove", "--force", path])
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    #[gpui::test]
    fn operator_management_preserves_worktrees_and_rejects_its_own_session(
        cx: &mut TestAppContext,
    ) {
        let (window, caller, root) = command_window(cx);
        assert!(
            command(
                window,
                caller,
                "self",
                "sessions.stop",
                json!({"sessionId":caller}),
                cx
            )
            .unwrap_err()
            .contains("calling")
        );
        let start = command(
            window,
            caller,
            "start",
            "sessions.start",
            json!({"prompt":"Save a draft","draft":true}),
            cx,
        )
        .unwrap();
        let child = start["sessionId"].as_i64().unwrap();
        command(
            window,
            caller,
            "stop",
            "sessions.stop",
            json!({"sessionId":child}),
            cx,
        )
        .unwrap();
        window
            .update(cx, |shell, _, cx| {
                assert_eq!(
                    shell.sessions[&child].read(cx).input.read(cx).content,
                    "Save a draft"
                );
                assert!(
                    locked(&shell.store)
                        .unwrap()
                        .follow_ups(child)
                        .unwrap()
                        .messages
                        .is_empty()
                );
            })
            .unwrap();
        command(
            window,
            caller,
            "archive",
            "sessions.archive",
            json!({"sessionId":child}),
            cx,
        )
        .unwrap();
        window
            .update(cx, |shell, _, _| {
                assert!(
                    locked(&shell.store)
                        .unwrap()
                        .bot_session(child)
                        .unwrap()
                        .unwrap()
                        .archived
                )
            })
            .unwrap();
        command(
            window,
            caller,
            "delete",
            "sessions.delete",
            json!({"sessionId":child}),
            cx,
        )
        .unwrap();
        window
            .update(cx, |shell, _, _| {
                assert!(
                    locked(&shell.store)
                        .unwrap()
                        .bot_session(child)
                        .unwrap()
                        .is_none()
                );
                assert!(
                    locked(&shell.store)
                        .unwrap()
                        .bot_session(caller)
                        .unwrap()
                        .is_some()
                );
            })
            .unwrap();
        assert!(root.path().join(".git").exists());
    }
    #[gpui::test]
    fn operator_splits_reuse_the_existing_pane_owner_and_reject_stale_targets(
        cx: &mut TestAppContext,
    ) {
        let (window, caller, _root) = command_window(cx);
        window
            .update(cx, |shell, window, cx| shell.open(caller, window, cx))
            .unwrap();
        cx.run_until_parked();
        let first = command(
            window,
            caller,
            "right",
            "sessions.start",
            json!({"prompt":"First","draft":true,"placement":"right","reveal":false}),
            cx,
        )
        .unwrap()["sessionId"]
            .as_i64()
            .unwrap();
        let second = command(
            window,
            caller,
            "down",
            "sessions.start",
            json!({"prompt":"Second","draft":true,"placement":"down","besideSessionId":first}),
            cx,
        )
        .unwrap()["sessionId"]
            .as_i64()
            .unwrap();
        window
            .update(cx, |shell, _, cx| {
                assert_eq!(
                    shell.workspace.tab_of(caller),
                    shell.workspace.tab_of(first)
                );
                assert_eq!(
                    shell.workspace.tab_of(first),
                    shell.workspace.tab_of(second)
                );
                let tab = shell.workspace.tab_of(caller).unwrap();
                assert_eq!(shell.workspace.layout(tab).leaf_ids().len(), 3);
                assert_eq!(
                    shell.sessions[&first].read(cx).input.read(cx).content,
                    "First"
                );
                assert_eq!(
                    shell.sessions[&second].read(cx).input.read(cx).content,
                    "Second"
                );
            })
            .unwrap();
        window
            .update(cx, |shell, window, cx| shell.close_pane(first, window, cx))
            .unwrap();
        cx.run_until_parked();
        let count = window
            .update(cx, |shell, _, _| {
                locked(&shell.store).unwrap().sessions().unwrap().len()
            })
            .unwrap();
        assert!(command(window,caller,"closed","sessions.start",json!({"prompt":"Do not create","draft":true,"placement":"right","besideSessionId":first}),cx).unwrap_err().contains("open session"));
        assert_eq!(
            count,
            window
                .update(cx, |shell, _, _| locked(&shell.store)
                    .unwrap()
                    .sessions()
                    .unwrap()
                    .len())
                .unwrap()
        );
        window
            .update(cx, |shell, window, cx| {
                let summary = locked(&shell.store)
                    .unwrap()
                    .create("codex", &shell.workspace.project)
                    .unwrap();
                let id = summary.id;
                // Preparation was valid while first was open; publication sees it
                // closed and must not fall back to the caller or another pane.
                assert!(
                    shell
                        .app_install_start(
                            caller,
                            AppStart {
                                summary,
                                settings: Settings::default(),
                                prompt: "Do not send".into(),
                                draft: false,
                                reveal: true,
                                placement: Some((first, Edge::Right)),
                                cleanup: None
                            },
                            window,
                            cx
                        )
                        .unwrap_err()
                        .contains("open session")
                );
                assert!(
                    locked(&shell.store)
                        .unwrap()
                        .bot_session(id)
                        .unwrap()
                        .is_none()
                );
            })
            .unwrap();
    }
    #[gpui::test]
    fn operator_follow_up_rejects_an_account_switch_before_consuming_text(cx: &mut TestAppContext) {
        let (window, caller, _root) = command_window(cx);
        let child = command(
            window,
            caller,
            "child",
            "sessions.start",
            json!({"prompt":"temporary","draft":true}),
            cx,
        )
        .unwrap()["sessionId"]
            .as_i64()
            .unwrap();
        window
            .update(cx, |shell, _, cx| {
                shell.sessions[&child].update(cx, |view, cx| {
                    view.input
                        .update(cx, |input, cx| input.set_text(String::new(), cx));
                    view.app_flush_draft(cx).unwrap();
                    view.app_hold_account_switch();
                })
            })
            .unwrap();
        assert!(
            command(
                window,
                caller,
                "blocked",
                "sessions.send",
                json!({"sessionId":child,"prompt":"Do not lose this"}),
                cx
            )
            .is_err()
        );
        window
            .update(cx, |shell, _, cx| {
                let view = shell.sessions[&child].read(cx);
                assert!(!view.busy);
                assert!(view.input.read(cx).content.is_empty());
                assert!(
                    locked(&shell.store)
                        .unwrap()
                        .load(child)
                        .unwrap()
                        .rows
                        .is_empty()
                );
            })
            .unwrap();
    }
    #[gpui::test]
    fn app_acceptance_keeps_the_prompt_durable_before_the_provider_worker_runs(
        cx: &mut TestAppContext,
    ) {
        let (window, caller, root) = command_window(cx);
        let child = command(
            window,
            caller,
            "child",
            "sessions.start",
            json!({"prompt":"temporary","draft":true}),
            cx,
        )
        .unwrap()["sessionId"]
            .as_i64()
            .unwrap();
        let (reply, rx) = std::sync::mpsc::channel();
        let input = json!({"sessionId":child,"prompt":"/plan Inspect the complete durable prompt"});
        let message = window
            .update(cx, |shell, window, cx| {
                shell.sessions[&child].update(cx, |view, cx| {
                    view.input
                        .update(cx, |input, cx| input.set_text(String::new(), cx));
                    view.app_flush_draft(cx).unwrap();
                });
                // The deterministic executor cannot poll the provider worker until
                // this UI callback returns. Acknowledgement already has its queue.
                shell.handle_app_call(
                    Call {
                        session: caller,
                        request_id: "durable-send".into(),
                        action: "sessions.send".into(),
                        input: input.clone(),
                        reply,
                    },
                    window,
                    cx,
                );
                assert!(rx.try_recv().unwrap().is_ok());
                let queue = locked(&shell.store).unwrap().follow_ups(child).unwrap();
                assert_eq!(queue.messages.len(), 1);
                let message = queue.messages[0].clone();
                assert_eq!(message.text, "Inspect the complete durable prompt");
                assert_eq!(message.mode.as_deref(), Some("plan"));
                assert!(!message.id.is_empty());
                shell.sessions[&child].update(cx, |view, cx| {
                    view.input
                        .update(cx, |input, cx| input.set_text("Newer user text".into(), cx));
                    view.app_stop(cx).unwrap();
                    assert_eq!(view.input.read(cx).content, "Newer user text");
                });
                message
            })
            .unwrap();
        let reopened = vorkspace_core::store::Store::open(&root.path().join("state.db")).unwrap();
        let queue = reopened.follow_ups(child).unwrap();
        assert!(queue.paused);
        assert_eq!(queue.messages, vec![message]);
        assert!(matches!(
            reopened
                .admit_app_command(caller, "durable-send", "sessions.send", &input)
                .unwrap(),
            vorkspace_core::store::AppCommandAdmission::Completed(Ok(_))
        ));
        assert_eq!(reopened.follow_ups(child).unwrap().messages.len(), 1);
        drop(reopened);
        cx.run_until_parked();
    }
    #[gpui::test]
    fn app_send_cannot_acknowledge_a_prompt_its_stale_queue_did_not_save(cx: &mut TestAppContext) {
        let (window, caller, root) = command_window(cx);
        let child = command(
            window,
            caller,
            "child",
            "sessions.start",
            json!({"prompt":"temporary","draft":true}),
            cx,
        )
        .unwrap()["sessionId"]
            .as_i64()
            .unwrap();
        window
            .update(cx, |shell, _, cx| {
                shell.sessions[&child].update(cx, |view, cx| {
                    view.input
                        .update(cx, |input, cx| input.set_text(String::new(), cx));
                    view.app_flush_draft(cx).unwrap();
                })
            })
            .unwrap();
        let second = vorkspace_core::store::Store::open(&root.path().join("state.db")).unwrap();
        let mut newer = second.follow_ups(child).unwrap();
        newer.revision = 100;
        newer.messages.push(vorkspace_core::follow_up::Message {
            id: "newer-owner-message".into(),
            text: "Keep this durable queue".into(),
            ..Default::default()
        });
        second.save_follow_ups(child, &newer).unwrap();
        let input = json!({"sessionId":child,"prompt":"Must not be falsely accepted"});
        assert!(
            command(
                window,
                caller,
                "stale-send",
                "sessions.send",
                input.clone(),
                cx
            )
            .unwrap_err()
            .contains("queue changed")
        );
        assert_eq!(second.follow_ups(child).unwrap().messages, newer.messages);
        window
            .update(cx, |shell, _, cx| {
                let view = shell.sessions[&child].read(cx);
                assert!(!view.busy);
                assert!(view.input.read(cx).content.is_empty());
                assert!(matches!(
                    locked(&shell.store)
                        .unwrap()
                        .admit_app_command(caller, "stale-send", "sessions.send", &input)
                        .unwrap(),
                    vorkspace_core::store::AppCommandAdmission::Completed(Err(_))
                ));
            })
            .unwrap();
    }
    #[gpui::test]
    fn operator_scopes_sessions_and_preserves_an_unsent_draft(cx: &mut TestAppContext) {
        let (window, other, active) = crate::shell::tests::window(cx);
        window
            .update(cx, |shell, window, cx| {
                let view = shell.sessions[&active].clone();
                view.update(cx, |v, cx| {
                    v.install_transfer(Settings::default(), None, cx);
                    v.enable_mode("operator", cx);
                    v.input
                        .update(cx, |i, cx| i.set_text("keep my draft".into(), cx));
                });
                for (action, input, should_succeed) in [
                    ("sessions.list", json!({}), true),
                    ("sessions.read", json!({"sessionId":other}), false),
                    (
                        "sessions.draft",
                        json!({"sessionId":active,"prompt":"replace draft"}),
                        false,
                    ),
                ] {
                    let (reply, result) = std::sync::mpsc::channel();
                    shell.handle_app_call(
                        Call {
                            session: active,
                            request_id: action.into(),
                            action: action.into(),
                            input,
                            reply,
                        },
                        window,
                        cx,
                    );
                    let result = result.try_recv().unwrap();
                    assert_eq!(result.is_ok(), should_succeed);
                    if action == "sessions.list" {
                        let list = result.unwrap();
                        assert!(
                            list.as_array()
                                .unwrap()
                                .iter()
                                .any(|s| s["sessionId"] == active)
                        );
                        assert!(
                            !list
                                .as_array()
                                .unwrap()
                                .iter()
                                .any(|s| s["sessionId"] == other)
                        );
                    }
                }
                assert_eq!(view.read(cx).input.read(cx).content, "keep my draft");
                view.update(cx, |v, _| {
                    v.settings.access = vorkspace_core::agent::AccessMode::ReadOnly
                });
                let (reply, result) = std::sync::mpsc::channel();
                shell.handle_app_call(
                    Call {
                        session: active,
                        request_id: "read-only-write".into(),
                        action: "notes.write".into(),
                        input: json!({"title":"Should not save","body":"Read-only"}),
                        reply,
                    },
                    window,
                    cx,
                );
                assert!(
                    result
                        .try_recv()
                        .unwrap()
                        .unwrap_err()
                        .contains("read-only")
                );
            })
            .unwrap();
    }
    #[gpui::test]
    fn operators_manage_their_project_board_and_open_views_reload(cx: &mut TestAppContext) {
        let (window, caller, root) = command_window(cx);
        let project = root.path().to_string_lossy().into_owned();
        let board = vorkspace_core::todos::TodoBoard::Project(project.clone());
        // An open project board, which must pick up the change.
        let view = window
            .update(cx, |shell, _, cx| {
                let store = shell.store.clone();
                let view = cx.new(|cx| crate::todos::TodoBoardView::new(store, false, cx));
                view.update(cx, |view, cx| view.set_board(Some(board.clone()), cx));
                view
            })
            .unwrap();
        cx.run_until_parked();
        let added = command(
            window,
            caller,
            "todo-add",
            "todos.add",
            json!({"title":"Review the board"}),
            cx,
        )
        .unwrap();
        assert_eq!(added["status"], "todo");
        view.read_with(cx, |view, _| assert_eq!(view.items().len(), 1));
        let id = added["id"].as_str().unwrap();
        command(
            window,
            caller,
            "todo-done",
            "todos.update",
            json!({"id":id,"status":"done"}),
            cx,
        )
        .unwrap();
        let listed = command(window, caller, "todo-list", "todos.list", json!({}), cx).unwrap();
        assert_eq!(listed["board"], board.key());
        assert_eq!(listed["items"][0]["status"], "done");
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.items()[0].status,
                vorkspace_core::todos::TodoStatus::Done
            )
        });
        // Another project's board is out of reach.
        assert!(
            command(
                window,
                caller,
                "todo-elsewhere",
                "todos.list",
                json!({"project":"/elsewhere"}),
                cx,
            )
            .is_err()
        );
    }
}
