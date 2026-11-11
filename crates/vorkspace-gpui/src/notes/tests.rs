use super::*;
use gpui::{TestAppContext, VisualTestContext};

fn store() -> Database {
    std::sync::Arc::new(std::sync::Mutex::new(
        vorkspace_core::store::Store::test(std::path::Path::new(":memory:")).unwrap(),
    ))
}

fn view(
    cx: &mut TestAppContext,
    store: &Database,
) -> (gpui::WindowHandle<Notes>, VisualTestContext) {
    cx.update(|cx| {
        crate::input::bind(cx);
        vorkspace_ui::clock::use_test_clock(cx);
    });
    let store = store.clone();
    let window = cx.add_window(move |_, cx| Notes::new(store, cx));
    let visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    (window, visual)
}

#[gpui::test]
fn deleting_a_note_checks_peer_drafts_and_the_saved_revision(cx: &mut TestAppContext) {
    let store = store();
    locked(&store)
        .unwrap()
        .upsert_note(&notes::NoteUpsert {
            id: "shared".into(),
            title: "Shared note".into(),
            body: "Original".into(),
            ..Default::default()
        })
        .unwrap();
    let (first, mut first_visual) = view(cx, &store);
    let (second, mut second_visual) = view(cx, &store);
    second
        .update(&mut second_visual, |notes, _, cx| {
            notes
                .body
                .update(cx, |body, cx| body.set_text("Peer draft".into(), cx));
            notes.edited(cx);
        })
        .unwrap();
    first
        .update(&mut first_visual, |notes, _, cx| {
            notes.delete(cx);
            notes.delete(cx);
            assert!(
                notes
                    .error
                    .as_ref()
                    .is_some_and(|error| error.contains("unfinished edits"))
            );
        })
        .unwrap();
    second
        .update(&mut second_visual, |notes, _, cx| notes.flush(cx))
        .unwrap();
    first
        .update(&mut first_visual, |notes, _, cx| {
            notes.delete(cx);
            notes.delete(cx);
            assert!(
                notes
                    .error
                    .as_ref()
                    .is_some_and(|error| error.contains("changed in another window"))
            );
        })
        .unwrap();
    assert_eq!(
        locked(&store)
            .unwrap()
            .get_note("shared")
            .unwrap()
            .unwrap()
            .body,
        "Peer draft"
    );
}

#[gpui::test]
fn stale_note_edits_are_preserved_without_overwriting_the_other_window(cx: &mut TestAppContext) {
    let store = store();
    locked(&store)
        .unwrap()
        .upsert_note(&notes::NoteUpsert {
            id: "shared".into(),
            title: "Shared note".into(),
            body: "Original body".into(),
            ..Default::default()
        })
        .unwrap();
    let (first, mut first_visual) = view(cx, &store);
    let (second, mut second_visual) = view(cx, &store);
    for (window, visual, text) in [
        (first, &mut first_visual, "The first editor's body"),
        (second, &mut second_visual, "The second editor's body"),
    ] {
        window
            .update(visual, |notes, _, cx| {
                notes
                    .body
                    .update(cx, |body, cx| body.set_text(text.into(), cx));
                notes.edited(cx);
                notes.flush(cx);
            })
            .unwrap();
    }
    second_visual.refresh().unwrap();
    second
        .update(&mut second_visual, |notes, _, cx| {
            assert!(notes.save_conflict && notes.dirty);
            assert!(!notes.prepare_close(cx));
            assert_eq!(notes.body.read(cx).content, "The second editor's body");
        })
        .unwrap();
    let button = second_visual.debug_bounds("note-save-copy").unwrap();
    second_visual.simulate_click(button.center(), Default::default());
    second_visual.run_until_parked();

    {
        let db = locked(&store).unwrap();
        let notes = db.list_notes().unwrap();
        assert_eq!(notes.len(), 2);
        assert_eq!(
            db.get_note("shared").unwrap().unwrap().body,
            "The first editor's body"
        );
        let copy = notes.iter().find(|note| note.id != "shared").unwrap();
        assert_eq!(copy.title, "Shared note (copy)");
        assert_eq!(copy.body, "The second editor's body");
        db.delete_note("shared").unwrap();
    }
    second
        .update(&mut second_visual, |notes, _, cx| {
            notes.body.update(cx, |body, cx| {
                body.set_text("Saved note was deleted".into(), cx)
            });
            notes.edited(cx);
            notes.flush(cx);
            assert!(notes.save_conflict);
        })
        .unwrap();
    assert!(
        locked(&store)
            .unwrap()
            .get_note("shared")
            .unwrap()
            .is_none()
    );
    second_visual.refresh().unwrap();
    let button = second_visual.debug_bounds("note-reload-latest").unwrap();
    second_visual.simulate_click(button.center(), Default::default());
    second_visual.run_until_parked();
    second
        .update(&mut second_visual, |notes, _, _| {
            assert!(!notes.save_conflict);
            assert!(!notes.dirty);
        })
        .unwrap();
}

#[gpui::test]
fn list_resize_highlight_follows_the_curved_detail_edge(cx: &mut TestAppContext) {
    let (window, mut visual) = view(cx, &store());
    visual.simulate_resize(gpui::size(px(1200.), px(800.)));
    visual.run_until_parked();
    let handle = visual.debug_bounds("resize-notes-list").unwrap();
    visual.simulate_mouse_move(handle.center(), None, Default::default());
    visual.run_until_parked();
    window
        .update(&mut visual, |_, window, _| {
            let scale = window.scale_factor();
            let edge = window
                .painted_quads()
                .into_iter()
                .find(|quad| {
                    (quad.bounds.left().0 - handle.center().x.as_f32() * scale).abs() < 0.5
                        && quad.border_widths.left.0 > 0.
                        && (quad.border_color.a - 0.25).abs() < 0.01
                })
                .expect("hover paints the curved outline");
            assert_eq!(edge.corner_radii.top_left.0, radius::SURFACE * scale);
            assert_eq!(edge.corner_radii.bottom_left.0, radius::SURFACE * scale);
            assert_eq!(edge.background.as_solid().unwrap().a, 0.);
        })
        .unwrap();
}

fn pinned_view(
    cx: &mut TestAppContext,
    store: &Database,
    id: &str,
) -> (gpui::WindowHandle<Notes>, VisualTestContext) {
    cx.update(|cx| {
        crate::input::bind(cx);
        vorkspace_ui::clock::use_test_clock(cx);
    });
    let store = store.clone();
    let id = id.to_owned();
    let window = cx.add_window(move |_, cx| Notes::open_note_tab(store, id, cx));
    let visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    (window, visual)
}

#[gpui::test]
fn pinned_editor_flushes_its_draft_when_the_app_quits(cx: &mut TestAppContext) {
    let store = store();
    locked(&store)
        .unwrap()
        .upsert_note(&notes::NoteUpsert {
            id: "quit-note".into(),
            title: "Quit note".into(),
            body: "Before quit".into(),
            ..Default::default()
        })
        .unwrap();
    let (window, mut visual) = pinned_view(cx, &store, "quit-note");
    window
        .update(&mut visual, |notes, _, cx| {
            notes.body.update(cx, |input, cx| {
                input.set_text("Unfinished at quit".into(), cx)
            });
            notes.input_changed(cx);
            assert!(notes.dirty);
        })
        .unwrap();
    assert_eq!(
        locked(&store)
            .unwrap()
            .get_note("quit-note")
            .unwrap()
            .unwrap()
            .body,
        "Before quit"
    );
    cx.quit();
    assert_eq!(
        locked(&store)
            .unwrap()
            .get_note("quit-note")
            .unwrap()
            .unwrap()
            .body,
        "Unfinished at quit"
    );
}

#[gpui::test]
fn failed_note_save_keeps_the_pinned_editor_for_retry(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("notes.db");
    let store = std::sync::Arc::new(std::sync::Mutex::new(
        vorkspace_core::store::Store::test(&path).unwrap(),
    ));
    locked(&store)
        .unwrap()
        .upsert_note(&notes::NoteUpsert {
            id: "close-note".into(),
            title: "Close note".into(),
            body: "Saved body".into(),
            ..Default::default()
        })
        .unwrap();
    let (window, mut visual) = pinned_view(cx, &store, "close-note");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_note_close BEFORE UPDATE ON notes \
             BEGIN SELECT RAISE(ABORT, 'simulated save failure'); END;",
        )
        .unwrap();
    window
        .update(&mut visual, |notes, _, cx| {
            notes
                .body
                .update(cx, |input, cx| input.set_text("Unfinished body".into(), cx));
            notes.input_changed(cx);
            assert!(!notes.prepare_close(cx));
            assert!(notes.dirty);
            assert!(
                notes
                    .error
                    .as_deref()
                    .is_some_and(|error| error.contains("simulated save failure"))
            );
            assert_eq!(notes.body.read(cx).content, "Unfinished body");
        })
        .unwrap();
    assert_eq!(
        locked(&store)
            .unwrap()
            .get_note("close-note")
            .unwrap()
            .unwrap()
            .body,
        "Saved body"
    );
    connection
        .execute_batch("DROP TRIGGER reject_note_close;")
        .unwrap();
    window
        .update(&mut visual, |notes, _, cx| {
            assert!(notes.prepare_close(cx));
            assert!(!notes.dirty);
        })
        .unwrap();
    assert_eq!(
        locked(&store)
            .unwrap()
            .get_note("close-note")
            .unwrap()
            .unwrap()
            .body,
        "Unfinished body"
    );
}

#[gpui::test]
fn external_write_gate_tracks_only_the_note_with_a_local_draft(cx: &mut TestAppContext) {
    let store = store();
    for id in ["first", "second"] {
        locked(&store)
            .unwrap()
            .upsert_note(&notes::NoteUpsert {
                id: id.into(),
                title: id.into(),
                body: format!("Original {id}"),
                ..Default::default()
            })
            .unwrap();
    }
    let (first, mut first_visual) = pinned_view(cx, &store, "first");
    let (second, mut second_visual) = pinned_view(cx, &store, "second");
    first
        .update(&mut first_visual, |notes, _, cx| {
            notes.body.update(cx, |input, cx| {
                input.set_text("Draft before Changed event".into(), cx)
            });
            assert!(notes.blocks_external_write("first", cx));
            assert!(!notes.blocks_external_write("second", cx));
        })
        .unwrap();
    second
        .update(&mut second_visual, |notes, _, cx| {
            assert!(!notes.blocks_external_write("second", cx));
        })
        .unwrap();
}

#[gpui::test]
fn pinned_notes_keep_independent_editors_selections_and_drafts(cx: &mut TestAppContext) {
    let store = store();
    let lines = (0..90)
        .map(|index| format!("Line {index}: this note has enough content to scroll."))
        .collect::<Vec<_>>()
        .join("\n");
    let first_draft = format!("First draft\n{lines}");
    let second_draft = format!("Second draft\n{lines}");
    for (id, title) in [("first", "First note"), ("second", "Second note")] {
        locked(&store)
            .unwrap()
            .upsert_note(&notes::NoteUpsert {
                id: id.into(),
                title: title.into(),
                body: format!("Original {id}\n{lines}"),
                ..Default::default()
            })
            .unwrap();
    }
    let (first, mut first_visual) = pinned_view(cx, &store, "first");
    let (second, mut second_visual) = pinned_view(cx, &store, "second");
    first_visual.simulate_resize(gpui::size(px(600.), px(480.)));
    second_visual.simulate_resize(gpui::size(px(600.), px(480.)));
    let first_body = first
        .update(&mut first_visual, |notes, _, cx| {
            assert_eq!(notes.pane_note_id().as_deref(), Some("first"));
            assert_eq!(notes.pane_title(cx), "First note");
            notes.source = true;
            notes.modes.insert("first".into(), true);
            notes.body.update(cx, |input, cx| {
                input.set_text(first_draft.clone(), cx);
                input.select_utf16(0..5, cx);
            });
            notes.body.clone()
        })
        .unwrap();
    first_visual.refresh().unwrap();
    first
        .update(&mut first_visual, |notes, _, _| {
            notes
                .detail_scroll
                .set_offset(gpui::point(px(0.), px(-120.)));
        })
        .unwrap();
    let second_body = second
        .update(&mut second_visual, |notes, _, cx| {
            assert_eq!(notes.pane_note_id().as_deref(), Some("second"));
            assert_eq!(notes.pane_title(cx), "Second note");
            notes.source = true;
            notes.modes.insert("second".into(), true);
            notes.body.update(cx, |input, cx| {
                input.set_text(second_draft.clone(), cx);
                input.select_utf16(7..12, cx);
            });
            notes.body.clone()
        })
        .unwrap();
    second_visual.refresh().unwrap();
    second
        .update(&mut second_visual, |notes, _, _| {
            notes
                .detail_scroll
                .set_offset(gpui::point(px(0.), px(-240.)));
        })
        .unwrap();
    assert_ne!(first_body, second_body);
    first
        .update(&mut first_visual, |notes, _, cx| {
            assert!(notes.dirty);
            notes.select("second".into(), cx);
            assert_eq!(notes.selected.as_deref(), Some("first"));
            assert_eq!(notes.body.read(cx).content, first_draft);
            assert_eq!(notes.body.read(cx).selection_utf16(), 0..5);
            assert_eq!(notes.detail_scroll.offset().y, px(-120.));
            notes.refresh(cx);
        })
        .unwrap();
    first_visual.run_until_parked();
    second
        .update(&mut second_visual, |notes, _, cx| {
            assert!(notes.dirty);
            assert_eq!(notes.selected.as_deref(), Some("second"));
            assert_eq!(notes.body.read(cx).content, second_draft);
            assert_eq!(notes.body.read(cx).selection_utf16(), 7..12);
            assert_eq!(notes.detail_scroll.offset().y, px(-240.));
        })
        .unwrap();
    assert_eq!(
        locked(&store)
            .unwrap()
            .get_note("first")
            .unwrap()
            .unwrap()
            .body,
        first_draft
    );
    assert_eq!(
        locked(&store)
            .unwrap()
            .get_note("second")
            .unwrap()
            .unwrap()
            .body,
        format!("Original second\n{lines}")
    );
}

#[gpui::test]
fn pinned_note_renders_detail_without_library_or_back_navigation(cx: &mut TestAppContext) {
    let store = store();
    locked(&store)
        .unwrap()
        .upsert_note(&notes::NoteUpsert {
            id: "reading".into(),
            title: "Reading".into(),
            body: "A saved document".into(),
            ..Default::default()
        })
        .unwrap();
    let (_, mut pinned_visual) = pinned_view(cx, &store, "reading");
    pinned_visual.simulate_resize(gpui::size(px(1100.), px(800.)));
    assert!(pinned_visual.debug_bounds("note-detail").is_some());
    assert!(pinned_visual.debug_bounds("notes-list").is_none());
    assert!(pinned_visual.debug_bounds("create-note").is_none());
    assert!(pinned_visual.debug_bounds("open-note-tab").is_none());
    pinned_visual.simulate_resize(gpui::size(px(500.), px(800.)));
    assert!(pinned_visual.debug_bounds("note-detail").is_some());
    assert!(pinned_visual.debug_bounds("notes-back").is_none());

    let (_, mut library_visual) = view(cx, &store);
    library_visual.simulate_resize(gpui::size(px(1100.), px(800.)));
    assert!(library_visual.debug_bounds("notes-list").is_some());
    assert!(library_visual.debug_bounds("open-note-tab").is_some());
}

#[gpui::test]
fn library_note_uses_narrow_tile_width_in_wide_window(cx: &mut TestAppContext) {
    let store = store();
    locked(&store)
        .unwrap()
        .upsert_note(&notes::NoteUpsert {
            id: "tile-note".into(),
            title: "Tile note".into(),
            body: "Original body".into(),
            ..Default::default()
        })
        .unwrap();
    let (window, mut visual) = view(cx, &store);
    visual.simulate_resize(gpui::size(px(1100.), px(700.)));
    window
        .update(&mut visual, |notes, _, cx| {
            notes.select("tile-note".into(), cx);
            notes.set_pane_width(420., cx);
        })
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("notes-list").is_none());
    assert!(visual.debug_bounds("note-detail").is_some());
    assert!(visual.debug_bounds("notes-back").is_some());

    let back = visual.debug_bounds("notes-back").unwrap().center();
    visual.simulate_click(back, gpui::Modifiers::none());
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("notes-list").is_some());
    assert!(visual.debug_bounds("note-detail").is_none());
    window
        .update(&mut visual, |notes, _, cx| {
            assert_eq!(notes.selected.as_deref(), Some("tile-note"));
            notes.set_pane_width(1000., cx);
        })
        .unwrap();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("notes-list").is_some());
    assert!(visual.debug_bounds("note-detail").is_some());
    assert!(visual.debug_bounds("notes-back").is_none());
}

#[gpui::test]
fn selected_library_editor_can_become_a_pinned_tab_without_reset(cx: &mut TestAppContext) {
    let store = store();
    locked(&store)
        .unwrap()
        .upsert_note(&notes::NoteUpsert {
            id: "handoff".into(),
            title: "Handoff".into(),
            body: "Original".into(),
            ..Default::default()
        })
        .unwrap();
    let (window, mut visual) = view(cx, &store);
    window
        .update(&mut visual, |notes, _, cx| {
            let body = notes.body.clone();
            let document = notes.document.clone();
            let recording = notes.recording.clone();
            notes.body.update(cx, |input, cx| {
                input.set_text("Unfinished handoff".into(), cx);
                input.select_utf16(0..10, cx);
            });
            notes
                .detail_scroll
                .set_offset(gpui::point(px(0.), px(-80.)));
            assert!(notes.pin_selected_note("handoff", cx));
            assert_eq!(notes.body, body);
            assert_eq!(notes.document, document);
            assert_eq!(notes.recording, recording);
            assert_eq!(notes.body.read(cx).content, "Unfinished handoff");
            assert_eq!(notes.body.read(cx).selection_utf16(), 0..10);
            assert_eq!(notes.detail_scroll.offset().y, px(-80.));
            assert_eq!(notes.pane_note_id().as_deref(), Some("handoff"));
        })
        .unwrap();
    assert!(visual.debug_bounds("note-detail").is_some());
    assert!(visual.debug_bounds("notes-list").is_none());
}

#[gpui::test]
fn library_routes_an_open_note_to_its_existing_tab(cx: &mut TestAppContext) {
    let store = store();
    for id in ["first", "second"] {
        locked(&store)
            .unwrap()
            .upsert_note(&notes::NoteUpsert {
                id: id.into(),
                title: id.into(),
                body: format!("Body of {id}"),
                ..Default::default()
            })
            .unwrap();
    }
    let (window, mut visual) = view(cx, &store);
    let opened = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    window
        .update(&mut visual, |notes, _, cx| {
            let events = opened.clone();
            let subscription = cx.subscribe(&cx.entity(), move |_, _, event: &Event, _| {
                if let Event::OpenTab(id) = event {
                    events.borrow_mut().push(id.clone());
                }
            });
            notes._subscriptions.push(subscription);
            notes.set_open_note_tabs(HashSet::from(["first".into()]), cx);
            assert_eq!(notes.selected_note_id().as_deref(), Some("second"));
            notes.select("first".into(), cx);
            assert_eq!(notes.selected_note_id().as_deref(), Some("second"));
            notes.set_open_note_tabs(HashSet::from(["first".into(), "second".into()]), cx);
            assert_eq!(notes.selected_note_id(), None);
            assert!(!notes.recording.read(cx).is_for_note("second"));
        })
        .unwrap();
    visual.run_until_parked();
    assert_eq!(&*opened.borrow(), &["first".to_owned()]);
}

#[gpui::test]
fn deleted_pinned_note_does_not_show_another_document(cx: &mut TestAppContext) {
    let store = store();
    for id in ["first", "second"] {
        locked(&store)
            .unwrap()
            .upsert_note(&notes::NoteUpsert {
                id: id.into(),
                title: id.into(),
                body: format!("Body of {id}"),
                ..Default::default()
            })
            .unwrap();
    }
    let (window, mut visual) = pinned_view(cx, &store, "first");
    locked(&store).unwrap().delete_note("first").unwrap();
    window
        .update(&mut visual, |notes, _, cx| notes.refresh(cx))
        .unwrap();
    visual.run_until_parked();
    window
        .update(&mut visual, |notes, _, cx| {
            assert_eq!(notes.pane_note_id().as_deref(), Some("first"));
            assert_eq!(notes.selected.as_deref(), Some("first"));
            assert!(notes.draft(cx).is_none());
            assert_eq!(notes.pane_title(cx), "Notes");
        })
        .unwrap();
    assert!(visual.debug_bounds("notes-list").is_none());
}

#[gpui::test]
fn returning_from_the_list_retains_the_note_editor_and_reading_position(cx: &mut TestAppContext) {
    let store = store();
    let (window, mut visual) = view(cx, &store);
    window
        .update(&mut visual, |notes, window, cx| {
            notes.create(window, cx);
            notes.body.update(cx, |input, cx| {
                input.set_text("Unfinished 🦊 note".into(), cx);
                input.select_utf16(11..13, cx);
            });
        })
        .unwrap();
    // Changed subscriptions publish after the enclosing app update.
    window
        .update(&mut visual, |notes, _, cx| {
            let id = notes.selected.clone().unwrap();
            let editor = notes.body.clone();
            let position = gpui::point(px(0.), px(-160.));
            notes.detail_scroll.set_offset(position);
            notes.show_list = true;
            notes.select(id.clone(), cx);
            assert!(!notes.show_list);
            assert_eq!(notes.selected.as_ref(), Some(&id));
            assert_eq!(notes.body, editor);
            assert_eq!(notes.body.read(cx).content, "Unfinished 🦊 note");
            assert_eq!(notes.body.read(cx).selection_utf16(), 11..13);
            assert_eq!(notes.detail_scroll.offset(), position);
            assert!(notes.dirty);
        })
        .unwrap();
}

#[gpui::test]
fn a_new_note_can_start_with_an_image_and_reopen(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("notes.db");
    let store = std::sync::Arc::new(std::sync::Mutex::new(
        vorkspace_core::store::Store::test(&path).unwrap(),
    ));
    let image = root.path().join("first.svg");
    std::fs::write(
        &image,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>"#,
    )
    .unwrap();
    let (window, mut visual) = view(cx, &store);
    let id = window
        .update(&mut visual, |notes, window, cx| {
            notes.create(window, cx);
            let id = notes.selected.clone().unwrap();
            assert!(locked(&store).unwrap().get_note(&id).unwrap().is_none());
            notes.add_images(vec![image], cx);
            notes.create(window, cx);
            assert!(
                notes.notes.iter().any(|note| note.id == id),
                "pending image belongs to its original draft"
            );
            id
        })
        .unwrap();
    visual.run_until_parked();
    window
        .update(&mut visual, |notes, _, cx| {
            assert!(notes.error.is_none(), "{:?}", notes.error);
            assert!(!notes.ephemeral.contains(&id));
            notes.select(id.clone(), cx);
            notes.flush(cx);
        })
        .unwrap();
    let reopened = vorkspace_core::store::Store::test(&path).unwrap();
    let saved = reopened.get_note(&id).unwrap().unwrap();
    assert_eq!(saved.body.matches("![").count(), 1);
    assert!(saved.body.contains("/note-assets/"));
    assert_eq!(
        std::fs::read_dir(root.path().join("note-assets").join(id))
            .unwrap()
            .count(),
        1
    );
}

#[gpui::test]
fn refreshing_the_selected_note_preserves_navigation_and_document(cx: &mut TestAppContext) {
    let store = store();
    let saved = locked(&store)
        .unwrap()
        .upsert_note(&notes::NoteUpsert {
            id: "reading".into(),
            title: "Reading".into(),
            body: "Keep this document".into(),
            ..Default::default()
        })
        .unwrap();
    let (window, mut visual) = view(cx, &store);
    let (scroll, document) = window
        .update(&mut visual, |notes, _, cx| {
            assert_eq!(notes.selected.as_deref(), Some(saved.id.as_str()));
            notes.show_list = true;
            notes
                .body
                .update(cx, |input, cx| input.select_utf16(5..9, cx));
            let scroll = notes.detail_scroll.clone();
            let document = notes.document.clone();
            notes.refresh(cx);
            (scroll, document)
        })
        .unwrap();
    visual.run_until_parked();
    window
        .update(&mut visual, |notes, _, cx| {
            let position = gpui::point(px(0.), px(-160.));
            scroll.set_offset(position);
            assert_eq!(
                notes.detail_scroll.offset(),
                position,
                "refresh retains the scroll owner"
            );
            assert!(
                notes.show_list,
                "background refresh must not close the narrow list"
            );
            assert_eq!(
                notes.document, document,
                "unchanged Markdown retains its selection owner"
            );
            assert_eq!(notes.body.read(cx).selection_utf16(), 5..9);
        })
        .unwrap();
}

#[gpui::test]
fn loading_an_empty_list_is_not_an_edit_and_new_note_works(cx: &mut TestAppContext) {
    let store = store();
    let (window, mut visual) = view(cx, &store);
    window
        .update(&mut visual, |notes, window, cx| {
            assert!(!notes.dirty);
            notes.create(window, cx);
            assert_eq!(notes.notes.len(), 1);
            assert_eq!(notes.selected.as_ref(), Some(&notes.notes[0].id));
        })
        .unwrap();
    visual.run_until_parked();
    window
        .update(&mut visual, |notes, window, cx| {
            assert!(!notes.dirty, "loading the new note must not look edited");
            notes.create(window, cx);
            assert_eq!(notes.notes.len(), 1, "an untouched draft is discarded");
            assert!(
                locked(&notes.store)
                    .unwrap()
                    .list_notes()
                    .unwrap()
                    .is_empty()
            );
        })
        .unwrap();
}

#[gpui::test]
fn refresh_shows_notes_saved_elsewhere_after_an_empty_first_load(cx: &mut TestAppContext) {
    let store = store();
    let (window, mut visual) = view(cx, &store);
    locked(&store)
        .unwrap()
        .save_note(&vorkspace_core::response::Note {
            id: 7,
            title: "From chat".into(),
            body: "Saved from a response".into(),
            tags: Vec::new(),
            source_session: 0,
            source_cwd: String::new(),
        })
        .unwrap();
    window
        .update(&mut visual, |notes, _, cx| notes.refresh(cx))
        .unwrap();
    visual.run_until_parked();
    window
        .update(&mut visual, |notes, _, _| {
            assert_eq!(notes.notes.len(), 1);
            assert_eq!(notes.notes[0].title, "From chat");
            assert!(!notes.dirty);
        })
        .unwrap();
}

#[gpui::test]
fn deleting_another_note_keeps_the_selected_unsaved_text(cx: &mut TestAppContext) {
    let store = store();
    let saved = |id: &str| {
        locked(&store)
            .unwrap()
            .upsert_note(&notes::NoteUpsert {
                id: id.into(),
                title: id.into(),
                body: "Original".into(),
                ..Default::default()
            })
            .unwrap()
    };
    let first = saved("first");
    let second = saved("second");
    let (window, mut visual) = view(cx, &store);
    window
        .update(&mut visual, |notes, _, cx| {
            notes.select(second.id.clone(), cx);
            notes.body.update(cx, |input, cx| {
                input.set_text("My unfinished edit".into(), cx)
            });
        })
        .unwrap();
    window
        .update(&mut visual, |notes, _, cx| {
            assert!(notes.dirty);
            notes.delete_saved_note(&first.id, cx);
            assert!(notes.dirty);
            assert_eq!(notes.selected.as_ref(), Some(&second.id));
            assert_eq!(notes.body.read(cx).content, "My unfinished edit");
        })
        .unwrap();
    assert!(
        locked(&store)
            .unwrap()
            .get_note(&first.id)
            .unwrap()
            .is_none()
    );
}

#[gpui::test]
fn typing_still_marks_the_note_edited_and_saves(cx: &mut TestAppContext) {
    let store = store();
    let (window, mut visual) = view(cx, &store);
    window
        .update(&mut visual, |notes, window, cx| notes.create(window, cx))
        .unwrap();
    visual.run_until_parked();
    window
        .update(&mut visual, |notes, _, cx| {
            notes
                .body
                .update(cx, |input, cx| input.set_text("Hello".into(), cx));
        })
        .unwrap();
    window
        .update(&mut visual, |notes, _, _| assert!(notes.dirty))
        .unwrap();
    window
        .update(&mut visual, |notes, _, cx| notes.flush(cx))
        .unwrap();
    let saved = locked(&store).unwrap().list_notes().unwrap();
    assert_eq!(saved[0].body, "Hello");
}
