use super::*;
use gpui::TestAppContext;
use vorkspace_core::{agent::Settings, store::Store};

#[gpui::test]
fn whole_project_sidebar_keeps_seven_today_sessions_across_worktrees_lazy(cx: &mut TestAppContext) {
    let store = Store::open_vorkspace(std::path::Path::new(":memory:")).unwrap();
    let mut summaries = Vec::new();
    let mut main = HashSet::new();
    let mut feature = HashSet::new();
    for i in 0..7 {
        let mut summary = store.create("codex", "/work/project").unwrap();
        summary.has_messages = true;
        summary.updated_at = vorkspace_core::time::now_ms();
        if i < 3 {
            main.insert(summary.id);
        } else {
            feature.insert(summary.id);
            store
                .save_agent_settings(
                    summary.id,
                    &Settings {
                        worktree_cwd: Some("/work/feature".into()),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        summaries.push(summary);
    }
    let archived = store.create("codex", "/work/project").unwrap();
    let mut archived = archived;
    archived.has_messages = true;
    archived.archived = true;
    summaries.push(archived);
    let mut other = store
        .create("codex", "/work/unlisted-other-project")
        .unwrap();
    other.has_messages = true;
    summaries.push(other);
    let database = Arc::new(Mutex::new(store));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| Shell::new(database, summaries, "/work/project".into(), None, cx))
        })
        .unwrap()
    });
    window
        .update(cx, |shell, _, cx| {
            let entities = shell.sessions.len();
            assert_eq!(shell.sidebar_session_ids(cx).len(), 7);
            shell.worktree_focus.insert(
                "/work/project".into(),
                crate::git_tools::worktree_switcher::WorktreeFocus {
                    path: "/work/feature".into(),
                    branch: Some("feature".into()),
                },
            );
            assert_eq!(
                shell
                    .sidebar_session_ids(cx)
                    .into_iter()
                    .collect::<HashSet<_>>(),
                feature
            );
            shell.worktree_focus.insert(
                "/work/project".into(),
                crate::git_tools::worktree_switcher::WorktreeFocus {
                    path: "/work/project".into(),
                    branch: None,
                },
            );
            assert_eq!(
                shell
                    .sidebar_session_ids(cx)
                    .into_iter()
                    .collect::<HashSet<_>>(),
                main
            );
            shell.worktree_focus.clear();
            assert_eq!(shell.sidebar_session_ids(cx).len(), 7);
            assert_eq!(
                shell.sessions.len(),
                entities,
                "counting and narrowing history stays metadata-only"
            );
            assert!(
                !shell
                    .workspace
                    .projects
                    .contains(&"/work/unlisted-other-project".into()),
                "a session cwd must not invent a project rail entry"
            );
        })
        .unwrap();
}

#[test]
fn remembered_controls_survive_the_mapped_native_model_default() {
    let remembered = Settings {
        model: Some("model-a".into()),
        effort: Some("high".into()),
        model_settings: [("reasoningEffort".into(), "high".into())].into(),
        ..Default::default()
    };
    let selected = new_session_settings(Some(remembered.clone()), Some("model-a".into()));
    assert_eq!(selected.effort.as_deref(), Some("high"));
    assert_eq!(
        selected
            .model_settings
            .get("reasoningEffort")
            .map(String::as_str),
        Some("high")
    );
    let changed = new_session_settings(Some(remembered), Some("model-b".into()));
    assert_eq!(changed.model.as_deref(), Some("model-b"));
    assert!(changed.effort.is_none());
    assert!(changed.model_settings.is_empty());
}

#[gpui::test]
fn restoring_large_history_materializes_only_visible_chats(cx: &mut TestAppContext) {
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let mut summaries = Vec::new();
    for i in 0..1200 {
        let cwd = if i < 1000 {
            "/work/history"
        } else {
            "/work/other"
        };
        let mut session = store.create("codex", cwd).unwrap();
        session.title = format!("Saved conversation {i}");
        session.has_messages = true;
        summaries.push(session);
    }
    let first = summaries[0].id;
    let oldest = summaries[999].id;
    let mut workspace = Workspace::default();
    workspace.open(oldest);
    workspace.open(first);
    let saved = serde_json::to_string(&workspace).unwrap();
    let database = Arc::new(Mutex::new(store));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| Shell::new(database, summaries, "/work/history".into(), Some(saved), cx))
        })
        .unwrap()
    });
    window
        .update(cx, |shell, window, cx| {
            assert_eq!(shell.session_summaries.len(), 1200);
            assert_eq!(shell.sessions.len(), 1);
            assert_eq!(shell.session_marks.len(), 1);
            assert_eq!(shell.tab_marks.len(), 1);
            assert_eq!(shell.sidebar_session_ids(cx).len(), 1000);
            assert_eq!(shell.visible_tabs(cx).len(), 2);
            assert_eq!(
                shell.tab_copy(oldest, cx).headline,
                "Saved conversation 999"
            );
            shell.query = "saved conversation 999".into();
            assert_eq!(shell.sidebar_session_ids(cx), [oldest]);
            assert_eq!(
                shell.sessions.len(),
                1,
                "filtering old history must remain metadata-only"
            );
            shell.query.clear();
            shell.run_session_menu(oldest, "pin", window, cx);
            assert!(shell.session_metadata(oldest, cx).summary.pinned);
            assert_eq!(
                shell.sessions.len(),
                1,
                "metadata actions must not construct chats"
            );
            shell.open(oldest, window, cx);
            assert_eq!(shell.workspace.active, Some(oldest));
            assert_eq!(shell.sessions.len(), 2);
        })
        .unwrap();
}
#[gpui::test]
fn inactive_history_cache_has_a_count_bound_and_reopens_evicted_history(cx: &mut TestAppContext) {
    let (window, _, active) = super::super::tests::window(cx);
    let ids = window
        .update(cx, |shell, _, cx| {
            let mut summaries = Vec::new();
            {
                let mut store = locked(&shell.store).unwrap();
                for i in 0..12 {
                    let summary = store.create("codex", "/work/cache").unwrap();
                    store
                        .write_rows(
                            summary.id,
                            0,
                            &[vorkspace_core::transcript::Row {
                                role: "assistant".into(),
                                text: format!("Saved answer {i}"),
                                first: true,
                                sent_at: None,
                            }],
                        )
                        .unwrap();
                    summaries.push(summary);
                }
            }
            let ids = summaries.iter().map(|s| s.id).collect::<Vec<_>>();
            for summary in summaries {
                let id = summary.id;
                shell.add(summary, cx);
                shell.history_clock += 1;
                shell.history_recency.insert(id, shell.history_clock);
                shell.sessions[&id].update(cx, |view, cx| view.load(cx));
            }
            ids
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, window, cx| {
            shell.update_chat_visibility(cx);
            assert_eq!(shell.workspace.active, Some(active));
            let retained = ids
                .iter()
                .filter(|&&id| shell.sessions[&id].read(cx).history_loaded())
                .count();
            assert!(
                retained <= 8,
                "inactive cache must stay bounded: {retained}"
            );
            assert!(!shell.sessions[&ids[0]].read(cx).history_loaded());
            shell.open(ids[0], window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            let chat = shell.sessions[&ids[0]].read(cx);
            assert!(chat.history_loaded());
            assert_eq!(
                chat.transcript.read(cx).source_rows()[0].text,
                "Saved answer 0"
            );
        })
        .unwrap();
}
