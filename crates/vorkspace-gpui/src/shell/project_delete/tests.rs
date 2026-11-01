
use super::*;
#[test]
fn git_worktree_check_accepts_plain_folder_and_refuses_nested_project() {
    let dir = tempfile::tempdir().unwrap();
    assert!(check_worktrees(dir.path().to_str().unwrap()).is_ok());
    let output = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let child = dir.path().join("nested");
    std::fs::create_dir(&child).unwrap();
    assert!(
        check_worktrees(child.to_str().unwrap())
            .unwrap_err()
            .contains("inside another")
    );
    assert!(check_worktrees(dir.path().to_str().unwrap()).is_ok());
}

#[test]
fn dropping_delete_lease_releases_admission() {
    let store = std::sync::Arc::new(std::sync::Mutex::new(
        Store::open(std::path::Path::new(":memory:")).unwrap(),
    ));
    locked(&store)
        .unwrap()
        .begin_project_deletion("/project")
        .unwrap();
    let lease = ProjectDeleteLease {
        store: store.clone(),
        path: "/project".into(),
    };
    assert!(locked(&store).unwrap().create("codex", "/project").is_err());
    drop(lease);
    assert!(locked(&store).unwrap().create("codex", "/project").is_ok());
}

#[test]
fn delete_exact_sessions_preserves_folder_and_other_projects() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let file = project.join("important.txt");
    std::fs::write(&file, "keep").unwrap();
    let path = project.to_str().unwrap();
    let mut store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let a = store.create("codex", path).unwrap();
    let other = store.create("codex", "/other").unwrap();
    let result = delete_saved(&mut store, path, &[a.id], |_| Ok(()));
    assert!(result.error.is_none(), "{:?}", result.error);
    assert_eq!(result.deleted, vec![a.id]);
    assert_eq!(std::fs::read_to_string(file).unwrap(), "keep");
    assert_eq!(
        store
            .sessions()
            .unwrap()
            .iter()
            .map(|s| s.id)
            .collect::<Vec<_>>(),
        vec![other.id]
    );
}
#[test]
fn changed_conversation_set_refuses_deletion() {
    let mut store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let a = store.create("codex", "/project").unwrap();
    let b = store.create("codex", "/project").unwrap();
    let result = delete_saved(&mut store, "/project", &[a.id], |_| Ok(()));
    assert!(result.error.unwrap().contains("changed"));
    assert!(result.deleted.is_empty());
    assert_eq!(store.sessions().unwrap().len(), 2);
    assert!(store.sessions().unwrap().iter().any(|s| s.id == b.id));
}
#[test]
fn failure_after_one_delete_reports_exact_partial_result() {
    let mut store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let a = store.create("codex", "/project").unwrap();
    let b = store.create("codex", "/project").unwrap();
    let mut ids = vec![a.id, b.id];
    ids.sort_unstable();
    let result = delete_saved(&mut store, "/project", &ids, |_| Err("test failure".into()));
    assert_eq!(result.deleted, vec![ids[0]]);
    assert!(result.error.unwrap().contains("checkpoint cleanup failed"));
    assert_eq!(
        store
            .sessions()
            .unwrap()
            .iter()
            .map(|s| s.id)
            .collect::<Vec<_>>(),
        vec![ids[1]]
    );
}
#[test]
fn bot_association_blocks_deletion() {
    let mut roster = vorkspace_core::bot::Roster::default();
    let mut bot = vorkspace_core::bot::Bot::default();
    bot.projects.push("/project".into());
    roster.bots.push(bot);
    assert!(check_bots(&roster, "/project", &[]).is_err());
    assert!(check_bots(&roster, "/other", &[]).is_ok());
}
#[test]
fn symlink_alias_to_project_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let alias = dir.path().join("alias");
    std::os::unix::fs::symlink(&project, &alias).unwrap();
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let session = store.create("codex", alias.to_str().unwrap()).unwrap();
    assert!(scoped_sessions(project.to_str().unwrap(), &[session], &HashMap::new()).is_err());
}
#[gpui::test]
fn delete_requires_confirmation_and_cancel_keeps_project(cx: &mut gpui::TestAppContext) {
    let (window, _, _) = crate::shell::tests::window(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.workspace.add_project("/tmp/delete-empty-fixture");
            shell.automation_state.service = Some(std::sync::Arc::new(std::sync::Mutex::new(
                vorkspace_core::automations::Service::open(std::path::Path::new(":memory:"))
                    .unwrap(),
            )));
            shell.confirm_delete_project("/tmp/delete-empty-fixture".into(), w, cx);
        })
        .unwrap();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert!(
                shell
                    .workspace
                    .projects
                    .iter()
                    .any(|p| p == "/tmp/delete-empty-fixture")
            )
        })
        .unwrap();
}
#[gpui::test]
fn confirmed_empty_project_removal_survives_workspace_restore(cx: &mut gpui::TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().to_string_lossy().to_string();
    let (window, _, _) = crate::shell::tests::window(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.workspace.add_project(&project);
            shell.automation_state.service = Some(std::sync::Arc::new(std::sync::Mutex::new(
                vorkspace_core::automations::Service::open(std::path::Path::new(":memory:"))
                    .unwrap(),
            )));
            shell.confirm_delete_project(project.clone(), w, cx);
        })
        .unwrap();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Delete");
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert!(
                !shell.workspace.projects.iter().any(|p| p == &project),
                "{:?}",
                shell.error
            );
            let saved = locked(&shell.store).unwrap().ui_state().unwrap().unwrap();
            let restored = Workspace::restore(Some(&saved), &[], "/work/vorkspace");
            assert!(!restored.projects.iter().any(|p| p == &project));
        })
        .unwrap();
}
#[gpui::test]
fn selected_project_and_open_session_need_release_before_delete(cx: &mut gpui::TestAppContext) {
    let (window, a, _) = crate::shell::tests::window(cx);
    window
        .update(cx, |shell, _, cx| {
            let current = shell.workspace.project.clone();
            // Once released, a selected project can't be deleted out from
            // under the screen; before release that's left to release.
            assert!(
                shell
                    .project_delete_guard(&current, &[], false, cx)
                    .unwrap_err()
                    .contains("Switch")
            );
            assert!(shell.project_delete_guard(&current, &[], true, cx).is_ok());
            shell.workspace.project = "/work/vorkspace".into();
            assert!(
                shell
                    .project_delete_guard("/work/another-project", &[a], false, cx)
                    .is_err()
            );
        })
        .unwrap();
}

#[test]
fn nested_and_worktree_sessions_are_refused() {
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let nested = store.create("codex", "/project/sub").unwrap();
    assert!(scoped_sessions("/project", &[nested], &HashMap::new()).is_err());
    let main = store.create("codex", "/project").unwrap();
    let trees = HashMap::from([(main.id, Some("/separate-worktree".into()))]);
    assert!(scoped_sessions("/project", &[main], &trees).is_err());
    assert!(scoped_sessions("remote://host/project", &[], &HashMap::new()).is_err());
}
