
use super::*;
use gpui::TestAppContext;
use std::sync::Mutex;
use vorkspace_core::store::Store;

#[gpui::test]
fn removing_partial_live_source_keeps_saved_global_search_hits(cx: &mut TestAppContext) {
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let session = store.create("codex", "/project").unwrap();
    let view = cx.new(|cx| {
        SearchView::new(
            "/project".into(),
            Arc::new(Mutex::new(store)),
            Sources {
                summaries: vec![session.clone()],
                ..Default::default()
            },
            cx,
        )
    });
    view.update(cx, |view, cx| {
        view.query = "needle".into();
        view.live.insert(session.id, Arc::new(vec![]));
        view.local
            .push(search::message_hit(&session, 0, "user", "needle live", "needle", true).unwrap());
        view.remote.push(
            search::message_hit(&session, 50, "user", "needle saved", "needle", false).unwrap(),
        );
        view.remove_live_session(session.id, cx);
        assert!(!view.live.contains_key(&session.id));
        assert!(view.local.is_empty());
        assert!(view.hits.iter().any(|hit| hit.target
            == Target::Message {
                session: session.id,
                row: 50
            }));
        assert!(!view.hits.iter().any(|hit| hit.target
            == Target::Message {
                session: session.id,
                row: 0
            }));
    });
}

struct TempDir(std::path::PathBuf);
impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "vorkspace-search-ui-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[gpui::test]
fn app_search_debounce_scope_keyboard_and_stale_results(cx: &mut TestAppContext) {
    let dir = TempDir::new();
    std::fs::write(
        dir.path().join("needle.ts"),
        "const value = 'needle content';\n",
    )
    .unwrap();
    let mut store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let mut session = store.create("codex", "/another-project").unwrap();
    store.title(session.id, "Needle conversation").unwrap();
    session.title = "Needle conversation".into();
    store
        .write_rows(
            session.id,
            0,
            &[Row {
                role: "user".into(),
                text: "saved needle message".into(),
                first: true,
                sent_at: None,
            }],
        )
        .unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let window = cx.update(|cx| {
        input::bind(cx);
        crate::shell::bind_keys(cx);
        cx.open_window(Default::default(), |w, cx| {
            let view = cx.new(|cx| {
                SearchView::new(
                    cwd,
                    Arc::new(Mutex::new(store)),
                    Sources {
                        summaries: vec![session],
                        projects: vec![dir.path().to_string_lossy().into_owned()],
                        ..Default::default()
                    },
                    cx,
                )
            });
            w.focus(&view.focus_handle(cx), cx);
            view
        })
        .unwrap()
    });
    cx.run_until_parked();
    {
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        assert_eq!(
            visual
                .debug_bounds("app-search-header")
                .unwrap()
                .size
                .height,
            px(36.)
        );
        assert_eq!(
            visual
                .debug_bounds("app-search-scopes")
                .unwrap()
                .size
                .height,
            px(36.)
        );
        for (selector, min_width) in [
            ("app-search-scope-0", 28.),
            ("app-search-scope-1", 70.),
            ("app-search-scope-2", 32.),
            ("app-search-scope-3", 45.),
        ] {
            assert!(
                visual.debug_bounds(selector).unwrap().size.width > px(min_width),
                "scope buttons must render their labels"
            );
        }
        let files = visual.debug_bounds("app-search-scope-2").unwrap();
        visual.simulate_click(files.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |view, _, cx| {
                assert_eq!(view.scope, Scope::Files);
                view.select_scope(Scope::All, cx);
            })
            .unwrap();
    }
    cx.simulate_input(window.into(), "needle");
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| {
            assert_eq!(view.hits.len(), 2); // Title and file name are immediate.
            assert!(view.sessions_loading && view.files_loading);
        })
        .unwrap();
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| {
            assert!(!view.sessions_loading && !view.files_loading);
            assert!(
                view.hits
                    .iter()
                    .any(|h| matches!(h.target, Target::Message { .. }))
            );
            assert!(view.hits.iter().any(|h| matches!(
                h.target,
                Target::File {
                    location: Some((1, 16)),
                    ..
                }
            )));
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "up");
    window
        .update(cx, |view, _, _| {
            assert_eq!(view.active, view.hits.len() - 1)
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "down");
    window
        .update(cx, |view, _, _| assert_eq!(view.active, 0))
        .unwrap();
    window
        .update(cx, |view, _, cx| view.select_scope(Scope::Files, cx))
        .unwrap();
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert!(
                view.hits
                    .iter()
                    .all(|h| matches!(h.target, Target::File { .. }))
            );
            let stale = view.cancel.clone();
            view.input
                .update(cx, |i, cx| i.set_text("absent".into(), cx));
            // Changed is delivered after this update.
            cx.spawn(async move |_, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(1))
                    .await;
                assert!(stale.load(Ordering::Acquire));
            })
            .detach();
        })
        .unwrap();
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| assert!(view.hits.is_empty()))
        .unwrap();
}
