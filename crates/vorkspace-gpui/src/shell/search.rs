use super::*;
use crate::search::{Event, SearchView, Sources};
use vorkspace_core::search::Target;

pub(super) struct Open {
    cwd: String,
    pub(super) view: Entity<SearchView>,
    _events: Subscription,
}

impl Open {
    pub(super) fn matches_working_copy(&self, cwd: &str) -> bool {
        vorkspace_core::files::path_key(&self.cwd) == vorkspace_core::files::path_key(cwd)
    }
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
        self.prepare_app_view(
            self.app_pane_id(&crate::workspace::AppPane::Search),
            window,
            cx,
        );
        self.leave_app_views(cx);
        let cwd = self.working_copy().to_owned();
        if let Some(search) = self
            .parked_search
            .take()
            .filter(|search| search.matches_working_copy(&cwd))
        {
            self.app_search = Some(search);
            self.admit_app_pane(crate::workspace::AppPane::Search);
            self.finish_app_pane_open(window, cx);
            return;
        }
        // A different working copy needs its own file index and content-search
        // owner. prepare_app_view retained the query before dropping the old view.
        if let Some(bots) = &self.bots {
            bots.update(cx, |m, cx| m.close(cx));
        }
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
                cwd.clone(),
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
        let source_cwd = cwd.clone();
        let events =
            cx.subscribe_in(
                &view,
                window,
                move |this, _, event: &Event, window, cx| match event {
                    Event::Close => this.close_app_search(window, cx),
                    Event::Open(target, query) => {
                        this.leave_app_search(cx);
                        match target {
                            Target::Conversation(id) => {
                                this.open_search_session(*id, None, window, cx)
                            }
                            Target::Message { session, row } => this.open_search_session(
                                *session,
                                Some((*row, query.clone())),
                                window,
                                cx,
                            ),
                            Target::File { path, location } => {
                                let cwd = source_cwd.clone();
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
            cwd,
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
    fn parked_search_refreshes_same_working_copy_files_and_content_as_a_split(
        cx: &mut TestAppContext,
    ) {
        use std::{path::Path, process::Command, time::Duration};
        use vorkspace_core::search::Scope;

        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().to_string_lossy().into_owned();
        for (path, text) in [
            ("needle-before.txt", "original filename"),
            ("needle-delete.txt", "to be deleted"),
            ("contents.txt", "needle before\n"),
        ] {
            std::fs::write(root.path().join(path), text).unwrap();
        }
        for args in [&["init", "--quiet"][..], &["add", "--", "."][..]] {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(root.path())
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let (window, _, _) = crate::shell::tests::window(cx);
        window
            .update(cx, |shell, w, cx| shell.select_project(cwd.clone(), w, cx))
            .unwrap();
        cx.run_until_parked();
        let (session, pane, view) = window
            .update(cx, |shell, w, cx| {
                let session = shell.workspace.active.unwrap();
                shell.open_app_search(w, cx);
                let pane = shell.workspace.active.unwrap();
                let view = shell.app_search.as_ref().unwrap().view.clone();
                view.update(cx, |view, cx| {
                    view.restore_draft(("needle".into(), Scope::Files), cx)
                });
                (session, pane, view)
            })
            .unwrap();
        let results = |cx: &mut TestAppContext| {
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(200));
            cx.run_until_parked();
            view.read_with(cx, |view, _| {
                let mut files = view
                    .hits()
                    .iter()
                    .filter_map(|hit| {
                        let Target::File { path, location } = &hit.target else {
                            return None;
                        };
                        Some((
                            Path::new(path)
                                .file_name()
                                .unwrap()
                                .to_string_lossy()
                                .into_owned(),
                            *location,
                        ))
                    })
                    .collect::<Vec<_>>();
                files.sort();
                files
            })
        };
        assert_eq!(
            results(cx),
            [
                ("contents.txt".into(), Some((1, 1))),
                ("needle-before.txt".into(), None),
                ("needle-delete.txt".into(), None),
            ]
        );
        window
            .update(cx, |shell, w, cx| {
                shell.open(session, w, cx);
                assert!(shell.app_search.is_none());
                assert_eq!(shell.parked_search.as_ref().unwrap().view, view);
            })
            .unwrap();
        std::fs::rename(
            root.path().join("needle-before.txt"),
            root.path().join("needle-renamed.txt"),
        )
        .unwrap();
        std::fs::remove_file(root.path().join("needle-delete.txt")).unwrap();
        std::fs::write(root.path().join("needle-created.txt"), "new file").unwrap();
        std::fs::write(
            root.path().join("contents.txt"),
            "no match here\nneedle after\n",
        )
        .unwrap();

        window
            .update(cx, |shell, w, cx| {
                // A prepared Search sibling becomes visible without an explicit
                // open_app_search call, and keeps the same view and query.
                shell.workspace.place(pane, session, Edge::Right);
                shell.open(session, w, cx);
                assert_eq!(shell.workspace.layout(session).leaf_ids(), [session, pane]);
                assert!(shell.app_search.is_none());
                assert_eq!(shell.parked_search.as_ref().unwrap().view, view);
                assert_eq!(view.read(cx).draft(cx), ("needle".into(), Scope::Files));
            })
            .unwrap();
        assert_eq!(
            results(cx),
            [
                ("contents.txt".into(), Some((2, 1))),
                ("needle-created.txt".into(), None),
                ("needle-renamed.txt".into(), None),
            ]
        );
    }

    #[gpui::test]
    fn parked_search_rebuilds_for_a_new_working_copy_and_opens_its_own_file(
        cx: &mut TestAppContext,
    ) {
        vorkspace_ui::clock::use_test_clock(cx);
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let b_tree = b.path().join("checkout");
        std::fs::create_dir(&b_tree).unwrap();
        for (root, text) in [(a.path(), "Project A"), (b_tree.as_path(), "Project B")] {
            std::fs::write(root.join("same.txt"), text).unwrap();
        }
        let b_project = b.path().to_string_lossy().into_owned();
        let b_cwd = b_tree.to_string_lossy().into_owned();
        let (window, _, _) = crate::shell::tests::window(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.select_project(a.path().to_string_lossy().into_owned(), w, cx);
                shell.open_app_search(w, cx);
                let first = shell.app_search.as_ref().unwrap().view.clone();
                first.update(cx, |view, cx| {
                    view.restore_draft(("same".into(), vorkspace_core::search::Scope::Files), cx)
                });
                shell.select_project(b_project.clone(), w, cx);
                shell.worktree_focus.insert(
                    b_project.clone(),
                    crate::git_tools::worktree_switcher::WorktreeFocus {
                        path: b_cwd.clone(),
                        branch: None,
                    },
                );
                shell.open_app_search(w, cx);
                let search = shell.app_search.as_ref().unwrap();
                assert_ne!(search.view, first);
                assert_eq!(search.cwd, b_cwd);
                assert_eq!(search.view.read(cx).draft(cx).0, "same");
                search.view.update(cx, |_, cx| {
                    cx.emit(Event::Open(
                        Target::File {
                            path: "same.txt".into(),
                            location: None,
                        },
                        "same".into(),
                    ))
                });
            })
            .unwrap();
        cx.run_until_parked();
        let (pane, review) = window
            .update(cx, |shell, w, cx| {
                let pane = shell.workspace.active.unwrap();
                let review = shell.active_review(cx).unwrap();
                assert_eq!(
                    review.read(cx).state.tabs,
                    [crate::git_tools::review::ReviewTab::File {
                        cwd: b_cwd.clone(),
                        path: "same.txt".into(),
                    }]
                );
                let editor = review.read(cx).active_editor().unwrap();
                assert_eq!(
                    editor.read(cx).path,
                    b_tree.join("same.txt").to_string_lossy()
                );
                assert_eq!(editor.read(cx).input.read(cx).content, "Project B");
                shell.select_project(a.path().to_string_lossy().into_owned(), w, cx);
                assert!(!shell.visible_tabs(cx).contains(&TitleTab::Workspace(pane)));
                shell.open(pane, w, cx);
                assert_eq!(shell.workspace.project, b_project);
                assert_eq!(shell.working_copy(), b_cwd);
                assert_eq!(shell.active_review(cx), Some(review.clone()));
                (pane, review)
            })
            .unwrap();
        cx.run_until_parked();
        let (store, summaries, saved) = window
            .update(cx, |shell, _, _| {
                let store = locked(&shell.store).unwrap();
                (
                    shell.store.clone(),
                    store.sessions().unwrap(),
                    store.ui_state().unwrap(),
                )
            })
            .unwrap();
        let restored = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| Shell::new(store, summaries, b_project.clone(), saved, cx))
            })
            .unwrap()
        });
        cx.run_until_parked();
        restored
            .update(cx, |shell, _, cx| {
                assert_eq!(shell.workspace.active, Some(pane));
                assert_eq!(shell.workspace.project, b_project);
                assert_eq!(shell.working_copy(), b_cwd);
                let reopened = shell.active_review(cx).unwrap();
                assert_ne!(reopened, review);
                let editor = reopened.read(cx).active_editor().unwrap();
                assert_eq!(
                    editor.read(cx).path,
                    b_tree.join("same.txt").to_string_lossy()
                );
                assert_eq!(editor.read(cx).input.read(cx).content, "Project B");
            })
            .unwrap();
    }

    #[gpui::test]
    fn restoring_a_mixed_layout_rebuilds_search_for_its_working_copy(cx: &mut TestAppContext) {
        let checkout = tempfile::tempdir().unwrap();
        let cwd = checkout.path().to_string_lossy().into_owned();
        let (window, _, session) = crate::shell::tests::window(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.open(session, w, cx);
                shell.open_app_search(w, cx);
                let search = shell.workspace.active.unwrap();
                let previous = shell.app_search.as_ref().unwrap().view.clone();
                previous.update(cx, |view, cx| {
                    view.restore_draft(
                        ("keep this".into(), vorkspace_core::search::Scope::Files),
                        cx,
                    )
                });
                shell.workspace.place(search, session, Edge::Right);
                shell.workspace.tab_workspaces.insert(session, cwd.clone());
                shell.open(session, w, cx);

                assert_eq!(shell.workspace.active, Some(session));
                assert_eq!(
                    shell.workspace.layout(session).leaf_ids(),
                    [session, search]
                );
                let rebuilt = shell.parked_search.as_ref().unwrap();
                assert_ne!(rebuilt.view, previous);
                assert_eq!(rebuilt.cwd, cwd);
                assert_eq!(rebuilt.view.read(cx).draft(cx).0, "keep this");
            })
            .unwrap();
    }

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
