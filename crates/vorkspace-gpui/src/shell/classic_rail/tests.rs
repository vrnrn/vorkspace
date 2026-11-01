use super::*;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::json;
use vorkspace_core::store::Store;

#[gpui::test]
fn hidden_project_rows_stop_ticks_and_resample_work_on_reveal(cx: &mut TestAppContext) {
    use std::time::Duration;
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, _, session) = crate::shell::tests::window(cx);
    let path = "/work/vorkspace";
    window
        .update(cx, |shell, _, cx| {
            let rail = shell.workspace.project_rail_mut();
            rail.groups.push(crate::workspace::ProjectRailGroup {
                id: "work".into(),
                name: "Work".into(),
                collapsed: false,
            });
            rail.assignments
                .insert(vorkspace_core::files::path_key(path), "work".into());
            shell.invalidate_chrome(cx);
            cx.notify();
        })
        .unwrap();
    for mode in ["compact", "hidden", "settings", "group"] {
        window
            .update(cx, |shell, _, cx| {
                shell.sessions[&session].update(cx, |view, cx| {
                    view.busy = true;
                    cx.emit(SessionChanged);
                    cx.notify();
                });
            })
            .unwrap();
        cx.run_until_parked();
        let mark = window
            .update(cx, |shell, _, cx| {
                assert_eq!(
                    shell.sidebar_status(session, cx),
                    Some(live_agents::Status::Working)
                );
                shell.project_marks[path].clone()
            })
            .unwrap();
        cx.executor().advance_clock(Duration::from_millis(33));
        cx.run_until_parked();
        assert!(mark.read_with(cx, |mark, _| mark.animation_state().0) > 0);
        // Hide and reveal while still working, then hide again and finish.
        for finish_hidden in [false, true] {
            let mark = window
                .update(cx, |shell, _, _| shell.project_marks[path].clone())
                .unwrap();
            window
                .update(cx, |shell, w, cx| {
                    match mode {
                        "compact" | "hidden" => {
                            shell.workspace.preferences.collapsed_project_rail = mode.into();
                            shell.workspace.project_rail_open = false;
                        }
                        "settings" => shell.open_settings(crate::settings::Section::General, w, cx),
                        "group" => {
                            shell
                                .workspace
                                .project_group_collapsed
                                .insert("work".into(), true);
                        }
                        _ => unreachable!(),
                    }
                    shell.invalidate_chrome(cx);
                    cx.notify();
                })
                .unwrap();
            cx.run_until_parked();
            let stopped = mark.read_with(cx, |mark, _| mark.animation_state());
            assert!(!stopped.1, "{mode} must cancel the retained mark's task");
            window
                .update(cx, |shell, _, cx| {
                    assert!(!shell.project_marks.contains_key(path), "{mode}");
                    assert_eq!(
                        shell.sidebar_status(session, cx),
                        Some(live_agents::Status::Working)
                    );
                    if finish_hidden {
                        shell.sessions[&session].update(cx, |view, cx| {
                            view.busy = false;
                            cx.emit(SessionChanged);
                            cx.notify();
                        });
                    }
                })
                .unwrap();
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(330));
            cx.run_until_parked();
            assert_eq!(
                mark.read_with(cx, |mark, _| mark.animation_state()),
                stopped,
                "{mode}: hidden timer must never advance"
            );
            window
                .update(cx, |shell, w, cx| {
                    if shell.settings.is_some() {
                        shell.close_settings(w, cx);
                    }
                    shell.workspace.project_rail_open = true;
                    shell
                        .workspace
                        .project_group_collapsed
                        .insert("work".into(), false);
                    shell.invalidate_chrome(cx);
                    cx.notify();
                })
                .unwrap();
            cx.run_until_parked();
            window
                .update(cx, |shell, _, cx| {
                    assert_eq!(
                        shell.project_marks[path].read(cx).animation_state().1,
                        !finish_hidden,
                        "{mode}: reveal must use live work state"
                    );
                    assert_eq!(shell.sessions[&session].read(cx).busy, !finish_hidden);
                })
                .unwrap();
        }
    }
}

#[gpui::test]
fn native_group_creation_rename_assignment_and_pin_render_and_persist(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        super::super::bind_keys(cx);
        crate::input::bind(cx);
    });
    let (window, _, _) = crate::shell::tests::window(cx);
    let path = "/work/another-project".to_string();
    window
        .update(cx, |shell, w, cx| {
            shell.run_menu(
                MenuTarget::Project {
                    path: path.clone(),
                    rail: true,
                },
                "project-group:new",
                None,
                w,
                cx,
            );
        })
        .unwrap();
    let group_id = window
        .update(cx, |shell, _, _| {
            shell.workspace.project_rail_state.as_ref().unwrap().groups[0]
                .id
                .clone()
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1400.), px(1000.)));
    visual.run_until_parked();
    visual.simulate_input("Work");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let selector: &'static str =
        Box::leak(format!("classic-project-group-{group_id}").into_boxed_str());
    assert!(visual.debug_bounds(selector).is_some());
    window
        .update(visual, |shell, w, cx| {
            assert_eq!(
                shell.workspace.project_rail_state.as_ref().unwrap().groups[0].name,
                "Work"
            );
            shell.run_menu(
                MenuTarget::Project {
                    path: path.clone(),
                    rail: true,
                },
                "pin",
                None,
                w,
                cx,
            );
            assert!(
                shell
                    .workspace
                    .project_rail_state
                    .as_ref()
                    .unwrap()
                    .pinned
                    .contains(&vorkspace_core::files::path_key(&path))
            );
            shell.run_menu(
                MenuTarget::Project {
                    path: path.clone(),
                    rail: true,
                },
                "pin",
                None,
                w,
                cx,
            );
            shell.run_menu(
                MenuTarget::Project {
                    path: path.clone(),
                    rail: true,
                },
                "project-group:set:",
                None,
                w,
                cx,
            );
            shell.run_menu(
                MenuTarget::Project {
                    path: path.clone(),
                    rail: true,
                },
                &format!("project-group:set:{group_id}"),
                None,
                w,
                cx,
            );
        })
        .unwrap();
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            let state = locked(&shell.store).unwrap().ui_state().unwrap().unwrap();
            let reopened = Workspace::restore(Some(&state), &[], &path);
            let rail = reopened.project_rail_state.unwrap();
            assert!(!rail.pinned.contains(&path));
            assert_eq!(
                rail.assignments.get(&path).map(String::as_str),
                Some(group_id.as_str())
            );
            assert_eq!(rail.groups[0].name, "Work");
        })
        .unwrap();
    window
        .update(visual, |shell, w, cx| {
            shell.run_menu(
                MenuTarget::Project {
                    path: path.clone(),
                    rail: true,
                },
                &format!("group-remove:{group_id}"),
                None,
                w,
                cx,
            );
        })
        .unwrap();
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            let saved = locked(&shell.store).unwrap().ui_state().unwrap().unwrap();
            let restored = Workspace::restore(Some(&saved), &[], &path);
            let rail = restored.project_rail_state.unwrap();
            assert!(rail.groups.is_empty());
            assert!(rail.assignments.is_empty());
        })
        .unwrap();
}

#[gpui::test]
fn original_rail_keeps_pins_groups_and_all_projects_through_a_saved_group_toggle(
    cx: &mut TestAppContext,
) {
    let root = std::env::temp_dir().join(format!(
        "classic-projects-{}",
        vorkspace_core::bot::new_id()
    ));
    let projects: Vec<_> = (0..7)
        .map(|ix| {
            let path = root.join(format!("project-{ix}"));
            std::fs::create_dir_all(&path).unwrap();
            path.to_string_lossy().into_owned()
        })
        .collect();
    let store = Arc::new(Mutex::new(
        Store::open(&root.join("native.sqlite")).unwrap(),
    ));
    let database = store.clone();
    let initial = projects[0].clone();
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| Shell::new(database, Vec::new(), initial, None, cx))
        })
        .unwrap()
    });
    cx.run_until_parked();
    let source = json!({
        "vorkspace.projectGroups": "[{\"id\":\"work\",\"name\":\"Work\",\"collapsed\":true,\"customColor\":\"#9988ff\"}]",
        "vorkspace.projectGroupAssignments": json!({vorkspace_core::files::path_key(&projects[5]): "work"}).to_string(),
        "vorkspace.projectRailPinned": json!([projects[6]]).to_string(),
    });
    window
        .update(cx, |shell, _, cx| {
            shell.workspace.projects = projects.clone();
            shell.workspace.desktop_preferences_metadata = Some(source.clone());
            shell.workspace.project_rail_state = None;
            shell.workspace.project_group_collapsed.clear();
            shell.invalidate_chrome(cx);
            cx.notify();
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1400.), px(1000.)));
    visual.run_until_parked();
    let pinned = visual.debug_bounds("classic-project-6").unwrap();
    let group = visual.debug_bounds("classic-project-group-work").unwrap();
    let ungrouped = visual.debug_bounds("classic-project-0").unwrap();
    assert!(pinned.top() < group.top());
    assert!(group.top() < ungrouped.top());
    assert!(visual.debug_bounds("classic-project-5").is_none());
    visual.simulate_click(group.center(), Default::default());
    visual.run_until_parked();
    for selector in [
        "classic-project-0",
        "classic-project-1",
        "classic-project-2",
        "classic-project-3",
        "classic-project-4",
        "classic-project-5",
        "classic-project-6",
    ] {
        assert!(visual.debug_bounds(selector).is_some());
    }
    let saved = locked(&store).unwrap().ui_state().unwrap().unwrap();
    let reopened = Workspace::restore(Some(&saved), &[], &projects[0]);
    assert_eq!(reopened.project_group_collapsed.get("work"), Some(&false));
    assert_eq!(reopened.desktop_preferences_metadata, Some(source));
    window
        .update(visual, |shell, _, cx| {
            shell.workspace = reopened;
            shell.invalidate_chrome(cx);
            cx.notify();
        })
        .unwrap();
    visual.run_until_parked();
    assert!(visual.debug_bounds("classic-project-5").is_some());
}
