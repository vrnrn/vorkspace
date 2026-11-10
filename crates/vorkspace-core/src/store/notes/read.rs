//! Read only the note payloads requested by the library, editor or composer.
use super::{read_note, valid_id, Note, Store};
use rusqlite::{Connection, OptionalExtension};
use std::collections::HashSet;

struct NoteColumns {
    tags: &'static str,
    pending: &'static str,
    filter: &'static str,
}

impl NoteColumns {
    fn discover(conn: &Connection) -> Result<Option<Self>, String> {
        if !super::super::shared::has_table(conn, "notes")? {
            return Ok(None);
        }
        let columns = conn
            .prepare("PRAGMA table_info(notes)")
            .map_err(|error| error.to_string())?
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|error| error.to_string())?
            .collect::<rusqlite::Result<HashSet<_>>>()
            .map_err(|error| error.to_string())?;
        Ok(Some(Self {
            tags: if columns.contains("tags_json") {
                "tags_json"
            } else {
                "'[]'"
            },
            pending: if columns.contains("slug_pending") {
                "slug_pending"
            } else {
                "0"
            },
            filter: if columns.contains("content_kind") {
                "content_kind='note'"
            } else {
                "1"
            },
        }))
    }

    fn select(&self) -> String {
        let Self {
            tags,
            pending,
            filter,
        } = self;
        format!("SELECT id,slug,title,body,source_session_id,source_cwd,{tags},created_at,updated_at,{pending} FROM notes WHERE {filter}")
    }
}

impl Store {
    /// Full-fidelity library records. Artifacts use a separate read path.
    pub fn list_notes(&self) -> Result<Vec<Note>, String> {
        let mut notes = Vec::new();
        if let Some(columns) = NoteColumns::discover(&self.conn)? {
            let mut query = self
                .conn
                .prepare(&format!(
                    "{} ORDER BY updated_at DESC,id ASC",
                    columns.select()
                ))
                .map_err(|error| error.to_string())?;
            notes = query
                .query_map([], read_note)
                .map_err(|error| error.to_string())?
                .collect::<rusqlite::Result<_>>()
                .map_err(|error| error.to_string())?;
        }
        if self.shared.is_none() {
            let mut ids: HashSet<_> = notes.iter().map(|note| note.id.clone()).collect();
            for note in self.legacy_notes()?.into_iter().map(legacy_note) {
                if ids.insert(note.id.clone()) {
                    notes.push(note);
                }
            }
            notes.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(a.id.cmp(&b.id)));
        }
        Ok(notes)
    }

    /// Identity-only view for reconciling recording links.
    pub fn note_ids(&self) -> Result<HashSet<String>, String> {
        if self.shared.is_none() {
            return Ok(self.list_notes()?.into_iter().map(|note| note.id).collect());
        }
        let Some(columns) = NoteColumns::discover(&self.conn)? else {
            return Ok(HashSet::new());
        };
        let mut query = self
            .conn
            .prepare(&format!("SELECT id FROM notes WHERE {}", columns.filter))
            .map_err(|error| error.to_string())?;
        let ids = query
            .query_map([], |row| row.get(0))
            .map_err(|error| error.to_string())?
            .collect::<rusqlite::Result<_>>()
            .map_err(|error| error.to_string());
        ids
    }

    pub fn get_note(&self, id: &str) -> Result<Option<Note>, String> {
        valid_id(id)?;
        if self.shared.is_none() {
            return Ok(self.list_notes()?.into_iter().find(|note| note.id == id));
        }
        let Some(columns) = NoteColumns::discover(&self.conn)? else {
            return Ok(None);
        };
        self.conn
            .prepare(&format!("{} AND id=?1", columns.select()))
            .and_then(|mut query| query.query_row([id], read_note).optional())
            .map_err(|error| error.to_string())
    }

    /// Resolve each unique mention in prompt order from one read snapshot.
    pub fn apply_notes_to_turn(&self, text: &str) -> Result<String, String> {
        let slugs = crate::notes::slugs_in_text(text);
        if slugs.is_empty() {
            return Ok(text.into());
        }
        let picked = if self.shared.is_none() {
            let notes = self.list_notes()?;
            slugs
                .iter()
                .filter_map(|slug| notes.iter().find(|note| &note.slug == slug).cloned())
                .collect()
        } else {
            let tx = self
                .conn
                .unchecked_transaction()
                .map_err(|error| error.to_string())?;
            let Some(columns) = NoteColumns::discover(&tx)? else {
                return Ok(text.into());
            };
            let mut notes = Vec::new();
            {
                let mut query = tx
                    .prepare(&format!("{} AND slug=?1", columns.select()))
                    .map_err(|error| error.to_string())?;
                for slug in slugs {
                    notes.extend(
                        query
                            .query_row([slug], read_note)
                            .optional()
                            .map_err(|error| error.to_string())?,
                    );
                }
            }
            tx.commit().map_err(|error| error.to_string())?;
            notes
        };
        Ok(crate::notes::inject_prompt(text, &picked))
    }
}

fn legacy_note(note: crate::response::Note) -> Note {
    let at = note.id.min(i64::MAX as u64) as i64;
    Note {
        id: format!("gpui-note-{}", note.id),
        slug: format!("note-{}", note.id),
        title: note.title,
        body: note.body,
        tags: note.tags,
        source_session_id: (note.source_session != 0).then(|| note.source_session.to_string()),
        source_cwd: (!note.source_cwd.is_empty()).then_some(note.source_cwd),
        slug_pending: false,
        created_at: at,
        updated_at: at,
    }
}
