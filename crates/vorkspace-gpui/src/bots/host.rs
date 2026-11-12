//! Capability-checked app actions used by Bot provider subprocesses.
use super::*;
use serde_json::{Value, json};
use std::path::Path;
use vorkspace_core::bot::{Document, Launch, bridge::Call, files, memory};
use vorkspace_core::time::now_ms;
enum Effect {
    Card(Value),
    None,
    Start(Box<super::host_api::AppStart>),
    Groups(String),
}
fn text(v: &Value, key: &str, max: usize) -> Result<String, String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty() && s.len() <= max)
        .map(str::to_owned)
        .ok_or_else(|| format!("Provide {key} (up to {max} bytes)"))
}
fn id(v: &Value, key: &str) -> Result<i64, String> {
    v.get(key)
        .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()))
        .filter(|n| *n > 0)
        .ok_or_else(|| format!("Provide {key}"))
}
fn flag(v: &Value, key: &str, default: bool) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(default)
}
fn fingerprint(call: &Call) -> String {
    files::content_hash(&format!("{}:{}", call.action, call.input))
}
impl Bots {
    fn finish_call(&mut self, call: Call, result: Result<Value, String>, cx: &mut Context<Self>) {
        if bot::api::is_read_only(&call.action) {
            let _ = call.reply.send(result);
            return;
        }
        let queued = self
            .roster
            .bots
            .iter()
            .map(|b| {
                (
                    b.id.clone(),
                    b.outbox.iter().map(|m| m.id.clone()).collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        let reply = call.reply.clone();
        // Acknowledgement follows the durable roster and command receipt.
        let saved = self.persist_roster(
            move |store| {
                store.finish_app_command(
                    call.session,
                    &call.request_id,
                    &call.action,
                    &call.input,
                    &result,
                )?;
                Ok(result)
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = saved.recv().await
                .unwrap_or_else(|_| Err("Bot roster write was interrupted".into()))
                .map_err(|error| format!("App action may have executed but its state could not be saved; do not replay: {error}"));
            let _ = this.update(cx, |this, cx| {
                if result.is_ok() {
                    for (bot, messages) in queued {
                        this.persisted_messages.extend(messages);
                        this.dispatch(&bot, cx);
                    }
                    this.persisted_messages.retain(|id| this.roster.bots.iter().any(|bot| bot.outbox.iter().any(|m| &m.id == id)));
                }
                this.roster_write_finished(cx);
            });
            let _ = reply.send(result.and_then(|result| result));
        }).detach();
    }
    fn call_work(
        &mut self,
        call: Call,
        actor_id: &str,
        bot_id: String,
        work: impl FnOnce(Database) -> Result<(Value, Effect), String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let permit = if self.roster.get(actor_id).is_some_and(Bot::is_minion) {
            let projects = self
                .roster
                .storage_owner(actor_id)
                .map(|owner| owner.projects.as_slice())
                .unwrap_or_default();
            match self
                .shared_roster
                .read(cx)
                .minion_permit(actor_id, projects)
            {
                Ok(permit) => Some(permit),
                Err(error) => {
                    self.finish_call(call, Err(error), cx);
                    return;
                }
            }
        } else {
            None
        };
        let store = self.store.clone();
        let refresh_todos = call.action.starts_with("todos.") && call.action != "todos.list";
        let work = cx.background_executor().spawn(async move {
            match permit {
                Some(permit) => permit.run(|| work(store)),
                None => work(store),
            }
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                // Legacy migration and recovery receipts can commit before a
                // later operation fails. Visible boards must see those writes.
                if refresh_todos {
                    crate::todos::changed(cx);
                }
                let result = match result {
                    Ok((value, Effect::Start(start))) => {
                        this.publish_start(call, bot_id, value, *start, window, cx);
                        return;
                    }
                    other => other,
                };
                let result = result.map(|(value, effect)| {
                    match effect {
                        Effect::None => (),
                        Effect::Card(value) => {
                            this.hold_card(call.session, &bot_id, value);
                            this.save(cx);
                        }
                        Effect::Groups(project) => this.owner.apply_groups(project, cx),
                        Effect::Start(_) => unreachable!(),
                    }
                    this.reload_files(&bot_id, cx);
                    this.changed(cx);
                    value
                });
                this.finish_call(call, result, cx);
            });
        })
        .detach();
    }
    fn publish_start(
        &mut self,
        call: Call,
        bot_id: String,
        value: Value,
        start: super::host_api::AppStart,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let caller = call.session;
        let origin = self
            .chats
            .values()
            .chain(self.running.values().map(|run| &run.view))
            .find(|view| view.read(cx).summary.id == caller)
            .map(|view| view.read(cx).bot_turn)
            .unwrap_or(0);
        let mut launch = Launch {
            session_id: start.summary.id,
            cwd: start.summary.cwd.clone(),
            title: start.summary.title.clone(),
            harness: start.summary.provider.clone(),
            model: start.settings.model.clone(),
            notify_on_complete: flag(&call.input, "notifyOnComplete", !start.draft),
            ..Default::default()
        };
        launch.set_origin(caller, origin);
        let owner = self.owner.clone();
        let manager = cx.entity().downgrade();
        let handle = window.window_handle();
        let app: &mut App = cx;
        app.defer(move |cx| {
            let result = handle
                .update(cx, |_, window, cx| {
                    owner.install_start(caller, start, window, cx)
                })
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()))
                .and_then(|r| r);
            let _ = manager.update(cx, |this, cx| {
                let result = result.map(|id| {
                    if let Some(bot) = this.roster.get_mut(&bot_id) {
                        bot.launches.push(launch);
                    }
                    this.group_launch(&bot_id, id, cx);
                    this.reload_files(&bot_id, cx);
                    this.changed(cx);
                    value
                });
                this.finish_call(call, result, cx);
            });
        });
    }
    pub(super) fn app_call(&mut self, call: Call, window: &mut Window, cx: &mut Context<Self>) {
        let Some((actor, bot, permit)) = self.app_call_actors(&call, cx) else {
            return;
        };
        if !self.admit_app_call(&call) {
            return;
        }
        if matches!(
            call.action.as_str(),
            "bots.read" | "bots.historyStatus" | "bots.recoverHistory"
        ) {
            self.app_peer_history(call, &actor, &bot, cx);
            return;
        }
        if call.action.starts_with("bots.") {
            let result = self.peer_action(&call, &actor.id, cx);
            self.finish_call(call, result, cx);
            return;
        }
        let input = call.input.clone();
        let action = call.action.clone();
        let root = self.root.clone();
        let target = bot.id.clone();
        let project = if action.starts_with("sessions.")
            || action.starts_with("workspace.")
            || action.starts_with("folders.")
            || action.starts_with("worktrees.")
            || action.starts_with("codebase.")
        {
            match bot.project(input.get("project").and_then(Value::as_str)) {
                Ok(p) => p,
                Err(e) => {
                    self.finish_call(call, Err(e), cx);
                    return;
                }
            }
        } else {
            String::new()
        };
        let hidden = self.roster.hidden_sessions.clone();
        let source = self.calling_view(call.session, cx);
        if action.starts_with("codebase.") {
            let service = match crate::codebase::service(cx) {
                Ok(s) => s,
                Err(e) => {
                    self.finish_call(call, Err(e), cx);
                    return;
                }
            };
            let root = Self::app_codebase_root(&input, &source, &project, cx);
            self.call_work(
                call,
                &actor.id,
                target,
                move |_| {
                    crate::codebase::action(&service, &project, &root, &action, &input)
                        .map(|v| (v, Effect::None))
                },
                cx,
            );
            return;
        }
        if action.starts_with("workspace.") {
            let owner = self.owner.clone();
            let manager = cx.entity().downgrade();
            let handle = window.window_handle();
            let app: &mut App = cx;
            app.defer(move |cx| {
                let result = handle
                    .update(cx, |_, window, cx| {
                        if let Some(permit) = permit {
                            permit.check()?;
                        }
                        owner
                            .workspace_action(call.session, &project, &action, &input, window, cx)
                            .map_err(|error| error.to_string())?
                    })
                    .map_err(|error| error.to_string())
                    .and_then(|result| result);
                let _ = manager.update(cx, |manager, cx| manager.finish_call(call, result, cx));
            });
            return;
        }
        if action.starts_with("notes.") {
            let owner = self.owner.clone();
            let manager = cx.entity().downgrade();
            let handle = window.window_handle();
            let projects = bot.projects.clone();
            let app: &mut App = cx;
            app.defer(move |cx| {
                let result = handle
                    .update(cx, |_, window, cx| {
                        if let Some(permit) = permit {
                            permit.check()?;
                        }
                        owner
                            .notes_action(call.session, &projects, &action, &input, window, cx)
                            .map_err(|error| error.to_string())?
                    })
                    .map_err(|error| error.to_string())
                    .and_then(|result| result);
                let _ = manager.update(cx, |manager, cx| manager.finish_call(call, result, cx));
            });
            return;
        }
        let result=match action.as_str(){
            "models.list"=>Ok(json!(Kind::ALL.into_iter().map(|kind|json!({"harness":kind.id(),"name":kind.label(),"models":self.chats.values().find(|v|{let v=v.read(cx);let profile=source.as_ref().filter(|s|s.read(cx).summary.provider==kind.id()).map(|s|s.read(cx).settings.account.clone()).unwrap_or_else(||crate::accounts::get(cx).map(|center|center.read(cx).selected(kind,&project)).unwrap_or_default());v.summary.provider==kind.id()&&v.settings.account.id==profile.id}).map(|v|v.read(cx).settings_models()).unwrap_or_default()})).collect::<Vec<_>>())),
            "sessions.start"=>{
                let placement=match self.owner.placement(call.session,&project,&input,cx){Ok(placement)=>placement,Err(error)=>{self.finish_call(call,Err(error),cx);return;}};

                let source_provider=source.as_ref().map(|v|v.read(cx).summary.provider.clone()).unwrap_or_else(||"codex".into());
                let source_project=source.as_ref().map(|v|v.read(cx).summary.cwd.clone()).unwrap_or_else(||project.clone());
                let settings=source.as_ref().map(|v|v.read(cx).bot_execution_settings()).unwrap_or_default();
                let mut launch=match bot::launch::Launch::parse(&input,&source_provider,&source_project,&project,settings,false){Ok(launch)=>launch,Err(error)=>{self.finish_call(call,Err(error),cx);return;}};
                if launch.provider.id()!=source_provider { launch.settings.account=crate::accounts::get(cx).map(|center|center.read(cx).selected(launch.provider,&project)).unwrap_or_default(); }
                let Some(mut install)=self.owner.begin_session_install(cx) else {self.finish_call(call,Err("Workspace window closed".into()),cx);return;};
                self.call_work(call,&actor.id,target,move|store|{
                    let mut store=locked(&store)?;
                    let summary=launch.create(&mut store,&project)?;
                    install.track(summary.id);
                    Ok((json!({"sessionId":summary.id,"accepted":true,"project":project}),Effect::Start(Box::new(super::host_api::AppStart{summary,install:Some(install),settings:launch.settings,prompt:launch.prompt,draft:launch.draft,reveal:launch.reveal,placement,cleanup:launch.created_worktree}))))
                },cx);return;
            }
            "worktrees.remove"=>{
                self.app_remove_worktree(call,&input,project,cx);return;
            }
            "minions.list"=>{
                Ok(json!(self.roster.team(&bot.id).filter(|b|b.is_minion()).map(|b|self.minion_value(b,cx)).collect::<Vec<_>>()))
            }
            "minions.hire"|"minions.spawn"=>{
                let name=if action=="minions.hire" {match text(&input,"name",240){Ok(v)=>v,Err(e)=>{self.finish_call(call,Err(e),cx);return;}}} else {input.get("name").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(||self.next_minion_name(&bot.id))};
                let role=if action=="minions.hire" {match text(&input,"role",1024){Ok(v)=>v,Err(e)=>{self.finish_call(call,Err(e),cx);return;}}} else {input.get("role").and_then(Value::as_str).unwrap_or("Team helper").into()};
                let specialties=input.get("specialties").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
                let prompt=if action=="minions.spawn" {match text(&input,"prompt",65536){Ok(p)=>Some(p),Err(e)=>{self.finish_call(call,Err(e),cx);return;}}} else {None};
                let member=match self.roster.hire_minion(&actor.id,&name,&role,specialties,input.get("project").and_then(Value::as_str)){Ok(id)=>id,Err(e)=>{self.finish_call(call,Err(e),cx);return;}};
                // Keep execution choices without allocating a conversation for an idle hire.
                if let Some(source)=source.as_ref() {
                    let provider=source.read(cx).summary.provider.clone();
                    let settings=source.read(cx).bot_execution_settings();
                    let mut choices=json!({"prompt":"Team configuration"});
                    for key in ["harness","model"] {if let Some(value)=input.get(key){choices[key]=value.clone();}}
                    match bot::launch::Launch::parse(&choices,&provider,"","",settings,false) {
                        Ok(mut launch)=>{
                            launch.settings.worktree_cwd=None;
                            if launch.provider.id()!=provider {launch.settings.account=crate::accounts::get(cx).map(|c|c.read(cx).selected(launch.provider,"")).unwrap_or_default();}
                            self.roster.get_mut(&member).unwrap().initial_configuration=Some(Box::new(bot::InitialConfiguration{provider:launch.provider.id().into(),settings:launch.settings,soul:None}));
                        }
                        Err(error)=>{self.roster.bots.retain(|b|b.id!=member);self.finish_call(call,Err(error),cx);return;}
                    }
                }
                let hop=self.collaboration_hop(call.session,cx);
                let start_task=prompt.is_some();
                if let Some(prompt)=prompt
                    && let Err(error)=bot::peers::enqueue(&mut self.roster,&actor.id,&member,&prompt,&format!("{}:{}",call.session,call.request_id),hop,true) {
                    self.roster.bots.retain(|b|b.id!=member);self.finish_call(call,Err(error),cx);return;
                }
                let value=self.minion_value(self.roster.get(&member).unwrap(),cx);
                self.save(cx);
                if start_task {self.ensure_chat(&member,cx);}
                Ok(value)
            }
            "minions.fire"=>{
                self.app_fire_minion(call,&input,actor.id,cx);return;
            }
            "minions.retry"|"minions.discard"=>{
                let query=match text(&input,"minion",160){Ok(v)=>v,Err(e)=>{self.finish_call(call,Err(e),cx);return;}};
                let message=match text(&input,"messageId",256){Ok(v)=>v,Err(e)=>{self.finish_call(call,Err(e),cx);return;}};
                let member=match self.roster.resolve_peer(&actor.id,&query,true){Ok(b)=>b.id.clone(),Err(e)=>{self.finish_call(call,Err(e),cx);return;}};
                let allowed=self.roster.get(&member).is_some_and(|b|b.minion.as_ref().is_some_and(|m|m.parent_id==actor.id && !m.retired));
                if !allowed {Err("Only the active Minion's parent can recover its work".into())}
                else if !self.roster.get(&member).is_some_and(|b|b.outbox.iter().any(|m|m.id==message && m.peer.is_some() && m.error.is_some())) {Err("Choose a failed peer message from minions.list".into())}
                else if self.inflight.get(&member).is_some_and(|m|m.id==message) {Err("Wait for the current task to stop before recovering it".into())}
                else {
                    self.ensure_chat(&member,cx);
                    self.recover_queue(&member,&message,action=="minions.discard",cx);
                    Ok(self.minion_value(self.roster.get(&member).unwrap(),cx))
                }
            }
            "sessions.list"|"sessions.read"=>{
                self.call_work(call,&actor.id,target,move|store|{let db=locked(&store)?;let all=db.sessions()?;let sessions:Vec<_>=all.iter().filter(|s|!hidden.contains(&s.id)&&vorkspace_core::files::path_key(&s.cwd)==vorkspace_core::files::path_key(&project)).collect();
                    let value=if action=="sessions.list"{json!(sessions.iter().map(|s|json!({"sessionId":s.id,"title":s.title,"harness":s.provider,"project":s.cwd,"archived":s.archived})).collect::<Vec<_>>())}else{let session=id(&input,"sessionId")?;if !sessions.iter().any(|s|s.id==session){return Err("Session is outside the assigned project".into());}let rows=db.load(session)?.rows;let end=input.get("before").and_then(Value::as_u64).unwrap_or(rows.len() as u64).min(rows.len() as u64) as usize;let limit=input.get("limit").and_then(Value::as_u64).unwrap_or(40).clamp(1,200) as usize;let start=end.saturating_sub(limit);let max=input.get("maxChars").and_then(Value::as_u64).unwrap_or(24000).clamp(100,200000) as usize;json!({"sessionId":session,"messages":rows[start..end].iter().map(|r|json!({"role":r.role,"text":vorkspace_core::response::limit(&r.text,max/limit.max(1))})).collect::<Vec<_>>(),"before":start,"hasMore":start>0})};Ok((value,Effect::None))},cx);return;
            }
            "sessions.stop"|"sessions.archive"|"sessions.delete"=>{
                let session=match id(&input,"sessionId"){Ok(id)=>id,Err(error)=>{self.finish_call(call,Err(error),cx);return;}};
                let owner=self.owner.clone();
                let manager=cx.entity().downgrade();
                let handle=window.window_handle();
                let bot_id=bot.id.clone();
                let app:&mut App=cx;
                app.defer(move|cx|{
                    // The Bot update has returned, so the Shell can inspect its
                    // current live ownership without reborrowing this entity.
                    let result=handle.update(cx,|_,window,cx|owner.session_action(call.session,session,&project,&action,window,cx)).map_err(|e|e.to_string()).and_then(|r|r.map_err(|e|e.to_string())).and_then(|r|r);
                    let _=manager.update(cx,|this,cx|{
                        if result.is_ok() && let Some(bot)=this.roster.get_mut(&bot_id) {
                            for launch in &mut bot.launches { if launch.session_id==session { launch.dismissed=true; } }
                        }
                        this.finish_call(call,result,cx);
                    });
                });
                return;
            }
            "sessions.send"|"sessions.draft"=>{
                let session=match id(&input,"sessionId"){Ok(id)=>id,Err(e)=>{self.finish_call(call,Err(e),cx);return;}};
                if let Err(error)=locked(&self.store).and_then(|store|store.check_app_target(call.session,session,&project)) { self.finish_call(call,Err(error),cx);return; }
                if self.protected_app_target(session,cx) { self.finish_call(call,Err("Protected sessions cannot receive app-directed input".into()),cx);return; }
                let view=self.owner.ensure_session(session,cx);
                let Some(view)=view else {self.finish_call(call,Err("Session is unavailable".into()),cx);return;};
                view.update(cx,|view,cx|view.load_complete(cx));
                if !view.read(cx).ready_for_external(cx) {self.finish_call(call,Err("Session is busy, loading, or has an unsent draft".into()),cx);return;}
                match text(&input,"prompt",65536) {
                    Ok(prompt)=>{
                        let sent=view.update(cx,|view,cx|{if action=="sessions.draft" {view.input.update(cx,|input,cx|input.set_text(prompt,cx));view.app_flush_draft(cx)?;} else {view.app_send(prompt,cx)?;}Ok::<_,String>(())});
                        if let Err(error)=sent {self.finish_call(call,Err(error),cx);return;}
                        if flag(&input,"notifyOnComplete",false) && let Some(bot)=self.roster.get_mut(&bot.id) {
                            let origin=source.map(|v|v.read(cx).bot_turn).unwrap_or(0);
                            if let Some(launch) = bot.launches.iter_mut().find(|launch| launch.session_id == session) {
                                launch.completed = false;
                                launch.dismissed = false;
                                launch.notify_on_complete = true;
                                launch.set_origin(call.session, origin);
                            } else {
                                let conversation = view.read(cx);
                                let summary = &conversation.summary;
                                let mut launch = Launch {
                                    session_id: session,
                                    cwd: summary.cwd.clone(),
                                    title: summary.title.clone(),
                                    harness: summary.provider.clone(),
                                    model: conversation.bot_model(),
                                    notify_on_complete: true,
                                    ..Default::default()
                                };
                                launch.set_origin(call.session, origin);
                                bot.launches.push(launch);
                            }
                        }
                        Ok(json!({"accepted":true}))
                    }
                    Err(error)=>Err(error),
                }
            }
            "folders.list"=>Ok(json!(self.owner.groups(&project,cx).folders)),
            "folders.move"=>{
                self.app_move_to_folder(call,input,project,&actor.id,target,cx);return;
            }
            "artifacts.list"=>Ok(json!(bot.documents.iter().filter(|d|input.get("kind").and_then(Value::as_str).is_none_or(|k|k==d.kind)).skip(input.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize).take(input.get("limit").and_then(Value::as_u64).unwrap_or(50).min(200) as usize).collect::<Vec<_>>())),
            "artifacts.read"=>input.get("id").and_then(Value::as_str).and_then(|id|bot.documents.iter().find(|d|d.id==id)).map(|d|json!(d)).ok_or("Document not found".into()),
            "chat.card"=>{
                let mut card=match bot::cards::parse(&input){Ok(c)=>c,Err(e)=>{self.finish_call(call,Err(e),cx);return;}};
                card["id"]=json!(bot::new_id());
                if card["type"]=="session"{
                    let session=card["sessionId"].as_i64().unwrap();
                    let view=self.owner.ensure_session(session,cx).filter(|v|bot.works_on(&v.read(cx).summary.cwd));
                    let Some(view)=view else{self.finish_call(call,Err("Session is outside assigned projects".into()),cx);return;};
                    card["title"]=json!(view.read(cx).summary.title);card["state"]=json!(if view.read(cx).responding(){"Working"}else{"Saved"});
                }
                if card["type"]=="pr"{
                    let cwd=bot.projects.first().cloned().unwrap_or_else(||std::env::var("HOME").unwrap_or_else(|_|".".into()));
                    self.call_work(call,&actor.id,target,move|_|{let metadata=bot::cards::pull_request(&cwd,card["repo"].as_str(),card["number"].as_u64().unwrap())?;for key in ["title","state","url","isDraft","additions","deletions"]{card[key]=metadata[key].clone();}Ok((card.clone(),Effect::Card(card)))},cx);return;
                }
                self.hold_card(call.session,&bot.id,card.clone());self.save(cx);Ok(card)
            }
            "artifacts.write"=>{
                let kind=input.get("kind").and_then(Value::as_str).unwrap_or("document");
                if !["document","html","markdown"].contains(&kind){Err("Use document or html".into())}else{match text(&input,"title",512).and_then(|title|Ok((title,text(&input,"body",512*1024)?))){Err(e)=>Err(e),Ok((title,body))=>{
                    let id=input.get("id").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(bot::new_id);let now=now_ms();let document=Document{id:id.clone(),kind:kind.into(),title,summary:input.get("summary").and_then(Value::as_str).unwrap_or_default().chars().take(4000).collect(),body,created_at:bot.documents.iter().find(|d|d.id==id).map(|d|d.created_at).unwrap_or(now),updated_at:now};
                    if let Some(m)=self.roster.get_mut(&bot.id){if let Some(old)=m.documents.iter_mut().find(|d|d.id==id){*old=document.clone();}else{m.documents.push(document.clone());}}
                    self.hold_card(call.session,&bot.id,json!({"id":bot::new_id(),"type":"document","documentId":id,"title":document.title,"summary":document.summary}));self.save(cx);Ok(json!(document))
                }}}
            }
            "todos.list"|"todos.add"|"todos.update"|"todos.remove"|"todos.publish"|"todos.reconcile"|"todos.retry"|"todos.reconcileEdit"|"todos.retryEdit"|"todos.discardEdit"|"todos.cancelPublish"=>{
                let project=match bot.project(input.get("project").and_then(Value::as_str)){
                    Ok(project)=>project,Err(error)=>{self.finish_call(call,Err(error),cx);return;}
                };
                let board=vorkspace_core::todos::TodoBoard::Project(project.clone());
                let bot_id=bot.id.clone();
                let unambiguous=bot.projects.len()==1;
                self.call_work(call,&actor.id,target,move|store|{
                    // A one-project Bot's old board has one unambiguous owner.
                    // Multiple-project legacy rows wait for an explicit move
                    // from the To-dos page, not an incidental list request.
                    if unambiguous && action!="todos.list" { locked(&store)?.assign_legacy_bot_todos(&bot_id,&project)?; }
                    let mut result=crate::todos::todo_action(&store,&board,&action,&input)?;
                    if unambiguous && action=="todos.list" {
                        let legacy=locked(&store)?.list_todos(&vorkspace_core::todos::TodoBoard::Bot(bot_id.clone()))?;
                        if let Some(object)=result.as_object_mut(){object.insert("legacyItems".into(),json!(legacy));}
                    }
                    Ok((result,Effect::None))
                },cx);return;
            }
            "soul.read"|"soul.update"|"memory.read"|"memory.search"|"memory.add"|"memory.replace"|"memory.remove"|"habits.list"|"habits.add"|"habits.update"|"habits.run"|"habits.remove"|"worktrees.list"|"worktrees.create"=>{
                self.call_work(call,&actor.id,target.clone(),move |store| Self::app_storage_action(store, root, target, project, action, input),cx);return;
            }
            _=>Err("Unknown action. Run app help to see supported actions".into()),
        };
        self.finish_call(call, result, cx);
    }

    fn app_remove_worktree(
        &mut self,
        call: Call,
        input: &Value,
        project: String,
        cx: &mut Context<Self>,
    ) {
        let path = match text(input, "path", 4096) {
            Ok(path) => path,
            Err(error) => {
                self.finish_call(call, Err(error), cx);
                return;
            }
        };
        let bots = cx.weak_entity();
        self.owner.remove_worktree(
            project,
            path,
            Box::new(move |result, cx| {
                let _ = bots.update(cx, |bots, cx| {
                    bots.finish_call(call, result.map(|()| json!({"removed":true})), cx)
                });
            }),
            cx,
        );
    }

    fn app_fire_minion(
        &mut self,
        call: Call,
        input: &Value,
        actor_id: String,
        cx: &mut Context<Self>,
    ) {
        let query = match text(input, "minion", 160) {
            Ok(v) => v,
            Err(e) => {
                self.finish_call(call, Err(e), cx);
                return;
            }
        };
        let member = match self.roster.resolve_peer(&actor_id, &query, true) {
            Ok(b) => b.id.clone(),
            Err(e) => {
                self.finish_call(call, Err(e), cx);
                return;
            }
        };
        self.fire_minion(call, actor_id, member, cx);
    }

    fn app_move_to_folder(
        &mut self,
        call: Call,
        input: Value,
        project: String,
        actor_id: &str,
        target: String,
        cx: &mut Context<Self>,
    ) {
        let session = match id(&input, "sessionId") {
            Ok(id) => id,
            Err(e) => {
                self.finish_call(call, Err(e), cx);
                return;
            }
        };
        let allowed = self
            .owner
            .ensure_session(session, cx)
            .is_some_and(|v| v.read(cx).summary.cwd == project);
        if !allowed {
            self.finish_call(call, Err("Session is outside this project".into()), cx);
            return;
        }
        self.call_work(
            call,
            actor_id,
            target,
            move |store| {
                let destination = vorkspace_core::session_groups::FolderDestination::from_parts(
                    input["folderId"].as_str(),
                    input["newFolderName"].as_str(),
                );
                locked(&store)?.move_sessions_to_folder(&project, &[session], destination)?;
                Ok((json!({"moved":true}), Effect::Groups(project)))
            },
            cx,
        );
    }

    fn app_codebase_root(
        input: &Value,
        source: &Option<Entity<Conversation>>,
        project: &str,
        cx: &App,
    ) -> String {
        input
            .get("worktreeCwd")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                source.as_ref().and_then(|view| {
                    let view = view.read(cx);
                    (vorkspace_core::files::path_key(&view.summary.cwd)
                        == vorkspace_core::files::path_key(project))
                    .then(|| view.settings.worktree_cwd.clone())
                    .flatten()
                })
            })
            .unwrap_or_else(|| project.to_owned())
    }

    fn app_call_actors(
        &self,
        call: &Call,
        cx: &App,
    ) -> Option<(Bot, Bot, Option<super::effects::Permit>)> {
        let bot = self
            .roster
            .of_session(call.session)
            .or_else(|| {
                self.running
                    .get(&call.session)
                    .and_then(|r| self.roster.get(&r.bot))
            })
            .cloned();

        let Some(actor) = bot
            .and_then(|b| self.roster.effective_bot(&b.id))
            .filter(|b| {
                self.roster.is_active(&b.id)
                    && !self.shared_roster.read(cx).removed.contains_key(&b.id)
                    && (!b.is_minion() || (self.prefs.bots && self.prefs.minions))
            })
        else {
            let _ = call.reply.send(Err(
                "This Bot no longer owns an active app capability".into()
            ));
            return None;
        };
        let bot = self.roster.storage_owner(&actor.id).unwrap().clone();
        let permit = if actor.is_minion() {
            match self
                .shared_roster
                .read(cx)
                .minion_permit(&actor.id, &bot.projects)
            {
                Ok(permit) => Some(permit),
                Err(error) => {
                    let _ = call.reply.send(Err(error));
                    return None;
                }
            }
        } else {
            None
        };
        if actor.is_minion() && !bot::api::minion_allowed(&call.action) {
            let _ = call
                .reply
                .send(Err(format!("Minions cannot use {}", call.action)));
            return None;
        }
        if matches!(call.action.as_str(), "minions.spawn" | "minions.hire") && !self.prefs.minions {
            let _ = call.reply.send(Err(
                "Minions are experimental; turn them on in Settings › Bots first".into(),
            ));
            return None;
        }
        if call.action.starts_with("habits.")
            && call.action != "habits.list"
            && self.running.contains_key(&call.session)
        {
            let _ = call
                .reply
                .send(Err("A habit run cannot schedule more runs".into()));
            return None;
        }
        if let Err(error) = bot::api::validate(&call.action, &call.input) {
            let _ = call.reply.send(Err(error));
            return None;
        }

        Some((actor, bot, permit))
    }

    fn admit_app_call(&self, call: &Call) -> bool {
        let needs_admission = locked(&self.store)
            .and_then(|store| store.app_command_known(call.session, &call.request_id))
            .map(|known| known || !bot::api::is_read_only(&call.action));
        let needs_admission = match needs_admission {
            Ok(value) => value,
            Err(error) => {
                let _ = call.reply.send(Err(error));
                return false;
            }
        };
        if needs_admission {
            let admission = (|| {
                let store = locked(&self.store)?;
                // Existing completed receipts remain read-through compatibility.
                // A newer admitted receipt always wins over this older cache.
                if !store.app_command_known(call.session, &call.request_id)? {
                    let key = format!(
                        "bot-app-receipt:{}:{}",
                        call.session,
                        files::content_hash(&call.request_id)
                    );
                    if let Some(raw) = store.setting(&key)? {
                        let (saved,result):(String,Result<Value,String>)=serde_json::from_str(&raw).map_err(|e|format!("Cannot verify the prior command result; it was not replayed: {e}"))?;
                        if saved != fingerprint(call) {
                            return Err(
                                "Request ID was already used with different action or input".into(),
                            );
                        }
                        return Ok(vorkspace_core::store::AppCommandAdmission::Completed(
                            result,
                        ));
                    }
                }
                store.admit_app_command(call.session, &call.request_id, &call.action, &call.input)
            })();
            match admission {
                Ok(vorkspace_core::store::AppCommandAdmission::Admitted) => (),
                Ok(vorkspace_core::store::AppCommandAdmission::Completed(result)) => {
                    let _ = call.reply.send(result);
                    return false;
                }
                Err(error) => {
                    let _ = call.reply.send(Err(error));
                    return false;
                }
            }
        }

        true
    }

    fn app_peer_history(&mut self, call: Call, actor: &Bot, bot: &Bot, cx: &mut Context<Self>) {
        let target = self
            .roster
            .resolve_peer(
                &actor.id,
                call.input["bot"].as_str().unwrap_or_default(),
                true,
            )
            .and_then(|target| {
                if target.id == actor.id {
                    return Err("Use your own conversation history".into());
                }
                Ok((
                    target
                        .session_id
                        .ok_or("That Bot's conversation is unavailable")?,
                    json!({"id":target.id,"name":target.label()}),
                ))
            });
        let (session, target) = match target {
            Ok(target) => target,
            Err(error) => {
                self.finish_call(call, Err(error), cx);
                return;
            }
        };
        if call.action != "bots.read" {
            let root = self.root.clone();
            let recovery = (call.action == "bots.recoverHistory").then(|| {
                call.input["expectedHistory"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            });
            self.call_work(
                call,
                &actor.id,
                bot.id.clone(),
                move |store| {
                    if let Some(expected) = recovery {
                        vorkspace_runtime::runtime::native_history::recover(
                            &store, session, &root, &expected,
                        )?;
                        Ok((json!({"bot":target,"recovered":true}), Effect::None))
                    } else {
                        let result = vorkspace_runtime::runtime::native_history::sync(
                            &store, session, &root,
                        )?;
                        Ok((
                            json!({"bot":target,"unfinished":result.active,
                        "expectedHistory":result.recovery,"more":result.more,
                        "superseded":result.superseded}),
                            Effect::None,
                        ))
                    }
                },
                cx,
            );
            return;
        }
        let before = call.input["before"].as_u64();
        let limit = call.input["limit"].as_u64().unwrap_or(40).clamp(1, 100) as usize;
        let max = call.input["maxChars"]
            .as_u64()
            .unwrap_or(24_000)
            .clamp(100, 100_000) as usize;
        self.call_work(
            call,
            &actor.id,
            bot.id.clone(),
            move |store| {
                let mut page = locked(&store)?.peer_history(session, before, limit, max)?;
                page["bot"] = target;
                Ok((page, Effect::None))
            },
            cx,
        );
    }

    fn app_storage_action(
        store: Database,
        root: PathBuf,
        target: String,
        project: String,
        action: String,
        input: Value,
    ) -> Result<(Value, Effect), String> {
        let value = match action.as_str() {
            "soul.read" => json!(files::read(&root, &target, None, "SOUL.md")?),
            "soul.update" => {
                let body = input
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|t| t.len() <= 128 * 1024)
                    .ok_or("Provide text")?;
                let expected = text(&input, "expectedHash", 128)?;
                json!({"hash":files::save(&root,&target,None,"SOUL.md",body,Some(&expected))?})
            }
            "memory.read" => json!(memory::read(
                &root,
                &target,
                input.get("topic").and_then(Value::as_str)
            )?),
            "memory.search" => json!(memory::search(
                &root,
                &target,
                &text(&input, "query", 4000)?,
                input.get("since").and_then(Value::as_str)
            )?),
            "memory.add" | "memory.replace" | "memory.remove" => {
                let entry = if action == "memory.remove" {
                    None
                } else {
                    Some(memory::entry(
                        &text(&input, "fact", 4000)?,
                        &memory::date(),
                        input.get("until").and_then(Value::as_str),
                    )?)
                };
                let find = input
                    .get("find")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let hash = memory::edit(
                    &root,
                    &target,
                    input.get("topic").and_then(Value::as_str),
                    |old| {
                        if action == "memory.add" {
                            Ok(memory::add(old, entry.as_deref().unwrap()))
                        } else {
                            memory::replace(old, find, entry.as_deref(), &memory::date())
                        }
                    },
                )?;
                json!({"hash":hash})
            }
            "habits.list" => json!(habits::load(&root, &target)?),
            "habits.add" => {
                let schedule: habits::Schedule = serde_json::from_value(
                    input.get("schedule").cloned().ok_or("Provide schedule")?,
                )
                .map_err(|e| e.to_string())?;
                let h = Habit::new(
                    text(&input, "name", 480)?,
                    text(&input, "instructions", 32000)?,
                    schedule,
                    now_ms(),
                )?;
                habits::update(&root, &target, |all| {
                    all.push(h.clone());
                    Ok(())
                })?;
                json!(h)
            }
            "habits.update" | "habits.run" | "habits.remove" => {
                Self::app_update_habit(root, target, action, input)?
            }
            "worktrees.list" => {
                vorkspace_core::store::describe_worktrees(Path::new(&project), &store)?
            }
            "worktrees.create" => {
                let branch = text(&input, "branch", 240)?;
                let tree = vorkspace_core::git::create_project_worktree(
                    Path::new(&project),
                    &branch,
                    input.get("base").and_then(Value::as_str),
                    flag(&input, "existing", false),
                )?;
                json!({"path":tree.path,"branch":tree.branch})
            }
            _ => unreachable!(),
        };
        Ok((value, Effect::None))
    }

    fn app_update_habit(
        root: PathBuf,
        target: String,
        action: String,
        input: Value,
    ) -> Result<Value, String> {
        let id = text(&input, "id", 128)?;
        habits::update(&root, &target, |all| {
            let index = all
                .iter()
                .position(|h| h.id == id)
                .ok_or("Habit not found")?;
            if action == "habits.remove" {
                all.remove(index);
                return Ok(json!({"removed":true}));
            }
            let h = &mut all[index];
            if action == "habits.run" {
                h.run_requested = true;
            } else {
                if let Some(name) = input.get("name").and_then(Value::as_str) {
                    if name.trim().is_empty() || name.chars().count() > 120 {
                        return Err("Invalid habit name".into());
                    }
                    h.name = name.trim().into();
                }
                if let Some(instructions) = input.get("instructions").and_then(Value::as_str) {
                    if instructions.trim().is_empty() || instructions.len() > 32000 {
                        return Err("Invalid habit instructions".into());
                    }
                    h.instructions = instructions.into();
                }
                if let Some(schedule) = input.get("schedule") {
                    h.schedule =
                        serde_json::from_value(schedule.clone()).map_err(|e| e.to_string())?;
                    h.schedule.validate()?;
                    h.next_run_at = h.schedule.next(now_ms())?;
                }
                if let Some(enabled) = input.get("enabled").and_then(Value::as_bool) {
                    h.enabled = enabled;
                    if enabled {
                        h.next_run_at = h.schedule.next(now_ms())?;
                    }
                }
            }
            Ok(json!(h))
        })
    }

    pub(super) fn group_launch(&mut self, id: &str, session: i64, cx: &mut Context<Self>) {
        let Some(bot) = self
            .roster
            .get(id)
            .filter(|m| m.show_started_sessions_in_sidebar && m.use_sidebar_folders)
            .cloned()
        else {
            return;
        };
        let Some(launch) = bot.launches.iter().find(|l| l.session_id == session) else {
            return;
        };
        let project = launch.cwd.clone();
        let store = self.store.clone();
        let owner = self.owner.clone();
        let saved_project = project.clone();
        let work = cx.background_executor().spawn(async move {
            locked(&store)?.edit_groups(&saved_project, |groups| {
                let label = bot.label();
                let folder = groups
                    .folders
                    .iter()
                    .find(|folder| folder.name == label)
                    .map(|folder| folder.id.clone())
                    .or_else(|| groups.create_folder(&[session], bot::new_id()));
                if let Some(folder) = folder {
                    groups.rename_folder(&folder, &label);
                    let color = bot::COLORS_BOT
                        .iter()
                        .position(|c| *c == bot.color)
                        .map(|n| if n == 8 { 6 } else { n + 1 });
                    groups.set_color(&folder, color);
                    groups.add_to_folder(&folder, session);
                }
                Ok(())
            })
        });
        cx.spawn(async move |this, cx| match work.await {
            Ok(()) => cx.update(|cx| owner.apply_groups(project, cx)),
            Err(error) => {
                let _ = this.update(cx, |bots, cx| {
                    bots.error = Some(format!("Could not group the Bot session: {error}"));
                    bots.changed(cx);
                });
            }
        })
        .detach();
    }
}

impl Bots {
    pub(super) fn member_status(&self, id: &str, cx: &App) -> &'static str {
        let mut working = false;
        if let Some(view) = self.chats.get(id) {
            match view.read(cx).bot_status() {
                "needs-you" => return "needs-you",
                "working" => working = true,
                _ => (),
            }
        }
        if working { "working" } else { "idle" }
    }
    fn fire_minion(&self, call: Call, parent: String, member: String, cx: &mut Context<Self>) {
        let shared = self.shared_roster.clone();
        let target = member.clone();
        let saved = shared.update(cx, |shared, cx| shared.retire_minion(parent, target, cx));
        let store = self.store.clone();
        cx.spawn(async move |this, cx| {
            let result = saved
                .recv()
                .await
                .unwrap_or_else(|_| Err("Minion retirement was interrupted".into()));
            let result = match result {
                Err(error) => Err(error),
                Ok(session) => {
                    // Publication has already revoked the permit and stopped
                    // the chat in whichever window owns it.
                    let fences = shared.update(cx, |shared, _| shared.minion_fences(&member));
                    cx.background_executor()
                        .spawn(async move {
                            for fence in fences {
                                fence.wait()?;
                            }
                            if let Some(session) = session {
                                let store = locked(&store)?;
                                let mut queue = store.follow_ups(session)?;
                                queue.paused = true;
                                queue.revision += 1;
                                store.accept_follow_ups(session, &queue)?;
                            }
                            Ok(())
                        })
                        .await
                }
            };
            let _ = this.update(cx, |this, cx| {
                let result = result.and_then(|()| {
                    let bot = this
                        .shared_roster
                        .read(cx)
                        .committed()
                        .and_then(|roster| roster.get(&member))
                        .ok_or("The retired Minion no longer exists")?;
                    Ok(this.minion_value(bot, cx))
                });
                this.finish_call(call, result, cx);
            });
        })
        .detach();
    }
    pub(super) fn stop_retired_chat(&self, id: &str, cx: &mut Context<Self>) {
        if let Some(view) = self.chats.get(id) {
            view.update(cx, |view, cx| {
                view.bot_stop(cx);
                let _ = view.app_stop(cx);
            });
        }
    }
    pub(crate) fn minion_value(&self, member: &Bot, cx: &App) -> Value {
        let metadata = member.minion.as_ref().unwrap();
        let owner = self.roster.storage_owner(&member.id);
        let working = self
            .chats
            .get(&member.id)
            .is_some_and(|v| v.read(cx).responding());
        json!({"id":member.id,"name":member.label(),"parentId":metadata.parent_id,"role":metadata.role,"specialties":metadata.specialties,"retired":metadata.retired,"createdAt":metadata.created_at,"sessionId":member.session_id,"working":working,"status":if metadata.retired {"retired"} else {self.member_status(&member.id,cx)},"pendingMessages":member.outbox.iter().map(|m|json!({"id":m.id,"error":m.error})).collect::<Vec<_>>(),"projects":owner.map(|p|&p.projects),"accepted":true})
    }
    fn next_minion_name(&self, parent: &str) -> String {
        (1..)
            .map(|n| format!("Minion {n}"))
            .find(|name| {
                !self
                    .roster
                    .active_minions(parent)
                    .any(|m| m.label() == *name)
            })
            .unwrap()
    }
    pub(super) fn sync_minions(&self, cx: &mut Context<Self>) {
        self.shared_roster.update(cx, |shared, _| {
            shared.configure_minions(self.prefs.bots && self.prefs.minions);
        });
    }
}
