use super::*;

#[gpui::test]
fn a_failed_pin_transaction_reconciles_both_windows_with_durable_values(
    cx: &mut gpui::TestAppContext,
) {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().to_str().unwrap();
    let file = directory.path().join("history.db");
    let store = vorkspace_core::store::Store::test(&file).unwrap();
    let first = store.create("codex", project).unwrap();
    let second = store.create("codex", project).unwrap();
    let ids = [first.id, second.id];
    let store = Arc::new(Mutex::new(store));
    let open = |cx: &mut gpui::TestAppContext| {
        cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| {
                    Shell::new(
                        store.clone(),
                        vec![first.clone(), second.clone()],
                        project.into(),
                        None,
                        cx,
                    )
                })
            })
            .unwrap()
        })
    };
    let source = open(cx);
    let peer = open(cx);
    let inject = rusqlite::Connection::open(file).unwrap();
    inject
        .execute_batch(&format!(
            "CREATE TRIGGER reject_pin BEFORE UPDATE OF pinned ON sessions
         WHEN OLD.id='{}' BEGIN SELECT RAISE(ABORT,'injected pin failure'); END;",
            locked(&store)
                .unwrap()
                .persisted_session_id(second.id)
                .unwrap()
        ))
        .unwrap();
    source
        .update(cx, |shell, _, cx| {
            shell.edit_session_metadata(ids.to_vec(), SessionEdit::Pin(true), cx);
            assert!(shell.session_metadata(second.id, cx).summary.pinned);
        })
        .unwrap();
    cx.run_until_parked();
    for window in [source, peer] {
        window
            .update(cx, |shell, _, cx| {
                assert!(!shell.session_metadata(first.id, cx).summary.pinned);
                assert!(!shell.session_metadata(second.id, cx).summary.pinned);
            })
            .unwrap();
    }
    source
        .update(cx, |shell, _, _| {
            assert!(
                shell
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("injected pin failure")
            );
        })
        .unwrap();
}

#[gpui::test]
fn a_reminder_survives_a_refused_open_until_its_project_is_reassigned(
    cx: &mut gpui::TestAppContext,
) {
    let (window, reminder_session, current) = crate::shell::tests::window(cx);
    let (vorkspace, project) = window
        .update(cx, |shell, _, cx| {
            let vorkspace = shell.workspace.active_vorkspace.clone();
            let project = shell
                .session_summary(reminder_session, cx)
                .unwrap()
                .cwd
                .clone();
            shell
                .workspace
                .remove_project_from_vorkspace(&vorkspace, &project);
            shell.persist(cx);
            shell.edit_reminder(
                app_core::ReminderEdit::Schedule {
                    sessions: vec![reminder_session],
                    due_at: 1,
                },
                cx,
            );
            (vorkspace, project)
        })
        .unwrap();
    let open_notice = |cx: &mut gpui::TestAppContext| {
        window
            .update(cx, |shell, _, cx| {
                shell.notification_state.reminders.update(cx, |_, cx| {
                    cx.emit(crate::notifications::reminders::Action::Open {
                        session_id: reminder_session,
                        due_at: 1,
                    });
                });
            })
            .unwrap();
        cx.run_until_parked();
    };
    cx.run_until_parked();
    open_notice(cx);
    window
        .update(cx, |shell, _, cx| {
            assert_eq!(shell.workspace.active, Some(current));
            assert_eq!(shell.reminders(cx)[0].session_id, reminder_session);
            assert_eq!(locked(&shell.store).unwrap().reminders().unwrap().len(), 1);
            shell
                .workspace
                .add_project_to_vorkspace(&vorkspace, &project);
            shell.persist(cx);
        })
        .unwrap();
    open_notice(cx);
    window
        .update(cx, |shell, _, _| {
            assert_eq!(shell.workspace.active, Some(reminder_session));
            assert!(
                locked(&shell.store)
                    .unwrap()
                    .reminders()
                    .unwrap()
                    .is_empty()
            );
        })
        .unwrap();
}
use gpui::{TestAppContext, WindowHandle};

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
            shell.sidebar.selected_sessions.clear();
            shell.sidebar.selected_sessions.insert(a);
            shell.delete_sidebar_selection(w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    window
        .update(cx, |shell, w, cx| {
            assert!(shell.has_session(a, cx));
            shell.delete_sidebar_selection(w, cx);
            shell.sidebar.selected_sessions.clear();
            shell.sidebar.selected_sessions.insert(b);
        })
        .unwrap();
    cx.run_until_parked();
    cx.simulate_prompt_answer("Delete");
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert!(!shell.has_session(a, cx));
            assert!(shell.has_session(b, cx));
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
        .update(cx, |shell, _, cx| {
            assert!(shell.has_session(a, cx));
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
    // Deletion visits handles in numeric order. Attempt the guarded owner
    // before deleting its detached pane, regardless of the UUID hashes.
    let (a, b) = (a.min(b), a.max(b));
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
        .update(cx, |shell, _, cx| {
            assert!(shell.has_session(a, cx));
            assert!(!shell.has_session(b, cx));
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
            let summary = shell.session_summary(a, cx).unwrap().clone();
            shell.workspace.close(a);
            shell.forget_session(a, cx);
            shell.add(summary, cx);
        })
        .unwrap();
    cx.run_until_parked();
    cx.simulate_prompt_answer("Delete");
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert!(shell.has_session(a, cx));
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
            shell.sidebar.selected_sessions.extend([a, b]);
            shell.run_session_menu(a, "delete", w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Delete");
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert!(!shell.has_session(a, cx));
            assert!(!shell.has_session(b, cx));
        })
        .unwrap();
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
            assert_eq!(shell.sidebar.selected_sessions.len(), 2);
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
fn session_rename_accepts_enter_and_escape_cancels(cx: &mut TestAppContext) {
    let (window, a, _) = setup(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.run_menu(MenuTarget::Session(a), "rename", None, w, cx);
            shell
                .sidebar
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
                .sidebar
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
            assert!(shell.sidebar.renaming.is_none());
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
                .sidebar
                .link_dialog
                .as_ref()
                .unwrap()
                .input
                .update(cx, |i, cx| i.set_text("invalid".into(), cx));
            shell.save_link(true, w, cx);
            assert!(shell.sidebar.link_dialog.as_ref().unwrap().error.is_some());
            shell
                .sidebar
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
            assert!(shell.sidebar.link_dialog.is_none());
            shell.run_menu(MenuTarget::Session(a), "reminder:cancel", None, w, cx);
            let due_at = vorkspace_core::time::now_ms().saturating_sub(1);
            shell.edit_reminder(
                app_core::ReminderEdit::Schedule {
                    sessions: vec![a],
                    due_at,
                },
                cx,
            );
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
            assert!(shell.reminders(cx).is_empty());
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
