//! Discovery of published recording documents that do not yet have a note.
use super::*;
use std::collections::HashSet;

#[derive(Debug, PartialEq, Eq)]
pub struct NoteCandidate {
    pub recording_id: String,
    pub title: String,
    pub created_at: u64,
}

impl Meetings {
    pub fn note_candidates(&self, linked: &HashSet<String>) -> Result<Vec<NoteCandidate>, String> {
        let root = self.directory.join("recordings");
        let dir = match directory::Directory::open(&root, false) {
            Ok(dir) => dir,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.to_string()),
        };
        let mut rows = Vec::new();
        for entry in dir.entries().map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let id = entry.file_name().to_string_lossy();
            if let Some(retired) = id.strip_prefix(".deleted-").filter(|id| valid_id(id)) {
                let _ = self.delete(retired, 0);
                continue;
            }
            if !valid_id(&id) || linked.contains(id.as_ref()) {
                continue;
            }
            // Capture publishes state.json last. A damaged published record
            // remains discoverable without interrupting the rest of the catalog.
            let marker = root.join(id.as_ref()).join("state.json");
            if !marker.exists() {
                continue;
            }
            match self.note_candidate(&id) {
                Ok(Some(row)) => rows.push(row),
                Ok(None) => {}
                Err(_) if marker.exists() => rows.push(NoteCandidate {
                    recording_id: id.into_owned(),
                    title: "Unreadable meeting".into(),
                    created_at: 0,
                }),
                Err(_) => {}
            }
        }
        rows.sort_by_key(|row| std::cmp::Reverse(row.created_at));
        Ok(rows)
    }

    fn note_candidate(&self, id: &str) -> Result<Option<NoteCandidate>, String> {
        let _lock = self.lock()?;
        let state = recording::state(&self.directory, id)?;
        let meeting = self.metadata(id)?;
        let draft = self.catalog_edits(id, meeting.applied_draft.as_ref())?;
        let titled = match draft.title.as_deref() {
            Some(title) => named(title, true),
            None => named(&meeting.title, meeting.title_set),
        };
        let summarized = draft.summary.unwrap_or_else(|| {
            meeting
                .summary
                .as_ref()
                .is_some_and(|summary| !summary.text.trim().is_empty())
        });
        if !titled && !summarized {
            let has_text = match draft.transcript {
                Some(has_text) => has_text,
                None => recording::has_saved_text(
                    &self.directory,
                    id,
                    meeting.transcript_override.as_deref(),
                    meeting.override_sequence,
                    meeting.transcript_sequence,
                )?,
            };
            if !has_text {
                return Ok(None);
            }
        }
        let title = draft.title.unwrap_or_else(|| {
            if meeting.title.trim().is_empty() {
                "Untitled meeting".into()
            } else {
                meeting.title
            }
        });
        let created_at = if state.started_at == 0 {
            meeting.created_at
        } else {
            state.started_at
        };
        Ok(Some(NoteCandidate {
            recording_id: id.into(),
            title,
            created_at,
        }))
    }
}
