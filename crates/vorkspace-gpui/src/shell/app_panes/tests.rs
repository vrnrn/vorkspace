use super::*;
use crate::shell::tests::window;
use gpui::TestAppContext;

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
