//! Capability-checked app actions used by Bot provider subprocesses.
use super::*;
use serde_json::{Value, json};
use std::path::Path;
use vorkspace_core::bot::{Document, Launch, bridge::Call, files, memory};
use vorkspace_core::{session_groups::Groups, time::now_ms};
enum Effect {
    Card(Value),
    None,
    /// A board changed; open views reload.
    Todos,
    Start(Box<super::host_api::AppStart>),
    Groups(String, Groups),
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
        let store = self.store.clone();
        let roster = self.roster.clone();
        let queued = roster
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
        let previous = self.writes.take();
        self.writes=Some(cx.spawn(async move|this,cx|{
            if let Some(previous)=previous { previous.await; }
            let work=cx.background_executor().spawn(async move{
                let store=locked(&store)?;
                // Acknowledgement follows durable owner publication. Any failed
                // write leaves admission unconfirmed, never safe to replay.
                roster.save(&store).map_err(|error|format!("App action may have executed but its state could not be saved; do not replay: {error}"))?;
                store.finish_app_command(call.session,&call.request_id,&call.action,&call.input,&result)?;
                Ok::<_,String>(result)
            });
            let result = work.await;
            if result.is_ok() {
                let _ = this.update(cx, |this, cx| {
                    for (bot, messages) in queued {
                        this.persisted_messages.extend(messages);
                        this.dispatch(&bot, cx);
                    }
                    this.persisted_messages.retain(|id| this.roster.bots.iter().any(|bot| bot.outbox.iter().any(|m| &m.id == id)));
                });
            }
            let _=reply.send(result.and_then(|result|result));
        }));
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
            match self.minion_effects.permit(actor_id) {
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
        let work = cx.background_executor().spawn(async move {
            match permit {
                Some(permit) => permit.run(|| work(store)),
                None => work(store),
            }
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
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
                        Effect::Todos => crate::todos::changed(cx),
                        Effect::Card(value) => {
                            this.hold_card(call.session, &bot_id, value);
                            this.save(cx);
                        }
                        Effect::Groups(project, groups) => {
                            this.owner.apply_groups(project, groups, cx);
                        }
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
            .chain(self.channels.views.values())
            .find(|view| view.read(cx).summary.id == caller)
            .map(|view| view.read(cx).bot_turn)
            .unwrap_or(0);
        let launch = Launch {
            session_id: start.summary.id,
            cwd: start.summary.cwd.clone(),
            title: start.summary.title.clone(),
            harness: start.summary.provider.clone(),
            model: start.settings.model.clone(),
            origin_turn: ((caller as u64) << 32) | origin,
            notify_on_complete: flag(&call.input, "notifyOnComplete", !start.draft),
            completed: false,
            dismissed: false,
        };
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
        let bot = self
            .roster
            .of_session(call.session)
            .or_else(|| {
                self.running
                    .get(&call.session)
                    .and_then(|r| self.roster.get(&r.bot))
            })
            .cloned();
        let bot = bot.or_else(|| {
            self.channels
                .views
                .iter()
                .find(|(_, v)| v.read(cx).summary.id == call.session)
                .and_then(|((_, bot), _)| self.roster.get(bot))
                .cloned()
        });
        let Some(actor) = bot
            .and_then(|b| self.roster.effective_bot(&b.id))
            .filter(|b| {
                self.roster.is_active(&b.id)
                    && (!b.is_minion() || (self.prefs.bots && self.prefs.minions))
            })
        else {
            let _ = call.reply.send(Err(
                "This Bot no longer owns an active app capability".into()
            ));
            return;
        };
        let bot = self.roster.storage_owner(&actor.id).unwrap().clone();
        if actor.is_minion() && !minion_allowed(&call.action) {
            let _ = call
                .reply
                .send(Err(format!("Minions cannot use {}", call.action)));
            return;
        }
        if matches!(call.action.as_str(), "minions.spawn" | "minions.hire") && !self.prefs.minions {
            let _ = call.reply.send(Err(
                "Minions are experimental; turn them on in Settings › Bots first".into(),
            ));
            return;
        }
        if call.action.starts_with("habits.")
            && call.action != "habits.list"
            && self.running.contains_key(&call.session)
        {
            let _ = call
                .reply
                .send(Err("A habit run cannot schedule more runs".into()));
            return;
        }
        if let Err(error) = bot::api::validate(&call.action, &call.input) {
            let _ = call.reply.send(Err(error));
            return;
        }
        let needs_admission = locked(&self.store)
            .and_then(|store| store.app_command_known(call.session, &call.request_id))
            .map(|known| known || !bot::api::is_read_only(&call.action));
        let needs_admission = match needs_admission {
            Ok(value) => value,
            Err(error) => {
                let _ = call.reply.send(Err(error));
                return;
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
                        if saved != fingerprint(&call) {
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
                    return;
                }
                Err(error) => {
                    let _ = call.reply.send(Err(error));
                    return;
                }
            }
        }
        if call.action == "bots.read" {
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
            return;
        }
        if call.action.starts_with("bots.") {
            let result = self.peer_action(&call, &actor.id, cx);
            self.finish_call(call, result, cx);
            return;
        }
        if call.action.starts_with("channels.") {
            if call.action == "channels.post"
                && self
                    .channels
                    .views
                    .values()
                    .any(|v| v.read(cx).summary.id == call.session)
            {
                self.finish_call(
                    call,
                    Err("Your final reply is posted to the channel automatically".into()),
                    cx,
                );
                return;
            }
            let hop = self.collaboration_hop(call.session, cx);
            let result = self.channel_action_with_hop(
                &call.action,
                &call.input,
                Some(&actor),
                &call.request_id,
                hop,
                cx,
            );
            self.finish_call(call, result, cx);
            return;
        }
        let input = call.input.clone();
        let action = call.action.clone();
        let root = self.root.clone();
        let target = bot.id.clone();
        let project = if action.starts_with("sessions.")
            || action.starts_with("folders.")
            || action.starts_with("worktrees.")
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
        let result=match action.as_str(){
            "models.list"=>Ok(json!(Kind::ALL.into_iter().map(|kind|json!({"harness":kind.id(),"name":kind.label(),"models":self.chats.values().find(|v|{let v=v.read(cx);let profile=source.as_ref().filter(|s|s.read(cx).summary.provider==kind.id()).map(|s|s.read(cx).settings.account.clone()).unwrap_or_else(||crate::accounts::get(cx).map(|center|center.read(cx).selected(kind,&project)).unwrap_or_default());v.summary.provider==kind.id()&&v.settings.account.id==profile.id}).map(|v|v.read(cx).settings_models()).unwrap_or_default()})).collect::<Vec<_>>())),
            "sessions.start"=>{
                let placement=match self.owner.placement(call.session,&project,&input,cx){Ok(placement)=>placement,Err(error)=>{self.finish_call(call,Err(error),cx);return;}};

                let source_provider=source.as_ref().map(|v|v.read(cx).summary.provider.clone()).unwrap_or_else(||"codex".into());
                let source_project=source.as_ref().map(|v|v.read(cx).summary.cwd.clone()).unwrap_or_else(||project.clone());
                let settings=source.as_ref().map(|v|v.read(cx).bot_execution_settings()).unwrap_or_default();
                let mut launch=match bot::launch::Launch::parse(&input,&source_provider,&source_project,&project,settings,false){Ok(launch)=>launch,Err(error)=>{self.finish_call(call,Err(error),cx);return;}};
                if launch.provider.id()!=source_provider { launch.settings.account=crate::accounts::get(cx).map(|center|center.read(cx).selected(launch.provider,&project)).unwrap_or_default(); }
                self.call_work(call,&actor.id,target,move|store|{
                    let mut store=locked(&store)?;
                    let summary=launch.create(&mut store,&project)?;
                    Ok((json!({"sessionId":summary.id,"accepted":true,"project":project}),Effect::Start(Box::new(super::host_api::AppStart{summary,settings:launch.settings,prompt:launch.prompt,draft:launch.draft,reveal:launch.reveal,placement,cleanup:launch.created_worktree}))))
                },cx);return;
            }
            "worktrees.remove"=>{
                let path=match text(&input,"path",4096){Ok(path)=>path,Err(error)=>{self.finish_call(call,Err(error),cx);return;}};
                let bots=cx.weak_entity();
                self.owner.remove_worktree(project,path,Box::new(move|result,cx|{
                    let _=bots.update(cx,|bots,cx|bots.finish_call(call,result.map(|()|json!({"removed":true})),cx));
                }),cx);return;
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
                let query=match text(&input,"minion",160){Ok(v)=>v,Err(e)=>{self.finish_call(call,Err(e),cx);return;}};
                let member=match self.roster.resolve_peer(&actor.id,&query,true){Ok(b)=>b.id.clone(),Err(e)=>{self.finish_call(call,Err(e),cx);return;}};
                let fences=self.minion_effects.fences(&member);
                match self.roster.retire_minion(&actor.id,&member) {
                    Err(e)=>Err(e),
                    Ok(())=>{
                        if let Some(view)=self.chats.get(&member){view.update(cx,|v,cx|{v.bot_stop(cx);let _=v.app_stop(cx);});}
                        let keys=self.channels.views.keys().filter(|(_,b)|b==&member).cloned().collect::<Vec<_>>();
                        for key in keys {if let Some(view)=self.channels.views.get(&key){view.update(cx,|v,cx|{v.bot_stop(cx);let _=v.app_stop(cx);});}}
                        let sessions=self.minion_sessions(&member);
                        self.quarantine_channel_work(cx);
                        let value=self.minion_value(self.roster.get(&member).unwrap(),cx);
                        self.save(cx);
                        self.call_work(call,&actor.id,target,move|store|{
                            for fence in fences {fence.wait()?;}
                            let store=locked(&store)?;
                            for session in sessions {
                                let mut queue=store.follow_ups(session)?;
                                queue.paused=true;
                                queue.revision+=1;
                                store.accept_follow_ups(session,&queue)?;
                            }
                            Ok((value,Effect::None))
                        },cx);return;
                    }
                }
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
                view.update(cx,|view,cx|view.load(cx));
                if !view.read(cx).ready_for_external(cx) {self.finish_call(call,Err("Session is busy, loading, or has an unsent draft".into()),cx);return;}
                match text(&input,"prompt",65536) {
                    Ok(prompt)=>{
                        let sent=view.update(cx,|view,cx|{if action=="sessions.draft" {view.input.update(cx,|input,cx|input.set_text(prompt,cx));view.app_flush_draft(cx)?;} else {view.app_send(prompt,cx)?;}Ok::<_,String>(())});
                        if let Err(error)=sent {self.finish_call(call,Err(error),cx);return;}
                        if flag(&input,"notifyOnComplete",false) && let Some(bot)=self.roster.get_mut(&bot.id) {
                            let origin=source.map(|v|v.read(cx).bot_turn).unwrap_or(0);
                            if let Some(launch)=bot.launches.iter_mut().find(|launch|launch.session_id==session) {launch.completed=false;launch.dismissed=false;launch.notify_on_complete=true;launch.origin_turn=((call.session as u64)<<32)|origin;}
                            else {let summary=&view.read(cx).summary;bot.launches.push(Launch{session_id:session,cwd:summary.cwd.clone(),title:summary.title.clone(),harness:summary.provider.clone(),model:view.read(cx).bot_model(),origin_turn:((call.session as u64)<<32)|origin,notify_on_complete:true,completed:false,dismissed:false});}
                        }
                        Ok(json!({"accepted":true}))
                    }
                    Err(error)=>Err(error),
                }
            }
            "folders.list"=>Ok(json!(self.owner.groups(&project,cx).folders)),
            "folders.move"=>{
                let session=match id(&input,"sessionId"){Ok(id)=>id,Err(e)=>{self.finish_call(call,Err(e),cx);return;}};
                let allowed=self.owner.ensure_session(session,cx).is_some_and(|v|v.read(cx).summary.cwd==project);
                if !allowed{self.finish_call(call,Err("Session is outside this project".into()),cx);return;}
                let mut groups=self.owner.groups(&project,cx);
                if let Some(name)=input.get("newFolderName").and_then(Value::as_str){if let Some(folder)=groups.create_folder(&[session],bot::new_id()){groups.rename_folder(&folder,name);}}
                else if let Some(folder)=input.get("folderId").and_then(Value::as_str){if groups.folder(folder).is_none(){self.finish_call(call,Err("Folder not found".into()),cx);return;}groups.add_to_folder(folder,session);}else{groups.remove_sessions(&[session]);}
                self.call_work(call,&actor.id,target,move|store|{locked(&store)?.save_groups(&project,&groups)?;Ok((json!({"moved":true}),Effect::Groups(project,groups)))},cx);return;
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
                    card["title"]=json!(view.read(cx).summary.title);card["state"]=json!(if view.read(cx).busy{"Working"}else{"Saved"});
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
                    self.documents.remove(&id);self.html_documents.remove(&id);self.hold_card(call.session,&bot.id,json!({"id":bot::new_id(),"type":"document","documentId":id,"title":document.title,"summary":document.summary}));self.save(cx);Ok(json!(document))
                }}}
            }
            "todos.list"|"todos.add"|"todos.update"|"todos.remove"=>{
                // Its own board, or an assigned project's.
                let board=match input.get("project").and_then(Value::as_str){
                    Some(path)=>match bot.project(Some(path)){Ok(p)=>vorkspace_core::todos::TodoBoard::Project(p),Err(e)=>{self.finish_call(call,Err(e),cx);return;}},
                    None=>vorkspace_core::todos::TodoBoard::Bot(bot.id.clone()),
                };
                let effect=if action=="todos.list"{Effect::None}else{Effect::Todos};
                self.call_work(call,&actor.id,target,move|store|Ok((locked(&store)?.todo_action(&board,&action,&input)?,effect)),cx);return;
            }
            "soul.read"|"soul.update"|"memory.read"|"memory.search"|"memory.add"|"memory.replace"|"memory.remove"|"habits.list"|"habits.add"|"habits.update"|"habits.run"|"habits.remove"|"notes.list"|"notes.read"|"notes.write"|"worktrees.list"|"worktrees.create"=>{
                let caller=call.session;let projects=bot.projects.clone();
                self.call_work(call,&actor.id,target.clone(),move|store|{
                    let value=match action.as_str(){
                        "soul.read"=>json!(files::read(&root,&target,None,"SOUL.md")?),
                        "soul.update"=>{let body=input.get("text").and_then(Value::as_str).filter(|t|t.len()<=128*1024).ok_or("Provide text")?;let expected=text(&input,"expectedHash",128)?;json!({"hash":files::save(&root,&target,None,"SOUL.md",body,Some(&expected))?})},
                        "memory.read"=>json!(memory::read(&root,&target,input.get("topic").and_then(Value::as_str))?),
                        "memory.search"=>json!(memory::search(&root,&target,&text(&input,"query",4000)?,input.get("since").and_then(Value::as_str))?),
                        "memory.add"|"memory.replace"|"memory.remove"=>{let entry=if action=="memory.remove"{None}else{Some(memory::entry(&text(&input,"fact",4000)?,&memory::date(),input.get("until").and_then(Value::as_str))?)};let find=input.get("find").and_then(Value::as_str).unwrap_or_default();let hash=memory::edit(&root,&target,input.get("topic").and_then(Value::as_str),|old|if action=="memory.add"{Ok(memory::add(old,entry.as_deref().unwrap()))}else{memory::replace(old,find,entry.as_deref(),&memory::date())})?;json!({"hash":hash})},
                        "habits.list"=>json!(habits::load(&root,&target)?),
                        "habits.add"=>{let schedule:habits::Schedule=serde_json::from_value(input.get("schedule").cloned().ok_or("Provide schedule")?).map_err(|e|e.to_string())?;let h=Habit::new(text(&input,"name",480)?,text(&input,"instructions",32000)?,schedule,now_ms())?;habits::update(&root,&target,|all|{all.push(h.clone());Ok(())})?;json!(h)},
                        "habits.update"|"habits.run"|"habits.remove"=>{let id=text(&input,"id",128)?;habits::update(&root,&target,|all|{let index=all.iter().position(|h|h.id==id).ok_or("Habit not found")?;if action=="habits.remove"{all.remove(index);return Ok(json!({"removed":true}));}let h=&mut all[index];if action=="habits.run"{h.run_requested=true;}else{if let Some(name)=input.get("name").and_then(Value::as_str){if name.trim().is_empty()||name.chars().count()>120{return Err("Invalid habit name".into());}h.name=name.trim().into();}
if let Some(instructions)=input.get("instructions").and_then(Value::as_str){if instructions.trim().is_empty()||instructions.len()>32000{return Err("Invalid habit instructions".into());}h.instructions=instructions.into();}
if let Some(schedule)=input.get("schedule"){h.schedule=serde_json::from_value(schedule.clone()).map_err(|e|e.to_string())?;h.schedule.validate()?;h.next_run_at=h.schedule.next(now_ms())?;}
if let Some(enabled)=input.get("enabled").and_then(Value::as_bool){h.enabled=enabled;if enabled{h.next_run_at=h.schedule.next(now_ms())?;}}}Ok(json!(h))})?},
                        "notes.list"|"notes.read"|"notes.write"=>{let db=locked(&store)?;let notes=db.notes()?;let allowed=|n:&&vorkspace_core::response::Note|projects.iter().any(|p|vorkspace_core::files::path_key(p)==vorkspace_core::files::path_key(&n.source_cwd));match action.as_str(){"notes.list"=>json!(notes.iter().filter(allowed).skip(input.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize).take(input.get("limit").and_then(Value::as_u64).unwrap_or(50).min(200) as usize).collect::<Vec<_>>()),"notes.read"=>{let id=id(&input,"id")? as u64;json!(notes.iter().filter(allowed).find(|n|n.id==id).ok_or("Note not found in assigned projects")?)},_=>{let id=input.get("id").and_then(Value::as_u64).unwrap_or_else(now_ms);if notes.iter().any(|n|n.id==id&&!projects.iter().any(|p|p==&n.source_cwd)){return Err("Note is outside assigned projects".into());}let cwd=notes.iter().find(|n|n.id==id).map(|n|n.source_cwd.clone()).or_else(||projects.first().cloned()).ok_or("Assign a project before writing notes")?;let note=vorkspace_core::response::Note{id,title:text(&input,"title",512)?,body:text(&input,"body",256*1024)?,tags:input.get("tags").and_then(Value::as_array).map(|tags|tags.iter().filter_map(Value::as_str).map(str::to_owned).collect()).or_else(||notes.iter().find(|n|n.id==id).map(|n|n.tags.clone())).unwrap_or_default(),source_session:caller,source_cwd:cwd};db.save_note(&note)?;json!(note)}}},
                        "worktrees.list"=>vorkspace_core::store::describe_worktrees(Path::new(&project),&store)?,
                        "worktrees.create"=>{let branch=text(&input,"branch",240)?;let tree=vorkspace_core::git::create_project_worktree(Path::new(&project),&branch,input.get("base").and_then(Value::as_str),flag(&input,"existing",false))?;json!({"path":tree.path,"branch":tree.branch})},
                        _=>unreachable!(),
                    };Ok((value,Effect::None))
                },cx);return;
            }
            _=>Err("Unknown action. Run app help to see supported actions".into()),
        };
        self.finish_call(call, result, cx);
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
        let mut groups = self.owner.groups(&project, cx);
        let folder = groups
            .folders
            .iter()
            .find(|f| f.name == bot.label())
            .map(|f| f.id.clone())
            .or_else(|| groups.create_folder(&[session], bot::new_id()));
        if let Some(folder) = folder {
            groups.rename_folder(&folder, &bot.label());
            let color = bot::COLORS_BOT
                .iter()
                .position(|c| *c == bot.color)
                .map(|n| if n == 8 { 6 } else { n + 1 });
            groups.set_color(&folder, color);
            groups.add_to_folder(&folder, session);
        }
        self.owner.apply_groups(project.clone(), groups.clone(), cx);
        let store = self.store.clone();
        cx.background_executor()
            .spawn(async move {
                let _ = locked(&store).and_then(|db| db.save_groups(&project, &groups));
            })
            .detach();
    }
    pub(super) fn check_completions(&mut self, cx: &mut Context<Self>) {
        let views = self.owner.session_views(cx);
        let mut ready = Vec::new();
        for bot in &self.roster.bots {
            let groups: HashSet<_> = bot
                .launches
                .iter()
                .filter(|l| l.notify_on_complete && !l.completed && !l.dismissed)
                .map(|l| l.origin_turn)
                .collect();
            for group in groups {
                let source = (group >> 32) as i64;
                if self
                    .chats
                    .values()
                    .chain(self.running.values().map(|r| &r.view))
                    .chain(self.channels.views.values())
                    .any(|v| v.read(cx).summary.id == source && v.read(cx).busy)
                {
                    continue;
                }
                let children: Vec<_> = bot
                    .launches
                    .iter()
                    .filter(|l| {
                        l.origin_turn == group
                            && l.notify_on_complete
                            && !l.completed
                            && !l.dismissed
                    })
                    .collect();
                if !children.is_empty()
                    && children.iter().all(|l| {
                        views
                            .iter()
                            .find(|v| v.id == l.session_id)
                            .is_none_or(|v| !v.busy && v.has_messages)
                    })
                    && !self.completion_work.contains(&(bot.id.clone(), group))
                {
                    ready.push((
                        bot.id.clone(),
                        group,
                        children
                            .iter()
                            .map(|l| (l.session_id, l.title.clone()))
                            .collect::<Vec<_>>(),
                    ));
                }
            }
        }
        for (id, group, children) in ready {
            self.completion_work.insert((id.clone(), group));
            let store = self.store.clone();
            let work = cx.background_executor().spawn(async move {
                children
                    .into_iter()
                    .map(|(id, title)| {
                        let rows = locked(&store)
                            .and_then(|db| db.load(id))
                            .map(|t| vorkspace_core::response::prose(&t.rows))
                            .unwrap_or_else(|_| "Session was removed".into());
                        format!(
                            "Session {id}: {title}\n{}",
                            vorkspace_core::response::limit(&rows, 12000)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n")
            });
            cx.spawn(async move |this,cx| {
                let report=work.await;
                let _=this.update(cx,|this,cx|{
                    this.completion_work.remove(&(id.clone(),group));
                    if let Some(bot)=this.roster.get_mut(&id){for launch in &mut bot.launches{if launch.origin_turn==group{launch.completed=true;}}}
                    let message=format!("[Delegated work completed]\nAll sessions in this group have stopped. Inspect their results and give an integrated update.\n\n{report}");
                    let source=(group>>32) as i64;
                    if let Some(run)=this.running.get_mut(&source){run.mailbox.push(message);this.save(cx);this.habit_changed(source,cx);}else{this.enqueue(&id,message,Vec::new(),true,cx);this.ensure_chat(&id,cx);}
                });
            }).detach();
        }
    }
}

/// What a minion may do with its parent's capability: share memory and the
/// board, read, and take notes; never start work or change its parent.
fn minion_allowed(action: &str) -> bool {
    action.starts_with("memory.")
        || action.starts_with("todos.")
        || action.starts_with("notes.")
        || matches!(
            action,
            "bots.list"
                | "bots.read"
                | "bots.send"
                | "channels.list"
                | "channels.read"
                | "channels.create"
                | "channels.post"
                | "models.list"
                | "soul.read"
                | "sessions.list"
                | "sessions.read"
                | "worktrees.list"
                | "minions.list"
                | "artifacts.list"
                | "artifacts.read"
        )
}

impl Bots {
    pub(super) fn member_status(&self, id: &str, cx: &App) -> &'static str {
        let mut working = false;
        for view in self.chats.get(id).into_iter().chain(
            self.channels
                .views
                .iter()
                .filter(|((_, member), _)| member == id)
                .map(|(_, view)| view),
        ) {
            match view.read(cx).bot_status() {
                "needs-you" => return "needs-you",
                "working" => working = true,
                _ => (),
            }
        }
        if working { "working" } else { "idle" }
    }
    fn minion_sessions(&self, id: &str) -> HashSet<i64> {
        let mut sessions = self
            .channels
            .state
            .channels
            .iter()
            .filter_map(|c| c.member_sessions.get(id).copied())
            .collect::<HashSet<_>>();
        sessions.extend(self.roster.get(id).and_then(|b| b.session_id));
        sessions
    }
    pub(crate) fn minion_value(&self, member: &Bot, cx: &App) -> Value {
        let metadata = member.minion.as_ref().unwrap();
        let owner = self.roster.storage_owner(&member.id);
        let working = self.chats.get(&member.id).is_some_and(|v| v.read(cx).busy)
            || self
                .channels
                .views
                .iter()
                .any(|((_, id), v)| id == &member.id && v.read(cx).busy);
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
    /// Kept as the Shell's shared attachment registry for source compatibility.
    /// Persistent members now receive Bot context through their own executor.
    pub(crate) fn sync_minions(&mut self) {
        self.minion_effects
            .reconcile(&self.roster, self.prefs.bots && self.prefs.minions);
        self.minion_registry.borrow_mut().clear();
    }
}
