//! Read only the note payloads requested by the library, editor or composer.
use super::{read_note, valid_id, Note, Store};
use crate::notes::NoteSummary;
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

        Ok(notes)
    }

    pub fn list_note_summaries(&self) -> Result<Vec<NoteSummary>, String> {
        let mut notes = Vec::new();
        if let Some(NoteColumns { tags, filter, .. }) = NoteColumns::discover(&self.conn)? {
            let mut query = self
                .conn
                .prepare(&format!(
                    "SELECT id,slug,title,{tags},source_cwd,updated_at FROM notes
                     WHERE {filter} ORDER BY updated_at DESC,id ASC"
                ))
                .map_err(|error| error.to_string())?;
            notes = query
                .query_map([], |row| {
                    let tags: String = row.get(3)?;
                    Ok(NoteSummary {
                        id: row.get(0)?,
                        slug: row.get(1)?,
                        title: row.get(2)?,
                        tags: serde_json::from_str(&tags).unwrap_or_default(),
                        source_cwd: row.get(4)?,
                        updated_at: row.get(5)?,
                    })
                })
                .map_err(|error| error.to_string())?
                .collect::<rusqlite::Result<_>>()
                .map_err(|error| error.to_string())?;
        }

        Ok(notes)
    }

    /// Identity-only view for reconciling recording links.
    pub fn note_ids(&self) -> Result<HashSet<String>, String> {
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
        if let Some(columns) = NoteColumns::discover(&self.conn)? {
            let note = self
                .conn
                .prepare(&format!("{} AND id=?1", columns.select()))
                .and_then(|mut query| query.query_row([id], read_note).optional())
                .map_err(|error| error.to_string())?;
            if note.is_some() {
                return Ok(note);
            }
        }
        Ok(None)
    }

    /// Resolve each unique mention in prompt order from one read snapshot.
    pub fn apply_notes_to_turn(&self, text: &str) -> Result<String, String> {
        let slugs = crate::notes::slugs_in_text(text);
        if slugs.is_empty() {
            return Ok(text.into());
        }
        let picked = {
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

    /// Old saved drafts can still contain numeric references. Read the
    /// original user request so note bodies cannot select additional notes.
    pub fn apply_legacy_notes_to_turn(
        &self,
        request: &str,
        prompt: String,
    ) -> Result<String, String> {
        if !request.contains("@note:") {
            return Ok(prompt);
        }
        let ids: HashSet<u64> = request
            .lines()
            .filter(|line| !line.trim_start().starts_with('>'))
            .flat_map(str::split_whitespace)
            .filter_map(|word| {
                word.trim_end_matches([',', '.', ';'])
                    .strip_prefix("@note:")
            })
            .filter(|id| id.bytes().all(|byte| byte.is_ascii_digit()))
            .filter(|id| id.len() == 1 || !id.starts_with('0'))
            .filter_map(|id| id.parse().ok())
            .collect();
        if ids.is_empty() {
            return Ok(prompt);
        }

        use std::fmt::Write as _;
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|error| error.to_string())?;
        let mut expanded = String::new();
        for entry in self
            .list_note_summaries()?
            .into_iter()
            .rev()
            .filter(|entry| ids.contains(&crate::notes::numeric_id(&entry.id)))
        {
            if let Some(note) = self.get_note(&entry.id)? {
                write!(
                    expanded,
                    "Referenced note: {}\n<note>\n{}\n</note>\n\n",
                    note.title,
                    crate::response::limit(&note.body, 64_000),
                )
                .expect("writing to a String succeeds");
            }
        }
        tx.commit().map_err(|error| error.to_string())?;
        if expanded.is_empty() {
            return Ok(prompt);
        }
        expanded.push_str(&prompt);
        Ok(expanded)
    }
}
