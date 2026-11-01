
use super::*;
use gpui::{TestAppContext, VisualTestContext};

#[gpui::test]
fn history_conflict_reaches_top_right_alert_and_preserves_composer(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history.db");
    let mut store = vorkspace_core::store::Store::open_vorkspace(&path).unwrap();
    let project = directory.path().to_string_lossy().to_string();
    let mut summary = store.create("codex", &project).unwrap();
    let id = summary.id;
    let original = store.persisted_session_id(id).unwrap();
    let mut history = vorkspace_core::transcript::Transcript::default();
    history.begin("user");
    history.append("Already saved history");
    store.write_rows(id, 0, &history.rows).unwrap();
    summary.has_messages = true;
    let mut workspace = Workspace::default();
    workspace.add_project(&project);
    workspace.open(id);
    let saved = serde_json::to_string(&workspace).unwrap();
    let store = Arc::new(Mutex::new(store));
    let db = store.clone();
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| Shell::new(db, vec![summary], project, Some(saved), cx))
        })
        .unwrap()
    });
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    window
        .update(visual, |shell, _, cx| {
            shell.sessions[&id].update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_text("Keep this unsent input".into(), cx)
                });
            });
        })
        .unwrap();
    visual.run_until_parked();
    visual.executor().advance_clock(Duration::from_millis(500));
    visual.run_until_parked();
    locked(&store)
        .unwrap()
        .write_rows(id, history.rows.len(), &[])
        .unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE external_schema_change(value TEXT)")
        .unwrap();
    let end = history.rows.len();
    window
        .update(visual, |shell, _, cx| {
            shell.store_write("save session", cx, move |store| {
                store.write_rows(id, end, &[])
            });
        })
        .unwrap();
    visual.run_until_parked();
    let alert = visual
        .debug_bounds("app-error")
        .expect("the storage conflict is the top-right red alert");
    let viewport = window
        .update(visual, |shell, window, cx| {
            assert!(
                shell
                    .error
                    .as_ref()
                    .unwrap()
                    .contains("This conversation changed in another window")
            );
            assert_eq!(
                shell.sessions[&id].read(cx).input.read(cx).content,
                "Keep this unsent input"
            );
            window.viewport_size()
        })
        .unwrap();
    assert_eq!(alert.top(), px(12.));
    assert_eq!(alert.right(), viewport.width - px(12.));
    let close = visual.debug_bounds("dismiss-error").unwrap();
    visual.simulate_click(close.center(), Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("app-error").is_none());
    let reopened = vorkspace_core::store::Store::read_vorkspace(&path).unwrap();
    let id = reopened.resolve_session_id(&original).unwrap();
    assert_eq!(reopened.load(id).unwrap().rows, history.rows);
    assert_eq!(
        reopened.composer_draft(id).unwrap().unwrap().payload["text"],
        "Keep this unsent input"
    );
}

#[gpui::test]
fn error_cards_share_top_right_stack_and_dismiss_independently(cx: &mut TestAppContext) {
    let (window, _, _) = crate::shell::tests::window(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let body = visual.debug_bounds("main-pane-frame").unwrap();
    window
        .update(visual, |shell, _, cx| {
            shell.error = Some("Could not save workspace".into());
            shell
                .notification_state
                .file_error
                .update(cx, |notice, cx| {
                    notice.set_message(Some("Could not rename file".into()), cx);
                });
            cx.notify();
        })
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
    let app = visual.debug_bounds("app-error").unwrap();
    let file = visual.debug_bounds("file-action-error").unwrap();
    let viewport = window.update(visual, |_, w, _| w.viewport_size()).unwrap();
    assert_eq!(app.top(), px(12.));
    assert_eq!(app.right(), viewport.width - px(12.));
    assert_eq!(app.size.width, px(360.));
    assert_eq!(file.size.width, app.size.width);
    assert_eq!(file.right(), app.right());
    assert_eq!(file.top(), app.bottom() + px(8.));
    assert_eq!(visual.debug_bounds("main-pane-frame").unwrap(), body);

    let close = visual.debug_bounds("dismiss-error").unwrap();
    visual.simulate_click(close.center(), Default::default());
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("app-error").is_none());
    assert_eq!(
        visual.debug_bounds("file-action-error").unwrap().top(),
        px(12.)
    );

    visual.simulate_resize(gpui::size(px(300.), px(800.)));
    visual.run_until_parked();
    visual.refresh().unwrap();
    let file = visual.debug_bounds("file-action-error").unwrap();
    assert_eq!(file.left(), px(12.));
    assert_eq!(file.right(), px(288.));
    let close = visual.debug_bounds("dismiss-file-action-error").unwrap();
    visual.simulate_click(close.center(), Default::default());
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("file-action-error").is_none());
    window
        .update(visual, |shell, _, cx| {
            assert!(shell.error.is_none());
            assert!(!shell.notification_state.file_error.read(cx).is_visible());
        })
        .unwrap();
}
