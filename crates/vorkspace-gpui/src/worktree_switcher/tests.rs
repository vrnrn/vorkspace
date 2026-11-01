
use super::*;
use gpui::TestAppContext;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};
use vorkspace_core::{git::Worktree, store::Store};
fn snapshot() -> Snapshot {
    Snapshot {
        worktrees: vec![
            Worktree {
                path: "/repo/project".into(),
                branch: Some("main".into()),
                main: true,
                ..Default::default()
            },
            Worktree {
                path: "/repo/project-worktrees/feature".into(),
                branch: Some("feature".into()),
                ..Default::default()
            },
            Worktree {
                path: "/repo/project-worktrees/detached".into(),
                head: "123456789abc".into(),
                ..Default::default()
            },
            Worktree {
                path: "/repo/project-worktrees/gone".into(),
                branch: Some("gone".into()),
                missing: true,
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}
#[test]
fn rows_match_the_react_main_entry_filter_and_detached_names() {
    let state = snapshot();
    let all = rows("/repo/project", Some(&state), "");
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].label, "main");
    assert_eq!(all[0].detail, "Project folder · all sessions");
    assert_eq!(all[2].label, "Detached 1234567");
    assert_eq!(rows("/repo/project", Some(&state), "FEATURE").len(), 1);
    assert_eq!(
        rows(
            "/repo/project",
            Some(&state),
            "/repo/project-worktrees/detached"
        )
        .len(),
        1
    );
    assert!(rows("/repo/project", Some(&state), "new-copy").is_empty());
}
#[gpui::test]
fn cached_worktrees_show_before_metadata_and_membership_never_loads_full_settings(
    cx: &mut TestAppContext,
) {
    let cwd = "/nonexistent/vorkspace-cached-worktree-menu";
    let feature = format!("{cwd}-feature");
    vorkspace_model::git_cache::remember(
        cwd,
        &Snapshot {
            worktrees: vec![
                Worktree {
                    path: cwd.into(),
                    branch: Some("main".into()),
                    main: true,
                    ..Default::default()
                },
                Worktree {
                    path: feature.clone(),
                    branch: Some("feature".into()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
    );
    let store = Store::open(Path::new(":memory:")).unwrap();
    let session = store.create("codex", cwd).unwrap();
    // Membership remains readable even when full turn metadata cannot be
    // deserialized. The old per-session agent_settings call fails here.
    store
        .save_setting(
            &format!("agent:{}", session.id),
            &serde_json::json!({
                "worktree_cwd": feature, "turns": "not a turn map"
            })
            .to_string(),
        )
        .unwrap();
    assert!(store.agent_settings(session.id).is_err());
    let view = cx.new(|cx| {
        WorktreeSwitcher::new(
            cwd.into(),
            None,
            vec![TabSession {
                id: session.id,
                tab: session.id,
                cwd: cwd.into(),
                workspace: None,
                busy: true,
            }],
            vec![session.id],
            Arc::new(Mutex::new(store)),
            cx,
        )
    });
    view.update(cx, |view, _| {
        assert_eq!(rows(cwd, view.state.as_ref(), "").len(), 2);
        assert_eq!(view.tabs[&path_key(cwd)].tabs.len(), 1);
    });
    cx.run_until_parked();
    view.update(cx, |view, _| {
        assert!(
            view.state.is_some(),
            "a Git refresh failure keeps the cached worktrees"
        );
        assert!(
            view.error.is_none(),
            "membership needs no full agent settings"
        );
        assert_eq!(view.tabs[&path_key(&feature)].tabs.len(), 1);
        assert!(view.tabs[&path_key(&feature)].busy);
    });
}
#[test]
fn cleared_membership_moves_cached_tab_counts_back_to_project_folder() {
    let sessions = vec![TabSession {
        id: 1,
        tab: 1,
        cwd: "/repo-feature".into(),
        workspace: None,
        busy: false,
    }];
    assert!(tab_stats(&sessions, None, "/repo").contains_key("/repo-feature"));
    let paths = HashMap::from([(1, None)]);
    let tabs = tab_stats(&sessions, Some(&paths), "/repo");
    assert!(tabs.contains_key("/repo"));
    assert!(!tabs.contains_key("/repo-feature"));
    let mut pinned = sessions;
    pinned[0].workspace = Some("/repo-selected".into());
    for paths in [None, Some(&paths)] {
        let tabs = tab_stats(&pinned, paths, "/repo");
        assert!(tabs.contains_key("/repo-selected"));
        assert!(!tabs.contains_key("/repo-feature"));
        assert!(!tabs.contains_key("/repo"));
    }
}
#[gpui::test]
fn search_keyboard_selection_and_create_footer_match_the_flyout(cx: &mut TestAppContext) {
    cx.update(input::bind);
    let store = Arc::new(Mutex::new(Store::open(Path::new(":memory:")).unwrap()));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                WorktreeSwitcher::new(
                    "/repo/project".into(),
                    Some(WorktreeFocus {
                        path: "/repo/project-worktrees/feature".into(),
                        branch: Some("feature".into()),
                    }),
                    Vec::new(),
                    Vec::new(),
                    store,
                    cx,
                )
            })
        })
        .unwrap()
    });
    let picked = Rc::new(RefCell::new(Vec::new()));
    let target = picked.clone();
    let entity = window
        .update(cx, |view, w, cx| {
            w.focus(&view.focus(cx), cx);
            cx.entity()
        })
        .unwrap();
    let _subscription = cx.update(|cx| {
        cx.subscribe(&entity, move |_, event, cx| {
            if let Event::Selected(focus) = event {
                target.borrow_mut().push(focus.clone());
            }
            let _ = cx;
        })
    });
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            view.state = Some(snapshot());
            view.load_error = None;
            view.error = None;
            cx.notify();
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert_eq!(
        visual
            .debug_bounds("worktree-switcher-search")
            .unwrap()
            .size
            .height,
        px(44.)
    );
    assert_eq!(
        visual.debug_bounds("working-copy-1").unwrap().size.height,
        px(45.)
    );
    visual.simulate_input("Detached");
    visual.run_until_parked();
    assert!(visual.debug_bounds("working-copy-1").is_none());
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert_eq!(
        picked.borrow().as_slice(),
        [Some(WorktreeFocus {
            path: "/repo/project-worktrees/detached".into(),
            branch: None
        })]
    );
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("new-copy");
    visual.run_until_parked();
    assert!(visual.debug_bounds("create-sidebar-worktree").is_some());
}
