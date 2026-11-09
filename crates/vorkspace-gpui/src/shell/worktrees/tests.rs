use super::*;
use crate::shell::tests::window;
use gpui::TestAppContext;

#[gpui::test]
fn splitting_reopened_notes_uses_the_selected_project_working_copy(cx: &mut TestAppContext) {
    let (window, first, _) = window(cx);
    let project = "/work/vorkspace";
    let checkout = "/work/trees/vorkspace-copy";
    let (notes, count) = window
        .update(cx, |shell, w, cx| {
            shell.open(first, w, cx);
            let first_project = shell.workspace.project.clone();
            let first_checkout = "/work/trees/other-copy";
            for (owner, path) in [(first_project, first_checkout), (project.into(), checkout)] {
                shell.worktree_focus.insert(
                    owner,
                    WorktreeFocus {
                        path: path.into(),
                        branch: None,
                    },
                );
            }
            shell.open_notes(w, cx);
            let notes = shell.workspace.active.unwrap();
            assert_eq!(shell.workspace.app_panes[&notes].worktree, first_checkout);
            shell.select_project(project.into(), w, cx);
            shell.open_notes(w, cx);
            assert_eq!(shell.workspace.active, Some(notes));
            assert_eq!(shell.working_copy(), checkout);
            let count = locked(&shell.store).unwrap().sessions().unwrap().len();
            shell.split_session(SplitDir::Right, cx);
            (notes, count)
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            let id = shell.workspace.active.unwrap();
            let store = locked(&shell.store).unwrap();
            let sessions = store.sessions().unwrap();
            assert_eq!(sessions.len(), count + 1);
            assert_eq!(
                sessions
                    .iter()
                    .find(|session| session.id == id)
                    .unwrap()
                    .cwd,
                project
            );
            assert_eq!(
                store
                    .agent_settings(id)
                    .unwrap()
                    .unwrap()
                    .worktree_cwd
                    .as_deref(),
                Some(checkout)
            );
            assert_eq!(shell.session_working_copy(id, cx), checkout);
            assert_eq!(shell.workspace.layout(notes).leaf_ids(), [notes, id]);
            assert_eq!(shell.workspace.tab_workspaces[&notes], checkout);
        })
        .unwrap();
}

#[gpui::test]
fn terminal_split_keeps_its_launch_scope_and_rejects_a_foreign_layout(cx: &mut TestAppContext) {
    let (window, _, session) = window(cx);
    let project = "/work/another-project";
    let checkout = "/work/trees/other-copy";
    let (terminal, count) = window
        .update(cx, |shell, _, cx| {
            // The saved descriptor supplies launch identity without starting a PTY.
            let terminal = shell
                .workspace
                .app_pane(
                    crate::workspace::AppPane::Terminal {
                        id: 1,
                        cwd: checkout.into(),
                    },
                    project.into(),
                    checkout.into(),
                )
                .unwrap();
            shell.workspace.place(terminal, session, Edge::Right);
            let count = locked(&shell.store).unwrap().sessions().unwrap().len();
            shell.split_session(SplitDir::Down, cx);
            assert!(
                shell
                    .error
                    .as_deref()
                    .is_some_and(|error| error.contains("project"))
            );
            assert_eq!(
                shell.workspace.layout(session).leaf_ids(),
                [session, terminal]
            );
            shell.workspace.detach(terminal, shell.workspace.tabs.len());
            shell.error = None;
            shell.split_session(SplitDir::Right, cx);
            (terminal, count)
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            let id = shell.workspace.active.unwrap();
            let store = locked(&shell.store).unwrap();
            let sessions = store.sessions().unwrap();
            assert_eq!(
                sessions.len(),
                count + 1,
                "only the standalone split creates a session"
            );
            assert_eq!(
                sessions
                    .iter()
                    .find(|session| session.id == id)
                    .unwrap()
                    .cwd,
                project
            );
            assert_eq!(
                store
                    .agent_settings(id)
                    .unwrap()
                    .unwrap()
                    .worktree_cwd
                    .as_deref(),
                Some(checkout)
            );
            assert_eq!(shell.workspace.project, project);
            assert_eq!(shell.working_copy(), checkout);
            assert_eq!(shell.session_working_copy(id, cx), checkout);
            assert_eq!(shell.workspace.layout(terminal).leaf_ids(), [terminal, id]);
        })
        .unwrap();
}

#[gpui::test]
fn header_settings_and_worktree_actions_open_focused_native_surfaces(cx: &mut TestAppContext) {
    cx.update(|cx| {
        input::bind(cx);
        crate::popover::bind(cx);
        crate::shell::bind_keys(cx);
    });
    let (window, _, active) = window(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.open_menu(
                MenuTarget::Header,
                Anchor::Slot(shell.project_anchor.clone()),
                w,
                cx,
            );
        })
        .unwrap();
    cx.run_until_parked();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    // The project name's menu is the picker with one worktree row; Settings
    // lives in the rail footer.
    assert!(visual.debug_bounds("menu-action-settings").is_none());
    window
        .update(visual, |shell, _, _| {
            assert_eq!(shell.workspace.active, Some(active))
        })
        .unwrap();
    let row = visual.debug_bounds("menu-action-switch-worktree").unwrap();
    visual.simulate_click(row.center(), Default::default());
    visual.run_until_parked();
    let panel = visual
        .debug_bounds("worktree-switcher")
        .expect("worktree flyout opens");
    assert!((panel.size.width - px(278.)).abs() < px(1.));
    assert_eq!(
        visual
            .debug_bounds("worktree-switcher-search")
            .unwrap()
            .size
            .height,
        px(44.)
    );
    visual.simulate_input("a-copy");
    visual.run_until_parked();
    window
        .update(visual, |shell, _, cx| {
            assert!(shell.menu.is_some());
            assert!(
                shell.sessions[&active]
                    .read(cx)
                    .input
                    .read(cx)
                    .content
                    .is_empty(),
                "typing belongs to the flyout"
            );
        })
        .unwrap();
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| assert!(shell.menu.is_none()))
        .unwrap();
}

#[gpui::test]
fn copies_keep_running_sessions_and_route_new_tabs_and_splits(cx: &mut TestAppContext) {
    let (window, feature_session, main_session) = window(cx);
    window
        .update(cx, |shell, w, cx| shell.open(feature_session, w, cx))
        .unwrap();
    cx.run_until_parked();
    let project = "/work/vorkspace";
    let feature = WorktreeFocus {
        path: "/work/vorkspace-worktrees/feature".into(),
        branch: Some("feature".into()),
    };
    let other = WorktreeFocus {
        path: "/work/vorkspace-worktrees/other".into(),
        branch: Some("other".into()),
    };
    window
        .update(cx, |shell, w, cx| {
            shell.sessions[&feature_session].update(cx, |session, cx| {
                session.summary.cwd = project.into();
                session.settings.worktree_cwd = Some(feature.path.clone());
                session.busy = true;
                session
                    .input
                    .update(cx, |input, cx| input.set_text("keep this draft".into(), cx));
            });
            shell
                .session_worktrees
                .insert(feature_session, Some(feature.path.clone()));
            shell
                .workspace
                .tab_workspaces
                .insert(feature_session, feature.path.clone());
            shell.sessions[&main_session]
                .update(cx, |session, _| session.summary.has_messages = true);
            shell.open(main_session, w, cx);
            shell
                .groups
                .entry(project.into())
                .or_default()
                .create_folder(&[main_session], "saved-folder".into());
            shell.select_worktree(Some(feature.clone()), w, cx);
            assert_eq!(shell.workspace.project, project);
            assert_eq!(shell.workspace.active, Some(feature_session));
            // The rail lists every session in the vorkspace, whichever
            // worktree the Explorer shows.
            assert!(shell.sidebar_session_ids(cx).contains(&feature_session));
            assert_eq!(shell.terminal_cwd(cx), feature.path);
            shell.select_worktree(None, w, cx);
            assert_eq!(shell.workspace.active, Some(main_session));
            assert_eq!(
                shell
                    .sidebar_session_ids(cx)
                    .into_iter()
                    .collect::<HashSet<_>>(),
                HashSet::from([main_session, feature_session]),
                "the default workspace lists the whole project"
            );
            assert_eq!(shell.terminal_cwd(cx), project);
            shell.open(feature_session, w, cx);
            assert_eq!(shell.working_copy(), project);
            assert!(shell.worktree_focus.is_empty());
            assert!(
                shell
                    .visible_tabs(cx)
                    .contains(&TitleTab::Workspace(feature_session)),
                "opening a worktree chat keeps its tab in the current workspace"
            );
            shell.open(main_session, w, cx);
            // A late blank from the other copy cannot take over after returning.
            shell.select_worktree(Some(other.clone()), w, cx);
            shell.select_worktree(None, w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, w, cx| {
            assert_eq!(shell.workspace.active, Some(main_session));
            assert_eq!(shell.sessions.len(), 3);
            shell.select_worktree(Some(other.clone()), w, cx);
            let id = shell
                .workspace
                .active
                .expect("reuse the completed blank for this copy");
            assert_eq!(shell.sessions.len(), 3);
            assert_eq!(shell.sessions[&id].read(cx).summary.cwd, project);
            assert_eq!(
                shell.sessions[&id]
                    .read(cx)
                    .settings
                    .worktree_cwd
                    .as_deref(),
                Some(other.path.as_str())
            );
            assert_eq!(
                locked(&shell.store)
                    .unwrap()
                    .agent_settings(id)
                    .unwrap()
                    .unwrap()
                    .worktree_cwd
                    .as_deref(),
                Some(other.path.as_str())
            );
            shell.create(Kind::Claude, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert_eq!(shell.sessions.len(), 4);
            assert_eq!(shell.terminal_cwd(cx), other.path);
            shell.split_session(SplitDir::Right, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, w, cx| {
            assert_eq!(shell.sessions.len(), 5);
            assert_eq!(shell.workspace.project, project);
            assert_eq!(shell.terminal_cwd(cx), other.path);
            let running = shell.sessions[&feature_session].read(cx);
            assert!(running.busy);
            assert_eq!(running.input.read(cx).content.as_ref(), "keep this draft");
            assert!(shell.worktree_focus.contains_key(project));
            // A directly opened chat joins the selected workspace without
            // changing either its checkout or the explicit sidebar filter.
            shell.open(main_session, w, cx);
            assert_eq!(shell.worktree_focus[project].path, other.path);
            assert!(
                shell
                    .visible_tabs(cx)
                    .contains(&TitleTab::Workspace(main_session))
            );
            assert_eq!(shell.terminal_cwd(cx), project);
            assert_eq!(
                shell.worktree_returns[&(project.to_owned(), other.path.clone())],
                main_session
            );
            shell.select_project("/work/another-project".into(), w, cx);
            shell.select_project(project.into(), w, cx);
            assert_eq!(shell.worktree_focus[project].path, other.path);
            assert_eq!(shell.workspace.active, Some(main_session));
        })
        .unwrap();
}
