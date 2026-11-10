//! Notes owns the list and titles; recording files retain their original identity.
use super::*;

type Loaded = (Vec<Note>, HashMap<String, String>);

pub(super) fn load(store: &Database) -> Result<Loaded, String> {
    let (root, known, deleted) = {
        let store = locked(store)?;
        let links = store.note_recordings()?;
        let ids = if links.iter().any(|link| !link.deleted) {
            store.note_ids()?
        } else {
            HashSet::new()
        };
        let deleted: Vec<_> = links
            .iter()
            .filter(|link| link.deleted)
            .map(|link| link.recording_id.clone())
            .collect();
        let known: HashSet<_> = links
            .into_iter()
            .filter(|link| link.deleted || ids.contains(&link.note_id))
            .map(|link| link.recording_id)
            .collect();
        (store.data_dir().map(PathBuf::from), known, deleted)
    };
    if let Some(root) = root {
        let meetings = vorkspace_meetings::Meetings::open(&root)?;
        for id in deleted {
            // SQLite is the durable deletion authority. A crash or file error
            // after its commit leaves cleanup to the next load, never import.
            let _ = meetings.delete_with_commit(&id, None, || Ok(()));
        }
        for candidate in meetings.note_candidates(&known)? {
            // Idempotent publication preserves any existing note edits. The
            // recording remains the authority for its transcript and summary.
            locked(store)?.import_recording_note(
                &candidate.recording_id,
                &candidate.title,
                candidate.created_at.min(i64::MAX as u64) as i64,
            )?;
        }
    }
    let store = locked(store)?;
    let links = store
        .note_recordings()?
        .into_iter()
        .filter(|link| !link.deleted)
        .map(|link| (link.note_id, link.recording_id))
        .collect();
    Ok((store.list_notes()?, links))
}

/// Pinned panes only need their own document. Repeating the library's full
/// recording import and note-body load for every open tab would retain every
/// other note's body in each pane.
pub(super) fn load_pinned(store: &Database, note_id: &str) -> Result<Loaded, String> {
    let store = locked(store)?;
    let notes = store.get_note(note_id)?.into_iter().collect();
    let links = store
        .note_recording(note_id)?
        .into_iter()
        .filter(|link| !link.deleted)
        .map(|link| (link.note_id, link.recording_id))
        .collect();
    Ok((notes, links))
}

impl Notes {
    pub(crate) fn configure_recording(&mut self, hidden: &[String], cx: &mut Context<Self>) {
        self.recording
            .update(cx, |page, _| page.configure_context(&self.cwd, hidden));
    }

    pub(crate) fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.recording
            .update(cx, |page, cx| page.set_active(active, cx));
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.recording
            .update(cx, |page, cx| page.set_enabled(enabled, cx));
    }

    pub(crate) fn new_recording_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.create(window, cx);
    }

    pub(crate) fn open_active_recording(&mut self, cx: &mut Context<Self>) {
        let Some(state) = crate::meetings::capture::current(cx) else {
            return;
        };
        let result = (|| {
            let store = locked(&self.store)?;
            let Some(link) = store
                .note_recordings()?
                .into_iter()
                .find(|link| link.recording_id == state.id && !link.deleted)
            else {
                return Ok(None);
            };
            let saved = store.get_note(&link.note_id)?;
            let ephemeral = saved.is_none();
            let note = saved.unwrap_or(Note {
                id: link.note_id,
                slug: String::new(),
                title: String::new(),
                body: String::new(),
                tags: vec![],
                source_session_id: None,
                source_cwd: None,
                slug_pending: true,
                created_at: state.started_at.min(i64::MAX as u64) as i64,
                updated_at: state.started_at.min(i64::MAX as u64) as i64,
            });
            Ok::<_, String>(Some((note, ephemeral)))
        })();
        match result {
            Ok(Some((note, ephemeral))) => {
                // The durable link, not a possibly stale view cache, decides
                // whether this recording belongs to a pinned document tab.
                if self.pinned.as_ref().is_some_and(|id| id != &note.id) {
                    return;
                }
                if self.pinned.is_none() && self.open_tabs.contains(&note.id) {
                    cx.emit(Event::OpenTab(note.id));
                    return;
                }
                self.flush(cx);
                if self.dirty {
                    return;
                }
                let id = note.id.clone();
                if !self.notes.iter().any(|saved| saved.id == id) {
                    if ephemeral {
                        self.ephemeral.insert(id.clone());
                    }
                    self.notes.insert(0, note);
                }
                self.recordings.insert(id.clone(), state.id);
                self.load_selection(Some(id), cx);
                self.show_recording = true;
                self.recording
                    .update(cx, |page, cx| page.note_section(true, cx));
            }
            Ok(None) => self.refresh(cx),
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    pub(super) fn discard_empty_draft(&mut self, cx: &App) {
        let Some(note) = self
            .draft(cx)
            .filter(|note| self.ephemeral.contains(&note.id))
        else {
            return;
        };
        if note.title.trim().is_empty()
            && note.body.trim().is_empty()
            && note.tags.is_empty()
            && !self.recordings.contains_key(&note.id)
            && self.image_note.as_ref() != Some(&note.id)
        {
            self.notes.retain(|saved| saved.id != note.id);
            self.ephemeral.remove(&note.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use vorkspace_core::{notes::NoteUpsert, store::Store};
    use vorkspace_meetings::{
        Meetings,
        recording::{CaptureSource, Phase},
    };

    fn fixture() -> (tempfile::TempDir, Database, Meetings) {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(
            Store::open(&root.path().join("notes.db")).unwrap(),
        ));
        let meetings = Meetings::open(root.path()).unwrap();
        (root, store, meetings)
    }

    #[test]
    fn deletion_commit_failure_keeps_the_note_and_recording_recoverable() {
        let (root, store, meetings) = fixture();
        let id = "0123456789abcdef0123456789abcdef";
        meetings.prepare_document(id).unwrap();
        let note = locked(&store)
            .unwrap()
            .import_recording_note(id, "Planning", 1)
            .unwrap()
            .unwrap();
        let connection = rusqlite::Connection::open(root.path().join("notes.db")).unwrap();
        connection.execute_batch("CREATE TRIGGER reject_delete BEFORE DELETE ON notes BEGIN SELECT RAISE(ABORT, 'injected failure'); END;").unwrap();
        let result =
            meetings.delete_with_commit(id, Some(0), || locked(&store)?.delete_note(&note.id));
        assert!(result.unwrap_err().contains("injected failure"));
        assert!(meetings.read(id).is_ok());
        assert!(
            locked(&store)
                .unwrap()
                .get_note(&note.id)
                .unwrap()
                .is_some()
        );
        assert!(!locked(&store).unwrap().note_recordings().unwrap()[0].deleted);
        connection
            .execute_batch("DROP TRIGGER reject_delete")
            .unwrap();
        meetings
            .delete_with_commit(id, Some(0), || locked(&store)?.delete_note(&note.id))
            .unwrap();
        assert!(load(&store).unwrap().0.is_empty());
        assert!(meetings.read(id).is_err());
    }

    #[test]
    fn deletion_recovers_after_the_database_commit_without_a_view_callback() {
        let (root, store, meetings) = fixture();
        let id = "0123456789abcdef0123456789abcdef";
        meetings.prepare_document(id).unwrap();
        let note = locked(&store)
            .unwrap()
            .import_recording_note(id, "Planning", 1)
            .unwrap()
            .unwrap();
        let result = meetings.delete_with_commit(id, Some(0), || {
            locked(&store)?.delete_note(&note.id)?;
            Err("interrupted after commit".into())
        });
        assert!(result.is_err());
        assert!(
            meetings.read(id).is_ok(),
            "no files removed before the interruption"
        );
        drop(store);
        let reopened = Arc::new(Mutex::new(
            Store::open(&root.path().join("notes.db")).unwrap(),
        ));
        for _ in 0..2 {
            let (notes, links) = load(&reopened).unwrap();
            assert!(notes.is_empty());
            assert!(links.is_empty());
            assert!(meetings.read(id).is_err());
            assert!(!root.path().join("recordings").join(id).exists());
            assert!(locked(&reopened).unwrap().note_recordings().unwrap()[0].deleted);
        }
    }

    #[test]
    fn recording_import_preserves_note_text_and_does_not_revive_deletion() {
        let (_root, store, meetings) = fixture();
        let id = meetings.prepare("Planning").unwrap();
        let note = locked(&store)
            .unwrap()
            .upsert_note(&NoteUpsert {
                id: "my-note".into(),
                title: "My agenda".into(),
                body: "Keep this text".into(),
                ..Default::default()
            })
            .unwrap();
        locked(&store)
            .unwrap()
            .attach_note_recording(&note.id, &id)
            .unwrap();
        for _ in 0..2 {
            let (notes, links) = load(&store).unwrap();
            assert_eq!(notes.as_slice(), std::slice::from_ref(&note));
            assert_eq!(links.get(&note.id), Some(&id));
        }
        locked(&store).unwrap().delete_note(&note.id).unwrap();
        let (notes, links) = load(&store).unwrap();
        assert!(notes.is_empty());
        assert!(links.is_empty());
    }

    #[test]
    fn empty_recording_is_not_saved_but_its_first_words_use_the_reserved_note() {
        let (_root, store, meetings) = fixture();
        let id = "0123456789abcdef0123456789abcdef";
        locked(&store)
            .unwrap()
            .attach_note_recording("draft-note", id)
            .unwrap();
        meetings.prepare_document(id).unwrap();
        let mut owner = meetings
            .start_recording(id, "take-one", None, CaptureSource::Microphone)
            .unwrap();
        assert!(load(&store).unwrap().0.is_empty());
        owner
            .append("microphone", 0, 1200, "Taylor owns the release.".into())
            .unwrap();
        owner.phase(Phase::Stopped, None).unwrap();
        drop(owner);
        let (notes, links) = load(&store).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].id, "draft-note");
        assert!(
            notes[0].body.is_empty(),
            "recording does not replace authored note text"
        );
        assert_eq!(links.get("draft-note").map(String::as_str), Some(id));
        let next = load(&store).unwrap();
        assert_eq!(next.0, notes);
        assert!(
            meetings
                .read(id)
                .unwrap()
                .transcript
                .contains("Taylor owns")
        );
    }

    #[test]
    fn legacy_import_keeps_pending_titles_and_transcript_only_drafts() {
        let (_root, store, meetings) = fixture();
        for (title, draft) in [
            (
                "Old title",
                serde_json::json!({"title": "My pending title"}),
            ),
            (
                "",
                serde_json::json!({"transcript": "My pending transcript"}),
            ),
        ] {
            let id = meetings.prepare(title).unwrap();
            meetings.prepare_document(&id).unwrap();
            let mut draft = draft;
            draft["revision"] = 0.into();
            draft["view"] = "transcript".into();
            draft["title_focused"] = false.into();
            draft["start"] = 0.into();
            draft["end"] = 0.into();
            let drafts = meetings.directory.join("drafts");
            std::fs::create_dir_all(&drafts).unwrap();
            let slot = serde_json::json!({
                "version": "0123456789abcdef0123456789abcdef",
                "draft": draft,
            });
            std::fs::write(drafts.join(format!("{id}.json")), slot.to_string()).unwrap();
            let (notes, links) = load(&store).unwrap();
            let note_id = links
                .iter()
                .find(|(_, recording)| **recording == id)
                .unwrap()
                .0;
            let note = notes.iter().find(|note| &note.id == note_id).unwrap();
            if !title.is_empty() {
                assert_eq!(note.title, "My pending title");
            }
            assert!(note.body.is_empty());
            let reopened = meetings.open_document(&id).unwrap().draft.unwrap();
            assert_eq!(serde_json::to_value(reopened).unwrap(), draft);
        }
    }
}
