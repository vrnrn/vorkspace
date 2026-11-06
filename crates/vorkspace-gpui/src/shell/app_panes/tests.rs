use super::*;
use crate::shell::tests::window;
use gpui::TestAppContext;

#[gpui::test]
fn remote_session_restores_beside_app_panes_and_uses_the_same_stack_limit(cx: &mut TestAppContext) {
    use serde_json::json;
    use vorkspace_core::store::Store;

    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("native.sqlite")).unwrap();
    let project = store
        .remember_remote_project("offline-layout", &json!({"id":"repo", "cwd":"/host/repo"}))
        .unwrap();
    let session = store
        .register_remote_summary(
            &project,
            &json!({"id":"chat","harness":"codex","title":"Remote chat","cwd":"/host/repo"}),
        )
        .unwrap();
    let id = session.id;
    assert!(id < 0);
    let cwd = project.key;
    let mut workspace = Workspace::default();
    workspace.add_project(&cwd);
    workspace.open(id);
    let notes = workspace
        .app_pane(AppPane::App(AppView::Notes), cwd.clone(), cwd.clone())
        .unwrap();
    let search = workspace
        .app_pane(AppPane::Search, cwd.clone(), cwd.clone())
        .unwrap();
    workspace.split(notes, SplitDir::Down);
    workspace.split(search, SplitDir::Down);
    workspace.open(id);
    workspace.project = "/local".into();
    workspace
        .scope_focus
        .insert(format!("{cwd}\0{cwd}"), TitleTab::Workspace(id));
    let saved = serde_json::to_string(&workspace).unwrap();
    let store = Arc::new(Mutex::new(store));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let shell = Shell::new(store, vec![session], "/local".into(), Some(saved), cx);
                if let Some(chat) = shell.sessions.get(&id) {
                    // This fixture owns the pane identity and has no host connection.
                    chat.update(cx, |chat, _| chat.mark_remote_snapshot_loaded_for_test());
                }
                shell
            })
        })
        .unwrap()
    });
    cx.run_until_parked();
    window
        .update(cx, |shell, w, cx| {
            assert_eq!(shell.workspace.project, cwd);
            assert_eq!(shell.workspace.active, Some(id));
            assert_eq!(shell.visible_tabs(cx), [TitleTab::Workspace(id)]);
            assert!(shell.has_session_panes());
            assert_eq!(shell.pane_title(id, cx), "Remote chat");
            assert_eq!(shell.workspace.session_for_pane(id), Some(id));
            assert_eq!(shell.workspace.session_for_pane(notes), None);
            assert_eq!(shell.workspace.session_for_pane(search), None);
            assert!(shell.sessions.contains_key(&id));
            assert!(!shell.sessions.contains_key(&notes));
            assert!(!shell.sessions.contains_key(&search));
            assert_eq!(shell.workspace.layout(id).leaf_ids(), [id, notes, search]);
            assert!(!shell.split_allowed(
                &shell.workspace.layout(id),
                id,
                SplitDir::Down,
                i64::MAX
            ));
            assert!(shell.placement_allowed(DragSource::Review(i64::MAX), id, Edge::Bottom));
            shell.bot_manager().update(cx, |manager, cx| {
                manager.create_named(vec![cwd.clone()], w, cx);
                manager.dismiss_settings(w, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, w, cx| {
            let caller = shell
                .bot_manager()
                .read(cx)
                .chat()
                .unwrap()
                .read(cx)
                .summary
                .id;
            let listed = shell
                .app_workspace_action(caller, &cwd, "workspace.list", &json!({}), w, cx)
                .unwrap();
            let tab = listed["tabs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|tab| tab["tabId"] == json!(id))
                .unwrap();
            assert_eq!(tab["panes"][0]["kind"], "session");
            assert_eq!(tab["panes"][0]["sessionId"], json!(id));
            let opened = shell
                .app_workspace_action(
                    caller,
                    &cwd,
                    "workspace.open",
                    &json!({"kind":"session","sessionId":id.to_string()}),
                    w,
                    cx,
                )
                .unwrap();
            assert_eq!(opened["paneId"], json!(id));
            assert_eq!(shell.workspace.active, Some(id));
            assert_eq!(
                shell
                    .app_workspace_action(
                        caller,
                        &cwd,
                        "workspace.open",
                        &json!({"kind":"session","sessionId":notes}),
                        w,
                        cx,
                    )
                    .unwrap_err(),
                "Session is unavailable"
            );
        })
        .unwrap();
}

#[gpui::test]
fn linked_github_activity_stays_with_each_visible_bot_pane(cx: &mut TestAppContext) {
    use crate::notifications::github_activity::Card;
    use vorkspace_core::session_groups::WorkItem;

    vorkspace_ui::clock::use_test_clock(cx);
    let (window, _, chat) = window(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1500.), px(1000.)));
    let mut bots = Vec::new();
    for number in [41, 42] {
        window
            .update(visual, |shell, w, cx| {
                shell.bot_manager().update(cx, |manager, cx| {
                    manager.create_named(vec!["/work/vorkspace".into()], w, cx);
                    manager.dismiss_settings(w, cx);
                });
            })
            .unwrap();
        visual.run_until_parked();
        let (pane, session) = window
            .update(visual, |shell, _, cx| {
                let pane = shell.workspace.active.unwrap();
                let session = shell
                    .bot_manager()
                    .read(cx)
                    .chat()
                    .unwrap()
                    .read(cx)
                    .summary
                    .id;
                let card = Card {
                    session,
                    item: WorkItem {
                        kind: "pr".into(),
                        repo: "example/project".into(),
                        number,
                        url: format!("https://github.com/example/project/pull/{number}"),
                    },
                    title: format!("Review feedback {number}"),
                    state: "open".into(),
                    updated_at: vorkspace_core::time::now_ms() as i64,
                    entries: vec![],
                    details_error: false,
                    truncated: false,
                };
                shell.notification_state.github.update(cx, |feed, _| {
                    feed.cards.insert(session, card);
                });
                cx.notify();
                (pane, session)
            })
            .unwrap();
        visual.run_until_parked();
        let notice =
            Box::leak(format!("linked-work-item-update-notice-{session}").into_boxed_str());
        assert!(
            visual.debug_bounds(notice).is_some(),
            "the lone Bot hosts its card"
        );
        bots.push((pane, session, notice as &'static str));
    }
    let [
        (first, first_session, first_notice),
        (second, _, second_notice),
    ] = bots[..]
    else {
        unreachable!()
    };
    assert!(visual.debug_bounds(first_notice).is_none());

    window
        .update(visual, |shell, w, cx| {
            shell.workspace.place(first, chat, Edge::Left);
            shell.open(chat, w, cx);
        })
        .unwrap();
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(first_notice).is_some(),
        "the mixed tab shows the Bot's card"
    );
    assert!(visual.debug_bounds(second_notice).is_none());

    window
        .update(visual, |shell, w, cx| {
            shell.workspace.place(second, first, Edge::Bottom);
            shell.open(second, w, cx);
        })
        .unwrap();
    visual.run_until_parked();
    let first_bounds = visual.debug_bounds(first_notice).unwrap();
    let second_bounds = visual.debug_bounds(second_notice).unwrap();
    assert!(
        first_bounds.bottom() < second_bounds.top(),
        "each card stays inside its Bot pane"
    );
    let dismiss = Box::leak(format!("dismiss-linked-activity-{first_session}").into_boxed_str());
    let dismiss = visual.debug_bounds(dismiss).unwrap().center();
    visual.simulate_click(dismiss, Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds(first_notice).is_none());
    assert!(visual.debug_bounds(second_notice).is_some());
}

#[gpui::test]
fn reopening_a_mixed_app_layout_restores_scope_and_cli_rejects_foreign_siblings(
    cx: &mut TestAppContext,
) {
    let (window, other, session) = window(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.sessions[&other].update(cx, |view, cx| view.enable_mode("operator", cx));
            shell.open(other, w, cx);
            shell.open_notes(w, cx);
            let notes = shell.workspace.active.unwrap();
            shell.open(session, w, cx);
            let project = shell.workspace.project.clone();
            let worktree = format!("{project}/tree");
            shell.worktree_focus.insert(
                project.clone(),
                crate::git_tools::worktree_switcher::WorktreeFocus {
                    path: worktree.clone(),
                    branch: None,
                },
            );
            shell
                .workspace
                .tab_workspaces
                .insert(session, worktree.clone());
            shell.workspace.place(notes, session, Edge::Right);
            shell.select_project("/work/another-project".into(), w, cx);
            let before = shell.workspace.active;
            let result = shell.app_workspace_action(
                other,
                "/work/another-project",
                "workspace.open",
                &serde_json::json!({"kind":"notes"}),
                w,
                cx,
            );
            assert!(result.is_err());
            assert_eq!(shell.workspace.project, "/work/another-project");
            assert_eq!(shell.workspace.active, before);
            shell.worktree_focus.remove(&project);
            shell.open_notes(w, cx);
            assert_eq!(shell.workspace.active, Some(notes));
            assert_eq!(shell.workspace.project, project);
            assert_eq!(shell.working_copy(), worktree);
            assert_eq!(shell.session_working_copy(notes, cx), worktree);
            shell.record_worktree_session(notes, cx);
            assert_eq!(shell.workspace.tab_workspaces[&session], worktree);
            assert!(
                shell
                    .visible_tabs(cx)
                    .contains(&TitleTab::Workspace(session))
            );
            assert_eq!(shell.workspace.layout(session).leaf_ids(), [session, notes]);
        })
        .unwrap();
}

#[test]
#[ignore = "Requires Metal and VORKSPACE_PAGES_SCREEN_DIR"]
fn native_mixed_workspace_capture() {
    let output = std::path::PathBuf::from(std::env::var_os("VORKSPACE_PAGES_SCREEN_DIR").unwrap());
    assert!(output.is_absolute() && output.is_dir());
    let mut cx = gpui::HeadlessAppContext::with_platform(
        Arc::new(gpui_apple::AppleTextSystem::new()),
        Arc::new(crate::assets::Assets),
        || {
            Ok(Some(Box::new(
                gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
            )))
        },
    );
    vorkspace_ui::clock::use_test_clock(&cx);
    for (label, width) in [("wide", 1440.), ("compact", 1080.)] {
        let store = Arc::new(Mutex::new(
            vorkspace_core::store::Store::open(std::path::Path::new(":memory:")).unwrap(),
        ));
        let session = locked(&store)
            .unwrap()
            .create("codex", "/work/release-platform")
            .unwrap();
        let session_id = session.id;
        for (id, title, body) in [
            (
                "plan",
                "Release plan",
                "## Before publishing\n\n- Confirm the signed build.\n- Review the upgrade path.\n- Share the preview with the team.",
            ),
            (
                "decisions",
                "Design decisions",
                "## Workspace behavior\n\nKeep notes beside the conversation. Each note retains its own draft and scroll position.",
            ),
        ] {
            locked(&store)
                .unwrap()
                .upsert_note(&vorkspace_core::notes::NoteUpsert {
                    id: id.into(),
                    title: title.into(),
                    body: body.into(),
                    source_cwd: Some("/work/release-platform".into()),
                    ..Default::default()
                })
                .unwrap();
        }
        let mut state = Workspace::default();
        state.add_project("/work/release-platform");
        state.project = "/work/release-platform".into();
        state.sidebar_visible = false;
        state.open(session_id);
        let saved = serde_json::to_string(&state).unwrap();
        let window = cx
            .open_window(gpui::size(px(width), px(900.)), |_, cx| {
                cx.new(|cx| {
                    Shell::new(
                        store,
                        vec![session],
                        "/work/release-platform".into(),
                        Some(saved),
                        cx,
                    )
                })
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(&mut cx, |shell, w, cx| {
                shell.open_note_tab("plan".into(), w, cx);
                let a = shell.workspace.active.unwrap();
                shell.open_note_tab("decisions".into(), w, cx);
                let b = shell.workspace.active.unwrap();
                shell.workspace.place(a, session_id, Edge::Right);
                shell.workspace.place(b, a, Edge::Bottom);
                shell.open(a, w, cx);
                shell.workspace.sidebar_visible = false;
                shell.sidebar_fraction = 0.;
                shell.invalidate_chrome(cx);
            })
            .unwrap();
        cx.run_until_parked();
        vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
        cx.capture_screenshot(window.into())
            .unwrap()
            .save(output.join(format!("workspace-mixed-{label}.png")))
            .unwrap();
        cx.update_window(window.into(), |_, window, _| window.remove_window())
            .unwrap();
    }
}

#[gpui::test]
fn independent_notes_tile_beside_sessions_and_close_the_originating_pane(cx: &mut TestAppContext) {
    let (window, _, session) = window(cx);
    let (a, b, first, second) = window
        .update(cx, |shell, w, cx| {
            for id in ["design", "release"] {
                locked(&shell.store)
                    .unwrap()
                    .upsert_note(&vorkspace_core::notes::NoteUpsert {
                        id: id.into(),
                        title: id.into(),
                        body: format!("# {id}\nIndependent note content"),
                        ..Default::default()
                    })
                    .unwrap();
            }
            shell.open_note_tab("design".into(), w, cx);
            let a = shell.workspace.active.unwrap();
            let first = shell.note_tabs[&a].clone();
            shell.open_note_tab("release".into(), w, cx);
            let b = shell.workspace.active.unwrap();
            let second = shell.note_tabs[&b].clone();
            assert_ne!(a, b);
            assert_ne!(first, second);
            shell.workspace.place(a, session, Edge::Right);
            shell.workspace.place(b, a, Edge::Bottom);
            shell.open(a, w, cx);
            (a, b, first, second)
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, w, cx| {
            assert_eq!(
                shell.workspace.layout(session).leaf_ids(),
                vec![session, a, b]
            );
            assert!(shell.has_session_panes());
            assert_eq!(shell.note_tabs[&a], first);
            assert_eq!(shell.note_tabs[&b], second);
            assert_eq!(shell.tab_copy(session, cx).headline, "design");
            shell.open_note_tab("release".into(), w, cx);
            assert!(
                shell.has_session_panes(),
                "project tools follow the layout, not note focus"
            );
            assert_eq!(shell.workspace.active, Some(b));
            assert_eq!(
                shell.note_tabs.len(),
                2,
                "opening an existing note focuses its editor"
            );
            first.update(cx, |_, cx| cx.emit(crate::notes::Event::Close));
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert!(
                shell.workspace.tab_of(a).is_none(),
                "close belongs to the emitting note"
            );
            assert_eq!(shell.workspace.active, Some(b));
            assert_eq!(shell.note_tabs[&b], second);
            assert!(shell.workspace.tab_of(session).is_some());
            assert_eq!(shell.tab_copy(session, cx).headline, "release");
        })
        .unwrap();
}

#[gpui::test]
fn disabling_a_feature_keeps_its_session_sibling_and_draft(cx: &mut TestAppContext) {
    let (window, _, session) = window(cx);
    window
        .update(cx, |shell, w, cx| {
            locked(&shell.store)
                .unwrap()
                .upsert_note(&vorkspace_core::notes::NoteUpsert {
                    id: "retained".into(),
                    title: "Retained".into(),
                    body: "Keep this note".into(),
                    ..Default::default()
                })
                .unwrap();
            shell.open_note_tab("retained".into(), w, cx);
            let note = shell.workspace.active.unwrap();
            let editor = shell.note_tabs[&note].clone();
            shell.workspace.place(note, session, Edge::Right);
            shell.workspace.preferences.notes = false;
            shell.open(session, w, cx);
            assert!(
                shell
                    .visible_tabs(cx)
                    .contains(&TitleTab::Workspace(session))
            );
            assert_eq!(shell.note_tabs[&note], editor);
            shell.workspace.preferences.notes = true;
            shell.open(note, w, cx);
            assert_eq!(shell.note_tabs[&note], editor);
        })
        .unwrap();
}

#[gpui::test]
fn standalone_channels_restore_as_active_without_replacing_the_project_tab(
    cx: &mut TestAppContext,
) {
    let (window, _, session) = window(cx);
    let channels = window
        .update(cx, |shell, w, cx| {
            shell.open(session, w, cx);
            shell.workspace.preferences.channels = true;
            shell.apply_preferences(cx);
            shell.open_classic_channels(w, cx);
            assert_eq!(
                shell.workspace.scope_focus.get(&shell.tab_scope()),
                Some(&TitleTab::Workspace(session))
            );
            shell.workspace.active.unwrap()
        })
        .unwrap();
    cx.run_until_parked();
    let (store, summaries, project, saved) = window
        .update(cx, |shell, w, _| {
            let store = locked(&shell.store).unwrap();
            let snapshot = (
                shell.store.clone(),
                store.sessions().unwrap(),
                shell.workspace.project.clone(),
                store.ui_state().unwrap(),
            );
            w.remove_window();
            snapshot
        })
        .unwrap();
    let restored = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| Shell::new(store, summaries, project.clone(), saved, cx))
        })
        .unwrap()
    });
    let visual = &mut gpui::VisualTestContext::from_window(restored.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("channel-surface").is_some());
    restored
        .update(visual, |shell, w, cx| {
            assert_eq!(shell.workspace.active, Some(channels));
            shell.select_project(project, w, cx);
            assert_eq!(shell.workspace.active, Some(session));
            assert_eq!(shell.workspace.tab_of(channels), Some(channels));
        })
        .unwrap();
    visual.run_until_parked();
    assert!(visual.debug_bounds("channel-surface").is_none());
}

#[gpui::test]
fn disabled_channels_stay_unavailable_in_a_restored_mixed_tab(cx: &mut TestAppContext) {
    let (window, _, session) = window(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    let channels = window
        .update(visual, |shell, w, cx| {
            shell.workspace.preferences.channels = true;
            shell.apply_preferences(cx);
            shell.open_classic_channels(w, cx);
            let channels = shell.workspace.active.unwrap();
            shell.workspace.place(channels, session, Edge::Right);
            shell.open(channels, w, cx);
            channels
        })
        .unwrap();
    visual.run_until_parked();
    assert!(visual.debug_bounds("channel-surface").is_some());
    window
        .update(visual, |shell, _, cx| {
            shell.workspace.preferences.channels = false;
            shell.apply_preferences(cx);
            shell.persist(cx);
        })
        .unwrap();
    visual.run_until_parked();
    assert!(visual.debug_bounds("channel-surface").is_none());
    let (store, summaries, project, saved) = window
        .update(visual, |shell, _, _| {
            let store = locked(&shell.store).unwrap();
            (
                shell.store.clone(),
                store.sessions().unwrap(),
                shell.workspace.project.clone(),
                store.ui_state().unwrap(),
            )
        })
        .unwrap();
    let restored = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| Shell::new(store, summaries, project, saved, cx))
        })
        .unwrap()
    });
    let visual = &mut gpui::VisualTestContext::from_window(restored.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("channel-surface").is_none());
    restored
        .update(visual, |shell, w, cx| {
            assert!(shell.workspace.preferences.bots);
            assert!(!shell.workspace.preferences.channels);
            assert_eq!(
                shell.workspace.layout(session).leaf_ids(),
                [session, channels]
            );
            assert!(!shell.bot_manager().read(cx).classic_channels_open());
            shell.workspace.preferences.channels = true;
            shell.apply_preferences(cx);
            shell.open(channels, w, cx);
        })
        .unwrap();
    visual.run_until_parked();
    assert!(visual.debug_bounds("channel-surface").is_some());
}

#[gpui::test]
fn missing_bot_session_does_not_move_an_unrelated_active_pane(cx: &mut TestAppContext) {
    let (window, _, session) = window(cx);
    window
        .update(cx, |shell, w, cx| {
            let bot = shell
                .workspace
                .app_pane(
                    AppPane::Bot("missing".into()),
                    shell.workspace.project.clone(),
                    shell.workspace.project.clone(),
                )
                .unwrap();
            shell.workspace.open(bot);
            shell.open(session, w, cx);
            shell.open_bot_workspace_pane(
                crate::bots::OpenWorkspacePane::Session {
                    bot: "missing".into(),
                    session: i64::MAX,
                },
                w,
                cx,
            );
            assert_eq!(shell.workspace.active, Some(session));
            assert_ne!(shell.workspace.tab_of(session), shell.workspace.tab_of(bot));
        })
        .unwrap();
}

#[gpui::test]
fn mixed_panes_use_existing_width_and_vertical_limits(cx: &mut TestAppContext) {
    let (window, _, session) = window(cx);
    window
        .update(cx, |shell, _, _| {
            shell.viewport_width = 2000.;
            shell.workspace.sidebar_visible = false;
            let mut leaves = vec![session];
            for name in ["a", "b", "c"] {
                leaves.push(
                    shell
                        .workspace
                        .app_pane(
                            AppPane::Note(name.into()),
                            shell.workspace.project.clone(),
                            shell.workspace.project.clone(),
                        )
                        .unwrap(),
                );
            }
            let one = PaneNode::Leaf { id: session };
            let two = one.split_pane(session, SplitDir::Down, leaves[1]);
            let three = two.split_pane(leaves[1], SplitDir::Down, leaves[2]);
            let four = three.split_pane(leaves[2], SplitDir::Down, leaves[3]);
            assert!(shell.pane_layout_allowed(&one, &two));
            assert!(shell.pane_layout_allowed(&two, &three));
            assert!(!shell.pane_layout_allowed(&three, &four));
            assert!(
                shell.pane_layout_allowed(&four, &three),
                "an old oversized layout can improve"
            );
            shell.viewport_width = 700.;
            assert!(!shell.split_allowed(&one, session, SplitDir::Right, leaves[1]));
        })
        .unwrap();
}

#[gpui::test]
fn switching_focus_keeps_bot_and_terminal_siblings_alive(cx: &mut TestAppContext) {
    let (window, _, session) = window(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.bot_manager().update(cx, |m, cx| {
                m.create_named(vec!["/work/vorkspace".into()], w, cx)
            });
        })
        .unwrap();
    cx.run_until_parked();
    let (bot_pane, terminal_pane, terminal, chat) = window
        .update(cx, |shell, w, cx| {
            let bot_pane = shell.workspace.active.unwrap();
            let bot = shell.workspace_bot_context().unwrap();
            let chat = shell
                .bot_manager()
                .update(cx, |m, cx| m.chat_for_action(&bot, cx))
                .unwrap();
            chat.update(cx, |chat, cx| {
                chat.input.update(cx, |input, cx| {
                    input.set_text("Unsent Bot draft".into(), cx)
                })
            });
            shell.open_terminal_tab(w, cx);
            let terminal_pane = shell.workspace.active.unwrap();
            let id = shell.active_terminal_tab.unwrap();
            let terminal = shell.terminal_tabs[&id].clone();
            shell.workspace.place(bot_pane, session, Edge::Right);
            shell.workspace.place(terminal_pane, bot_pane, Edge::Bottom);
            shell.open(session, w, cx);
            (bot_pane, terminal_pane, terminal, chat)
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, w, cx| {
            assert!(
                !shell.bot_active(cx),
                "the focused session owns keyboard actions"
            );
            assert!(
                shell.workspace_bot_context().is_some(),
                "the Bot's browse panel stays available"
            );
            assert!(terminal.read(cx).visible);
            assert_eq!(chat.read(cx).input.read(cx).content, "Unsent Bot draft");
            shell.open(terminal_pane, w, cx);
            assert_eq!(
                shell.workspace.tab_of(terminal_pane),
                shell.workspace.tab_of(bot_pane)
            );
            assert!(shell.workspace_bot_context().is_some());
            shell.close_pane(terminal_pane, w, cx);
            assert!(shell.workspace.tab_of(bot_pane).is_some());
            assert_eq!(chat.read(cx).input.read(cx).content, "Unsent Bot draft");
            shell.open(session, w, cx);
            shell.close_pane(bot_pane, w, cx);
            assert_eq!(shell.workspace.active, Some(session));
            assert!(
                shell.workspace_bot_context().is_none(),
                "closing an unfocused Bot clears its browser panel"
            );
            assert_eq!(chat.read(cx).input.read(cx).content, "Unsent Bot draft");
        })
        .unwrap();
}
