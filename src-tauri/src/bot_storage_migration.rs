//! One-time namespace conversion, before webviews or transcript readers start.
use std::path::Path;

use rusqlite::{params, Connection, Transaction, TransactionBehavior};
use serde_json::Value;

fn table_exists(conn: &Connection, name: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [name],
        |row| row.get(0),
    )
}

fn rename_field(object: &mut serde_json::Map<String, Value>, old: &str, new: &str) {
    if !object.contains_key(new) {
        if let Some(value) = object.remove(old) {
            object.insert(new.into(), value);
        }
    }
}

/// Only app-owned block metadata is renamed. Tool inputs and user payloads are opaque.
fn convert_block(block: &mut Value) {
    let Some(object) = block.as_object_mut() else {
        return;
    };
    for (old, new) in [
        ("monoHabit", "botHabit"),
        ("monoCard", "botCard"),
        ("monoReaction", "botReaction"),
        ("monoSessionCompletion", "botSessionCompletion"),
        ("monoPeerMessage", "botPeerMessage"),
        ("monoSpawnedSessions", "botSpawnedSessions"),
    ] {
        rename_field(object, old, new);
    }
    if let Some(peer) = object
        .get_mut("botPeerMessage")
        .and_then(Value::as_object_mut)
    {
        rename_field(peer, "fromMonoId", "fromBotId");
        rename_field(peer, "toMonoId", "toBotId");
    }
    if let Some(threads) = object.get_mut("btwThreads").and_then(Value::as_array_mut) {
        for thread in threads {
            if let Some(messages) = thread.get_mut("messages").and_then(Value::as_array_mut) {
                for message in messages {
                    if let Some(blocks) = message.get_mut("blocks").and_then(Value::as_array_mut) {
                        for block in blocks {
                            convert_block(block);
                        }
                    }
                }
            }
        }
    }
}

fn converted(raw: &str, collection: bool) -> String {
    let Ok(mut value) = serde_json::from_str::<Value>(raw) else {
        return raw.into();
    };
    let before = value.clone();
    if collection {
        if let Some(blocks) = value.as_array_mut() {
            for block in blocks {
                convert_block(block);
            }
        }
    } else {
        convert_block(&mut value);
    }
    if value == before {
        raw.into()
    } else {
        value.to_string()
    }
}

fn convert_column(
    tx: &Transaction<'_>,
    table: &str,
    column: &str,
    collection: bool,
) -> rusqlite::Result<()> {
    let mut statement = tx.prepare(&format!(
        "SELECT rowid, {column} FROM {table} WHERE {column} LIKE '%mono%'"
    ))?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        let raw: String = row.get(1)?;
        let next = converted(&raw, collection);
        if raw != next {
            tx.execute(
                &format!("UPDATE {table} SET {column}=?1 WHERE rowid=?2"),
                params![next, id],
            )?;
        }
    }
    Ok(())
}

pub(crate) fn migrate_database(conn: &Connection) -> rusqlite::Result<()> {
    // Use the existing schema-version authority. Recheck under write admission
    // so concurrent app openers cannot publish two different conversions.
    let done = |conn: &Connection| {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version=22)",
            [],
            |row| row.get::<_, bool>(0),
        )
    };
    if done(conn)? {
        return Ok(());
    }
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    if done(&tx)? {
        return tx.commit();
    }
    if table_exists(&tx, "mono_transcripts")? {
        let turns = table_exists(&tx, "mono_turns")?;
        let blocks = table_exists(&tx, "mono_blocks")?;
        let mut statement = tx.prepare("SELECT session_id FROM mono_transcripts WHERE NOT EXISTS (SELECT 1 FROM bot_blocks WHERE bot_blocks.session_id=mono_transcripts.session_id) AND NOT EXISTS (SELECT 1 FROM bot_turns WHERE bot_turns.session_id=mono_transcripts.session_id)")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            tx.execute(
                "INSERT OR IGNORE INTO bot_transcripts(session_id) VALUES (?1)",
                [&id],
            )?;
            if turns {
                tx.execute("INSERT INTO bot_turns(session_id,seq) SELECT session_id,seq FROM mono_turns WHERE session_id=?1", [&id])?;
            }
            if blocks {
                tx.execute("INSERT INTO bot_blocks(session_id,seq,block_id,block_json,search_text,is_user,is_draft) SELECT session_id,seq,block_id,block_json,search_text,is_user,is_draft FROM mono_blocks WHERE session_id=?1", [&id])?;
            }
            // A prior empty-window save may have cleared the sidebar/search
            // projection as well as the displayed history.
            tx.execute("UPDATE sessions SET has_user_message=EXISTS(SELECT 1 FROM bot_blocks WHERE session_id=?1 AND is_user=1), is_draft=EXISTS(SELECT 1 FROM bot_blocks WHERE session_id=?1 AND is_user=1 AND is_draft=1) WHERE id=?1", [&id])?;
        }
    }
    for (table, column, collection) in [
        ("bot_blocks", "block_json", false),
        ("ordinary_blocks", "block_json", false),
        ("sessions", "blocks_json", true),
        ("sessions", "queued_messages_json", true),
        ("local_submissions", "user_turn_json", false),
    ] {
        convert_column(&tx, table, column, collection)?;
    }
    // This marker and every destination row become visible in the same commit.
    tx.execute("INSERT INTO schema_migrations(version,applied_at) VALUES (22,CAST(strftime('%s','now') AS INTEGER)*1000)", [])?;
    tx.commit()
}

fn exists(path: &Path) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    std::fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn copy_entry(source: &Path, destination: &Path) -> std::io::Result<()> {
    let kind = std::fs::symlink_metadata(source)?.file_type();
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        // A stopped provider can leave IPC sockets behind. They have no
        // durable contents and a successor must create its own endpoint.
        if kind.is_socket() {
            return Ok(());
        }
    }
    if kind.is_symlink() {
        let target = std::fs::read_link(source)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, destination)?;
        #[cfg(windows)]
        if source.is_dir() {
            std::os::windows::fs::symlink_dir(target, destination)?;
        } else {
            std::os::windows::fs::symlink_file(target, destination)?;
        }
    } else if kind.is_dir() {
        std::fs::create_dir(destination)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(destination, std::fs::Permissions::from_mode(0o700))?;
        }
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_entry(&entry.path(), &destination.join(entry.file_name()))?;
        }
        sync_directory(destination)?;
    } else if kind.is_file() {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            options.mode(std::fs::metadata(source)?.permissions().mode() & 0o700);
        }
        let mut output = options.open(destination)?;
        std::io::copy(&mut std::fs::File::open(source)?, &mut output)?;
        output.sync_all()?;
    } else {
        return Err(std::io::Error::other(
            "Unsupported entry in legacy Bot storage",
        ));
    }
    Ok(())
}

/// An atomic claim must never replace even an empty directory owned by a new run.
fn publish(source: &Path, destination: &Path) -> std::io::Result<()> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::os::unix::ffi::OsStrExt;
        let source = std::ffi::CString::new(source.as_os_str().as_bytes())?;
        let destination = std::ffi::CString::new(destination.as_os_str().as_bytes())?;
        #[cfg(target_os = "macos")]
        let result =
            unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
        #[cfg(target_os = "linux")]
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let source = source
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let destination = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        if unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                0,
            )
        } == 0
        {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    std::fs::rename(source, destination)
}

fn copy_missing(source: &Path, destination: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if std::fs::symlink_metadata(source)?.file_type().is_socket() {
            return Ok(());
        }
    }
    if exists(destination)? {
        return Ok(());
    }
    let parent = destination
        .parent()
        .ok_or_else(|| std::io::Error::other("Missing Bot storage parent"))?;
    let stage = parent.join(format!(".vorkspace-bot-migration-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&stage)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stage, std::fs::Permissions::from_mode(0o700))?;
    }
    let payload = stage.join("payload");
    let result = copy_entry(source, &payload).and_then(|()| {
        sync_directory(&stage)?;
        publish(&payload, destination)?;
        sync_directory(parent)
    });
    let cleanup = std::fs::remove_dir_all(&stage);
    result.and(cleanup)
}

pub(crate) fn migrate_directories(data: &Path) -> std::io::Result<()> {
    for (old, next) in [("monos", "bots"), ("codex-monos", "codex-bots")] {
        let source = data.join(old);
        if !std::fs::symlink_metadata(&source)
            .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
        {
            continue;
        }
        let destination = data.join(next);
        if !exists(&destination)? {
            copy_missing(&source, &destination)?;
        } else if std::fs::symlink_metadata(&destination)?
            .file_type()
            .is_dir()
        {
            // Owners already present remain authoritative. Copy each missing
            // owner as one complete unit, never mix two versions of a soul/home.
            for entry in std::fs::read_dir(&source)? {
                let entry = entry?;
                copy_missing(&entry.path(), &destination.join(entry.file_name()))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_store::{SessionStore, SessionUpsert};
    use serde_json::json;

    struct Directory(std::path::PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("bot-namespace-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn put(&self, name: &str, text: &str) {
            let path = self.0.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn legacy(conn: &Connection) {
        let session: SessionUpsert = serde_json::from_value(json!({"id":"saved-chat","cwd":"/repo","harness":"codex","model":"model","modelSettings":{},"runtimeMode":"default","title":"Saved","blocks":[]})).unwrap();
        crate::session_store::upsert_session(conn, &session).unwrap();
        conn.execute_batch("DELETE FROM schema_migrations WHERE version=22;
            CREATE TABLE mono_transcripts AS SELECT * FROM bot_transcripts;
            CREATE TABLE mono_turns AS SELECT * FROM bot_turns;
            CREATE TABLE mono_blocks AS SELECT * FROM bot_blocks;
            INSERT INTO mono_transcripts VALUES ('saved-chat');
            INSERT INTO mono_turns VALUES ('saved-chat',1);
            INSERT INTO mono_blocks VALUES ('saved-chat',1,'u1','{\"id\":\"u1\",\"role\":\"user\",\"text\":\"Hello\"}','hello',1,0);
            INSERT INTO mono_blocks VALUES ('saved-chat',2,'a1','{\"id\":\"a1\",\"role\":\"assistant\",\"text\":\"Welcome\",\"monoReaction\":\"👍\"}','welcome',0,0);").unwrap();
    }

    #[test]
    fn startup_reopens_indexed_history_and_does_not_resurrect_it_after_conversion() {
        let dir = Directory::new();
        let db = dir.0.join("monocode.db");
        {
            let store = SessionStore::open(db.clone()).unwrap();
            let conn = store.lock_conn().unwrap();
            legacy(&conn);
            // Recover a marker left by a version that opened the cleared blob.
            conn.execute("INSERT INTO bot_transcripts VALUES ('saved-chat')", [])
                .unwrap();
        }
        {
            let store = SessionStore::open(db.clone()).unwrap();
            let conn = store.lock_conn().unwrap();
            crate::bot_transcript::migrate_transcript(&conn, "saved-chat").unwrap();
            let blocks =
                crate::bot_transcript::matching_blocks(&conn, "saved-chat", "", 10).unwrap();
            assert_eq!(blocks.as_array().unwrap().len(), 2);
            assert_eq!(blocks[1]["botReaction"], "👍");
            assert!(conn
                .query_row(
                    "SELECT has_user_message FROM sessions WHERE id='saved-chat'",
                    [],
                    |r| r.get::<_, bool>(0)
                )
                .unwrap());
            assert_eq!(
                conn.query_row("SELECT count(*) FROM mono_blocks", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                2
            );
            conn.execute("DELETE FROM bot_blocks", []).unwrap();
            conn.execute("DELETE FROM bot_turns", []).unwrap();
        }
        let store = SessionStore::open(db).unwrap();
        let conn = store.lock_conn().unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM bot_blocks", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn failed_database_conversion_rolls_back_rows_and_marker_then_retries() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        legacy(&conn);
        conn.execute_batch("CREATE TRIGGER fail_copy BEFORE INSERT ON bot_blocks BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
        assert!(migrate_database(&conn).is_err());
        assert_eq!(
            conn.query_row("SELECT count(*) FROM bot_transcripts", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM schema_migrations WHERE version=22",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        conn.execute_batch("DROP TRIGGER fail_copy;").unwrap();
        migrate_database(&conn).unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM bot_blocks", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }

    #[test]
    fn populated_or_partial_destination_history_remains_authoritative() {
        for turns_only in [false, true] {
            let store = SessionStore::open_in_memory().unwrap();
            let conn = store.lock_conn().unwrap();
            legacy(&conn);
            conn.execute("INSERT INTO bot_transcripts VALUES ('saved-chat')", [])
                .unwrap();
            if turns_only {
                conn.execute("INSERT INTO bot_turns VALUES ('saved-chat',3)", [])
                    .unwrap();
            } else {
                conn.execute(
                    "INSERT INTO bot_blocks VALUES ('saved-chat',3,'new','{}','',0,0)",
                    [],
                )
                .unwrap();
            }
            migrate_database(&conn).unwrap();
            assert_eq!(
                conn.query_row(
                    "SELECT count(*) FROM bot_blocks WHERE seq IN (1,2)",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                0
            );
            assert_eq!(
                conn.query_row("SELECT count(*) FROM bot_turns WHERE seq=1", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
    }

    #[test]
    fn converts_queued_and_nested_block_metadata_but_preserves_tool_payloads() {
        let raw = json!({"id":"u","role":"user","monoPeerMessage":{"fromMonoId":"a","toMonoId":"b"},"tool":{"input":{"monoCard":"user value","fromMonoId":"opaque"}},"btwThreads":[{"messages":[{"blocks":[{"monoCard":{"type":"choices","options":["yes"]}}]}]}]});
        let converted: Value =
            serde_json::from_str(&super::converted(&raw.to_string(), false)).unwrap();
        assert_eq!(converted["botPeerMessage"]["fromBotId"], "a");
        assert_eq!(converted["tool"], raw["tool"]);
        assert_eq!(
            converted["btwThreads"][0]["messages"][0]["blocks"][0]["botCard"]["type"],
            "choices"
        );
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        legacy(&conn);
        conn.execute(
            "UPDATE sessions SET queued_messages_json=?1 WHERE id='saved-chat'",
            [json!([raw]).to_string()],
        )
        .unwrap();
        migrate_database(&conn).unwrap();
        let queued: String = conn
            .query_row(
                "SELECT queued_messages_json FROM sessions WHERE id='saved-chat'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(queued.contains("botPeerMessage"));
        assert!(queued.contains("user value"));
    }

    #[test]
    fn copies_missing_owners_without_overwriting_existing_files_or_following_roots() {
        let dir = Directory::new();
        dir.put("monos/resident/SOUL.md", "existing soul");
        dir.put("monos/resident/memory/topic.md", "topic");
        dir.put("monos/current/SOUL.md", "old current");
        dir.put("bots/vox/MEMORY.md", "new vox");
        dir.put("bots/current/SOUL.md", "new current");
        dir.put("codex-monos/default/sessions/rollout.jsonl", "context");
        migrate_directories(&dir.0).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.0.join("bots/resident/SOUL.md")).unwrap(),
            "existing soul"
        );
        assert_eq!(
            std::fs::read_to_string(dir.0.join("bots/current/SOUL.md")).unwrap(),
            "new current"
        );
        assert!(dir.0.join("monos/resident/SOUL.md").exists());
        assert_eq!(
            std::fs::read_to_string(dir.0.join("codex-bots/default/sessions/rollout.jsonl"))
                .unwrap(),
            "context"
        );
        dir.put("bots/resident/SOUL.md", "edited");
        migrate_directories(&dir.0).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.0.join("bots/resident/SOUL.md")).unwrap(),
            "edited"
        );
    }

    #[test]
    fn publication_never_replaces_a_successors_empty_directory() {
        let dir = Directory::new();
        let source = dir.0.join("stage");
        let target = dir.0.join("claimed");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&target).unwrap();
        assert!(publish(&source, &target).is_err());
        assert!(source.exists());
        assert!(target.exists());
        dir.put("old-file", "copied");
        dir.put("new-file", "successor");
        assert!(publish(&dir.0.join("old-file"), &dir.0.join("new-file")).is_err());
        assert_eq!(
            std::fs::read_to_string(dir.0.join("new-file")).unwrap(),
            "successor"
        );
    }

    #[cfg(unix)]
    #[test]
    fn provider_ipc_sockets_are_not_copied_into_the_successors_home() {
        let dir = Directory(
            std::path::PathBuf::from("/tmp").join(format!("bot-{}", uuid::Uuid::new_v4())),
        );
        dir.put(
            "codex-monos/default/sessions/rollout.jsonl",
            "saved context",
        );
        std::fs::create_dir(dir.0.join("codex-monos/default/tmp")).unwrap();
        let _socket =
            std::os::unix::net::UnixListener::bind(dir.0.join("codex-monos/default/tmp/ipc"))
                .unwrap();
        migrate_directories(&dir.0).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.0.join("codex-bots/default/sessions/rollout.jsonl"))
                .unwrap(),
            "saved context"
        );
        assert!(!dir.0.join("codex-bots/default/tmp/ipc").exists());
    }

    #[cfg(unix)]
    #[test]
    fn failed_copy_does_not_publish_partial_data_and_symlinks_remain_links() {
        use std::os::unix::{ffi::OsStrExt, fs::symlink};
        let dir = Directory::new();
        dir.put("monos/resident/MEMORY.md", "saved");
        let pipe = dir.0.join("monos/resident/pipe");
        let name = std::ffi::CString::new(pipe.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(migrate_directories(&dir.0).is_err());
        assert!(!dir.0.join("bots").exists());
        assert!(!std::fs::read_dir(&dir.0).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".vorkspace-")));
        std::fs::remove_file(pipe).unwrap();
        dir.put("external/auth.json", "private");
        symlink(
            dir.0.join("external/auth.json"),
            dir.0.join("monos/resident/auth.json"),
        )
        .unwrap();
        migrate_directories(&dir.0).unwrap();
        assert!(
            std::fs::symlink_metadata(dir.0.join("bots/resident/auth.json"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let other = Directory::new();
        symlink(dir.0.join("monos"), other.0.join("monos")).unwrap();
        migrate_directories(&other.0).unwrap();
        assert!(!other.0.join("bots").exists());
    }
}
