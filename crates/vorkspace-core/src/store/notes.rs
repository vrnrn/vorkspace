//! Desktop-compatible note persistence, deliberately separate from session handles.
mod read;

use super::Store;
use crate::notes::{normalize_tags, Note, NoteImageAsset, NoteUpsert, MAX_BODY, MAX_TITLE};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};

/// A recording retains its note identity after deletion so recovery never
/// imports it into a new note. A live link may precede its first saved note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteRecording {
    pub note_id: String,
    pub recording_id: String,
    pub deleted: bool,
}

fn now_millis() -> i64 {
    crate::time::now_ms().min(i64::MAX as u64) as i64
}
pub fn ensure_notes_table(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS notes (
           id TEXT PRIMARY KEY,
           slug TEXT NOT NULL UNIQUE,
           title TEXT NOT NULL,
           body TEXT NOT NULL DEFAULT '',
           tags_json TEXT NOT NULL DEFAULT '[]',
           source_session_id TEXT,
           source_cwd TEXT,
           slug_pending INTEGER NOT NULL DEFAULT 0,
           created_at INTEGER NOT NULL,
           updated_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS notes_updated_idx
           ON notes (updated_at DESC, id);",
    )?;
    let tags_present: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('notes') WHERE name = 'tags_json'",
        [],
        |row| row.get(0),
    )?;
    if tags_present == 0 {
        conn.execute(
            "ALTER TABLE notes ADD COLUMN tags_json TEXT NOT NULL DEFAULT '[]'",
            [],
        )?;
    }
    let pending_present: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('notes') WHERE name = 'slug_pending'",
        [],
        |row| row.get(0),
    )?;
    if pending_present == 0 {
        // Existing slugs may already be referenced, so every existing row
        // starts established (0) whatever its slug looks like.
        conn.execute(
            "ALTER TABLE notes ADD COLUMN slug_pending INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    let kind_present: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('notes') WHERE name = 'content_kind'",
        [],
        |row| row.get(0),
    )?;
    if kind_present == 0 {
        conn.execute(
            "ALTER TABLE notes ADD COLUMN content_kind TEXT NOT NULL DEFAULT 'note'",
            [],
        )?;
    }
    let artifact_kind_present: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('notes') WHERE name = 'artifact_kind'",
        [],
        |row| row.get(0),
    )?;
    if artifact_kind_present == 0 {
        conn.execute("ALTER TABLE notes ADD COLUMN artifact_kind TEXT", [])?;
    }
    // History writers prepare this schema before publishing guards. Adding
    // recording tables later would invalidate an active conversation's guard.
    ensure_note_recordings_table(conn)
}

fn ensure_note_recordings_table(conn: &Connection) -> rusqlite::Result<()> {
    // There is deliberately no foreign key to notes: reservation precedes
    // capture, and deletion preserves the link as a durable tombstone.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS note_recordings (
           note_id TEXT PRIMARY KEY NOT NULL,
           recording_id TEXT NOT NULL UNIQUE,
           deleted INTEGER NOT NULL DEFAULT 0 CHECK (deleted IN (0, 1))
         );
         CREATE TRIGGER IF NOT EXISTS note_recordings_note_deleted
           AFTER DELETE ON notes WHEN OLD.content_kind = 'note'
           BEGIN
             UPDATE note_recordings SET deleted = 1 WHERE note_id = OLD.id AND deleted = 0;
           END;
         CREATE TRIGGER IF NOT EXISTS note_recordings_note_inserted
           BEFORE INSERT ON notes
           WHEN EXISTS (SELECT 1 FROM note_recordings
                        WHERE note_id = NEW.id AND deleted = 1)
           BEGIN
             SELECT RAISE(ABORT, 'This recording note was deleted');
           END;
         CREATE TRIGGER IF NOT EXISTS note_recordings_note_updated
           BEFORE UPDATE ON notes
           WHEN EXISTS (SELECT 1 FROM note_recordings
                        WHERE note_id = NEW.id AND deleted = 1)
           BEGIN
             SELECT RAISE(ABORT, 'This recording note was deleted');
           END;",
    )
}

fn read_note_recording(row: &rusqlite::Row<'_>) -> rusqlite::Result<NoteRecording> {
    Ok(NoteRecording {
        note_id: row.get(0)?,
        recording_id: row.get(1)?,
        deleted: row.get(2)?,
    })
}

fn attach_note_recording(
    conn: &Connection,
    note_id: &str,
    recording_id: &str,
) -> Result<(), String> {
    let existing_kind: Option<String> = conn
        .query_row(
            "SELECT content_kind FROM notes WHERE id = ?1",
            [note_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if existing_kind.is_some_and(|kind| kind != "note") {
        return Err("Only notes can have recordings".into());
    }
    let links = conn
        .prepare(
            "SELECT note_id, recording_id, deleted FROM note_recordings
             WHERE note_id = ?1 OR recording_id = ?2",
        )
        .map_err(|e| e.to_string())?
        .query_map(params![note_id, recording_id], read_note_recording)
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    if links.iter().any(|link| link.deleted) {
        return Err("This recording note was deleted".into());
    }
    if links.iter().any(|link| link.note_id != note_id) {
        return Err("Recording already belongs to another note".into());
    }
    if links.iter().any(|link| link.recording_id != recording_id) {
        return Err("Note already has a recording".into());
    }
    if links.is_empty() {
        conn.execute(
            "INSERT INTO note_recordings (note_id, recording_id) VALUES (?1, ?2)",
            params![note_id, recording_id],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn get_content(conn: &Connection, id: &str, kind: &str) -> rusqlite::Result<Option<Note>> {
    conn.query_row(
        "SELECT id, slug, title, body, source_session_id, source_cwd, tags_json,
                created_at, updated_at, slug_pending
         FROM notes
         WHERE id = ?1 AND content_kind = ?2",
        params![id, kind],
        read_note,
    )
    .optional()
}

fn upsert_note(conn: &Connection, note: &NoteUpsert) -> rusqlite::Result<Note> {
    upsert_content(conn, note, "note", None)
}

pub(crate) fn upsert_content(
    conn: &Connection,
    note: &NoteUpsert,
    kind: &str,
    artifact_kind: Option<&str>,
) -> rusqlite::Result<Note> {
    let existing_kind: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT content_kind, artifact_kind FROM notes WHERE id = ?1",
            params![note.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if existing_kind
        .as_ref()
        .is_some_and(|(category, subtype)| category != kind || subtype.as_deref() != artifact_kind)
    {
        return Err(rusqlite::Error::InvalidParameterName(
            "Content belongs to a different kind".into(),
        ));
    }
    let title = normalize_title(&note.title);
    let body = note.body.replace("\r\n", "\n").replace('\r', "\n");
    let tags = normalize_tags(&note.tags);
    let tags_json = serde_json::to_string(&tags)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    let source_session_id = note
        .source_session_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let source_cwd = note
        .source_cwd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let now = now_millis();

    if let Some(existing) = get_content(conn, &note.id, kind)? {
        let project_cwd = source_cwd.map(str::to_string).or(existing.source_cwd);
        // Project changes keep the note in its current position in the list.
        let updated_at =
            if title == existing.title && body == existing.body && tags == existing.tags {
                existing.updated_at
            } else {
                now
            };
        // A pending slug is replaced once, by the first finalizing save with
        // a real title; every other slug stays put so @note/ references keep
        // working. Nothing here sets pending back on.
        let (slug, slug_pending) =
            if note.finalize_slug && existing.slug_pending && !is_placeholder_title(&title) {
                (unique_slug(conn, &title)?, false)
            } else {
                (existing.slug, existing.slug_pending)
            };
        conn.execute(
            "UPDATE notes
             SET title = ?1, body = ?2, tags_json = ?3, updated_at = ?4,
                 source_cwd = ?6, slug = ?7, slug_pending = ?8
             WHERE id = ?5",
            params![
                title,
                body,
                tags_json,
                updated_at,
                note.id,
                project_cwd,
                slug,
                slug_pending
            ],
        )?;
        Ok(Note {
            id: note.id.clone(),
            slug,
            title,
            body,
            tags,
            source_session_id: existing.source_session_id,
            source_cwd: project_cwd,
            slug_pending,
            created_at: existing.created_at,
            updated_at,
        })
    } else {
        // Decided before normalization: a blank title gets a generated slug,
        // while an explicit "Untitled" or "Untitled 2" owns the slug it gets.
        let slug_pending = note.title.trim().is_empty();
        let slug = unique_slug(conn, &title)?;
        conn.execute(
            "INSERT INTO notes (
               id, slug, title, body, source_session_id, source_cwd, tags_json,
               created_at, updated_at, slug_pending, content_kind, artifact_kind
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                note.id,
                slug,
                title,
                body,
                source_session_id,
                source_cwd,
                tags_json,
                now,
                now,
                slug_pending,
                kind,
                artifact_kind
            ],
        )?;
        Ok(Note {
            id: note.id.clone(),
            slug,
            title,
            body,
            tags,
            source_session_id: source_session_id.map(str::to_string),
            source_cwd: source_cwd.map(str::to_string),
            slug_pending,
            created_at: now,
            updated_at: now,
        })
    }
}

fn delete_note(conn: &Connection, id: &str) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM notes WHERE id = ?1 AND content_kind = 'note'",
        params![id],
    )?;
    Ok(())
}

fn read_note(row: &rusqlite::Row<'_>) -> rusqlite::Result<Note> {
    let tags_json: String = row.get(6)?;
    let tags = serde_json::from_str::<Vec<String>>(&tags_json).unwrap_or_default();
    Ok(Note {
        id: row.get(0)?,
        slug: row.get(1)?,
        title: row.get(2)?,
        body: row.get(3)?,
        tags,
        source_session_id: row.get(4)?,
        source_cwd: row.get(5)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        slug_pending: row.get(9)?,
    })
}

fn normalize_title(title: &str) -> String {
    let trimmed = title.trim();
    let sliced: String = trimmed.chars().take(MAX_TITLE).collect();
    let sliced = sliced.trim().to_string();
    if sliced.is_empty() {
        "Untitled".into()
    } else {
        sliced
    }
}

/// Lowercase ASCII words of `title` joined by single hyphens, untruncated.
fn slug_words(title: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in title.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            dash = false;
        } else if !out.is_empty() && !dash {
            out.push('-');
            dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

fn slugify(title: &str) -> String {
    let words = slug_words(title);
    let slug = words[..words.len().min(48)].trim_end_matches('-');
    if slug.is_empty() {
        "note".into()
    } else {
        slug.into()
    }
}

/// "Untitled" and "Untitled 2" read like generated slugs, so finalizing a
/// pending note under such a title would just trade one placeholder for
/// another; the note stays pending until it gets a real title. Checks the
/// whole title so text past the slug length cutoff still counts.
fn is_placeholder_title(title: &str) -> bool {
    let words = slug_words(title);
    match words.strip_prefix("untitled") {
        Some("") => true,
        Some(rest) => rest
            .strip_prefix('-')
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())),
        None => false,
    }
}

fn unique_slug(conn: &Connection, title: &str) -> rusqlite::Result<String> {
    let base = slugify(title);
    for index in 0..1000 {
        let candidate = if index == 0 {
            base.clone()
        } else {
            format!("{base}-{}", index + 1)
        };
        let taken: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notes WHERE slug = ?1",
            params![candidate],
            |row| row.get(0),
        )?;
        if taken == 0 {
            return Ok(candidate);
        }
    }
    Ok(format!("{base}-{}", now_millis()))
}

fn valid_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 200
        || !id
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'-' | b'_'))
    {
        return Err("Invalid note ID".into());
    }
    Ok(())
}

fn generated_recording_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|ch| ch.is_ascii_hexdigit())
}

fn legacy_recording_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .bytes()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || b"-._".contains(&ch))
        && !id.starts_with('.')
        && !id.ends_with('.')
        && !id.contains("..")
}

impl Store {
    /// Durable links, including deletion tombstones and unsaved reservations.
    /// Reading older databases never creates or migrates their schema.
    pub fn note_recordings(&self) -> Result<Vec<NoteRecording>, String> {
        if !super::shared::has_table(&self.conn, "note_recordings")? {
            return Ok(Vec::new());
        }
        self.conn
            .prepare(
                "SELECT note_id, recording_id, deleted FROM note_recordings
                 ORDER BY note_id ASC",
            )
            .map_err(|e| e.to_string())?
            .query_map([], read_note_recording)
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())
    }

    pub fn note_recording(&self, note_id: &str) -> Result<Option<NoteRecording>, String> {
        valid_id(note_id)?;
        if !super::shared::has_table(&self.conn, "note_recordings")? {
            return Ok(None);
        }
        self.conn
            .query_row(
                "SELECT note_id, recording_id, deleted FROM note_recordings WHERE note_id=?1",
                [note_id],
                read_note_recording,
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    /// Reserve the identity before capture starts, even when the note is still
    /// ephemeral. An existing or deleted identity cannot be transferred.
    pub fn attach_note_recording(&self, note_id: &str, recording_id: &str) -> Result<(), String> {
        valid_id(note_id)?;
        if !generated_recording_id(recording_id) {
            return Err("Invalid recording ID".into());
        }
        self.shared_writable()?;
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        ensure_notes_table(&tx).map_err(|e| e.to_string())?;
        attach_note_recording(&tx, note_id, recording_id)?;
        tx.commit().map_err(|e| e.to_string())
    }

    /// Materialize a visible recording document exactly once. Recordings keep
    /// their transcript and summary; the note stores only user-authored text.
    pub fn import_recording_note(
        &self,
        recording_id: &str,
        title: &str,
        created_at: i64,
    ) -> Result<Option<Note>, String> {
        let generated = generated_recording_id(recording_id);
        if !generated && !legacy_recording_id(recording_id) {
            return Err("Invalid recording ID".into());
        }
        self.shared_writable()?;
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        ensure_notes_table(&tx).map_err(|e| e.to_string())?;
        let link = tx
            .query_row(
                "SELECT note_id, recording_id, deleted FROM note_recordings
                 WHERE recording_id = ?1",
                [recording_id],
                read_note_recording,
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if link.as_ref().is_some_and(|link| link.deleted) {
            tx.commit().map_err(|e| e.to_string())?;
            return Ok(None);
        }
        let note_id = link.map(|link| link.note_id).unwrap_or_else(|| {
            if generated {
                format!("meeting-{recording_id}")
            } else {
                // Legacy recording keys can contain dots. Keep their filename
                // exact in the link while giving Notes an ordinary stable ID.
                format!(
                    "meeting-legacy-{:x}",
                    Sha256::digest(recording_id.as_bytes())
                )
            }
        });
        valid_id(&note_id)?;
        attach_note_recording(&tx, &note_id, recording_id)?;
        let note =
            if let Some(note) = get_content(&tx, &note_id, "note").map_err(|e| e.to_string())? {
                note
            } else {
                let title = normalize_title(title);
                let slug = unique_slug(&tx, &title).map_err(|e| e.to_string())?;
                tx.execute(
                    "INSERT INTO notes (id, slug, title, body, tags_json, created_at,
                                    updated_at, slug_pending, content_kind)
                 VALUES (?1, ?2, ?3, '', '[]', ?4, ?4, 0, 'note')",
                    params![note_id, slug, title, created_at],
                )
                .map_err(|e| e.to_string())?;
                Note {
                    id: note_id,
                    slug,
                    title,
                    body: String::new(),
                    tags: Vec::new(),
                    source_session_id: None,
                    source_cwd: None,
                    slug_pending: false,
                    created_at,
                    updated_at: created_at,
                }
            };
        tx.commit().map_err(|e| e.to_string())?;
        Ok(Some(note))
    }

    pub fn create_note(&self, cwd: Option<String>) -> Result<Note, String> {
        self.upsert_note(&NoteUpsert {
            id: format!(
                "note-{}-{:016x}",
                crate::time::now_ms(),
                rand::random::<u64>()
            ),
            source_cwd: cwd.filter(|cwd| crate::notes::is_project(cwd)),
            ..Default::default()
        })
    }

    pub fn upsert_note(&self, note: &NoteUpsert) -> Result<Note, String> {
        self.prepare_note_upsert(note)?;
        upsert_note(&self.conn, note).map_err(|e| e.to_string())
    }

    /// Save an editor's draft only over the exact note it loaded. None means
    /// the editor started a new draft. A mismatch preserves both versions.
    pub fn upsert_note_if_current(
        &self,
        note: &NoteUpsert,
        expected: Option<&Note>,
    ) -> Result<Option<Note>, String> {
        self.prepare_note_upsert(note)?;
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let current = get_content(&tx, &note.id, "note").map_err(|e| e.to_string())?;

        if current.as_ref() != expected {
            return Ok(None);
        }
        let saved = upsert_note(&tx, note).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(Some(saved))
    }

    /// Save a recovered draft under a new identity. The new note owns its
    /// images, so deleting the original cannot break the recovered copy.
    pub fn copy_note_draft(
        &self,
        source: &str,
        copy: &NoteUpsert,
    ) -> Result<(Note, usize), String> {
        valid_id(source)?;
        self.prepare_note_upsert(copy)?;
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        if source == copy.id
            || get_content(&tx, &copy.id, "note")
                .map_err(|e| e.to_string())?
                .is_some()
        {
            return Err("Could not reserve a new note for the copy".into());
        }
        static ASSET: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(r"/note-assets/([A-Za-z0-9_-]+)/([A-Za-z0-9_.-]+)").unwrap()
        });
        let assets: std::collections::HashSet<_> = ASSET
            .captures_iter(&copy.body)
            .filter(|asset| asset.get(1).unwrap().as_str() == source)
            .map(|asset| asset.get(2).unwrap().as_str())
            .collect();
        let existing: std::collections::HashMap<_, _> = assets
            .iter()
            .filter_map(|&name| {
                self.note_image_path(&format!("/note-assets/{source}/{name}"))
                    .ok()
                    .map(|path| (name, path))
            })
            .collect();
        let missing = assets.len() - existing.len();
        let directory = if existing.is_empty() {
            None
        } else {
            let directory = self
                .data_dir()
                .ok_or("Note image storage is unavailable")?
                .join("note-assets")
                .join(&copy.id);
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&directory).map_err(|e| e.to_string())?;
            Some(directory)
        };
        let saved = (|| {
            let mut copy = copy.clone();
            if let Some(directory) = &directory {
                for (&name, path) in &existing {
                    let mut options = std::fs::OpenOptions::new();
                    options.write(true).create_new(true);
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::OpenOptionsExt;
                        options.mode(0o600);
                    }
                    let mut output = options
                        .open(directory.join(name))
                        .map_err(|e| e.to_string())?;
                    let mut input = std::fs::File::open(path).map_err(|e| e.to_string())?;
                    std::io::copy(&mut input, &mut output).map_err(|e| e.to_string())?;
                    output.sync_all().map_err(|e| e.to_string())?;
                }
                for path in [directory.as_path(), directory.parent().expect("asset root")] {
                    std::fs::File::open(path)
                        .and_then(|f| f.sync_all())
                        .map_err(|e| e.to_string())?;
                }
                copy.body = ASSET
                    .replace_all(&copy.body, |asset: &regex::Captures<'_>| {
                        if &asset[1] == source && existing.contains_key(&asset[2]) {
                            format!("/note-assets/{}/{}", copy.id, &asset[2])
                        } else {
                            asset[0].to_owned()
                        }
                    })
                    .into_owned();
            }
            if copy.body.len() > MAX_BODY {
                return Err("Content is too large".into());
            }
            let saved = upsert_note(&tx, &copy).map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            Ok((saved, missing))
        })();
        if let (Err(_), Some(directory)) = (&saved, directory) {
            let _ = std::fs::remove_dir_all(directory);
        }
        saved
    }

    fn prepare_note_upsert(&self, note: &NoteUpsert) -> Result<(), String> {
        valid_id(&note.id)?;
        if note.body.len() > MAX_BODY {
            return Err("Content is too large".into());
        }
        if let Some(id) = note
            .source_session_id
            .as_deref()
            .filter(|id| !id.is_empty())
        {
            valid_id(id)?;
        }
        self.shared_writable()?;
        ensure_notes_table(&self.conn).map_err(|e| e.to_string())
    }

    /// Update only the note the caller authorized, preserving its artifact
    /// subtype and any pending slug. Scope and content may change between
    /// reading the note and beginning this write.
    pub fn update_note_preserving_artifact(
        &self,
        mut note: NoteUpsert,
        expected: &Note,
    ) -> Result<Note, String> {
        self.prepare_note_upsert(&note)?;
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let current = get_content(&tx, &note.id, "note").map_err(|e| e.to_string())?;

        if current.as_ref() != Some(expected) {
            return Err(
                "The note changed before it could be saved. Read it again and retry.".into(),
            );
        }
        let subtype: Option<Option<String>> = tx
            .query_row(
                "SELECT artifact_kind FROM notes WHERE id = ?1 AND content_kind = 'note'",
                [&note.id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let subtype = subtype.flatten();
        note.finalize_slug = false;
        let saved =
            upsert_content(&tx, &note, "note", subtype.as_deref()).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(saved)
    }

    pub fn delete_note(&self, id: &str) -> Result<(), String> {
        self.delete_note_checked(id, None).map(drop)
    }

    pub fn delete_note_if_current(
        &self,
        id: &str,
        expected: Option<&Note>,
    ) -> Result<bool, String> {
        self.delete_note_checked(id, Some(expected))
    }

    fn delete_note_checked(
        &self,
        id: &str,
        expected: Option<Option<&Note>>,
    ) -> Result<bool, String> {
        valid_id(id)?;
        self.shared_writable()?;
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        if let Some(expected) = expected {
            let current = if super::shared::has_table(&tx, "notes")? {
                ensure_notes_table(&tx).map_err(|e| e.to_string())?;
                get_content(&tx, id, "note").map_err(|e| e.to_string())?
            } else {
                None
            };

            if current.as_ref() != expected {
                return Ok(false);
            }
        }
        // Reservations have no note row yet, so the delete trigger alone is
        // insufficient. Keep this mutation in the same transaction as deletion.
        if super::shared::has_table(&tx, "note_recordings")? {
            tx.execute(
                "UPDATE note_recordings SET deleted = 1 WHERE note_id = ?1 AND deleted = 0",
                [id],
            )
            .map_err(|e| e.to_string())?;
        }
        if super::shared::has_table(&tx, "notes")? {
            ensure_notes_table(&tx).map_err(|e| e.to_string())?;
            delete_note(&tx, id).map_err(|e| e.to_string())?;
        }

        tx.commit().map_err(|e| e.to_string())?;
        // A deleted row stays deleted even if best-effort asset cleanup fails.
        if let Some(root) = self.data_dir() {
            let _ = std::fs::remove_dir_all(root.join("note-assets").join(id));
        }
        Ok(true)
    }

    pub fn note_source_session(&self, note: &Note) -> Option<i64> {
        note.source_session_id
            .as_deref()
            .and_then(|id| self.resolve_existing_session_id(id))
    }

    pub(super) fn save_response_note(&self, note: &crate::response::Note) -> Result<(), String> {
        if note.body.len() > MAX_BODY {
            return Err("Content is too large".into());
        }
        let native_id = format!("gpui-note-{}", note.id);
        let existing = self.list_note_summaries()?.into_iter().find(|saved| {
            saved.id == native_id || super::shared::handle(&saved.id) as u64 == note.id
        });
        let update = NoteUpsert {
            id: existing
                .as_ref()
                .map(|note| note.id.clone())
                .unwrap_or(native_id),
            title: note.title.clone(),
            body: note.body.clone(),
            tags: note.tags.clone(),
            source_session_id: (note.source_session != 0)
                .then(|| self.persisted_session_id(note.source_session).ok())
                .flatten(),
            source_cwd: (!note.source_cwd.is_empty()).then(|| note.source_cwd.clone()),
            finalize_slug: true,
        };
        // The legacy facade also edits older note rows carrying an artifact
        // subtype. Preserve it instead of reclassifying or duplicating the row.
        if existing.is_some() && super::shared::has_table(&self.conn, "notes")? {
            self.shared_writable()?;
            ensure_notes_table(&self.conn).map_err(|e| e.to_string())?;
            let subtype: Option<String> = self
                .conn
                .query_row(
                    "SELECT artifact_kind FROM notes WHERE id=?1",
                    [&update.id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?
                .flatten();
            upsert_content(&self.conn, &update, "note", subtype.as_deref())
                .map_err(|e| e.to_string())?;
        } else {
            self.upsert_note(&update)?;
        }
        Ok(())
    }

    pub fn save_note_image(
        &self,
        note_id: &str,
        source: &Path,
        new_note: Option<&NoteUpsert>,
    ) -> Result<NoteImageAsset, String> {
        valid_id(note_id)?;
        self.shared_writable()?;
        let missing = self.get_note(note_id)?.is_none();
        if missing && new_note.is_none_or(|note| note.id != note_id) {
            return Err("Note no longer exists".into());
        }
        let source = source.canonicalize().map_err(|e| e.to_string())?;
        let meta = source.metadata().map_err(|e| e.to_string())?;
        if !meta.is_file() {
            return Err("Drop an image file.".into());
        }
        if meta.len() > 20 * 1024 * 1024 {
            return Err("Image is too large (maximum 20 MB).".into());
        }
        let extension = source
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !["png", "jpg", "jpeg", "gif", "webp", "svg"].contains(&extension.as_str()) {
            return Err("Drop a PNG, JPG, GIF, WebP, or SVG image.".into());
        }
        let name = source
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let stem = source
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                    ch
                } else {
                    '-'
                }
            })
            .take(80)
            .collect::<String>()
            .trim_matches('-')
            .to_owned();
        let stem = if stem.is_empty() { "image" } else { &stem };
        let filename = format!(
            "{}-{:016x}-{stem}.{extension}",
            crate::time::now_ms(),
            rand::random::<u64>()
        );
        let root = self.data_dir().ok_or("Note image storage is unavailable")?;
        let directory = root.join("note-assets").join(note_id);
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let destination = directory.join(&filename);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut output = options.open(&destination).map_err(|e| e.to_string())?;
        let copied = (|| -> std::io::Result<()> {
            std::io::copy(&mut std::fs::File::open(source)?, &mut output)?;
            output.sync_all()?;
            std::fs::File::open(&directory)?.sync_all()
        })();
        if let Err(error) = copied {
            let _ = std::fs::remove_file(&destination);
            return Err(error.to_string());
        }
        let image = NoteImageAsset {
            name,
            markdown_path: format!("/note-assets/{note_id}/{filename}"),
        };
        if missing {
            // The first image is content. Publish its reference with the new
            // note so a dropped view cannot leave an empty saved note behind.
            // Only an explicitly new draft may create a row; ordinary imports
            // still reject deleted notes, and recording tombstones reject both.
            let mut note = new_note.unwrap().clone();
            if !note.body.is_empty() && !note.body.ends_with("\n\n") {
                note.body.push_str("\n\n");
            }
            note.body.push_str(&crate::notes::image_markdown(&image));
            if let Err(error) = self.upsert_note(&note) {
                let _ = std::fs::remove_file(destination);
                return Err(error);
            }
        }
        Ok(image)
    }

    pub fn note_image_path(&self, asset: &str) -> Result<PathBuf, String> {
        let path = Path::new(asset.strip_prefix('/').ok_or("Invalid note image path")?);
        let parts = path
            .components()
            .map(|part| match part {
                Component::Normal(value) => value.to_str(),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .ok_or("Invalid note image path")?;
        if parts.len() != 3 || parts[0] != "note-assets" {
            return Err("Invalid note image path".into());
        }
        valid_id(parts[1])?;
        if !parts[2]
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'-' | b'_' | b'.'))
        {
            return Err("Invalid note image path".into());
        }
        let path = self
            .data_dir()
            .ok_or("Note image storage is unavailable")?
            .join(path);
        if !path.is_file() {
            return Err("Note image was not found".into());
        }
        Ok(path)
    }
}

#[cfg(test)]
mod tests;
