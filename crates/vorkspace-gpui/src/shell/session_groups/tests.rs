use super::*;
use gpui::{TestAppContext, VisualTestContext, WindowHandle};

fn setup(cx: &mut TestAppContext) -> (WindowHandle<Shell>, i64, i64) {
    cx.update(|cx| {
        crate::shell::bind_keys(cx);
        crate::input::bind(cx);
        crate::popover::bind(cx);
    });
    let (window, a, b) = crate::shell::tests::window(cx);
    window
        .update(cx, |shell, _, cx| {
            for id in [a, b] {
                shell.sessions[&id].update(cx, |session, _| {
                    session.summary.cwd = shell.workspace.project.clone();
                    session.summary.has_messages = true;
                });
            }
            shell.invalidate_chrome(cx);
        })
        .unwrap();
    cx.run_until_parked();
    cx.refresh().unwrap();
    (window, a, b)
}

#[gpui::test]
fn delete_confirmation_cancels_and_uses_selection_snapshot(cx: &mut TestAppContext) {
    let (window, a, b) = setup(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.selected_sessions.clear();
            shell.selected_sessions.insert(a);
            shell.delete_sidebar_selection(w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    window
        .update(cx, |shell, w, cx| {
            assert!(shell.session_summaries.contains_key(&a));
            shell.delete_sidebar_selection(w, cx);
            shell.selected_sessions.clear();
            shell.selected_sessions.insert(b);
        })
        .unwrap();
    cx.run_until_parked();
    cx.simulate_prompt_answer("Delete");
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert!(!shell.session_summaries.contains_key(&a));
            assert!(shell.session_summaries.contains_key(&b));
        })
        .unwrap();
}

#[gpui::test]
fn delete_rechecks_detached_panes_after_confirmation(cx: &mut TestAppContext) {
    let (window, a, b) = setup(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.request_delete_sessions(vec![a], w, cx);
            // The pane becomes detached while the prompt is open.
            assert!(shell.workspace.tab_of(b).is_some());
            shell.workspace.review_owners.insert(b, a);
        })
        .unwrap();
    cx.run_until_parked();
    cx.simulate_prompt_answer("Delete");
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert!(shell.session_summaries.contains_key(&a));
            assert!(
                shell
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("detached file panes")
            );
        })
        .unwrap();
}

#[gpui::test]
fn bulk_delete_keeps_guarded_session_and_deletes_the_other(cx: &mut TestAppContext) {
    let (window, a, b) = setup(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.workspace.review_owners.insert(b, a);
            shell.request_delete_sessions(vec![a, b], w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    cx.simulate_prompt_answer("Delete");
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert!(shell.session_summaries.contains_key(&a));
            assert!(!shell.session_summaries.contains_key(&b));
            assert!(
                shell
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("detached file panes")
            );
        })
        .unwrap();
}

#[gpui::test]
fn deleting_session_cannot_reopen(cx: &mut TestAppContext) {
    let (window, a, _) = setup(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.open(a, w, cx);
            shell.delete_session(a, w, cx);
            shell.open(a, w, cx);
            assert_ne!(shell.workspace.active, Some(a));
        })
        .unwrap();
    cx.run_until_parked();
}

#[gpui::test]
fn stale_confirmation_does_not_delete_reused_id(cx: &mut TestAppContext) {
    let (window, a, _) = setup(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.request_delete_sessions(vec![a], w, cx);
            // Simulate removal and reuse while consent is pending.
            let summary = shell.session_summaries[&a].clone();
            shell.workspace.close(a);
            shell.forget_session(a, cx);
            shell.add(summary, cx);
        })
        .unwrap();
    cx.run_until_parked();
    cx.simulate_prompt_answer("Delete");
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert!(shell.session_summaries.contains_key(&a));
            assert!(
                shell
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("confirm deletion again")
            );
        })
        .unwrap();
}

#[gpui::test]
fn menu_delete_confirms_multiple_sessions(cx: &mut TestAppContext) {
    let (window, a, b) = setup(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.selected_sessions.extend([a, b]);
            shell.run_session_menu(a, "delete", w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Delete");
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert!(!shell.session_summaries.contains_key(&a));
            assert!(!shell.session_summaries.contains_key(&b));
        })
        .unwrap();
}

#[gpui::test]
fn expanded_folders_use_compact_connected_rows_and_keep_operator_children(cx: &mut TestAppContext) {
    let (window, a, b) = setup(cx);
    let (plain, child) = window
        .update(cx, |shell, _, cx| {
            let mut ids = Vec::new();
            for _ in 0..2 {
                let mut summary = locked(&shell.store)
                    .unwrap()
                    .create("codex", &shell.workspace.project)
                    .unwrap();
                summary.has_messages = true;
                ids.push(summary.id);
                shell.add(summary, cx);
                shell.sessions[ids.last().unwrap()]
                    .update(cx, |chat, _| chat.summary.has_messages = true);
            }
            let (plain, child) = (ids[0], ids[1]);
            shell.groups.insert(
                shell.workspace.project.clone(),
                Groups {
                    folders: vec![vorkspace_core::session_groups::Folder {
                        id: "visual-folder".into(),
                        name: "Review work".into(),
                        session_ids: vec![a, b],
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            );
            shell.started_by.insert(child, a);
            shell.workspace.active = Some(a);
            shell.invalidate_chrome(cx);
            (plain, child)
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let row = |cx: &mut VisualTestContext, id| {
        cx.debug_bounds(Box::leak(format!("sidebar-session-{id}").into_boxed_str()))
    };
    let header = visual
        .debug_bounds("session-group-folder:visual-folder")
        .unwrap();
    let first = row(visual, a).unwrap();
    let second = row(visual, b).unwrap();
    assert_eq!(first.left() - header.left(), px(30.));
    assert_eq!(first.size.height, px(28.));
    assert_eq!(second.left(), first.left());
    assert_eq!(second.size.height, px(28.));
    assert_eq!(row(visual, plain).unwrap().size.height, px(32.));
    let started = row(visual, child).expect("a folder member keeps its Operator rail");
    assert_eq!(started.left() - first.left(), px(30.));
    window
        .update(visual, |shell, _, cx| {
            shell.change_sidebar_option("compact-session-cards", cx);
        })
        .unwrap();
    visual.run_until_parked();
    assert_eq!(row(visual, a).unwrap().size.height, px(28.));
    assert_eq!(row(visual, b).unwrap().size.height, px(28.));
    assert_eq!(row(visual, child).unwrap().size.height, px(28.));
    assert_eq!(
        row(visual, plain).unwrap().size.height,
        px(32.),
        "the retired density action keeps the standard row height"
    );
    window
        .update(visual, |_, window, _| {
            let scale = window.scale_factor();
            assert!(
                window.painted_quads().iter().any(|quad| {
                    (quad.bounds.left().0 - f32::from(header.left()) * scale).abs() < 1.
                        && (quad.bounds.top().0 - f32::from(header.top()) * scale).abs() < 1.
                        && (quad.bounds.size.width.0 - f32::from(header.size.width) * scale).abs()
                            < 1.
                        && quad
                            .background
                            .as_solid()
                            .is_some_and(|color| (color.a - 0.05).abs() < 0.001)
                }),
                "an open folder has a persistent header fill"
            );
            assert!(
                window.painted_quads().iter().any(|quad| {
                    (quad.bounds.left().0 - f32::from(header.left() + px(16.)) * scale).abs() < 1.
                        && (quad.bounds.size.width.0 - scale).abs() < 0.1
                        && quad.bounds.top().0 > f32::from(header.bottom()) * scale
                        && quad.bounds.top().0 < f32::from(first.bottom()) * scale
                        && quad
                            .background
                            .as_solid()
                            .is_some_and(|color| color == gpui::Hsla::from(rgb(0x323232)))
                }),
                "the same spine as Operator rows connects folder members"
            );
        })
        .unwrap();
    window
        .update(visual, |shell, _, cx| {
            shell.update_groups(cx, |groups| groups.toggle_folder("visual-folder"))
        })
        .unwrap();
    visual.refresh().unwrap();
    assert!(row(visual, a).is_none());
    assert!(row(visual, b).is_none());
    assert!(row(visual, child).is_none());
    window
        .update(visual, |shell, _, cx| {
            shell.query = "new".into();
            shell.notify_sidebar(cx);
        })
        .unwrap();
    visual.refresh().unwrap();
    assert_eq!(
        row(visual, a).unwrap().size.height,
        px(28.),
        "search opens a collapsed folder with the same compact layout"
    );
}

#[gpui::test]
fn original_sidebar_header_controls_follow_their_own_columns(cx: &mut TestAppContext) {
    let (window, _, _) = setup(cx);
    // The header's plus appears once this project has a Bot. The empty
    // state uses a full-width creation row instead.
    window
        .update(cx, |shell, _, cx| {
            if let Some(bots) = &shell.bots {
                bots.update(cx, |bots, cx| {
                    bots.roster.bots.push(vorkspace_core::bot::Bot {
                        projects: vec![shell.workspace.project.clone()],
                        ..Default::default()
                    });
                    cx.notify();
                });
            }
            shell.invalidate_chrome(cx);
        })
        .unwrap();
    cx.refresh().unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let options = visual.debug_bounds("sidebar-options").unwrap();
    let bot = visual.debug_bounds("sidebar-bot-button").unwrap();
    let filter = visual.debug_bounds("sidebar-filter-button").unwrap();
    let rail = visual.debug_bounds("classic-project-rail").unwrap();
    let sidebar = visual.debug_bounds("sidebar-frame").unwrap();
    assert!(
        rail.contains(&bot.center()),
        "Bot controls belong to the project rail"
    );
    assert!(rail.contains(&options.center()));
    assert!(sidebar.contains(&filter.center()));
    let project = visual.debug_bounds("sidebar-project-button").unwrap();
    let search = visual.debug_bounds("sidebar-search-field").unwrap();
    assert!(project.left() >= sidebar.left());
    assert!(search.right() <= rail.right());
    assert_eq!(project.size.height, px(28.));
    assert!(project.left() > options.right());
}

#[gpui::test]
fn session_actions_are_enabled_and_bulk_selection_shapes_the_menu(cx: &mut TestAppContext) {
    let (window, a, b) = setup(cx);
    window
        .update(cx, |shell, w, cx| {
            let items = shell.session_menu_items(a, cx);
            for item in items {
                if let MenuItem::Action(action) = item {
                    assert!(!action.disabled, "{} must be usable", action.label);
                }
            }
            shell.select_sidebar_session(
                a,
                gpui::Modifiers {
                    platform: true,
                    ..Default::default()
                },
                w,
                cx,
            );
            shell.select_sidebar_session(
                b,
                gpui::Modifiers {
                    shift: true,
                    ..Default::default()
                },
                w,
                cx,
            );
            assert_eq!(shell.selected_sessions.len(), 2);
            let actions: Vec<_> = shell
                .session_menu_items(a, cx)
                .into_iter()
                .filter_map(|item| match item {
                    MenuItem::Action(action) => Some(action.id),
                    _ => None,
                })
                .collect();
            assert!(
                !actions
                    .iter()
                    .any(|id| id == "rename" || id == "copy-session-id" || id == "link-work-item")
            );
            shell.run_menu(MenuTarget::Session(a), "pin", None, w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert!(shell.sessions[&a].read(cx).summary.pinned);
            assert!(shell.sessions[&b].read(cx).summary.pinned);
            let stored = locked(&shell.store).unwrap().sessions().unwrap();
            assert!(stored.iter().all(|s| s.pinned));
        })
        .unwrap();
}

#[gpui::test]
fn folders_rename_move_collapse_color_and_ungroup_with_saved_state(cx: &mut TestAppContext) {
    let (window, a, b) = setup(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.run_menu(MenuTarget::Session(a), "folder-new", None, w, cx)
        })
        .unwrap();
    cx.refresh().unwrap();
    window
        .update(cx, |shell, _, cx| {
            let renaming = shell.renaming.as_ref().unwrap();
            renaming
                .input
                .update(cx, |input, cx| input.set_text("Work".into(), cx));
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "enter");
    cx.run_until_parked();
    let folder = window
        .update(cx, |shell, w, cx| {
            assert!(shell.renaming.is_none());
            let folder = shell.project_groups().folder_of(a).unwrap().id.clone();
            shell.run_menu(
                MenuTarget::Session(b),
                &format!("folder-add:{folder}"),
                None,
                w,
                cx,
            );
            shell.update_groups(cx, |g| {
                g.toggle_folder(&folder);
                g.set_color(&folder, Some(3));
            });
            assert!(shell.sidebar_navigation_ids(cx).is_empty());
            shell.query = "new".into();
            assert_eq!(
                shell.sidebar_navigation_ids(cx).len(),
                2,
                "search opens folded groups"
            );
            shell.query.clear();
            folder
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, w, cx| {
            let saved = locked(&shell.store)
                .unwrap()
                .groups(&shell.workspace.project)
                .unwrap();
            let saved = saved.folder(&folder).unwrap();
            assert_eq!(saved.name, "Work");
            assert_eq!(saved.color_index, Some(3));
            assert!(saved.collapsed);
            assert!(saved.session_ids.contains(&a) && saved.session_ids.contains(&b));
            shell.run_menu(MenuTarget::Session(b), "folder-remove", None, w, cx);
            assert_eq!(
                shell.project_groups().folder(&folder).unwrap().session_ids,
                [a]
            );
            shell.run_menu(MenuTarget::Folder(folder.clone()), "ungroup", None, w, cx);
            assert_eq!(shell.sidebar_navigation_ids(cx).len(), 2);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert!(
                locked(&shell.store)
                    .unwrap()
                    .groups(&shell.workspace.project)
                    .unwrap()
                    .folders
                    .is_empty()
            );
        })
        .unwrap();
}

#[gpui::test]
fn session_rename_accepts_enter_and_escape_cancels(cx: &mut TestAppContext) {
    let (window, a, _) = setup(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.run_menu(MenuTarget::Session(a), "rename", None, w, cx);
            shell
                .renaming
                .as_ref()
                .unwrap()
                .input
                .update(cx, |i, cx| i.set_text("First name".into(), cx));
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.simulate_keystrokes(window.into(), "enter");
    cx.run_until_parked();
    window
        .update(cx, |shell, w, cx| {
            assert_eq!(shell.sessions[&a].read(cx).summary.title, "First name");
            let saved = locked(&shell.store).unwrap().sessions().unwrap();
            assert_eq!(
                saved.iter().find(|s| s.id == a).unwrap().title,
                "First name"
            );
            shell.run_menu(MenuTarget::Session(a), "rename", None, w, cx);
            shell
                .renaming
                .as_ref()
                .unwrap()
                .input
                .update(cx, |i, cx| i.set_text("Discard".into(), cx));
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.simulate_keystrokes(window.into(), "escape");
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert!(shell.renaming.is_none());
            assert_eq!(shell.sessions[&a].read(cx).summary.title, "First name");
        })
        .unwrap();
}

#[gpui::test]
fn links_and_reminders_validate_persist_and_clear_when_due_session_opens(cx: &mut TestAppContext) {
    let (window, a, _) = setup(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.run_menu(MenuTarget::Session(a), "link-work-item", None, w, cx);
            shell
                .link_dialog
                .as_ref()
                .unwrap()
                .input
                .update(cx, |i, cx| i.set_text("invalid".into(), cx));
            shell.save_link(true, w, cx);
            assert!(shell.link_dialog.as_ref().unwrap().error.is_some());
            shell
                .link_dialog
                .as_ref()
                .unwrap()
                .input
                .update(cx, |i, cx| {
                    i.set_text("https://github.com/acme/app/pull/42".into(), cx)
                });
            shell.save_link(true, w, cx);
            shell.run_menu(MenuTarget::Session(a), "reminder:tomorrow", None, w, cx);
            shell.run_menu(MenuTarget::Session(a), "reminder:1h", None, w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, w, cx| {
            let store = locked(&shell.store).unwrap();
            assert_eq!(store.reminders().unwrap().len(), 1);
            assert_eq!(
                store
                    .sessions()
                    .unwrap()
                    .iter()
                    .find(|s| s.id == a)
                    .unwrap()
                    .work_item
                    .as_ref()
                    .unwrap()
                    .number,
                42
            );
            drop(store);
            assert!(shell.link_dialog.is_none());
            shell.run_menu(MenuTarget::Session(a), "reminder:cancel", None, w, cx);
            let due_at = vorkspace_core::time::now_ms().saturating_sub(1);
            shell.reminders.push(Reminder {
                session_id: a,
                due_at,
                fired_at: None,
            });
            shell.store_write("set test reminder", cx, move |store| {
                store.set_reminders(&[a], due_at)
            });
            shell.poll_reminders(cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, w, cx| {
            assert!(
                locked(&shell.store).unwrap().reminders().unwrap()[0]
                    .fired_at
                    .is_some()
            );
            shell.open(a, w, cx);
            assert!(shell.reminders.is_empty());
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert!(
                locked(&shell.store)
                    .unwrap()
                    .reminders()
                    .unwrap()
                    .is_empty()
            )
        })
        .unwrap();
}

#[gpui::test]
fn dragging_sessions_creates_a_folder_and_moves_into_an_existing_one(cx: &mut TestAppContext) {
    let (window, a, b) = setup(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let a_row = visual
        .debug_bounds(Box::leak(format!("sidebar-session-{a}").into_boxed_str()))
        .unwrap();
    let b_row = visual
        .debug_bounds(Box::leak(format!("sidebar-session-{b}").into_boxed_str()))
        .unwrap();
    visual.simulate_mouse_down(a_row.center(), MouseButton::Left, Default::default());
    visual.simulate_mouse_move(b_row.center(), Some(MouseButton::Left), Default::default());
    visual.simulate_mouse_up(b_row.center(), MouseButton::Left, Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, w, cx| {
            let groups = shell.project_groups();
            assert_eq!(groups.folders.len(), 1);
            assert_eq!(groups.folders[0].session_ids, [a, b]);
            assert!(shell.renaming.is_some());
            shell.finish_rename(false, w, cx);
            shell.update_groups(cx, |g| g.remove_sessions(&[a]));
        })
        .unwrap();
    visual.refresh().unwrap();
    let folder = window
        .update(visual, |shell, _, _| {
            shell.project_groups().folders[0].id.clone()
        })
        .unwrap();
    let row = visual
        .debug_bounds(Box::leak(format!("sidebar-session-{a}").into_boxed_str()))
        .unwrap();
    let header = visual
        .debug_bounds(Box::leak(
            format!("session-group-folder:{folder}").into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_mouse_down(row.center(), MouseButton::Left, Default::default());
    visual.simulate_mouse_move(header.center(), Some(MouseButton::Left), Default::default());
    visual.simulate_mouse_up(header.center(), MouseButton::Left, Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            assert!(
                shell
                    .project_groups()
                    .folder(&folder)
                    .unwrap()
                    .session_ids
                    .contains(&a)
            )
        })
        .unwrap();
}

#[gpui::test]
fn delete_closes_the_session_and_cleans_up_folders_and_reminders(cx: &mut TestAppContext) {
    let (window, a, b) = setup(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.run_menu(MenuTarget::Session(a), "pin", None, w, cx);
            shell.run_menu(MenuTarget::Session(a), "reminder:1h", None, w, cx);
            shell.update_groups(cx, |g| {
                g.create_folder(&[a], "remove-me".into());
            });
            shell.open(a, w, cx);
            shell.run_menu(MenuTarget::Session(a), "delete", None, w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Delete");
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert!(!shell.sessions.contains_key(&a));
            assert!(shell.sessions.contains_key(&b));
            assert!(shell.project_groups().folders.is_empty());
            let store = locked(&shell.store).unwrap();
            assert!(!store.sessions().unwrap().iter().any(|s| s.id == a));
            assert!(store.load(a).unwrap().rows.is_empty());
            assert!(store.reminders().unwrap().is_empty());
            assert!(
                store
                    .groups(&shell.workspace.project)
                    .unwrap()
                    .folders
                    .is_empty()
            );
        })
        .unwrap();
}
#[gpui::test]
fn folder_headers_reorder_and_the_color_menu_updates_the_saved_folder(cx: &mut TestAppContext) {
    let (window, a, b) = setup(cx);
    window
        .update(cx, |shell, _, cx| {
            shell.update_groups(cx, |g| {
                g.create_folder(&[a], "first".into());
                g.create_folder(&[b], "second".into());
            });
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    let first = visual
        .debug_bounds("session-group-folder:first")
        .unwrap()
        .center();
    let second = visual
        .debug_bounds("session-group-folder:second")
        .unwrap()
        .center();
    visual.simulate_mouse_down(second, MouseButton::Left, Default::default());
    visual.simulate_mouse_move(first, Some(MouseButton::Left), Default::default());
    visual.simulate_mouse_up(first, MouseButton::Left, Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            let g = shell.project_groups();
            assert_eq!(g.folders[0].id, "first");
            assert_eq!(g.folders[1].id, "second");
            assert!(!g.folders[0].collapsed && !g.folders[1].collapsed);
        })
        .unwrap();
    visual.refresh().unwrap();
    let first = visual
        .debug_bounds("session-group-folder:first")
        .unwrap()
        .center();
    visual.simulate_mouse_down(first, MouseButton::Right, Default::default());
    visual.run_until_parked();
    visual.refresh().unwrap();
    let swatch = visual.debug_bounds("folder-color-3").unwrap().center();
    visual.simulate_click(swatch, Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            let saved = locked(&shell.store)
                .unwrap()
                .groups(&shell.workspace.project)
                .unwrap();
            assert_eq!(saved.folders[0].id, "first");
            assert_eq!(saved.folder("first").unwrap().color_index, Some(3));
        })
        .unwrap();
}

#[gpui::test]
fn f2_renames_the_active_session_and_the_focused_folder(cx: &mut TestAppContext) {
    let (window, a, _) = setup(cx);
    window
        .update(cx, |shell, w, cx| shell.open(a, w, cx))
        .unwrap();
    cx.simulate_keystrokes(window.into(), "f2");
    cx.run_until_parked();
    window.update(cx, |shell, w, cx| {
            assert!(matches!(shell.renaming.as_ref().unwrap().target, RenameTarget::Session(id) if id == a));
            shell.finish_rename(false, w, cx);
            shell.update_groups(cx, |g| { g.create_folder(&[a], "work".into()); });
        }).unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    let folder = visual
        .debug_bounds("session-group-folder:work")
        .unwrap()
        .center();
    visual.simulate_click(folder, Default::default());
    visual.simulate_keystrokes("f2");
    visual.run_until_parked();
    window.update(visual, |shell, _, _| {
            assert!(matches!(&shell.renaming.as_ref().unwrap().target, RenameTarget::Folder(id) if id == "work"));
        }).unwrap();
}
