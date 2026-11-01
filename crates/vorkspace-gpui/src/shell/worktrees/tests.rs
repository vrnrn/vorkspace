use super::*;
use crate::shell::tests::window;
use gpui::TestAppContext;

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
    let settings = visual.debug_bounds("menu-action-settings").unwrap();
    visual.simulate_click(settings.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, w, cx| {
            let settings = shell.settings.as_ref().expect("Settings is enabled");
            assert!(settings.focus_handle(cx).contains_focused(w, cx));
            assert!(shell.menu.is_none());
            assert_eq!(shell.workspace.active, Some(active));
        })
        .unwrap();
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    window
        .update(visual, |shell, w, cx| {
            assert!(shell.settings.is_none());
            shell.open_menu(
                MenuTarget::Header,
                Anchor::Slot(shell.project_anchor.clone()),
                w,
                cx,
            );
        })
        .unwrap();
    visual.run_until_parked();
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
                assert_eq!(shell.sidebar_session_ids(cx), [feature_session]);
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
                        .contains(&TitleTab::Session(feature_session)),
                    "opening a worktree chat keeps its tab in the current workspace"
                );
                assert!(shell
                    .session_entries(&shell.sidebar_session_ids(cx), cx)
                    .iter()
                    .any(|entry| matches!(entry, vorkspace_core::session_groups::Entry::Folder { id, .. } if id == "saved-folder")),
                    "opening a worktree chat must not hide the project's other folders");
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
                    .contains(&TitleTab::Session(main_session))
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
