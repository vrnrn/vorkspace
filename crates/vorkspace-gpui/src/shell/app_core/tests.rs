use super::*;
use gpui::{TestAppContext, WindowHandle};
use vorkspace_core::store::Store;

fn two_windows(
    cx: &mut TestAppContext,
) -> (
    WindowHandle<Shell>,
    WindowHandle<Shell>,
    i64,
    tempfile::TempDir,
) {
    two_windows_with(cx, |_, _| {})
}

fn two_windows_with(
    cx: &mut TestAppContext,
    seed: impl FnOnce(&Store, &str),
) -> (
    WindowHandle<Shell>,
    WindowHandle<Shell>,
    i64,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project").to_string_lossy().into_owned();
    std::fs::create_dir_all(&project).unwrap();
    let store = Store::open(&dir.path().join("native.sqlite")).unwrap();
    seed(&store, &project);
    let session = store.create("codex", &project).unwrap();
    let id = session.id;
    let store: Database = Arc::new(Mutex::new(store));
    let open = |cx: &mut TestAppContext| {
        let (store, session, project) = (store.clone(), session.clone(), project.clone());
        cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| Shell::new(store, vec![session], project, None, cx))
            })
            .unwrap()
        })
    };
    let a = open(cx);
    cx.run_until_parked();
    let b = open(cx);
    cx.run_until_parked();
    (a, b, id, dir)
}

#[gpui::test]
fn windows_share_vorkspace_definitions(cx: &mut TestAppContext) {
    let (a, b, _, _dir) = two_windows(cx);
    let id = a
        .update(cx, |shell, _, cx| {
            let id = shell.workspace.create_vorkspace("Shared");
            shell.persist(cx);
            id
        })
        .unwrap();
    cx.run_until_parked();
    b.update(cx, |shell, _, _| {
        assert!(shell.workspace.vorkspaces.iter().any(|v| v.id == id));
        // Each window keeps its own choice of which one it shows.
        assert_ne!(shell.workspace.active_vorkspace, id);
    })
    .unwrap();
}

#[gpui::test]
fn concurrent_bot_creations_keep_their_own_vorkspaces_and_share_the_roster(
    cx: &mut TestAppContext,
) {
    let (a, b, _, _dir) = two_windows(cx);
    let side = b
        .update(cx, |shell, window, cx| {
            let id = shell.workspace.create_vorkspace("Side");
            let project = shell.workspace.project.clone();
            shell.workspace.add_project_to_vorkspace(&id, &project);
            shell.persist(cx);
            shell.switch_vorkspace(&id, window, cx);
            id
        })
        .unwrap();
    let create = |window: WindowHandle<Shell>, cx: &mut TestAppContext| {
        window
            .update(cx, |shell, w, cx| {
                let project = shell.workspace.project.clone();
                shell.bot_manager().update(cx, |bots, cx| {
                    bots.create_named(vec![project], w, cx);
                    bots.roster.bots.last().unwrap().id.clone()
                })
            })
            .unwrap()
    };
    let first = create(a, cx);
    let second = create(b, cx);
    cx.run_until_parked();
    for (window, own, other) in [(a, &first, &second), (b, &second, &first)] {
        window
            .update(cx, |shell, _, cx| {
                let manager = shell.bot_manager();
                let bots = manager.read(cx);
                assert!(bots.roster.get(own).unwrap().session_id.is_some());
                assert!(bots.roster.get(other).unwrap().session_id.is_some());
                assert!(shell.workspace.vorkspace_has_bot(own));
                assert!(!shell.workspace.vorkspace_has_bot(other));
                shell.persist(cx);
            })
            .unwrap();
    }

    let visual = &mut gpui::VisualTestContext::from_window(b.into(), cx);
    visual.refresh().unwrap();
    let delete = visual.debug_bounds("bot-delete").unwrap();
    visual.simulate_click(delete.center(), Default::default());
    visual.simulate_prompt_answer("Delete");
    visual.run_until_parked();
    a.update(visual, |shell, _, cx| {
        assert!(shell.bot_manager().read(cx).roster.get(&second).is_none());
        assert!(shell.workspace.vorkspace_has_bot(&first));
        assert!(
            shell
                .workspace
                .vorkspaces
                .iter()
                .find(|v| v.id == side)
                .unwrap()
                .bots
                .is_empty()
        );
        let durable = vorkspace_core::bot::Roster::load(&locked(&shell.store).unwrap()).unwrap();
        assert!(durable.get(&second).is_none());
        assert!(durable.get(&first).is_some());
    })
    .unwrap();
}

#[gpui::test]
fn accepted_bot_creation_finishes_when_its_window_closes(cx: &mut TestAppContext) {
    let (a, b, _, _dir) = two_windows(cx);
    let id = b
        .update(cx, |shell, w, cx| {
            let project = shell.workspace.project.clone();
            let id = shell.bot_manager().update(cx, |bots, cx| {
                bots.create_named(vec![project], w, cx);
                bots.roster.bots.last().unwrap().id.clone()
            });
            w.remove_window();
            id
        })
        .unwrap();
    cx.run_until_parked();
    a.update(cx, |shell, _, cx| {
        assert!(shell.bot_manager().read(cx).roster.get(&id).is_some());
        assert!(shell.workspace.vorkspace_has_bot(&id));
        let durable = vorkspace_core::bot::Roster::load(&locked(&shell.store).unwrap()).unwrap();
        assert!(durable.get(&id).is_some());
    })
    .unwrap();
}

#[gpui::test]
fn two_windows_prepare_one_backing_conversation_even_if_the_first_closes(cx: &mut TestAppContext) {
    let id = "shared-new-bot";
    let (a, b, _, _dir) = two_windows_with(cx, |store, project| {
        let mut roster = vorkspace_core::bot::Roster::default();
        roster.bots.push(vorkspace_core::bot::Bot {
            id: id.into(),
            name: "Shared".into(),
            projects: vec![project.into()],
            ..Default::default()
        });
        roster.save(store).unwrap();
    });
    for window in [a, b] {
        window
            .update(cx, |shell, _, cx| {
                assert!(
                    shell
                        .bot_manager()
                        .update(cx, |bots, cx| {
                            assert!(bots.roster.get(id).unwrap().session_id.is_none());
                            bots.chat_for_action(id, cx)
                        })
                        .is_none()
                );
            })
            .unwrap();
    }
    cx.update_window(a.into(), |_, w, _| w.remove_window())
        .unwrap();
    cx.run_until_parked();
    b.update(cx, |shell, _, cx| {
        shell.bot_manager().update(cx, |bots, cx| {
            bots.chat_for_action(id, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    b.update(cx, |shell, _, cx| {
        let db = locked(&shell.store).unwrap();
        let durable = vorkspace_core::bot::Roster::load(&db).unwrap();
        let backing = durable.get(id).unwrap().session_id.unwrap();
        assert_eq!(durable.hidden_sessions, [backing]);
        let home = db.bot_session(backing).unwrap().unwrap().cwd;
        assert_eq!(
            db.sessions()
                .unwrap()
                .iter()
                .filter(|s| s.cwd == home)
                .count(),
            1
        );
        assert_eq!(
            shell.bot_manager().update(cx, |bots, cx| bots
                .chat_for_action(id, cx)
                .unwrap()
                .read(cx)
                .summary
                .id),
            backing
        );
    })
    .unwrap();
}

#[gpui::test]
fn one_window_fires_scheduled_work_and_hands_it_on_when_closed(cx: &mut TestAppContext) {
    let (a, b, _, _dir) = two_windows(cx);
    assert!(
        a.update(cx, |shell, _, cx| shell.holds_scheduler(cx))
            .unwrap()
    );
    assert!(
        !b.update(cx, |shell, _, cx| shell.holds_scheduler(cx))
            .unwrap()
    );
    cx.update_window(a.into(), |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert!(
        b.update(cx, |shell, _, cx| shell.holds_scheduler(cx))
            .unwrap()
    );
}

#[gpui::test]
fn scheduler_delivers_a_reminder_created_in_another_window(cx: &mut TestAppContext) {
    let (a, b, _, _dir) = two_windows(cx);
    assert!(
        a.update(cx, |shell, _, cx| shell.holds_scheduler(cx))
            .unwrap()
    );
    let id = b
        .update(cx, |shell, _, cx| {
            let session = locked(&shell.store)
                .unwrap()
                .create("codex", &shell.workspace.project)
                .unwrap();
            let id = session.id;
            shell.add(session, cx);
            shell.edit_reminder(
                ReminderEdit::Schedule {
                    sessions: vec![id],
                    due_at: 1,
                },
                cx,
            );
            id
        })
        .unwrap();
    cx.run_until_parked();
    a.update(cx, |shell, _, cx| {
        let durable = locked(&shell.store).unwrap().reminders().unwrap();
        assert!(shell.session_summaries.contains_key(&id));
        assert_eq!(durable.as_slice(), shell.reminders(cx));
        assert_eq!(durable[0].session_id, id);
        assert!(durable[0].fired_at.is_some());
        shell.sync_notices(cx);
        assert!(!shell.notification_state.reminders.read(cx).is_empty());
    })
    .unwrap();
    let later = vorkspace_core::time::now_ms() + 3_600_000;
    b.update(cx, |shell, _, _| {
        assert_ne!(shell.workspace.active, Some(id));
        locked(&shell.store)
            .unwrap()
            .set_reminders(&[id], later)
            .unwrap();
    })
    .unwrap();
    a.update(cx, |shell, _, cx| {
        shell.notification_state.reminders.update(cx, |_, cx| {
            cx.emit(crate::notifications::reminders::Action::Open {
                session_id: id,
                due_at: 1,
            });
        });
    })
    .unwrap();
    cx.run_until_parked();
    b.update(cx, |shell, _, _| {
        assert_eq!(shell.workspace.active, Some(id));
        assert_eq!(
            locked(&shell.store).unwrap().reminders().unwrap()[0].due_at,
            later
        );
    })
    .unwrap();
    a.update(cx, |shell, _, cx| {
        assert_eq!(shell.reminders(cx)[0].due_at, later);
    })
    .unwrap();
}

#[gpui::test]
fn quick_composer_toggles_one_popup_across_windows(cx: &mut TestAppContext) {
    let (a, b, _, _dir) = two_windows(cx);
    for (window, expected) in [(a, 3), (b, 2), (b, 3), (a, 2)] {
        window
            .update(cx, |shell, window, cx| {
                shell.toggle_quick_composer(window, cx)
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(cx.update(|cx| cx.windows().len()), expected);
    }
}

#[gpui::test]
fn a_session_is_private_during_preparation_and_peers_receive_its_worktree(cx: &mut TestAppContext) {
    let (a, b, _, _dir) = two_windows(cx);
    let (summary, mut install) = a
        .update(cx, |shell, window, cx| {
            let mut install = shell.session_install(cx);
            let summary = {
                let db = locked(&shell.store).unwrap();
                let summary = db.create("codex", &shell.workspace.project).unwrap();
                install.track(summary.id);
                assert!(
                    !shell
                        .app_core
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .session_installs
                        .visible_sessions(&db)
                        .unwrap()
                        .iter()
                        .any(|row| row.id == summary.id)
                );
                summary
            };
            assert!(!shell.allow_window_close(window, cx));
            (summary, install)
        })
        .unwrap();
    let id = summary.id;
    b.update(cx, |shell, _, cx| {
        assert!(!shell.session_available_here(id, cx));
        assert!(shell.add_loaded(summary.clone(), cx).is_none());
        assert!(!shell.session_summaries.contains_key(&id));
    })
    .unwrap();
    let worktree = format!("{}/checkout", summary.cwd);
    a.update(cx, |shell, _, cx| {
        let view = shell.install_prepared(summary, &install, cx).unwrap();
        view.update(cx, |view, cx| {
            view.seed_new_settings(
                vorkspace_core::agent::Settings {
                    worktree_cwd: Some(worktree.clone()),
                    ..Default::default()
                },
                cx,
            )
        });
        shell.publish_prepared(&view, Some(&mut install), cx);
    })
    .unwrap();
    drop(install);
    cx.run_until_parked();
    b.update(cx, |shell, _, cx| {
        assert!(shell.session_summaries.contains_key(&id));
        assert_eq!(shell.session_worktrees.get(&id), Some(&Some(worktree)));
        assert!(shell.ensure_session(id, cx).is_none());
    })
    .unwrap();
}

#[gpui::test]
fn a_peer_cannot_claim_a_cold_session_until_its_archive_commits(cx: &mut TestAppContext) {
    let (a, b, _, _dir) = two_windows(cx);
    let summary = a
        .update(cx, |shell, _, _| {
            locked(&shell.store)
                .unwrap()
                .create("codex", &shell.workspace.project)
                .unwrap()
        })
        .unwrap();
    let id = summary.id;
    cx.update(|cx| {
        a.update(cx, |shell, _, cx| {
            shell.session_summaries.insert(id, summary.clone());
            shell.toggle_archive(id, cx);
        })
        .unwrap();
        b.update(cx, |shell, _, cx| {
            shell.session_summaries.insert(id, summary);
            assert!(shell.ensure_session(id, cx).is_none());
        })
        .unwrap();
    });
    cx.run_until_parked();
    b.update(cx, |shell, _, _| {
        assert!(shell.session_summaries[&id].archived)
    })
    .unwrap();
}

#[gpui::test]
fn scheduler_handoff_keeps_new_connector_retries_and_the_durable_snapshot(cx: &mut TestAppContext) {
    let (a, b, _, _dir) = two_windows(cx);
    let item = vorkspace_core::inbox::Item {
        provider: "github".into(),
        kind: "issue".into(),
        id: "event".into(),
        updated_at: "2026-11-09T12:00:00Z".into(),
        ..Default::default()
    };
    let newer = vorkspace_core::inbox::Item {
        updated_at: "2026-11-09T12:01:00Z".into(),
        ..item.clone()
    };
    cx.update(|cx| {
        a.update(cx, |shell, _, cx| {
            assert!(shell.holds_scheduler(cx));
            shell.app_core.as_ref().unwrap().update(cx, |core, cx| {
                core.queue_automation_events(vec![item.clone()], cx);
            });
        })
        .unwrap();
        b.update(cx, |shell, _, cx| {
            assert!(!shell.holds_scheduler(cx));
            shell.app_core.as_ref().unwrap().update(cx, |core, cx| {
                core.queue_automation_events(vec![newer.clone()], cx);
                core.settle_automation_events(vec![item], Vec::new(), cx);
            });
        })
        .unwrap();
        a.update(cx, |_, window, _| window.remove_window()).unwrap();
    });
    cx.run_until_parked();
    b.update(cx, |shell, _, cx| {
        assert!(shell.holds_scheduler(cx));
        assert_eq!(
            shell
                .app_core
                .as_ref()
                .unwrap()
                .read(cx)
                .automation_events(),
            std::slice::from_ref(&newer)
        );
        let raw = locked(&shell.store)
            .unwrap()
            .setting(AUTOMATION_EVENTS_KEY)
            .unwrap()
            .unwrap();
        let saved: Vec<vorkspace_core::inbox::Item> = serde_json::from_str(&raw).unwrap();
        assert_eq!(saved, [newer]);
    })
    .unwrap();
}

#[gpui::test]
fn a_project_pending_deletion_cannot_be_selected_in_a_peer(cx: &mut TestAppContext) {
    let (a, b, _, dir) = two_windows(cx);
    let path = dir.path().join("deleting");
    std::fs::create_dir(&path).unwrap();
    let path = path.to_string_lossy().into_owned();
    let admissions = a
        .update(cx, |shell, _, cx| {
            shell
                .app_core
                .as_ref()
                .unwrap()
                .read(cx)
                .project_deletions
                .clone()
        })
        .unwrap();
    let key = vorkspace_core::files::path_key(&path);
    admissions.lock().unwrap().insert(key.clone());
    b.update(cx, |shell, window, cx| {
        let original = shell.workspace.project.clone();
        shell.select_project(path.clone(), window, cx);
        assert_eq!(shell.workspace.project, original);
        assert!(
            shell
                .error
                .as_ref()
                .is_some_and(|error| error.contains("being deleted"))
        );
    })
    .unwrap();
    admissions.lock().unwrap().remove(&key);
    b.update(cx, |shell, window, cx| {
        shell.select_project(path.clone(), window, cx);
        assert_eq!(shell.workspace.project, path);
    })
    .unwrap();
}

#[gpui::test]
fn committed_folder_moves_are_applied_to_the_latest_groups_and_shared(cx: &mut TestAppContext) {
    let (a, b, first, _dir) = two_windows(cx);
    let (project, second) = a
        .update(cx, |shell, _, _| {
            let db = locked(&shell.store).unwrap();
            let project = shell.workspace.project.clone();
            let second = db.create("codex", &project).unwrap().id;
            (project, second)
        })
        .unwrap();
    cx.update(|cx| {
        for (window, id, label) in [(a, first, "First"), (b, second, "Second")] {
            window
                .update(cx, |shell, _, cx| {
                    locked(&shell.store)
                        .unwrap()
                        .move_sessions_to_folder(
                            &project,
                            &[id],
                            vorkspace_core::session_groups::FolderDestination::New(label.into()),
                        )
                        .unwrap();
                    shell.publish_groups(project.clone(), cx);
                })
                .unwrap();
        }
    });
    for window in [a, b] {
        window
            .update(cx, |shell, _, _| {
                assert_eq!(
                    shell.groups[&project].folder_of(first).unwrap().name,
                    "First"
                );
                assert_eq!(
                    shell.groups[&project].folder_of(second).unwrap().name,
                    "Second"
                );
            })
            .unwrap();
    }
}

#[gpui::test]
fn a_session_keeps_one_live_owner_and_peers_follow_its_catalog(cx: &mut TestAppContext) {
    let (a, b, _, _dir) = two_windows(cx);
    let id = a
        .update(cx, |shell, window, cx| {
            let summary = locked(&shell.store)
                .unwrap()
                .create("codex", &shell.workspace.project)
                .unwrap();
            let id = summary.id;
            shell.add(summary, cx);
            shell.open(id, window, cx);
            id
        })
        .unwrap();
    cx.run_until_parked();
    b.update(cx, |shell, window, cx| {
        assert!(shell.session_summaries.contains_key(&id));
        assert!(!shell.sessions.contains_key(&id));
        let previous = shell.workspace.active;
        shell.open(id, window, cx);
        assert_eq!(shell.workspace.active, previous);
        shell.delete_session(id, window, cx);
        assert!(shell.error.as_deref().unwrap().contains("another window"));
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.active_window()), Some(a.into()));
    a.update(cx, |shell, _, cx| {
        assert_eq!(shell.workspace.active, Some(id));
        locked(&shell.store)
            .unwrap()
            .title(id, "Shared session")
            .unwrap();
        shell.sessions[&id].update(cx, |view, cx| view.rename("Shared session".into(), cx));
    })
    .unwrap();
    cx.run_until_parked();
    b.update(cx, |shell, _, _| {
        assert_eq!(shell.session_summaries[&id].title, "Shared session");
    })
    .unwrap();

    a.update(cx, |shell, window, cx| {
        shell.request_close_window(window, cx)
    })
    .unwrap();
    cx.run_until_parked();
    b.update(cx, |shell, window, cx| {
        shell.open(id, window, cx);
        assert!(shell.sessions.contains_key(&id));
        assert_eq!(shell.workspace.active, Some(id));
    })
    .unwrap();
}

#[gpui::test]
fn quitting_saves_the_latest_vorkspace_definitions(cx: &mut TestAppContext) {
    let (a, _, _, _dir) = two_windows(cx);
    let store = a
        .update(cx, |shell, _, cx| {
            shell.workspace.create_vorkspace("Previous name");
            shell.persist(cx);
            let id = shell.workspace.create_vorkspace("Final name");
            shell
                .workspace
                .vorkspace_mut(&id)
                .unwrap()
                .set_icon_letters("FN");
            shell.persist(cx);
            shell.store.clone()
        })
        .unwrap();
    cx.quit();
    let saved = locked(&store).unwrap().setting(STORE_KEY).unwrap().unwrap();
    let saved: Saved = serde_json::from_str(&saved).unwrap();
    assert!(
        saved
            .vorkspaces
            .iter()
            .any(|v| v.name == "Final name" && v.icon_letters() == "FN")
    );
}

#[gpui::test]
fn closing_a_deleted_vorkspace_background_returns_to_manage(cx: &mut TestAppContext) {
    let (a, b, _, _dir) = two_windows(cx);
    let deleted = b
        .update(cx, |shell, _, cx| {
            let id = shell.workspace.create_vorkspace("Background");
            shell.persist(cx);
            id
        })
        .unwrap();
    a.update(cx, |shell, window, cx| {
        shell.open_vorkspace_background(deleted.clone(), window, cx);
        assert!(shell.background_dialog.is_some());
    })
    .unwrap();
    b.update(cx, |shell, _, cx| {
        shell.workspace.delete_vorkspace(&deleted);
        shell.persist(cx);
    })
    .unwrap();
    a.update(cx, |shell, window, cx| {
        shell.close_background_dialog(window, cx);
        assert!(shell.background_dialog.is_none());
        assert_eq!(
            shell.vorkspace_dialog.as_ref().unwrap().editing,
            shell.workspace.active_vorkspace
        );
    })
    .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(a.into(), cx);
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("vorkspace-dialog").is_some());
}

#[gpui::test]
fn a_session_is_live_in_one_window_at_a_time(cx: &mut TestAppContext) {
    let (a, b, id, _dir) = two_windows(cx);
    // Whichever window opened it first keeps it; the other only lists it.
    let a_has = a
        .update(cx, |shell, _, cx| shell.ensure_session(id, cx).is_some())
        .unwrap();
    let b_has = b
        .update(cx, |shell, _, cx| shell.ensure_session(id, cx).is_some())
        .unwrap();
    assert!(a_has ^ b_has, "exactly one window holds the live session");
    let (holder, other) = if a_has { (a, b) } else { (b, a) };
    other
        .update(cx, |shell, _, _| {
            assert!(shell.session_summaries.contains_key(&id))
        })
        .unwrap();
    // Once the holder lets it go, the other window can take it.
    holder
        .update(cx, |shell, _, cx| {
            shell.release_live_session(id, cx);
        })
        .unwrap();
    assert!(
        other
            .update(cx, |shell, _, cx| shell.ensure_session(id, cx).is_some())
            .unwrap()
    );
}

#[gpui::test]
fn agent_calls_go_to_the_window_with_their_session(cx: &mut TestAppContext) {
    let (a, b, id, _dir) = two_windows(cx);
    // Make sure B is the window holding the session.
    a.update(cx, |shell, _, cx| {
        shell.release_live_session(id, cx);
    })
    .unwrap();
    assert!(
        b.update(cx, |shell, _, cx| shell.ensure_session(id, cx).is_some())
            .unwrap()
    );
    let (a_root, b_root) = (
        a.update(cx, |_, _, cx| cx.entity()).unwrap(),
        b.update(cx, |_, _, cx| cx.entity()).unwrap(),
    );
    let core = a
        .update(cx, |shell, _, _| shell.app_core.clone().unwrap())
        .unwrap();
    cx.update(|cx| {
        let core = core.read(cx);
        assert_eq!(core.route(id, cx), Some(b_root.clone()));
        // A call from no known session goes to the oldest open window.
        assert_eq!(core.route(-42, cx), Some(a_root.clone()));
    });
}

#[gpui::test]
fn shared_vorkspace_changes_reconcile_other_windows_project_and_dialog(cx: &mut TestAppContext) {
    let (a, b, _, dir) = two_windows(cx);
    let second_project = dir.path().join("second").to_string_lossy().into_owned();
    std::fs::create_dir_all(&second_project).unwrap();
    let side_id = a
        .update(cx, |shell, _, cx| {
            let main = shell.workspace.active_vorkspace.clone();
            shell
                .workspace
                .add_project_to_vorkspace(&main, &second_project);
            let side = shell.workspace.create_vorkspace("Side");
            shell.persist(cx);
            side
        })
        .unwrap();
    cx.run_until_parked();
    b.update(cx, |shell, window, cx| {
        shell.select_project(second_project.clone(), window, cx);
        shell.open_vorkspace_dialog(Some(side_id.clone()), window, cx);
        assert_eq!(shell.workspace.project, second_project);
        assert_eq!(
            shell.vorkspace_dialog.as_ref().map(|d| d.editing.as_str()),
            Some(side_id.as_str())
        );
    })
    .unwrap();
    a.update(cx, |shell, _, cx| {
        let main = shell.workspace.active_vorkspace.clone();
        shell
            .workspace
            .remove_project_from_vorkspace(&main, &second_project);
        shell.workspace.delete_vorkspace(&side_id);
        shell.persist(cx);
    })
    .unwrap();
    cx.run_until_parked();
    b.update(cx, |shell, _, cx| {
        assert_ne!(shell.workspace.project, second_project);
        let dialog = shell.vorkspace_dialog.as_ref().unwrap();
        let active = shell.workspace.active_vorkspace().unwrap();
        assert_eq!(dialog.editing, active.id);
        assert_eq!(dialog.name.read(cx).content.as_ref(), active.name);
        dialog.name.update(cx, |field, cx| {
            field.set_text("Renamed after deletion".into(), cx)
        });
        dialog
            .icon_letters
            .update(cx, |field, cx| field.set_text("RD".into(), cx));
    })
    .unwrap();
    cx.run_until_parked();
    a.update(cx, |shell, _, _| {
        let active = shell.workspace.active_vorkspace().unwrap();
        assert_eq!(active.name, "Renamed after deletion");
        assert_eq!(active.icon_letters(), "RD");
        let durable = locked(&shell.store)
            .unwrap()
            .setting(STORE_KEY)
            .unwrap()
            .unwrap();
        let saved: Saved = serde_json::from_str(&durable).unwrap();
        assert!(saved.vorkspaces.contains(active));
    })
    .unwrap();
}
