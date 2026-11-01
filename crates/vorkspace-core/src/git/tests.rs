use super::*;
#[test]
fn remote_selection_reuses_only_its_tracking_branch_and_preserves_work() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("project");
    std::fs::create_dir(&cwd).unwrap();
    git(&cwd, &["init", "-b", "main"]).unwrap();
    git(&cwd, &["config", "user.name", "Test"]).unwrap();
    git(&cwd, &["config", "user.email", "test@example.invalid"]).unwrap();
    std::fs::write(cwd.join("file"), "main\n").unwrap();
    git(&cwd, &["add", "file"]).unwrap();
    git(
        &cwd,
        &["-c", "commit.gpgsign=false", "commit", "-m", "initial"],
    )
    .unwrap();
    git(&cwd, &["remote", "add", "origin", "/unused"]).unwrap();
    git(&cwd, &["remote", "add", "other", "/unused"]).unwrap();
    for reference in [
        "refs/remotes/origin/main",
        "refs/remotes/other/main",
        "refs/remotes/other/new",
    ] {
        git(&cwd, &["update-ref", reference, "HEAD"]).unwrap();
    }
    git(&cwd, &["branch", "--set-upstream-to=origin/main", "main"]).unwrap();
    let remote = |name: &str, remote: &str| Branch {
        name: name.into(),
        remote: Some(remote.into()),
    };
    create_branch(&cwd, "feature").unwrap();
    std::fs::write(cwd.join("file"), "feature\n").unwrap();
    git(&cwd, &["add", "file"]).unwrap();
    git(
        &cwd,
        &["-c", "commit.gpgsign=false", "commit", "-m", "feature"],
    )
    .unwrap();
    git(&cwd, &["branch", "other/main"]).unwrap();
    git(&cwd, &["branch", "no-upstream"]).unwrap();
    git(
        &cwd,
        &["update-ref", "refs/remotes/origin/no-upstream", "HEAD"],
    )
    .unwrap();
    assert!(checkout(&cwd, &remote("no-upstream", "origin"))
        .unwrap_err()
        .contains("does not track"));
    git(&cwd, &["branch", "-d", "no-upstream"]).unwrap();
    assert!(
        checkout(
            &cwd,
            &Branch {
                name: "no-upstream".into(),
                remote: None
            }
        )
        .is_err(),
        "a stale local row must not create a tracking branch"
    );
    std::fs::write(cwd.join("file"), "unsaved\n").unwrap();
    assert!(checkout(&cwd, &remote("main", "origin")).is_err());
    assert_eq!(
        std::fs::read_to_string(cwd.join("file")).unwrap(),
        "unsaved\n"
    );
    std::fs::write(cwd.join("file"), "feature\n").unwrap();
    let original_main = git(&cwd, &["rev-parse", "refs/heads/main"]).unwrap();
    checkout(&cwd, &remote("main", "origin")).unwrap();
    assert_eq!(
        git(&cwd, &["rev-parse", "refs/heads/main"]).unwrap(),
        original_main
    );
    assert!(checkout(&cwd, &remote("main", "other"))
        .unwrap_err()
        .contains("does not track"));
    assert!(checkout(&cwd, &remote("main", "missing"))
        .unwrap_err()
        .contains("available remote"));
    checkout(&cwd, &remote("new", "other")).unwrap();
    assert_eq!(
        git(&cwd, &["rev-parse", "--abbrev-ref", "@{upstream}"])
            .unwrap()
            .trim(),
        "other/new"
    );
    let occupied = temp.path().join("occupied");
    git(
        &cwd,
        &["worktree", "add", "--", occupied.to_str().unwrap(), "main"],
    )
    .unwrap();
    let error = checkout(&cwd, &remote("main", "origin")).unwrap_err();
    assert!(
        error.contains("already") && error.contains("worktree"),
        "{error}"
    );
    assert_eq!(snapshot(&cwd).unwrap().current.as_deref(), Some("new"));
    assert_eq!(
        snapshot(&occupied).unwrap().current.as_deref(),
        Some("main")
    );
    assert_eq!(
        git(&cwd, &["rev-parse", "refs/heads/main"]).unwrap(),
        original_main
    );
    let tree = create_worktree(&cwd, "refs/remotes/other/main", 8).unwrap();
    assert_eq!(git(&tree, &["rev-parse", "HEAD"]).unwrap(), original_main);
    assert_ne!(snapshot(&tree).unwrap().current.as_deref(), Some("main"));
}
#[test]
fn sidebar_named_copies_preserve_dirty_files_and_refuse_collisions() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("project");
    std::fs::create_dir(&cwd).unwrap();
    git(&cwd, &["init", "-b", "main"]).unwrap();
    git(&cwd, &["config", "core.autocrlf", "false"]).unwrap();
    std::fs::write(cwd.join("file"), "committed\n").unwrap();
    git(&cwd, &["add", "file"]).unwrap();
    git(
        &cwd,
        &[
            "-c",
            "user.name=Native test",
            "-c",
            "user.email=native@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            "Initial",
        ],
    )
    .unwrap();
    std::fs::write(cwd.join("file"), "unsaved checkout changes\n").unwrap();
    let tree = create_named_worktree(&cwd, "feature/sidebar").unwrap();
    assert_eq!(tree.branch.as_deref(), Some("feature/sidebar"));
    assert_eq!(
        Path::new(&tree.path).file_name().unwrap(),
        "feature-sidebar"
    );
    assert_eq!(
        std::fs::read_to_string(cwd.join("file")).unwrap(),
        "unsaved checkout changes\n"
    );
    assert_eq!(
        std::fs::read_to_string(Path::new(&tree.path).join("file")).unwrap(),
        "committed\n"
    );
    assert!(create_named_worktree(&cwd, "feature/sidebar")
        .unwrap_err()
        .contains("already has a working copy"));
    assert!(create_named_worktree(&cwd, "-invalid").is_err());
    let collision = temp.path().join("project-worktrees/collision");
    std::fs::create_dir(&collision).unwrap();
    std::fs::write(collision.join("keep"), "keep").unwrap();
    assert!(create_named_worktree(&cwd, "collision")
        .unwrap_err()
        .contains("already exists"));
    assert_eq!(
        std::fs::read_to_string(collision.join("keep")).unwrap(),
        "keep"
    );
}
#[test]
fn working_copy_operations_preserve_dirty_files_and_track_remotes() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("project");
    std::fs::create_dir(&cwd).unwrap();
    git(&cwd, &["init", "-b", "main"]).unwrap();
    git(&cwd, &["config", "core.autocrlf", "false"]).unwrap();
    git(&cwd, &["config", "user.name", "Native review"]).unwrap();
    git(&cwd, &["config", "user.email", "native@example.invalid"]).unwrap();
    std::fs::write(cwd.join("file"), "main\n").unwrap();
    git(&cwd, &["add", "file"]).unwrap();
    git(
        &cwd,
        &["-c", "commit.gpgsign=false", "commit", "-m", "Initial"],
    )
    .unwrap();
    create_branch(&cwd, "feature").unwrap();
    std::fs::write(cwd.join("file"), "feature\n").unwrap();
    git(&cwd, &["add", "file"]).unwrap();
    git(
        &cwd,
        &["-c", "commit.gpgsign=false", "commit", "-m", "Feature"],
    )
    .unwrap();
    checkout(
        &cwd,
        &Branch {
            name: "main".into(),
            remote: None,
        },
    )
    .unwrap();
    std::fs::write(cwd.join("file"), "my unsaved work\n").unwrap();
    assert!(checkout(
        &cwd,
        &Branch {
            name: "feature".into(),
            remote: None
        }
    )
    .is_err());
    assert_eq!(
        std::fs::read_to_string(cwd.join("file")).unwrap(),
        "my unsaved work\n"
    );
    assert_eq!(snapshot(&cwd).unwrap().current.as_deref(), Some("main"));
    let tree = create_worktree(&cwd, "feature", 7).unwrap();
    let state = snapshot(&tree).unwrap();
    assert!(state.linked);
    assert_eq!(state.worktrees.len(), 2);
    assert_eq!(
        std::fs::read_to_string(tree.join("file")).unwrap(),
        "feature\n"
    );
    git(&cwd, &["remote", "add", "origin", "/unused"]).unwrap();
    git(
        &cwd,
        &["update-ref", "refs/remotes/origin/remote-feature", "HEAD"],
    )
    .unwrap();
    checkout(
        &cwd,
        &Branch {
            name: "remote-feature".into(),
            remote: Some("origin".into()),
        },
    )
    .unwrap();
    assert_eq!(
        snapshot(&cwd).unwrap().current.as_deref(),
        Some("remote-feature")
    );
    assert_eq!(
        git(&cwd, &["rev-parse", "--abbrev-ref", "@{upstream}"])
            .unwrap()
            .trim(),
        "origin/remote-feature"
    );
    assert!(create_branch(&cwd, "--orphan").is_err());
}
