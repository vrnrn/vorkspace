use super::*;
use gpui::{Entity, TestAppContext};
use std::sync::atomic::AtomicBool;
use tempfile::TempDir;
use vorkspace_meetings::recording::{CaptureSource, Owner, Phase};

fn storage() -> (TempDir, Meetings) {
    let root = tempfile::tempdir().unwrap();
    let meetings = Meetings::open(root.path()).unwrap();
    (root, meetings)
}

fn recorded(meetings: &Meetings, title: &str) -> String {
    let id = meetings.prepare(title).unwrap();
    let mut owner = Owner::create(&meetings.directory, &id).unwrap();
    owner.phase(Phase::Recording, None).unwrap();
    owner
        .append(
            "microphone",
            0,
            1000,
            "Review the release checklist.".into(),
        )
        .unwrap();
    owner
        .append(
            "system",
            1100,
            2000,
            "Taylor will publish the draft tomorrow.".into(),
        )
        .unwrap();
    owner.phase(Phase::Stopped, None).unwrap();
    id
}

fn summarized(meetings: &Meetings, id: &str) -> Snapshot {
    meetings
        .save_settings(&vorkspace_meetings::Settings {
            provider: "codex".into(),
            model: "fixture".into(),
            ..Default::default()
        })
        .unwrap();
    meetings.summarize(id, &AtomicBool::new(false), |_, _| Ok(
        "## Summary\n\nThe team reviewed the **release checklist**.\n\n## Action items\n\n- Taylor will publish the draft tomorrow.\n- Verify the build with `cargo test`.\n\n```sh\ncargo test --workspace\n```".into()
    )).unwrap()
}

fn save_edit(meetings: &Meetings, id: &str, draft: Draft) {
    let version = uuid::Uuid::new_v4().simple().to_string();
    let write = DraftWrite::new(id.into(), None, version.clone(), draft).unwrap();
    meetings.checkpoint_draft(&write).unwrap();
    meetings.save_draft(id, &version).unwrap();
}

fn page(meetings: &Meetings, id: &str, cx: &mut TestAppContext) -> Entity<Page> {
    let doc = DraftBuffer::default().open(meetings, id, None).unwrap();
    let meetings = meetings.clone();
    let page = cx.new(|cx| Page::with_meetings(Ok(meetings), cx));
    page.update(cx, |page, cx| page.install(Some(doc), cx));
    page
}

fn type_into(
    page: &Entity<Page>,
    field: Field,
    text: &str,
    selection: std::ops::Range<usize>,
    cx: &mut TestAppContext,
) {
    page.update(cx, |page, cx| {
        page.input(field).update(cx, |input, cx| {
            input.set_text(text.into(), cx);
            input.select_utf16(selection, cx);
        })
    });
}

fn events(
    page: &Entity<Page>,
    cx: &mut TestAppContext,
) -> std::rc::Rc<std::cell::RefCell<Vec<&'static str>>> {
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let output = events.clone();
    page.update(cx, |page, cx| {
        let subscription = cx.subscribe(&cx.entity(), move |_, _, event: &Event, _| {
            output.borrow_mut().push(match event {
                Event::Changed => "changed",
                Event::Attached { .. } => "attached",
                Event::Deleted(_) => "deleted",
                Event::SummaryReady(_) => "summary",
            });
        });
        page._subscriptions.push(subscription);
    });
    events
}

#[gpui::test]
fn a_durable_native_draft_reopens_with_its_section_and_utf16_selection(cx: &mut TestAppContext) {
    let (_root, meetings) = storage();
    let id = recorded(&meetings, "Planning");
    let original = summarized(&meetings, &id);
    let first = page(&meetings, &id, cx);
    first.update(cx, |page, cx| {
        page.navigate(Action::Section(Section::Source), cx)
    });
    type_into(&first, Field::Summary, "a🦊b edited summary", 1..3, cx);
    first.read_with(cx, |page, _| assert!(page.drafts.has_pending()));

    let read_only = page(&meetings, &id, cx);
    read_only.read_with(cx, |page, cx| {
        assert!(!page.editable());
        assert_eq!(
            page.input(Field::Summary).read(cx).content.as_ref(),
            original.meeting.summary.as_ref().unwrap().text
        );
    });
    // GPUI drops released entities at the end of an app update. Close the
    // view before the next synchronous open attempts to claim its edit lease.
    cx.update(|_| drop(first));
    let reopened = page(&meetings, &id, cx);
    reopened.read_with(cx, |page, cx| {
        let doc = page.document.as_ref().unwrap();
        assert!(doc.editable());
        assert_eq!(page.section, Section::Source);
        assert_eq!(page.pending_focus, Some(Field::Summary));
        assert_eq!(page.input(Field::Summary).read(cx).selection_utf16(), 1..3);
        assert_eq!(doc.text(Field::Summary), "a🦊b edited summary");
        assert_eq!(
            doc.saved.as_ref().unwrap().meeting.revision,
            original.meeting.revision
        );
    });
    reopened.update(cx, |page, cx| page.save(cx));
    cx.run_until_parked();
    let saved = meetings.open_document(&id).unwrap();
    assert_eq!(saved.meeting.summary.unwrap().text, "a🦊b edited summary");
    assert!(saved.draft.is_none());
    reopened.read_with(cx, |page, _| {
        assert!(!page.document.as_ref().unwrap().dirty());
    });
}

#[gpui::test]
fn a_save_receipt_keeps_newer_native_input_and_its_selection(cx: &mut TestAppContext) {
    let (_root, meetings) = storage();
    let id = recorded(&meetings, "Planning");
    let page = page(&meetings, &id, cx);
    type_into(&page, Field::Transcript, "First revision", 2..2, cx);

    // Commit the captured draft but delay publishing the save result.
    let (version, submitted, saved) = page.update(cx, |page, _| {
        let doc = page.document.as_ref().unwrap();
        let version = doc.version.clone().unwrap();
        page.drafts.flush(&meetings).unwrap();
        let saved = meetings.save_draft(&id, &version).unwrap();
        page.saving = true;
        (version, doc.draft.clone().unwrap(), saved)
    });
    type_into(&page, Field::Transcript, "A🦊 new revision", 1..3, cx);
    page.update(cx, |page, cx| {
        // Also exercise the receipt's durable rebase before the UI sees it.
        page.drafts.flush(&meetings).unwrap();
        page.complete_save(&id, version, submitted, Ok(Some(saved)), cx);
        let doc = page.document.as_ref().unwrap();
        assert_eq!(doc.text(Field::Transcript), "A🦊 new revision");
        assert_eq!(doc.saved.as_ref().unwrap().transcript, "First revision");
        assert!(doc.dirty());
        assert!(!doc.conflict());
        assert_eq!(
            page.input(Field::Transcript).read(cx).selection_utf16(),
            1..3
        );
        let persisted: Snapshot = meetings.open_document(&id).unwrap();
        let draft = persisted.draft.unwrap();
        assert_eq!(draft.revision, persisted.meeting.revision);
        assert_eq!(draft.transcript.as_deref(), Some("A🦊 new revision"));
    });
}

#[test]
fn reopening_after_a_failed_checkpoint_preserves_the_durable_base_for_the_next_edit() {
    let (_root, meetings) = storage();
    let id = recorded(&meetings, "Planning");
    let previous = "aaaaaaaaaaaaaaaaaaaaaaaa00000001";
    let pending = "bbbbbbbbbbbbbbbbbbbbbbbb00000001";
    let next = "bbbbbbbbbbbbbbbbbbbbbbbb00000002";
    let initial = DraftWrite::new(
        id.clone(),
        None,
        previous.into(),
        Draft {
            transcript: Some("Prior correction".into()),
            ..Draft::default()
        },
    )
    .unwrap();
    meetings.checkpoint_draft(&initial).unwrap();
    let saved = meetings.save_draft(&id, previous).unwrap();
    let lease = Arc::new(EditorLease(Mutex::new(meetings.try_edit(&id).unwrap())));
    let mut capture = meetings
        .start_recording(&id, "new-take", None, CaptureSource::Microphone)
        .unwrap();

    let drafts = DraftBuffer::default();
    let write = DraftWrite::new(
        id.clone(),
        Some(previous.into()),
        pending.into(),
        Draft {
            revision: saved.revision,
            transcript: Some("Pending correction".into()),
            ..Draft::default()
        },
    )
    .unwrap();
    drafts.stage(write, lease.clone(), false).unwrap();
    let mut doc = drafts.open(&meetings, &id, Some(lease.clone())).unwrap();
    assert_eq!(doc.text(Field::Transcript), "Pending correction");
    assert_eq!(doc.version.as_deref(), Some(pending));
    assert!(doc.checkpoint_error.is_some());
    capture.phase(Phase::Stopped, None).unwrap();
    drop(capture);

    doc.draft.as_mut().unwrap().transcript = Some("Next correction".into());
    doc.version = Some(next.into());
    drafts.stage(doc.write().unwrap(), lease, false).unwrap();
    drafts.flush(&meetings).unwrap();
    let reopened = meetings.open_document(&id).unwrap();
    assert_eq!(reopened.draft_version.as_deref(), Some(next));
    assert_eq!(reopened.transcript, "Prior correction");
    assert_eq!(
        reopened.draft.unwrap().transcript.as_deref(),
        Some("Next correction")
    );
}

#[gpui::test]
fn a_stale_open_cannot_replace_input_or_take_its_lease(cx: &mut TestAppContext) {
    let (_root, meetings) = storage();
    let selected = recorded(&meetings, "Planning");
    let destination = recorded(&meetings, "Review");
    let page = page(&meetings, &selected, cx);
    let pending_open = DraftBuffer::default()
        .open(&meetings, &destination, None)
        .unwrap();
    let (epoch, intent) = page.read_with(cx, |page, _| (page.open_epoch, page.intent));
    type_into(&page, Field::Transcript, "Planning edits", 8..8, cx);
    page.update(cx, |page, cx| {
        page.complete_open(epoch, intent, true, Ok(pending_open), cx);
        let doc = page.document.as_ref().unwrap();
        assert_eq!(doc.id(), selected);
        assert_eq!(doc.text(Field::Transcript), "Planning edits");
        assert!(meetings.try_edit(&selected).unwrap().is_none());
        assert!(meetings.try_edit(&destination).unwrap().is_some());
    });
}

#[gpui::test]
fn returning_to_authored_text_does_not_cancel_a_recording_open(cx: &mut TestAppContext) {
    let (_root, meetings) = storage();
    let id = recorded(&meetings, "Planning");
    let pending = DraftBuffer::default().open(&meetings, &id, None).unwrap();
    let page = cx.new(|cx| Page::with_meetings(Ok(meetings.clone()), cx));
    page.update(cx, |page, cx| {
        page.note_ids.insert("note".into(), id.clone());
        page.opening = Some(PendingOpen {
            epoch: page.open_epoch,
            intent: page.intent,
            id: "held-load".into(),
            retry: true,
        });
        let epoch = page.open_epoch;
        let intent = page.intent;
        page.hide_note_content(cx);
        page.complete_open(epoch, intent, true, Ok(pending), cx);
        assert!(page.is_for_note("note"));
        assert!(
            page.document
                .as_ref()
                .unwrap()
                .text(Field::Transcript)
                .contains("release checklist")
        );
        assert!(page.opening.is_none());
    });
}

#[gpui::test]
fn returning_to_the_loaded_note_supersedes_another_pending_open(cx: &mut TestAppContext) {
    let (_root, meetings) = storage();
    let id = recorded(&meetings, "Planning");
    let other = recorded(&meetings, "Review");
    let page = page(&meetings, &id, cx);
    let pending = DraftBuffer::default()
        .open(&meetings, &other, None)
        .unwrap();
    page.update(cx, |page, cx| {
        page.opening = Some(PendingOpen {
            epoch: page.open_epoch,
            intent: page.intent,
            id: "held-load".into(),
            retry: true,
        });
        let epoch = page.open_epoch;
        let intent = page.intent;
        page.show_note("planning-note", Some(&id), cx);
        page.complete_open(epoch, intent, true, Ok(pending), cx);
        assert!(page.is_for_note("planning-note"));
        assert!(page.next_open.is_none());
        assert!(meetings.try_edit(&other).unwrap().is_some());
    });
}

#[gpui::test]
fn a_newer_tab_choice_survives_direct_and_queued_opens_with_a_saved_draft(cx: &mut TestAppContext) {
    for queued in [false, true] {
        for saved_draft in [false, true] {
            for chosen in [Section::Summary, Section::Transcript] {
                let (_root, meetings) = storage();
                let previous = recorded(&meetings, "Previous");
                summarized(&meetings, &previous);
                let id = recorded(&meetings, "Planning");
                let snapshot = summarized(&meetings, &id);
                if saved_draft {
                    let write = DraftWrite::new(
                        id.clone(),
                        None,
                        "0123456789abcdef0123456789abcdef".into(),
                        Draft {
                            revision: snapshot.meeting.revision,
                            summary: Some("Saved summary draft".into()),
                            view: DraftView::Summary,
                            ..Draft::default()
                        },
                    )
                    .unwrap();
                    meetings.checkpoint_draft(&write).unwrap();
                }
                let page = page(&meetings, &previous, cx);
                page.update(cx, |page, cx| {
                    // The selected enum still belongs to the previous document.
                    page.note_section(chosen == Section::Transcript, cx);
                    if queued {
                        page.opening = Some(PendingOpen {
                            epoch: page.open_epoch,
                            intent: page.intent,
                            id: "held-load".into(),
                            retry: true,
                        });
                    }
                    page.show_note("planning", Some(&id), cx);
                    page.note_section(chosen == Section::Transcript, cx);
                    assert_eq!(page.section, chosen);
                    let epoch = page.open_epoch;
                    let intent = page.intent;
                    // An unchanged Notes refresh must not restart the request.
                    page.show_note("planning", Some(&id), cx);
                    assert_eq!((page.open_epoch, page.intent), (epoch, intent));
                    if queued {
                        page.opening = None;
                        page.open_next(cx);
                    } else {
                        assert!(page.next_open.is_none());
                    }
                });
                cx.run_until_parked();
                page.read_with(cx, |page, _| {
                    assert!(page.is_for_note("planning"));
                    assert_eq!(page.section, chosen);
                    assert!(page.pending_focus.is_none());
                    assert!(page.pending_note.is_none());
                    if saved_draft {
                        assert_eq!(
                            page.document.as_ref().unwrap().text(Field::Summary),
                            "Saved summary draft"
                        );
                    }
                });
                cx.update(|_| drop(page));
            }
        }
    }
}

#[gpui::test]
fn note_selection_during_discard_or_deletion_replays_the_latest_identity_and_tab(
    cx: &mut TestAppContext,
) {
    for discard in [false, true] {
        let (_root, meetings) = storage();
        let first = recorded(&meetings, "First");
        let second = recorded(&meetings, "Second");
        let third = recorded(&meetings, "Third");
        summarized(&meetings, &third);
        let page = page(&meetings, &first, cx);
        if discard {
            type_into(&page, Field::Transcript, "Discard these edits", 0..0, cx);
        }
        page.update(cx, |page, cx| {
            if discard {
                page.discard(cx);
                assert!(page.discarding);
            } else {
                page.confirm_delete = true;
                page.delete(cx);
                assert!(page.deleting);
            }
            page.show_note("second", Some(&second), cx);
            page.show_note("third", Some(&third), cx);
            page.note_section(false, cx);
            assert_eq!(page.section, Section::Summary);
            page.show_note("third", Some(&third), cx);
        });
        cx.run_until_parked();
        page.read_with(cx, |page, _| {
            assert!(page.is_for_note("third"));
            assert_eq!(page.section, Section::Summary);
            assert!(page.pending_note.is_none());
            assert!(!page.deleting && !page.discarding && page.opening.is_none());
        });
        assert_eq!(meetings.read(&first).is_ok(), discard);
        cx.update(|_| drop(page));
    }
}

#[gpui::test]
fn failed_discard_or_delete_retains_the_selected_note_through_saving(cx: &mut TestAppContext) {
    for discard in [false, true] {
        let (root, meetings) = storage();
        let first = recorded(&meetings, "First");
        let second = recorded(&meetings, "Second");
        summarized(&meetings, &second);
        let store = Arc::new(Mutex::new(
            vorkspace_core::store::Store::open(&root.path().join("notes.db")).unwrap(),
        ));
        let first_note = store
            .lock()
            .unwrap()
            .import_recording_note(&first, "First", 1)
            .unwrap()
            .unwrap();
        let connection = rusqlite::Connection::open(root.path().join("notes.db")).unwrap();
        connection.execute_batch("CREATE TRIGGER reject_delete BEFORE DELETE ON notes BEGIN SELECT RAISE(ABORT, 'injected failure'); END;").unwrap();
        let page = page(&meetings, &first, cx);
        type_into(
            &page,
            Field::Transcript,
            "Keep these transcript edits",
            0..0,
            cx,
        );
        page.update(cx, |page, cx| {
            page.note_store = Some(store.clone());
            if discard {
                // Inject a failed I/O completion, retaining the unsaved draft.
                page.discarding = true;
            } else {
                page.confirm_delete = true;
                page.delete(cx);
                assert!(page.deleting);
            }
            page.show_note("second", Some(&second), cx);
            page.note_section(false, cx);
            if discard {
                page.complete_discard(Err("injected discard failure".into()), cx);
                assert!(page.saving);
            }
            // Refresh while the selection waits for the preceding write.
            page.show_note("second", Some(&second), cx);
        });
        cx.run_until_parked();
        page.read_with(cx, |page, _| {
            assert!(page.is_for_note("second"));
            assert_eq!(page.section, Section::Summary);
            assert!(page.pending_note.is_none());
            assert!(!page.saving && !page.deleting && !page.discarding && page.opening.is_none());
        });
        let saved: Snapshot = meetings.read(&first).unwrap();
        assert_eq!(saved.transcript, "Keep these transcript edits");
        assert!(
            store
                .lock()
                .unwrap()
                .get_note(&first_note.id)
                .unwrap()
                .is_some()
        );
        cx.update(|_| drop(page));
    }
}

#[gpui::test]
fn a_delayed_summary_does_not_replace_a_later_saved_revision(cx: &mut TestAppContext) {
    let (_root, meetings) = storage();
    let id = recorded(&meetings, "Planning");
    let old_result = summarized(&meetings, &id);
    save_edit(
        &meetings,
        &id,
        Draft {
            revision: old_result.meeting.revision,
            summary: Some("Manually edited summary".into()),
            ..Draft::default()
        },
    );
    let page = page(&meetings, &id, cx);
    page.update(cx, |page, cx| {
        let preview = page.preview.clone().unwrap();
        page.complete_analysis(page.intent, old_result, cx);
        assert_eq!(
            page.document.as_ref().unwrap().text(Field::Summary),
            "Manually edited summary"
        );
        assert_eq!(page.preview.as_ref(), Some(&preview));
    });
}

#[gpui::test]
fn returning_to_note_text_keeps_a_finished_summary_without_stealing_navigation(
    cx: &mut TestAppContext,
) {
    let (_root, meetings) = storage();
    let id = recorded(&meetings, "Planning");
    let page = page(&meetings, &id, cx);
    let intent = page.read_with(cx, |page, _| page.intent);
    let summary = summarized(&meetings, &id);
    let revision = summary.meeting.revision;
    let events = events(&page, cx);
    page.update(cx, |page, cx| {
        page.hide_note_content(cx);
        assert_ne!(page.intent, intent);
        page.complete_analysis(intent, summary, cx);
        let doc = page.document.as_ref().unwrap();
        assert_eq!(doc.saved.as_ref().unwrap().meeting.revision, revision);
        assert!(doc.text(Field::Summary).contains("release checklist"));
        assert!(page.preview.is_some());
        assert_eq!(page.section, Section::Transcript);
        assert_eq!(page.pending_focus, None);
    });
    assert_eq!(events.borrow().as_slice(), ["changed"]);
    let durable: Snapshot = meetings.read(&id).unwrap();
    assert_eq!(durable.meeting.revision, revision);
    assert!(durable.meeting.summary.is_some());
}

#[gpui::test]
fn finishing_an_unselected_recording_refreshes_notes_without_changing_the_open_document(
    cx: &mut TestAppContext,
) {
    let (_root, meetings) = storage();
    let selected = recorded(&meetings, "Selected note");
    let finished = recorded(&meetings, "Other recording");
    let snapshot: Snapshot = meetings.read(&finished).unwrap();
    let page = page(&meetings, &selected, cx);
    let events = events(&page, cx);
    page.update(cx, |page, cx| {
        let lease = page.document.as_ref().unwrap().lease.clone().unwrap();
        page.observe_live(
            State {
                phase: Phase::Recording,
                ..snapshot.state.clone()
            },
            cx,
        );
        page.finish_live(snapshot.state, cx);
        let doc = page.document.as_ref().unwrap();
        assert_eq!(doc.id(), selected);
        assert!(Arc::ptr_eq(doc.lease.as_ref().unwrap(), &lease));
        assert!(!doc.awaiting_transcript);
        assert!(page.opening.is_none());
        assert_eq!(page.live.as_ref().unwrap().state.id, finished);
        assert_eq!(page.live.as_ref().unwrap().state.phase, Phase::Stopped);
    });
    assert_eq!(events.borrow().as_slice(), ["changed"]);
}

#[gpui::test]
fn deleting_a_recording_leaves_the_next_note_to_the_notes_owner(cx: &mut TestAppContext) {
    let (_root, meetings) = storage();
    let selected = recorded(&meetings, "Deleted note");
    let other = recorded(&meetings, "Unrelated note");
    let page = page(&meetings, &selected, cx);
    let events = events(&page, cx);
    page.update(cx, |page, cx| {
        page.note_ids
            .insert("deleted-note".into(), selected.clone());
        page.confirm_delete = true;
        page.delete(cx);
    });
    cx.run_until_parked();
    page.read_with(cx, |page, _| {
        assert!(page.document.is_none());
        assert!(page.next_open.is_none());
        assert!(page.opening.is_none() && !page.deleting);
    });
    assert_eq!(events.borrow().as_slice(), ["deleted"]);
    assert!(meetings.read(&selected).is_err());
    let retained: Snapshot = meetings.read(&other).unwrap();
    assert_eq!(retained.meeting.id, other);
    assert_eq!(retained.meeting.title, "Unrelated note");
}

#[gpui::test]
fn an_unrecorded_note_can_be_deleted_before_its_first_saved_snapshot(cx: &mut TestAppContext) {
    for materialized in [false, true] {
        let (_root, meetings) = storage();
        let page = cx.new(|cx| Page::with_meetings(Ok(meetings.clone()), cx));
        let events = events(&page, cx);
        let id = page.update(cx, |page, cx| {
            page.show_note("unsaved-note", None, cx);
            let doc = page.document.as_ref().unwrap();
            assert!(doc.saved.is_none() && doc.unrecorded() && doc.editable());
            let id = doc.id().to_owned();
            if materialized {
                doc.lease
                    .as_ref()
                    .unwrap()
                    .materialize(&meetings, &id)
                    .unwrap();
                assert!(meetings.read(&id).is_ok());
            } else {
                assert!(meetings.read(&id).is_err());
            }
            page.confirm_delete = true;
            page.delete(cx);
            id
        });
        cx.run_until_parked();
        page.read_with(cx, |page, _| {
            assert!(page.document.is_none());
            assert!(!page.deleting && page.opening.is_none());
        });
        assert_eq!(events.borrow().as_slice(), ["deleted"]);
        assert!(meetings.read(&id).is_err());
    }
}

#[gpui::test]
fn legacy_title_drafts_survive_without_focusing_a_removed_title_editor(cx: &mut TestAppContext) {
    let (_root, meetings) = storage();
    let id = recorded(&meetings, "Planning");
    let snapshot: Snapshot = meetings.read(&id).unwrap();
    let write = DraftWrite::new(
        id.clone(),
        None,
        "0123456789abcdef0123456789abcdef".into(),
        Draft {
            revision: snapshot.meeting.revision,
            title: Some("Legacy title edit".into()),
            title_focused: true,
            start: 2,
            end: 5,
            ..Draft::default()
        },
    )
    .unwrap();
    meetings.checkpoint_draft(&write).unwrap();
    let page = page(&meetings, &id, cx);
    page.read_with(cx, |page, cx| {
        let doc = page.document.as_ref().unwrap();
        assert_eq!(
            doc.draft.as_ref().unwrap().title.as_deref(),
            Some("Legacy title edit")
        );
        assert!(doc.dirty() && doc.has_content());
        assert_eq!(page.pending_focus, Some(Field::Transcript));
        assert_eq!(page.transcript.read(cx).selection_utf16(), 0..0);
        assert_eq!(doc.text(Field::Transcript), snapshot.transcript);
    });
    type_into(&page, Field::Transcript, "New transcript edit", 2..4, cx);
    page.update(cx, |page, _| page.drafts.flush(&meetings).unwrap());
    let reopened: Snapshot = meetings.open_document(&id).unwrap();
    let draft = reopened.draft.unwrap();
    assert_eq!(draft.title.as_deref(), Some("Legacy title edit"));
    assert_eq!(draft.transcript.as_deref(), Some("New transcript edit"));
    assert!(!draft.title_focused);
}

#[gpui::test]
fn sharing_uses_the_selected_recording_text_and_waits_for_final_transcripts(
    cx: &mut TestAppContext,
) {
    let (_root, meetings) = storage();
    let id = recorded(&meetings, "Planning");
    let saved = summarized(&meetings, &id);
    let page = page(&meetings, &id, cx);
    page.update(cx, |page, cx| {
        page.note_section(false, cx);
        assert_eq!(
            page.selected_text(),
            Some(saved.meeting.summary.as_ref().unwrap().text.as_str())
        );
        page.note_section(true, cx);
        assert_eq!(page.selected_text(), Some(saved.transcript.as_str()));
        page.observe_live(
            State::starting(id, "next-take".into(), CaptureSource::Microphone),
            cx,
        );
        assert_eq!(page.selected_text(), None);
    });
}

#[gpui::test]
fn document_editors_keep_the_storage_byte_limits_and_accept_long_lines(cx: &mut TestAppContext) {
    let (_root, meetings) = storage();
    let id = recorded(&meetings, "Planning");
    let page = page(&meetings, &id, cx);
    page.update(cx, |page, cx| {
        let long_line = "🦊".repeat(6000);
        page.transcript.update(cx, |input, cx| {
            input.set_text(long_line.clone(), cx);
            input.replace_bytes(0..0, "a", 1, cx);
            assert_eq!(input.content.as_ref(), format!("a{long_line}"));
        });
        page.summary.update(cx, |input, cx| {
            input.set_text("s".repeat(64 * 1024), cx);
            input.replace_bytes(0..0, "a", 1, cx);
            assert_eq!(input.content.len(), 64 * 1024);
            assert!(input.content.starts_with('s'));
        });
    });
}

#[gpui::test]
fn hiding_notes_and_reselecting_the_same_note_preserve_its_recording_draft(
    cx: &mut TestAppContext,
) {
    let (_root, meetings) = storage();
    let id = recorded(&meetings, "Planning");
    let saved = summarized(&meetings, &id);
    let page = page(&meetings, &id, cx);
    page.update(cx, |page, cx| {
        page.navigate(Action::Section(Section::Source), cx)
    });
    type_into(&page, Field::Summary, "a🦊b unsaved summary", 1..3, cx);
    page.update(cx, |page, cx| {
        let doc = page.document.as_ref().unwrap();
        let lease = doc.lease.clone().unwrap();
        let version = doc.version.clone();
        let preview = page.preview.clone();
        page.set_active(false, cx);
        page.set_active(true, cx);
        page.show_note("selected-note", Some(&id), cx);

        let doc = page.document.as_ref().unwrap();
        assert!(page.active);
        assert!(!page.saving && page.opening.is_none());
        assert!(Arc::ptr_eq(doc.lease.as_ref().unwrap(), &lease));
        assert_eq!(doc.version, version);
        assert_eq!(doc.text(Field::Summary), "a🦊b unsaved summary");
        assert_eq!(page.section, Section::Source);
        assert_eq!(page.input(Field::Summary).read(cx).selection_utf16(), 1..3);
        assert_eq!(page.preview, preview);
        let current: Snapshot = meetings.read(&id).unwrap();
        assert_eq!(current.meeting.revision, saved.meeting.revision);
    });
}

#[gpui::test]
fn clearing_a_note_waits_for_its_recording_draft_before_releasing_the_lease(
    cx: &mut TestAppContext,
) {
    let (_root, meetings) = storage();
    let id = recorded(&meetings, "Planning");
    let page = page(&meetings, &id, cx);
    page.update(cx, |page, cx| page.show_note("planning", Some(&id), cx));
    type_into(
        &page,
        Field::Transcript,
        "Unfinished transcript edit",
        3..3,
        cx,
    );
    page.update(cx, |page, cx| {
        assert!(page.document.as_ref().unwrap().dirty());
        page.clear_note(cx);
        assert!(page.pending_note.is_some());
    });
    cx.run_until_parked();
    page.read_with(cx, |page, _| {
        assert!(!page.is_for_note("planning"));
        assert!(!page.saving);
        assert!(page.pending_note.is_none());
        assert_ne!(page.document.as_ref().unwrap().id(), id);
    });
    let saved: Snapshot = meetings.open_document(&id).unwrap();
    assert_eq!(saved.transcript, "Unfinished transcript edit");
    assert!(meetings.try_edit(&id).unwrap().is_some());
}

#[gpui::test]
fn note_close_waits_for_a_recording_edit_save(cx: &mut TestAppContext) {
    let (_root, meetings) = storage();
    let id = recorded(&meetings, "Planning");
    let page = page(&meetings, &id, cx);
    page.update(cx, |page, cx| page.show_note("planning", Some(&id), cx));
    type_into(
        &page,
        Field::Transcript,
        "Transcript changed before close",
        3..3,
        cx,
    );
    page.update(cx, |page, cx| {
        assert!(page.document.as_ref().unwrap().dirty());
        assert!(page.has_pending_note_edit("planning"));
        assert!(!page.has_pending_note_edit("other-note"));
        assert!(!page.prepare_note_close(cx));
        assert!(page.saving);
    });
    cx.run_until_parked();
    page.update(cx, |page, cx| {
        assert!(!page.saving);
        assert!(!page.document.as_ref().unwrap().dirty());
        assert!(!page.has_pending_note_edit("planning"));
        assert!(page.prepare_note_close(cx));
    });
    let saved: Snapshot = meetings.open_document(&id).unwrap();
    assert_eq!(saved.transcript, "Transcript changed before close");
}

#[gpui::test]
fn a_blank_note_recording_never_materializes_until_there_is_content(cx: &mut TestAppContext) {
    let (_root, meetings) = storage();
    let page = cx.new(|cx| Page::with_meetings(Ok(meetings.clone()), cx));
    let id = page.update(cx, |page, cx| {
        page.show_note("new-note", None, cx);
        let doc = page.document.as_ref().unwrap();
        assert!(page.editable() && doc.unrecorded());
        assert_eq!(page.transcript.read(cx).content.as_ref(), "");
        doc.id().to_owned()
    });
    assert!(meetings.read(&id).is_err());

    type_into(&page, Field::Transcript, "  ", 2..2, cx);
    page.update(cx, |page, _| page.drafts.flush(&meetings).unwrap());
    assert!(meetings.read(&id).is_err());
    assert!(
        meetings
            .note_candidates(&Default::default())
            .unwrap()
            .is_empty()
    );

    type_into(&page, Field::Transcript, "Planning", 8..8, cx);
    page.update(cx, |page, _| page.drafts.flush(&meetings).unwrap());
    let saved: Snapshot = meetings.open_document(&id).unwrap();
    assert!(saved.meeting.title.is_empty());
    assert_eq!(saved.draft.unwrap().transcript.as_deref(), Some("Planning"));
    assert!(meetings.try_edit(&id).unwrap().is_none());
}

#[gpui::test]
fn a_resumed_run_keeps_the_corrected_document_and_read_only_baseline(cx: &mut TestAppContext) {
    let (_root, meetings) = storage();
    let id = recorded(&meetings, "Planning");
    let prior: Snapshot = meetings.read(&id).unwrap();
    save_edit(
        &meetings,
        &id,
        Draft {
            revision: prior.meeting.revision,
            transcript: Some("Corrected discussion".into()),
            ..Draft::default()
        },
    );
    let page = page(&meetings, &id, cx);
    page.update(cx, |page, cx| {
        let lease = page.document.as_ref().unwrap().lease.clone().unwrap();
        page.observe_live(
            State::starting(id.clone(), "fixture-take".into(), CaptureSource::Microphone),
            cx,
        );
        let doc = page.document.as_ref().unwrap();
        assert_eq!(doc.id(), id);
        assert!(!page.editable());
        assert_eq!(doc.text(Field::Transcript), "Corrected discussion");
        assert!(Arc::ptr_eq(doc.lease.as_ref().unwrap(), &lease));
        assert_eq!(
            page.live.as_ref().unwrap().prefix.as_deref(),
            Some("Corrected discussion")
        );
    });
}
