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
mod tests;
