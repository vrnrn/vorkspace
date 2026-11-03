use super::*;

#[test]
fn archived_project_is_not_reopened_by_active_session_or_stale_metadata() {
    let saved = serde_json::json!({
            "projects": ["/work/old", "/work/other"], "project": "/work/old", "active": 42,
            "tabs": [42], "project_archive": {"/work/old": true},
            "desktop_preferences_metadata": {"vorkspace.archivedProjects": "[{\"path\":\"/work/old/\"}]"}
        }).to_string();
    let mut state = Workspace::restore(Some(&saved), &[(42, "/work/old".into())], "/work/old");
    assert_eq!(state.project, "/work/other");
    assert!(!state.projects.contains(&"/work/old".into()));
    assert!(state.tabs.contains(&42));
    state.add_project("/work/old/");
    let saved = serde_json::to_string(&state).unwrap();
    let state = Workspace::restore(Some(&saved), &[(42, "/work/old".into())], "/work/other");
    assert_eq!(state.project_archive.get("/work/old"), Some(&false));
    assert!(
        state
            .projects
            .iter()
            .any(|p| vorkspace_core::files::path_key(p) == "/work/old")
    );
}

#[test]
fn terminal_legacy_restore_preserves_other_tabs_and_upgrades_scope_identity() {
    let raw = r#"{"tabs":[42],"active":42,"project":"/work","title_order":[{"Session":42},{"terminal":{"cwd":"/work"}},{"App":"Notes"},{"terminal":{"cwd":"/work"}},{"terminal":{"id":700,"cwd":"/work"}},{"terminal":{"cwd":"/other"}},"Search",{"Bot":{"id":"bot","project":"/work","worktree":"/work"}}],"scope_focus":{"first":{"terminal":{"cwd":"/work"}},"also-first":{"terminal":{"cwd":"/work"}},"other":{"terminal":{"cwd":"/other"}},"modern":{"terminal":{"id":700,"cwd":"/work"}}}}"#;
    let workspace = Workspace::restore(Some(raw), &[(42, "/work".into())], "/work");
    // Search, app views and Bots are sidebar pages now, never tabs.
    assert_eq!(workspace.title_order.len(), 5);
    assert_eq!(workspace.title_order[0], TitleTab::Session(42));
    assert!(workspace.title_order.iter().all(TitleTab::in_strip));
    assert_eq!(workspace.active, Some(42));
    let terminals: Vec<_> = workspace
        .title_order
        .iter()
        .filter_map(|tab| match tab {
            TitleTab::Terminal { id, cwd } => Some((*id, cwd.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(terminals.len(), 4);
    assert_eq!(
        terminals
            .iter()
            .map(|(id, _)| *id)
            .collect::<HashSet<_>>()
            .len(),
        4
    );
    assert!(terminals.iter().all(|(id, _)| *id != 0));
    assert_eq!(
        terminals.iter().filter(|(_, cwd)| *cwd == "/work").count(),
        3
    );
    assert_eq!(workspace.scope_focus["first"], workspace.title_order[1]);
    assert_eq!(
        workspace.scope_focus["also-first"],
        workspace.title_order[1]
    );
    assert_eq!(workspace.scope_focus["other"], workspace.title_order[4]);
    assert_eq!(workspace.scope_focus["modern"], workspace.title_order[3]);
    assert!(matches!(
        workspace.title_order[3],
        TitleTab::Terminal { id: 700, .. }
    ));
    let saved = serde_json::to_string(&workspace).unwrap();
    let reopened = Workspace::restore(Some(&saved), &[(42, "/work".into())], "/work");
    assert_eq!(reopened.title_order, workspace.title_order);
    assert_eq!(reopened.scope_focus, workspace.scope_focus);
    let TitleTab::Terminal { id, .. } = reopened.new_terminal_tab("/work".into()) else {
        unreachable!()
    };
    assert!(!terminals.iter().any(|(used, _)| *used == id));
    assert!(id > 700, "saved identities stay reserved after closing");
}

#[test]
fn original_layout_is_used_for_fresh_and_previously_experimental_workspaces() {
    assert!(Workspace::default().classic_layout);
    assert!(
        serde_json::from_str::<Workspace>("{}")
            .unwrap()
            .classic_layout
    );
    for classic in [false, true] {
        let saved =
            serde_json::json!({"classic_layout":classic,"new_layout_prompt_dismissed":false});
        let restored = Workspace::restore(Some(&saved.to_string()), &[], "/work");
        assert!(restored.classic_layout);
        let reopened = Workspace::restore(
            Some(&serde_json::to_string(&restored).unwrap()),
            &[],
            "/work",
        );
        assert!(reopened.classic_layout);
    }
}
#[test]
fn sidebar_navigation_normalizes_ids_and_preserves_disabled_slots_and_hidden_state() {
    let saved = serde_json::json!({
        "sidebar_navigation_order": ["notes", "unknown", "github", "notes", "automations"],
        "sidebar_hidden_connectors": ["github"],
        "compact_session_cards": false,
        "sidebar_activity_visible": true,
    });
    let mut state = Workspace::restore(Some(&saved.to_string()), &[], "/work");
    assert_eq!(
        state.sidebar_navigation_order,
        ["notes", "github", "automations", "gitlab", "jira", "todos"]
    );
    use SidebarNavigation::{Automations, Connector as Provider, Notes};
    let available = [
        Provider(Connector::Github),
        Provider(Connector::Jira),
        Notes,
        Automations,
    ];
    assert!(state.reorder_sidebar_navigation(Provider(Connector::Jira), Notes, false, &available));
    assert_eq!(
        state.sidebar_navigation_order,
        ["jira", "notes", "github", "gitlab", "automations", "todos"]
    );
    // GitLab's slot survives its absence; hidden GitHub is still reorderable.
    assert!(state.reorder_sidebar_navigation(
        Provider(Connector::Github),
        Automations,
        true,
        &available
    ));
    assert_eq!(
        state.sidebar_navigation_order,
        ["jira", "notes", "automations", "gitlab", "github", "todos"]
    );
    assert!(!state.reorder_sidebar_navigation(
        Provider(Connector::Gitlab),
        Notes,
        false,
        &available
    ));
    assert!(!state.reorder_sidebar_navigation(Notes, Notes, true, &available));
    let restored = Workspace::restore(Some(&serde_json::to_string(&state).unwrap()), &[], "/work");
    assert_eq!(
        restored.sidebar_navigation_order,
        state.sidebar_navigation_order
    );
    assert_eq!(
        restored.sidebar_hidden_connectors,
        state.sidebar_hidden_connectors
    );
    assert!(!restored.compact_session_cards);
    assert_eq!(SidebarNavigation::normalize(&[]), SidebarNavigation::ALL);
    assert!(state.reorder_sidebar_navigation(
        Provider(Connector::Gitlab),
        Notes,
        false,
        &SidebarNavigation::ALL
    ));
    assert_eq!(
        state.sidebar_navigation_order,
        ["jira", "gitlab", "notes", "automations", "github", "todos"]
    );
}

#[test]
fn deleted_project_stays_hidden_until_explicit_reopen() {
    let mut state = Workspace::default();
    state.add_project("/keep");
    state.project = "/keep".into();
    state.deleted_projects.insert("/gone".into());
    state.add_project("/gone/");
    assert!(!state.projects.contains(&"/gone/".into()));
    let saved = serde_json::to_string(&state).unwrap();
    let mut state = Workspace::restore(Some(&saved), &[], "/keep");
    state.restore_project("/gone/");
    assert!(state.projects.contains(&"/gone/".into()));
    assert!(!state.deleted_projects.contains("/gone"));
}

#[test]
fn malformed_legacy_rail_remains_unmigrated_and_preserved() {
    let source = serde_json::json!({"vorkspace.projectGroups":"not-json", "unrelated":"keep"});
    let serialized = serde_json::json!({"desktop_preferences_metadata":source}).to_string();
    let mut state = Workspace::restore(Some(&serialized), &[], "/work");
    assert!(state.project_rail_state.is_none());
    assert!(state.migrate_project_rail().is_err());
    assert_eq!(state.desktop_preferences_metadata.unwrap(), source);
}

#[test]
fn delayed_legacy_rail_merges_unrelated_data_and_keeps_native_overrides() {
    let mut state = Workspace::default();
    let rail = state.project_rail_mut();
    rail.groups.push(ProjectRailGroup {
        id: "native".into(),
        name: "Native".into(),
        collapsed: false,
    });
    rail.assignments.insert("/native".into(), "native".into());
    rail.assignment_overrides
        .insert("/native".into(), Some("native".into()));
    rail.assignment_overrides.insert("/removed".into(), None);
    rail.pin_overrides.insert("/removed".into(), false);
    rail.pin_overrides.insert("/native".into(), true);
    rail.pinned.insert("/native".into());
    let saved = serde_json::to_string(&state).unwrap();
    let mut state = Workspace::restore(Some(&saved), &[], "/native");
    state.desktop_preferences_metadata = Some(serde_json::json!({
        "vorkspace.projectGroups": "[{\"id\":\"legacy\",\"name\":\"Legacy\"}]",
        "vorkspace.projectGroupAssignments": "{\"/legacy\":\"legacy\",\"/removed\":\"legacy\"}",
        "vorkspace.projectRailPinned": "[\"/legacy\",\"/removed\"]"
    }));
    state.migrate_project_rail().unwrap();
    let rail = state.project_rail_state.unwrap();
    assert_eq!(
        rail.groups
            .iter()
            .map(|g| g.id.as_str())
            .collect::<Vec<_>>(),
        vec!["legacy", "native"]
    );
    assert_eq!(
        rail.assignments.get("/legacy").map(String::as_str),
        Some("legacy")
    );
    assert_eq!(
        rail.assignments.get("/native").map(String::as_str),
        Some("native")
    );
    assert!(!rail.assignments.contains_key("/removed"));
    assert!(rail.pinned.contains("/legacy") && rail.pinned.contains("/native"));
    assert!(!rail.pinned.contains("/removed"));
}

#[test]
fn project_rail_migration_and_native_edits_survive_stale_source_restart() {
    let legacy = serde_json::json!({
        "vorkspace.projectGroups": "[{\"id\":\"work\",\"name\":\"Work\",\"collapsed\":true}]",
        "vorkspace.projectGroupAssignments": "{\"/work/\":\"work\"}",
        "vorkspace.projectRailPinned": "[\"/work/\"]",
        "unrelated": "preserved"
    });
    let saved = serde_json::json!({"desktop_preferences_metadata": legacy}).to_string();
    let mut state = Workspace::restore(Some(&saved), &[], "/work");
    let rail = state.project_rail_mut();
    assert_eq!(rail.groups[0].name, "Work");
    assert_eq!(
        rail.assignments.get("/work").map(String::as_str),
        Some("work")
    );
    assert!(rail.pinned.remove("/work"));
    rail.assignments.remove("/work");
    rail.groups[0].name = "Renamed".into();
    state.project_group_collapsed.insert("work".into(), false);
    let serialized = serde_json::to_string(&state).unwrap();
    let reopened = Workspace::restore(Some(&serialized), &[], "/work");
    let rail = reopened.project_rail_state.unwrap();
    assert!(rail.pinned.is_empty());
    assert!(rail.assignments.is_empty());
    assert_eq!(rail.groups[0].name, "Renamed");
    assert_eq!(reopened.project_group_collapsed.get("work"), Some(&false));
    assert_eq!(
        reopened.desktop_preferences_metadata.unwrap()["unrelated"],
        "preserved"
    );
}

#[test]
fn sidebar_tools_keep_per_project_selection_and_normalize_main_tab_order() {
    assert_eq!(
        SidebarTab::normalize(&[SidebarTab::Changes, SidebarTab::Graph, SidebarTab::Changes]),
        [
            SidebarTab::Changes,
            SidebarTab::Sessions,
            SidebarTab::Files,
            SidebarTab::Todos
        ]
    );
    let saved = serde_json::json!({
        "project":"/work",
        "project_rail_open":false,
        "project_rail_width":900.,
        "sidebar_tabs":{"/work":"graph","/other":"files"},
        "sidebar_tab_order":["changes","graph","changes"]
    });
    let mut state = Workspace::restore(Some(&saved.to_string()), &[], "/work");
    assert_eq!(state.sidebar_tab(), SidebarTab::Graph);
    assert!(!state.project_rail_open);
    assert_eq!(state.project_rail_width, 360.);
    assert_eq!(
        state.sidebar_tab_order,
        [
            SidebarTab::Changes,
            SidebarTab::Sessions,
            SidebarTab::Files,
            SidebarTab::Todos
        ]
    );
    state.project = "/other".into();
    assert_eq!(state.sidebar_tab(), SidebarTab::Files);
    state
        .sidebar_tabs
        .insert("/other".into(), SidebarTab::Sessions);
    state.project_rail_width = 225.;
    let reopened = Workspace::restore(Some(&serde_json::to_string(&state).unwrap()), &[], "/other");
    assert_eq!(reopened.sidebar_tab(), SidebarTab::Sessions);
    assert_eq!(reopened.project_rail_width, 225.);
    state.project = "/new".into();
    assert_eq!(state.sidebar_tab(), SidebarTab::Changes);
}
#[test]
fn earlier_native_save_recovers_original_sidebar_from_retained_desktop_metadata() {
    let saved = serde_json::json!({
        "project":"/work",
        "preferences":{"sounds":false},
        "desktop_preferences_metadata":{
            "vorkspace.projectRailOpen":"0",
            "vorkspace.projectRailWidth":"246",
            "vorkspace.projectSidebarTabs.v1":r#"{"/work":"files"}"#,
            "vorkspace.sidebarTabOrder":r#"["changes","inbox","sessions"]"#
        }
    });
    let state = Workspace::restore(Some(&saved.to_string()), &[], "/work");
    assert!(!state.project_rail_open);
    assert_eq!(state.project_rail_width, 246.);
    assert_eq!(state.sidebar_tab(), SidebarTab::Files);
    assert!(!state.preferences.sounds);
    assert_eq!(
        serde_json::to_value(state.desktop_preferences_metadata).unwrap(),
        saved["desktop_preferences_metadata"]
    );
    let malformed = serde_json::json!({
        "project":"/work",
        "project_rail_open":"invalid",
        "project_rail_width":-20.,
        "sidebar_tabs":{"/work":"files","/bad":7},
        "sidebar_tab_order":["changes",null,"graph"]
    });
    let restored = Workspace::restore(Some(&malformed.to_string()), &[], "/work");
    assert_eq!(restored.project, "/work");
    assert!(restored.project_rail_open);
    assert_eq!(restored.project_rail_width, 180.);
    assert_eq!(restored.sidebar_tab(), SidebarTab::Files);
}
#[test]
fn navigation_prunes_closed_tabs_and_a_new_visit_discards_forward() {
    let mut h = VisitHistory::default();
    h.record(1);
    h.record(2);
    h.record(3);
    assert_eq!(h.step(-1), Some(2));
    h.prune(&[1, 3], Some(3));
    assert_eq!(h.step(-1), Some(1));
    assert_eq!(h.step(1), Some(3));
    h.step(-1);
    h.record(4);
    assert!(h.forward.is_empty());
    for id in 5..100 {
        h.record(id);
    }
    assert_eq!(h.back.len(), 50);
}
#[test]
fn close_and_reopen_keep_session_identity() {
    let mut s = Workspace::default();
    s.open(1);
    s.open(2);
    s.open(3);
    s.open(2);
    assert_eq!(s.tabs, vec![1, 2, 3]);
    s.close(1);
    assert_eq!(s.active, Some(2));
    s.close(2);
    assert_eq!(s.active, Some(3));
    s.open(2);
    s.open(3);
    assert_eq!(s.active, Some(3));
    s.close(3);
    s.close(2);
    assert_eq!(s.active, None);
    s.open(1);
    assert_eq!(s.tabs, vec![1]);
}
#[test]
fn split_tabs_place_detach_and_close_panes_without_losing_sessions() {
    let mut s = Workspace::default();
    s.open(1);
    s.open(2);
    s.open(1);
    s.split(3, SplitDir::Right);
    s.tab_workspaces.insert(1, "/selected".into());
    s.tab_workspaces.insert(2, "/other".into());
    assert_eq!(s.tabs, [1, 2]);
    assert_eq!(s.layout(1).leaf_ids(), [1, 3]);
    assert_eq!((s.active, s.active_tab()), (Some(3), Some(1)));
    // Another tab's session joins this one; its old tab closes.
    s.place(2, 1, Edge::Bottom);
    assert_eq!(s.tabs, [1]);
    assert!(!s.tab_workspaces.contains_key(&2));
    assert_eq!(s.layout(1).leaf_ids(), [1, 2, 3]);
    assert_eq!(s.open_sessions(), [1, 2, 3]);
    // Closing the key pane rekeys the tab and keeps its place.
    s.open(4);
    s.open(1);
    assert!(s.close_pane(1));
    assert_eq!(s.tabs, [2, 4]);
    assert_eq!(s.tab_workspaces[&2], "/selected");
    assert!(!s.tab_workspaces.contains_key(&1));
    assert_eq!(s.active, Some(2));
    assert_eq!(s.tab_focus(2), 2);
    s.open(3);
    s.open(s.tab_focus(4));
    assert_eq!(s.active, Some(4));
    s.open(s.tab_focus(2));
    assert_eq!(s.active, Some(3));
    // A pane dragged to the strip becomes a tab where it was dropped.
    s.detach(3, 0);
    assert_eq!(s.tabs, [3, 2, 4]);
    assert_eq!(s.tab_workspaces[&3], "/selected");
    assert!(!s.layouts.contains_key(&2));
    assert!(!s.close_pane(3));
    // A whole tab dropped on a pane moves in intact.
    s.split(5, SplitDir::Down);
    s.place_tab(3, 4, Edge::Left);
    assert_eq!(s.tabs, [2, 4]);
    assert!(!s.tab_workspaces.contains_key(&3));
    assert_eq!(s.layout(4).leaf_ids(), [3, 5, 4]);
    assert_eq!(s.active, Some(5));
    s.close(5);
    assert_eq!(s.tabs, [2]);
    assert_eq!(s.tab_workspaces.len(), 1);
    assert_eq!(s.active, Some(2));
}
#[test]
fn restore_keeps_split_tabs_and_prunes_their_missing_panes() {
    let mut s = Workspace::default();
    s.open(1);
    s.split(2, SplitDir::Right);
    s.split(3, SplitDir::Down);
    s.open(9);
    s.open(2);
    let json = serde_json::to_string(&s).unwrap();
    let sessions: Vec<_> = [2, 3].iter().map(|id| (*id, "/w".into())).collect();
    let s = Workspace::restore(Some(&json), &sessions, "/w");
    assert_eq!(s.tabs, [2]);
    assert_eq!(s.layout(2).leaf_ids(), [2, 3]);
    assert_eq!(s.active, Some(2));
    assert_eq!(s.tab_focus(2), 2);
}
#[test]
fn terminal_docks_add_close_move_and_clamp_like_project_terminal() {
    let mut dock = TerminalDock::default();
    assert_eq!((dock.side, dock.size), (DockSide::Bottom, 220.));
    dock.add("/w/a".into());
    dock.add("/w/b".into());
    dock.add("/w/c".into());
    assert_eq!(dock.active, 2);
    dock.active = 1;
    assert!(dock.close(1));
    assert_eq!((dock.terminals.len(), dock.active), (2, 1));
    assert!(dock.close(0));
    assert_eq!(dock.active, 0);
    assert!(!dock.close(0));
    dock.set_side(DockSide::Right);
    assert_eq!(dock.size, 360.);
    dock.size = 500.;
    dock.set_side(DockSide::Left);
    assert_eq!(dock.size, 500.);
    assert_eq!(DockSide::Bottom.clamp(10., (1280., 800.)), 88.);
    assert_eq!(DockSide::Bottom.clamp(900., (1280., 800.)), 560.);
    assert_eq!(DockSide::Left.clamp(f32::NAN, (1280., 800.)), 360.);
    let mut s = Workspace::default();
    s.docks.insert("/w".into(), dock.clone());
    let mut kept = dock.clone();
    kept.add("/w".into());
    s.docks.insert("/k".into(), kept);
    let json = serde_json::to_string(&s).unwrap();
    let s = Workspace::restore(Some(&json), &[], "/w");
    assert!(!s.docks.contains_key("/w"));
    assert_eq!(s.docks["/k"].terminals, ["/w"]);
}
#[test]
fn terminal_docks_reorder_keeping_the_active_terminal_and_close_others() {
    let mut dock = TerminalDock::default();
    for cwd in ["/a", "/b", "/c", "/d"] {
        dock.add(cwd.into());
    }
    dock.active = 1;
    dock.reorder(1, 3);
    assert_eq!(dock.terminals, ["/a", "/c", "/d", "/b"]);
    assert_eq!(dock.active, 3);
    dock.reorder(0, 2);
    assert_eq!(dock.terminals, ["/c", "/d", "/a", "/b"]);
    assert_eq!(dock.active, 3);
    dock.reorder(3, 0);
    assert_eq!(dock.terminals, ["/b", "/c", "/d", "/a"]);
    assert_eq!(dock.active, 0);
    dock.active = 2;
    dock.reorder(3, 1);
    assert_eq!(dock.terminals, ["/b", "/a", "/c", "/d"]);
    assert_eq!(dock.active, 3);
    dock.keep_only(2);
    assert_eq!(
        (dock.terminals.clone(), dock.active),
        (vec!["/c".to_string()], 0)
    );
}
#[test]
fn restore_keeps_project_selection_and_prunes_stale_project_focus() {
    let sessions = vec![(3, "/b".into()), (2, "/a".into()), (1, "/a".into())];
    let mut state = Workspace::default();
    for id in [1, 2, 3] {
        state.open(id);
    }
    state.project = "/a".into();
    state.active = Some(99);
    state.project_focus.insert("/a".into(), 1);
    state.project_focus.insert("/b".into(), 99);
    state.worktree_focus.insert(
        "/a".into(),
        crate::git_tools::worktree_switcher::WorktreeFocus {
            path: "/a-tree".into(),
            branch: Some("feature".into()),
        },
    );
    state.tab_workspaces.insert(1, "/a-tree".into());
    let mut saved = serde_json::to_value(&state).unwrap();
    assert!(saved.get("worktree_focus").is_none());
    assert!(saved.get("tab_workspaces").is_none());
    // Older native sidecars saved this transient filter. Ignore it too.
    saved["worktree_focus"] = serde_json::json!({
        "/a": {"path":"/a-tree", "branch":"feature"}
    });
    let json = serde_json::to_string(&saved).unwrap();
    let restored = Workspace::restore(Some(&json), &sessions, "/b");
    assert_eq!(restored.active, Some(1));
    assert_eq!(restored.project, "/a");
    assert!(restored.worktree_focus.is_empty());
    assert!(restored.tab_workspaces.is_empty());
    assert!(!restored.project_focus.contains_key("/b"));
    state.active = Some(3);
    let json = serde_json::to_string(&state).unwrap();
    let restored = Workspace::restore(Some(&json), &sessions, "/a");
    assert_eq!(restored.active, Some(3));
    assert_eq!(restored.project, "/b");
    let fresh = Workspace::restore(None, &sessions, "/a");
    assert_eq!(fresh.active, Some(2));
    assert_eq!(fresh.project, "/a");
}
#[test]
fn restoring_history_keeps_only_remembered_or_selected_projects() {
    let saved = serde_json::json!({
        "project": "/code/app",
        "projects": ["/code/app", "/code/other"],
        "tabs": [2],
        "active": 2,
        "desktop_project_preferences": {"projects": ["/code/app", "/code/other"]}
    })
    .to_string();
    let sessions = vec![
        (1, "/code/app".into()),
        (2, "/tmp/internal-bot-chat".into()),
        (3, "/private/var/folders/temporary".into()),
    ];
    let restored = Workspace::restore(Some(&saved), &sessions, "/code/app");
    assert_eq!(restored.projects, ["/code/app", "/code/other"]);
    assert_eq!(restored.project, "/code/app");
    assert_eq!(restored.active, None);
    let serialized = serde_json::to_value(restored).unwrap();
    assert_eq!(
        serialized["desktop_project_preferences"]["projects"],
        serde_json::json!(["/code/app", "/code/other"])
    );
}
#[test]
fn restore_prunes_stale_tabs_and_preserves_an_intentionally_empty_workspace() {
    let mut s = Workspace::default();
    s.open(1);
    s.open(8);
    s.tabs.push(1);
    s.sidebar_width = 900.;
    let json = serde_json::to_string(&s).unwrap();
    let s = Workspace::restore(Some(&json), &[(1, "/work/a".into())], "/work/b");
    assert_eq!(s.tabs, vec![1]);
    assert_eq!(s.active, None);
    assert_eq!(s.project, "/work/b");
    assert_eq!(s.sidebar_width, 560.);
    assert_eq!(s.projects, vec!["/work/b"]);
    let s = Workspace::restore(Some("{}"), &[(1, "/work/a".into())], "/work/a");
    assert!(s.tabs.is_empty());
    assert!(
        s.compact_session_cards,
        "older workspaces retain compact cards"
    );
    assert!(
        s.sidebar_activity_visible,
        "the retired activity preference remains readable"
    );
    assert!(
        s.sidebar_subscriptions_visible,
        "existing users keep usage visible"
    );
}
#[test]
fn desktop_snapshot_translation_deserializes_and_retains_unported_metadata() {
    let source = serde_json::json!({
        "projectCwd":"/work", "activeTabId":"tab",
        "tabs":[{"id":"tab", "focusedId":"b", "groupId":"assigned", "editorPanes":[], "layout":{"type":"split", "dir":"down", "children":[{"type":"leaf","id":"a"},{"type":"leaf","id":"b"}], "sizes":[0.4,0.6]}}],
        "projectTerminals":[{"projectPath":"/work", "side":"bottom", "size":240, "open":false, "pane":{"activeFileId":"term", "files":[{"id":"term", "terminal":true, "cwd":"/work/sub"}]}}]
    });
    let mapped = vorkspace_core::desktop_preferences::DesktopPreferences::workspace_snapshot(
        &source,
        |id| match id {
            "a" => Some(1),
            "b" => Some(2),
            _ => None,
        },
    );
    let saved = mapped.to_string();
    let restored = Workspace::restore(
        Some(&saved),
        &[(1, "/work".into()), (2, "/work".into())],
        "/work",
    );
    assert_eq!(restored.tabs, [1]);
    assert_eq!(restored.active, Some(2));
    assert!(restored.layouts[&1].is_split());
    assert_eq!(restored.focused[&1], 2);
    assert_eq!(restored.docks["/work"].terminals, ["/work/sub"]);
    assert!(!restored.docks["/work"].open);
    assert_eq!(restored.desktop_workspace_snapshot.as_ref(), Some(&source));
}
