use super::*;
use crate::search::{Event, SearchView, Sources};
use vorkspace_core::search::Target;

pub(super) struct Open {
    pub(super) view: Entity<SearchView>,
    _events: Subscription,
}

impl Shell {
    pub(super) fn leave_app_search(&mut self, cx: &mut Context<Self>) {
        if let Some(search) = self.app_search.take() {
            self.saved_search = Some(search.view.read(cx).draft(cx));
            self.parked_search = Some(search);
        }
    }
    pub(super) fn close_app_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_app_resource(crate::workspace::AppPane::Search, window, cx);
    }
    pub(super) fn open_app_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.app_search.is_some() || self.parked_search.is_some() {
            self.prepare_app_view(window, cx);
            self.leave_app_views(cx);
            self.app_search = self.parked_search.take();
            self.admit_app_pane(crate::workspace::AppPane::Search);
            self.finish_app_pane_open(window, cx);
            return;
        }
        // Only restore focus captured from the conversation surface. Leaving a
        // sidebar page parks its focused input, so that handle is no longer visible.
        self.close_menu(cx);
        self.close_preview(cx);
        self.leave_app_views(cx);
        if let Some(bots) = &self.bots {
            bots.update(cx, |m, cx| m.close(cx));
        }
        if self.settings.is_some() {
            self.close_settings(window, cx);
        }
        self.navigation_revision += 1;
        let conversations = self
            .sessions
            .values()
            .cloned()
            .chain(
                self.bots
                    .as_ref()
                    .map(|m| m.read(cx).search_conversations())
                    .unwrap_or_default(),
            )
            .collect::<Vec<_>>();
        let mut summaries: HashMap<_, _> = self
            .order
            .iter()
            .map(|&id| (id, self.session_metadata(id, cx).summary.clone()))
            .collect();
        summaries.extend(
            conversations
                .iter()
                .map(|s| (s.read(cx).summary.id, s.read(cx).summary.clone())),
        );
        let summaries = summaries.into_values().collect();
        let live = conversations
            .iter()
            .filter(|s| s.read(cx).history_loaded())
            .map(|s| {
                (
                    s.read(cx).summary.id,
                    Arc::new(s.read(cx).transcript.read(cx).source_rows().to_vec()),
                )
            })
            .collect();
        let cwd = self.working_copy().to_owned();
        let store = self.store.clone();
        let projects = self.workspace.projects.clone();
        let recent = self
            .workspace
            .recent_files
            .get(&cwd)
            .cloned()
            .unwrap_or_default();
        let view = cx.new(|cx| {
            SearchView::new(
                cwd,
                store,
                Sources {
                    summaries,
                    live,
                    projects,
                    recent,
                    logos: self
                        .workspace
                        .project_appearance
                        .iter()
                        .filter_map(|(path, a)| a.logo.clone().map(|logo| (path.clone(), logo)))
                        .collect(),
                },
                cx,
            )
        });
        if let Some(draft) = self.saved_search.clone() {
            view.update(cx, |v, cx| v.restore_draft(draft, cx));
        }
        let events = cx.subscribe_in(
            &view,
            window,
            |this, _, event: &Event, window, cx| match event {
                Event::Close => this.close_app_search(window, cx),
                Event::Open(target, query) => {
                    this.leave_app_search(cx);
                    match target {
                        Target::Conversation(id) => this.open_search_session(*id, None, window, cx),
                        Target::Message { session, row } => this.open_search_session(
                            *session,
                            Some((*row, query.clone())),
                            window,
                            cx,
                        ),
                        Target::File { path, location } => {
                            let cwd = this.working_copy().to_owned();
                            this.open_review(
                                crate::git_tools::review::ReviewTab::File {
                                    cwd,
                                    path: path.clone(),
                                },
                                window,
                                cx,
                            );
                            if let Some((line, column)) = location
                                && let Some(pane) = this.active_review(cx)
                            {
                                pane.update(cx, |p, cx| {
                                    p.navigate_file(*line as usize, *column as usize, cx)
                                });
                            }
                        }
                        Target::Project(path) => this.select_project(path.clone(), window, cx),
                    }
                }
            },
        );
        window.focus(&view.focus_handle(cx), cx);
        self.app_search = Some(Open {
            view,
            _events: events,
        });
        self.admit_app_pane(crate::workspace::AppPane::Search);
        self.reveal_active_tab(cx);
        self.update_chat_visibility(cx);
        self.persist(cx);
        self.invalidate_chrome(cx);
        cx.notify();
    }
    fn open_search_session(
        &mut self,
        id: i64,
        jump: Option<(usize, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(view) = self.ensure_session(id, cx) {
            self.open(id, window, cx);
            if let Some((row, query)) = jump {
                view.update(cx, |v, cx| v.reveal_search(row, query, cx));
            }
            return;
        }
        if let Some(bots) = &self.bots
            && let Some(view) = bots.update(cx, |m, cx| m.open_search_session(id, window, cx))
        {
            if let Some((row, query)) = jump {
                view.update(cx, |v, cx| v.reveal_search(row, query, cx));
            }
            return;
        }
        let store = self.store.clone();
        let revision = self.navigation_revision;
        let work = cx.background_executor().spawn(async move {
            locked(&store)?
                .sessions()?
                .into_iter()
                .find(|s| s.id == id)
                .ok_or_else(|| "Conversation no longer exists".to_owned())
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.navigation_revision != revision {
                    return;
                }
                match result {
                    Ok(summary) => {
                        this.add(summary, cx);
                        this.open(id, window, cx);
                        if let Some((row, query)) = jump {
                            this.sessions[&id].update(cx, |v, cx| v.reveal_search(row, query, cx));
                        }
                    }
                    Err(error) => {
                        this.error = Some(error);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
    pub(super) fn sync_bot_search(&mut self, cx: &mut Context<Self>) {
        let Some(search) = self.app_search.as_ref().or(self.parked_search.as_ref()) else {
            return;
        };
        let views = self
            .bots
            .as_ref()
            .map(|m| m.read(cx).search_conversations())
            .unwrap_or_default();
        for view in views {
            if !view.read(cx).history_loaded() {
                let id = view.read(cx).summary.id;
                search
                    .view
                    .update(cx, |v, cx| v.remove_live_session(id, cx));
                continue;
            }
            let summary = view.read(cx).summary.clone();
            let rows = Arc::new(view.read(cx).transcript.read(cx).source_rows().to_vec());
            search
                .view
                .update(cx, |v, cx| v.update_session(summary, rows, cx));
        }
    }
    pub(super) fn sync_search_session(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(search) = self.app_search.as_ref().or(self.parked_search.as_ref()) else {
            return;
        };
        let Some(session) = self.sessions.get(&id) else {
            return;
        };
        let session = session.read(cx);
        if !session.history_loaded() {
            search
                .view
                .update(cx, |v, cx| v.remove_live_session(id, cx));
            return;
        }
        let summary = session.summary.clone();
        let rows = Arc::new(session.transcript.read(cx).source_rows().to_vec());
        search
            .view
            .update(cx, |v, cx| v.update_session(summary, rows, cx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    #[gpui::test]
    fn app_search_restores_draft_and_opens_cross_project_result(cx: &mut TestAppContext) {
        cx.update(|cx| {
            input::bind(cx);
            super::super::bind_keys(cx);
            vorkspace_ui::clock::use_test_clock(cx);
        });
        let (window, a, b) = crate::shell::tests::window(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.open(b, w, cx);
                let input = shell.sessions[&b].read(cx).input.clone();
                input.update(cx, |i, cx| i.set_text("preserve this draft".into(), cx));
                w.focus(&input.focus_handle(cx), cx);
                shell.sessions[&a]
                    .update(cx, |v, _| v.summary.title = "Needle conversation".into());
                shell.focus_search(w, cx);
            })
            .unwrap();
        cx.run_until_parked();
        cx.refresh().unwrap();
        window
            .update(cx, |shell, w, cx| {
                let view = &shell.app_search.as_ref().unwrap().view;
                assert!(view.focus_handle(cx).is_focused(w));
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "escape");
        cx.run_until_parked();
        window
            .update(cx, |shell, w, cx| {
                assert!(shell.app_search.is_none());
                let input = &shell.sessions[&b].read(cx).input;
                assert_eq!(input.read(cx).content, "preserve this draft");
                assert!(input.focus_handle(cx).is_focused(w));
                shell.focus_search(w, cx);
            })
            .unwrap();
        cx.run_until_parked();
        cx.simulate_input(window.into(), "Needle");
        cx.run_until_parked();
        cx.simulate_keystrokes(window.into(), "enter");
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                assert!(shell.app_search.is_none());
                assert_eq!(shell.workspace.active, Some(a));
                assert_eq!(shell.workspace.project, "/work/another-project");
                assert_eq!(
                    shell.sessions[&b].read(cx).input.read(cx).content,
                    "preserve this draft"
                );
            })
            .unwrap();
    }
}
