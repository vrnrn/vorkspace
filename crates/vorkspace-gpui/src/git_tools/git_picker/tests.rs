use super::*;
use gpui::TestAppContext;
use std::{cell::RefCell, rc::Rc};

#[gpui::test]
fn clicking_base_choices_keeps_the_exact_remote_or_local_ref(cx: &mut TestAppContext) {
    let cwd = "/nonexistent/vorkspace-base-picker-test";
    vorkspace_model::git_cache::remember(
        cwd,
        &Snapshot {
            branches: vec![
                git::Branch {
                    name: "origin/main".into(),
                    remote: None,
                },
                git::Branch {
                    name: "main".into(),
                    remote: Some("origin".into()),
                },
            ],
            ..Default::default()
        },
    );
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| GitPicker::new(cwd.into(), Mode::Base, None, cx))
        })
        .unwrap()
    });
    let picker = window.update(cx, |_, _, cx| cx.entity()).unwrap();
    let picked = Rc::new(RefCell::new(Vec::new()));
    let events = picked.clone();
    let _subscription = cx.update(|cx| {
        cx.subscribe(&picker, move |_, event: &Picked, _| {
            if let Picked::Base(base) = event {
                events.borrow_mut().push(base.clone());
            }
        })
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    for selector in ["git-option-0", "git-option-1"] {
        let bounds = visual.debug_bounds(selector).unwrap();
        visual.simulate_click(bounds.center(), Default::default());
        visual.run_until_parked();
    }
    assert_eq!(
        *picked.borrow(),
        ["refs/heads/origin/main", "refs/remotes/origin/main"]
    );
}

#[gpui::test]
fn clicking_a_remote_tracking_row_checks_out_the_existing_local_branch(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.name", "Test"]);
    git(&["config", "user.email", "test@example.invalid"]);
    std::fs::write(cwd.join("file"), "initial").unwrap();
    git(&["add", "file"]);
    git(&["-c", "commit.gpgsign=false", "commit", "-m", "initial"]);
    git(&["remote", "add", "origin", "/unused"]);
    git(&["remote", "add", "other", "/unused"]);
    git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(&["update-ref", "refs/remotes/other/main", "HEAD"]);
    git(&["branch", "--set-upstream-to=origin/main", "main"]);
    git(&["switch", "-c", "feature"]);
    let mut state = git::snapshot(cwd).unwrap();
    state.branches.sort_by_key(|branch| branch.remote.is_some());
    vorkspace_model::git_cache::remember(&cwd.to_string_lossy(), &state);
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| GitPicker::new(cwd.to_owned(), Mode::Branch, None, cx))
        })
        .unwrap()
    });
    let picker = window.update(cx, |_, _, cx| cx.entity()).unwrap();
    let changed = Rc::new(RefCell::new(false));
    let event_changed = changed.clone();
    let _subscription = cx.update(|cx| {
        cx.subscribe(&picker, move |_, event: &Picked, _| {
            if matches!(event, Picked::Changed) {
                *event_changed.borrow_mut() = true;
            }
        })
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    // The snapshot refresh can reorder rows, so select from the rendered state.
    visual.run_until_parked();
    for remote in ["other", "origin"] {
        let ix = window
                .update(visual, |view, _, cx| {
                    view.choices(cx)
                        .iter()
                        .position(|choice| matches!(choice, Choice::Branch(b) if b.remote.as_deref() == Some(remote) && b.name == "main"))
                        .unwrap()
                })
                .unwrap();
        let selector = [
            "git-option-0",
            "git-option-1",
            "git-option-2",
            "git-option-3",
        ][ix];
        let bounds = visual.debug_bounds(selector).unwrap();
        visual.simulate_click(bounds.center(), Default::default());
        visual.run_until_parked();
        if remote == "other" {
            assert!(visual.debug_bounds("git-picker-error").is_some());
            window
                .update(visual, |view, _, _| {
                    assert!(
                        view.error
                            .as_deref()
                            .is_some_and(|error| error.contains("does not track"))
                    );
                })
                .unwrap();
            assert!(!*changed.borrow());
            assert_eq!(
                git::snapshot(cwd).unwrap().current.as_deref(),
                Some("feature")
            );
        }
    }
    window
        .update(visual, |view, _, _| {
            assert!(view.error.is_none(), "{:?}", view.error)
        })
        .unwrap();
    assert!(*changed.borrow());
    assert_eq!(git::snapshot(cwd).unwrap().current.as_deref(), Some("main"));
}

#[gpui::test]
fn all_picker_modes_start_with_cached_choices_and_keep_them_after_refresh_failure(
    cx: &mut TestAppContext,
) {
    let cwd = "/nonexistent/vorkspace-cached-git-picker";
    vorkspace_model::git_cache::remember(
        cwd,
        &Snapshot {
            current: Some("main".into()),
            branches: vec![
                git::Branch {
                    name: "main".into(),
                    remote: None,
                },
                git::Branch {
                    name: "feature".into(),
                    remote: None,
                },
            ],
            worktrees: vec![git::Worktree {
                path: cwd.into(),
                branch: Some("main".into()),
                main: true,
                ..Default::default()
            }],
            ..Default::default()
        },
    );
    let pickers: Vec<_> = [Mode::Branch, Mode::Base, Mode::Worktree]
        .into_iter()
        .map(|mode| {
            let picker = cx.new(|cx| GitPicker::new(cwd.into(), mode, None, cx));
            picker.update(cx, |view, cx| {
                assert!(
                    view.state.is_some(),
                    "choices are available before any background work"
                );
                assert_eq!(
                    view.choices(cx).len(),
                    if mode == Mode::Worktree { 1 } else { 2 }
                );
            });
            picker
        })
        .collect();
    cx.run_until_parked();
    for picker in pickers {
        picker.update(cx, |view, cx| {
            assert!(view.error.is_some());
            assert!(
                !view.choices(cx).is_empty(),
                "a failed refresh retains usable choices"
            );
        });
    }
    assert!(vorkspace_model::git_cache::get(cwd).is_some());
}
