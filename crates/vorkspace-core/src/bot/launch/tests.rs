use super::*;
use serde_json::json;
fn settings() -> Settings {
    Settings {
        access: AccessMode::ReadOnly,
        effort: Some("low".into()),
        worktree_cwd: Some("/other-project".into()),
        operator: true,
        ..Default::default()
    }
}
#[test]
fn explicit_launch_settings_are_preserved_and_ephemeral_capabilities_are_not_inherited() {
    let launch=Launch::parse(&json!({"prompt":"hello","runtimeMode":"supervised","modelSettings":{"effort":"high","custom":"kept"},"workspaceMode":"worktree"}),"codex","/old","/project",settings(),true).unwrap();
    assert_eq!(launch.settings.access, AccessMode::Supervised);
    assert_eq!(launch.settings.effort.as_deref(), Some("high"));
    assert_eq!(launch.settings.model_settings["custom"], "kept");
    assert!(!launch.settings.operator);
    assert!(launch.settings.worktree_cwd.is_none());
    assert!(matches!(launch.workspace, Workspace::New(_)));
    let inherited = Launch::parse(
        &json!({"prompt":"x"}),
        "codex",
        "/project",
        "/project",
        settings(),
        false,
    )
    .unwrap();
    assert_eq!(inherited.settings.access, AccessMode::ReadOnly);
}
#[test]
fn malformed_or_ambiguous_settings_are_rejected_before_any_creation() {
    for input in [
        json!({"runtimeMode":"unsafe"}),
        json!({"workspaceMode":"unsafe"}),
        json!({"modelSettings":[]}),
        json!({"modelSettings":{"effort":4}}),
        json!({"workspaceMode":"worktree","worktreeCwd":"/outside"}),
        json!({"worktreeBase":"HEAD"}),
        json!({"placement":"wrong"}),
        json!({"besideSessionId":4}),
    ] {
        let mut input = input;
        input["prompt"] = json!("hello");
        assert!(
            Launch::parse(
                &input,
                "codex",
                "/project",
                "/project",
                Settings::default(),
                false
            )
            .is_err(),
            "{input}"
        );
    }
}
#[test]
fn outside_worktree_and_invalid_base_fail_without_allocating_sessions_or_directories() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let git = |args: &[&str]| {
        assert!(std::process::Command::new("git")
            .current_dir(&project)
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    };
    git(&["init", "-q"]);
    git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "commit",
        "--allow-empty",
        "-qm",
        "initial",
    ]);
    for input in [
        json!({"prompt":"x","worktreeCwd":directory.path().to_string_lossy()}),
        json!({"prompt":"x","workspaceMode":"worktree","worktreeBase":"missing-ref"}),
    ] {
        let mut launch = Launch::parse(
            &input,
            "codex",
            project.to_str().unwrap(),
            project.to_str().unwrap(),
            Settings::default(),
            false,
        )
        .unwrap();
        assert!(launch
            .validate_workspace(project.to_str().unwrap())
            .is_err());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
#[test]
fn failed_configuration_removes_its_allocated_session() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("db");
    let mut store = crate::store::Store::open_vorkspace(&path).unwrap();
    // Session creation succeeds; title publication fails after allocation.
    rusqlite::Connection::open(&path).unwrap().execute_batch("CREATE TRIGGER reject_title BEFORE UPDATE OF title ON sessions BEGIN SELECT RAISE(ABORT,'title failure'); END").unwrap();
    let mut launch = Launch::parse(
        &json!({"prompt":"hello"}),
        "codex",
        "/project",
        "/project",
        Settings::default(),
        false,
    )
    .unwrap();
    assert!(launch
        .create(&mut store, "/project")
        .unwrap_err()
        .contains("title failure"));
    assert!(store.sessions().unwrap().is_empty());
}
#[test]
fn started_launch_persists_parent_link_in_both_schemas() {
    for shared in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.db");
        let open = || {
            if shared {
                crate::store::Store::open_vorkspace(&path)
            } else {
                crate::store::Store::open(&path)
            }
        };
        let mut store = open().unwrap();
        let caller = store.create("codex", "/project").unwrap().id;
        let mut launch = Launch::parse(
            &json!({"prompt":"Child request"}),
            "codex",
            "/project",
            "/project",
            Settings::default(),
            true,
        )
        .unwrap();
        let summary = launch
            .create_started(&mut store, "/project", caller)
            .unwrap();
        assert_eq!(
            store.started_links().unwrap().get(&summary.id),
            Some(&caller)
        );
        drop(store);
        let reopened = open().unwrap();
        assert_eq!(
            reopened.started_links().unwrap().get(&summary.id),
            Some(&caller)
        );
        assert_eq!(reopened.sessions().unwrap().len(), 2);
    }
}
#[test]
fn failed_configuration_keeps_worktree_if_session_deletion_fails() {
    for shared in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        std::fs::create_dir(&project).unwrap();
        for args in [
            vec!["init", "-q"],
            vec![
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "initial",
            ],
        ] {
            assert!(std::process::Command::new("git")
                .current_dir(&project)
                .args(args)
                .output()
                .unwrap()
                .status
                .success());
        }
        let path = root.path().join("state.db");
        let mut store = if shared {
            crate::store::Store::open_vorkspace(&path)
        } else {
            crate::store::Store::open(&path)
        }
        .unwrap();
        let table = if shared {
            "sessions"
        } else {
            "native_sessions"
        };
        rusqlite::Connection::open(&path).unwrap().execute_batch(&format!("CREATE TRIGGER reject_title BEFORE UPDATE OF title ON {table} BEGIN SELECT RAISE(ABORT,'injected title failure'); END;
                CREATE TRIGGER reject_cleanup BEFORE DELETE ON {table} BEGIN SELECT RAISE(ABORT,'injected cleanup failure'); END;")).unwrap();
        let project_text = project.to_str().unwrap();
        let mut launch = Launch::parse(
            &json!({"prompt":"Child request","workspaceMode":"worktree"}),
            "codex",
            project_text,
            project_text,
            Settings::default(),
            true,
        )
        .unwrap();
        let error = launch
            .create_started(&mut store, project_text, 1)
            .unwrap_err();
        assert!(error.contains("injected title failure"), "{error}");
        assert!(error.contains("injected cleanup failure"), "{error}");
        assert!(error.contains("preserved"), "{error}");
        let tree = launch.settings.worktree_cwd.as_ref().unwrap();
        assert!(error.contains(tree), "{error}");
        assert!(Path::new(tree).is_dir());
        assert_eq!(store.sessions().unwrap().len(), 1);
        assert_eq!(crate::git::snapshot(&project).unwrap().worktrees.len(), 2);
    }
}
#[test]
fn new_worktree_is_rolled_back_after_a_later_database_failure() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    for args in [
        vec!["init", "-q"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "initial",
        ],
    ] {
        assert!(std::process::Command::new("git")
            .current_dir(&project)
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    }
    let path = root.path().join("db");
    let mut store = crate::store::Store::open_vorkspace(&path).unwrap();
    rusqlite::Connection::open(&path).unwrap().execute_batch("CREATE TRIGGER reject_title BEFORE UPDATE OF title ON sessions BEGIN SELECT RAISE(ABORT,'title failure'); END").unwrap();
    let mut launch = Launch::parse(
        &json!({"prompt":"hello","workspaceMode":"worktree"}),
        "codex",
        project.to_str().unwrap(),
        project.to_str().unwrap(),
        Settings::default(),
        false,
    )
    .unwrap();
    let error = launch
        .create(&mut store, project.to_str().unwrap())
        .unwrap_err();
    assert!(error.contains("title failure"), "{error}");
    assert!(!error.contains("preserved"), "{error}");
    assert!(store.sessions().unwrap().is_empty());
    let snapshot = crate::git::snapshot(&project).unwrap();
    assert_eq!(snapshot.worktrees.len(), 1);
    assert!(!snapshot
        .branches
        .iter()
        .any(|branch| branch.name.starts_with("mc/native-")));
}
#[test]
fn rollback_preserves_concurrent_work_in_its_created_checkout() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    for args in [
        vec!["init", "-q"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "initial",
        ],
    ] {
        assert!(std::process::Command::new("git")
            .current_dir(&project)
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    }
    let mut store = crate::store::Store::open_vorkspace(&root.path().join("db")).unwrap();
    let mut launch = Launch::parse(
        &json!({"prompt":"hello","workspaceMode":"worktree"}),
        "codex",
        project.to_str().unwrap(),
        project.to_str().unwrap(),
        Settings::default(),
        false,
    )
    .unwrap();
    launch
        .create(&mut store, project.to_str().unwrap())
        .unwrap();
    let path = Path::new(launch.settings.worktree_cwd.as_deref().unwrap());
    std::fs::write(path.join("user-work.txt"), "preserve me").unwrap();
    let error = launch
        .created_worktree
        .take()
        .unwrap()
        .rollback()
        .unwrap_err();
    assert!(error.contains("preserved"));
    assert_eq!(
        std::fs::read_to_string(path.join("user-work.txt")).unwrap(),
        "preserve me"
    );
    assert_eq!(crate::git::snapshot(&project).unwrap().worktrees.len(), 2);
}
