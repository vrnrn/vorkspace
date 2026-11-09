//! Project indexes are fetched on workers; opening a row materializes the chat.
use super::*;
use serde_json::json;

impl Shell {
    pub(super) fn refresh_remote_projects(&mut self, cx: &mut Context<Self>) {
        let projects: Vec<_> = self
            .workspace
            .vorkspace_projects()
            .into_iter()
            .filter(|path| path.starts_with("remote://"))
            .collect();
        // A slow host gets one slot. Rotate through the remaining projects
        // so a cached row can refresh even when another project is focused.
        for _ in 0..projects.len() {
            if self.remote_refreshing.len() >= 2 {
                break;
            }
            let ix = self.remote_refresh_cursor % projects.len();
            self.remote_refresh_cursor = ix + 1;
            let project = &projects[ix];
            if self.remote_refreshing.insert(project.clone()) {
                self.refresh_remote_project(project.clone(), cx);
            }
        }
    }

    fn refresh_remote_project(&mut self, project_key: String, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let client = self.remote.clone();
        let requested = project_key.clone();
        let work = cx.background_executor().spawn(async move {
            let project = locked(&store)?
                .remote_projects()?
                .into_values()
                .find(|p| p.key == project_key)
                .ok_or(
                    "Remote project identity is missing. Open this folder on its machine again.",
                )?;
            let machine = client.machine_for_environment(&project.environment_id)?;
            let response = client.request(
                &machine.id,
                "sessions.list",
                json!({"projectId":project.project_id}),
            )?;
            let values = response.as_array().ok_or("Invalid remote session list")?;
            let store = locked(&store)?;
            let mut summaries = Vec::new();
            let host_ids: HashSet<String> = values
                .iter()
                .map(|value| {
                    value["id"]
                        .as_str()
                        .filter(|id| !id.is_empty())
                        .map(str::to_owned)
                })
                .collect::<Option<_>>()
                .ok_or("Invalid remote session identity")?;
            store.prune_remote_sessions(&project, &host_ids)?;
            for value in values {
                let summary = store.register_remote_summary(&project, value)?;
                let state = store
                    .remote_session(summary.id)?
                    .ok_or("Remote session identity is missing")?;
                summaries.push((
                    summary,
                    state.running,
                    state.needs_input,
                    value["draft"] == true,
                ));
            }
            let retained: HashSet<i64> = store
                .sessions()?
                .iter()
                .filter(|s| s.cwd == project.key)
                .map(|s| s.id)
                .collect();
            Ok::<_, String>((project.key, summaries, retained))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.remote_refreshing.remove(&requested);
                match result {
                    Ok((project, summaries, retained)) => {
                        let removed: Vec<_> = this
                            .order
                            .iter()
                            .copied()
                            .filter(|id| {
                                this.session_summaries
                                    .get(id)
                                    .is_some_and(|s| s.cwd == project && !retained.contains(id))
                            })
                            .collect();
                        for id in removed {
                            this.session_removal_epoch = this.session_removal_epoch.wrapping_add(1);
                            this.remote_marks.remove(&id);
                            this.order.retain(|existing| *existing != id);
                            if let Some(view) = this.sessions.get(&id) {
                                view.update(cx, |view, cx| {
                                    view.remote_connection_error(
                                        "This conversation was deleted on the host.",
                                        cx,
                                    )
                                });
                            }
                            // Open tabs retain an explanatory placeholder. A stale
                            // tab must not be rebound to a new host conversation.
                            if this.workspace.tab_of(id).is_none() {
                                this.session_summaries.remove(&id);
                                this.sessions.remove(&id);
                            }
                        }
                        for (summary, running, waiting, draft) in summaries {
                            let id = summary.id;
                            this.remote_marks.insert(id, (running, waiting));
                            if draft {
                                this.draft_sessions.insert(id);
                            } else {
                                this.draft_sessions.remove(&id);
                            }
                            if !this.session_summaries.contains_key(&id) {
                                this.order.push(id);
                            }
                            if let Some(view) = this.sessions.get(&id) {
                                view.update(cx, |view, cx| {
                                    if !view.busy {
                                        view.summary = summary.clone();
                                        cx.notify();
                                    }
                                });
                            }
                            this.session_summaries.insert(id, summary);
                        }
                        if this.workspace.vorkspace_has_project(&project) {
                            this.invalidate_chrome(cx);
                        }
                    }
                    Err(error) => {
                        // A transient disconnect retains the last host index.
                        // The active conversation exposes the error and Retry.
                        if let Some(id) = this.workspace.active
                            && let Some(owner) = this.workspace.session_for_pane(id)
                            && let Some(view) = this.sessions.get(&owner)
                            && view.read(cx).summary.cwd == requested
                        {
                            view.update(cx, |view, cx| view.remote_connection_error(&error, cx));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}
