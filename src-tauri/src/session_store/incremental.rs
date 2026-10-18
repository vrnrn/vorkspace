//! Ordinary transcripts use independent block rows after their first delta save.
//! The legacy blob remains readable until that session is converted atomically.
use super::*;
use std::collections::{HashMap, HashSet};

pub(crate) fn ensure_tables(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS ordinary_transcripts (
        session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
        revision INTEGER NOT NULL DEFAULT 0, normalized INTEGER NOT NULL DEFAULT 0);
      CREATE TABLE IF NOT EXISTS ordinary_blocks (
        session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
        block_id TEXT NOT NULL, position INTEGER NOT NULL, block_json TEXT NOT NULL,
        has_user INTEGER NOT NULL, is_draft INTEGER NOT NULL,
        PRIMARY KEY(session_id, block_id));
      CREATE INDEX IF NOT EXISTS ordinary_blocks_order ON ordinary_blocks(session_id, position);
      CREATE INDEX IF NOT EXISTS ordinary_blocks_flags ON ordinary_blocks(session_id,has_user,is_draft);
      CREATE TABLE IF NOT EXISTS ordinary_detached_leads(session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,lead_id TEXT NOT NULL,PRIMARY KEY(session_id,lead_id));",
    )
}

pub(crate) fn indexed(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM ordinary_transcripts WHERE session_id=?1 AND normalized=1)",
        [id],
        |r| r.get(0),
    )
}

pub(crate) fn blocks(conn: &Connection, id: &str) -> rusqlite::Result<Value> {
    let mut query = conn
        .prepare("SELECT block_json FROM ordinary_blocks WHERE session_id=?1 ORDER BY position")?;
    let raw = query.query_map([id], |row| row.get::<_, String>(0))?;
    let mut blocks = Vec::new();
    for raw in raw {
        blocks.push(serde_json::from_str(&raw?).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?);
    }
    Ok(Value::Array(blocks))
}

fn put(
    conn: &Connection,
    id: &str,
    block_id: &str,
    position: i64,
    block: &Value,
) -> rusqlite::Result<()> {
    conn.execute("INSERT INTO ordinary_blocks(session_id,block_id,position,block_json,has_user,is_draft)
        VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(session_id,block_id) DO UPDATE SET
        position=excluded.position,block_json=excluded.block_json,has_user=excluded.has_user,is_draft=excluded.is_draft
        WHERE ordinary_blocks.block_json != excluded.block_json OR ordinary_blocks.position != excluded.position",
        params![id, block_id, position, block.to_string(), i64::from(block["role"] == "user"), i64::from(block["role"] == "user" && block["draft"] == true)])?;
    Ok(())
}

pub(crate) fn save(
    conn: &Connection,
    session: &SessionUpsert,
    ids: &[String],
    expected: Option<i64>,
) -> Result<(SessionSummary, i64), String> {
    if crate::bot_transcript::is_indexed(conn, &session.id).map_err(|e| e.to_string())? {
        return Err("Bot sessions use their own transcript store".into());
    }
    let changed = session.blocks.as_array().ok_or("blocks must be an array")?;
    let unique: HashSet<&str> = ids.iter().map(String::as_str).collect();
    let positions: HashMap<&str, i64> = ids
        .iter()
        .enumerate()
        .map(|(position, id)| (id.as_str(), position as i64))
        .collect();
    if unique.len() != ids.len() {
        return Err("Duplicate block ids".into());
    }
    let mut changes = HashSet::new();
    for block in changed {
        let id = block["id"].as_str().ok_or("Block id is required")?;
        if !unique.contains(id) || !changes.insert(id) {
            return Err("Invalid changed block ids".into());
        }
    }
    if expected.is_none() && changes.len() != ids.len() {
        return Err("A baseline requires all blocks".into());
    }
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let result = (|| -> rusqlite::Result<(SessionSummary, i64)> {
        let revision: Option<i64> = tx
            .query_row(
                "SELECT revision FROM ordinary_transcripts WHERE session_id=?1",
                [&session.id],
                |r| r.get(0),
            )
            .optional()?;
        if expected != revision {
            return Err(rusqlite::Error::InvalidParameterName(
                "SESSION_BASELINE_MISMATCH".into(),
            ));
        }
        if !indexed(&tx, &session.id)? {
            let existing = get_session(&tx, &session.id)?;
            if existing.is_none() {
                upsert_session_inner(&tx, session, true)?;
            }
            tx.execute(
                "INSERT INTO ordinary_transcripts(session_id,revision,normalized) VALUES (?1,0,1) ON CONFLICT(session_id) DO UPDATE SET normalized=1",
                [&session.id],
            )?;
            if let Some(existing) = existing {
                for (position, block) in
                    existing.blocks.as_array().into_iter().flatten().enumerate()
                {
                    if let Some(block_id) = block["id"].as_str() {
                        put(&tx, &session.id, block_id, position as i64, block)?;
                    }
                }
            }
            tx.execute(
                "UPDATE sessions SET blocks_json='[]' WHERE id=?1",
                [&session.id],
            )?;
        }
        // The order map contains only small IDs/positions, never transcript JSON.
        let mut statement =
            tx.prepare("SELECT block_id,position FROM ordinary_blocks WHERE session_id=?1")?;
        let existing: HashMap<String, i64> = statement
            .query_map([&session.id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        drop(statement);
        let mut modified = false;
        for old in existing.keys().filter(|id| !unique.contains(id.as_str())) {
            modified |= tx.execute(
                "DELETE FROM ordinary_blocks WHERE session_id=?1 AND block_id=?2",
                params![session.id, old],
            )? != 0;
        }
        for (position, id) in ids.iter().enumerate() {
            if !changes.contains(id.as_str()) && !existing.contains_key(id) {
                return Err(rusqlite::Error::InvalidParameterName(
                    "Missing unchanged block".into(),
                ));
            }
            if existing.get(id).is_some_and(|old| *old != position as i64) {
                modified |= tx.execute(
                    "UPDATE ordinary_blocks SET position=?3 WHERE session_id=?1 AND block_id=?2",
                    params![session.id, id, position as i64],
                )? != 0;
            }
        }
        for incoming in changed {
            let id = incoming["id"].as_str().unwrap();
            let previous: Option<String> = tx
                .query_row(
                    "SELECT block_json FROM ordinary_blocks WHERE session_id=?1 AND block_id=?2",
                    params![session.id, id],
                    |r| r.get(0),
                )
                .optional()?;
            let mut incoming = incoming.clone();
            if let Some(lead) = incoming["orchestrationLeadId"].as_str() {
                if tx.query_row("SELECT EXISTS(SELECT 1 FROM ordinary_detached_leads WHERE session_id=?1 AND lead_id=?2)", params![session.id,lead], |r| r.get::<_,bool>(0))? {
                    incoming.as_object_mut().unwrap().remove("orchestrationLeadId");
                }
            }
            let current = Value::Array(vec![incoming]);
            let clean = crate::artifacts::retain_existing_cards(&tx, &current)?;
            let previous = previous
                .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                .map(|b| Value::Array(vec![b]));
            let restored = previous
                .as_ref()
                .map(|old| crate::tool_detail::restore_details(old, &clean));
            let block = &restored.as_ref().map(|b| b.as_ref()).unwrap_or(&clean)[0];
            let before = tx.total_changes();
            put(
                &tx,
                &session.id,
                id,
                positions[id],
                block,
            )?;
            modified |= tx.total_changes() != before;
        }
        let mut summary = upsert_session_inner(&tx, session, true)?;
        if modified {
            tx.execute(
                "UPDATE sessions SET updated_at=?2 WHERE id=?1",
                params![session.id, now_millis()],
            )?;
            summary.updated_at = now_millis();
        }
        let next: i64 =
            tx.query_row("SELECT abs(random() % 9007199254740991)", [], |r| r.get(0))?;
        tx.execute(
            "UPDATE ordinary_transcripts SET revision=?2 WHERE session_id=?1",
            params![session.id, next],
        )?;
        Ok((summary, next))
    })()
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(result)
}

/// Raw JSON matches are candidates, not semantic message hits. Bound their
/// combined bytes here; the caller applies its hit limit after `block_hits`.
pub(crate) fn matching_blocks(
    conn: &Connection,
    id: &str,
    pattern: &str,
    budget: usize,
) -> rusqlite::Result<(Value, bool)> {
    let mut query = conn.prepare(
        "SELECT CASE WHEN octet_length(block_json)<=?3 THEN block_json END
        FROM ordinary_blocks WHERE session_id=?1 AND CASE WHEN octet_length(block_json)<=?3
        THEN LOWER(block_json) LIKE LOWER(?2) ESCAPE '\\' ELSE 1 END ORDER BY position",
    )?;
    let rows = query.query_map(params![id, pattern, budget as i64], |r| {
        r.get::<_, Option<String>>(0)
    })?;
    let mut bytes = 0;
    let mut matches = Vec::new();
    let mut truncated = false;
    for raw in rows {
        let Some(raw) = raw? else {
            truncated = true;
            continue;
        };
        bytes += raw.len();
        if bytes > budget {
            truncated = true;
            break;
        }
        if let Ok(block) = serde_json::from_str(&raw) {
            matches.push(block);
        }
    }
    Ok((Value::Array(matches), truncated))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn session(id: &str, blocks: Value) -> SessionUpsert {
        serde_json::from_value(json!({"id":id,"cwd":"/nonexistent/incremental-test","harness":"codex","model":"test","modelSettings":{},"runtimeMode":"supervised","title":"Test","blocks":blocks})).unwrap()
    }
    #[test]
    fn one_changed_row_keeps_legacy_output_and_does_not_rewrite_history() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.conn.lock().unwrap();
        let mut history = vec![json!({"id":"u","role":"user","text":"run"})];
        for index in 0..100 {
            history.push(json!({"id":format!("t{index}"),"role":"tool","text":"command","tool":{"detail":"x".repeat(4000)}}));
        }
        let original = session("delta", Value::Array(history.clone()));
        upsert_session(&conn, &original).unwrap();
        let ids: Vec<String> = history
            .iter()
            .map(|b| b["id"].as_str().unwrap().to_owned())
            .collect();
        let base = get_session(&conn, "delta")
            .unwrap()
            .unwrap()
            .persistence_revision;
        let (_, revision) = save(&conn, &original, &ids, base).unwrap();
        conn.execute_batch("CREATE TABLE touched(id TEXT, bytes INTEGER); CREATE TEMP TRIGGER observe_delta AFTER UPDATE ON ordinary_blocks BEGIN INSERT INTO touched VALUES(new.block_id,length(new.block_json)); END;").unwrap();
        let mut loaded = history[100].clone();
        loaded["tool"] = json!({"status":"completed","detailOmitted":true});
        loaded["text"] = json!("changed");
        let patch = session("delta", json!([loaded]));
        let (_, next) = save(&conn, &patch, &ids, Some(revision)).unwrap();
        assert_ne!(next, revision);
        let (count, bytes): (i64, i64) = conn
            .query_row("SELECT COUNT(*), SUM(bytes) FROM touched", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(count, 1);
        assert!(bytes < 5000);
        let value = get_session(&conn, "delta").unwrap().unwrap();
        assert_eq!(
            value.blocks[100]["tool"]["detail"],
            history[100]["tool"]["detail"]
        );
        assert_eq!(value.blocks[99], history[99]);
        assert_eq!(
            conn.query_row(
                "SELECT blocks_json FROM sessions WHERE id='delta'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "[]"
        );
        assert_eq!(
            crate::tool_detail::stored_detail(&conn, "delta", "t99", None).unwrap(),
            Some("x".repeat(4000))
        );
        assert!(search_sessions(
            &conn,
            &SessionSearchOptions {
                query: "changed".into(),
                cwd: None,
                include_archived: false,
                search_owner: "incremental-test".into()
            }
        )
        .unwrap()
        .hits
        .iter()
        .any(|h| h.block_id.as_deref() == Some("t99")));
    }
    #[test]
    fn edits_reorders_rewinds_and_full_native_writes_remain_compatible() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.conn.lock().unwrap();
        let original = session(
            "edit",
            json!([{"id":"u","role":"user","text":"hi"},{"id":"a","role":"assistant","text":"old"}]),
        );
        let (_, rev) = save(&conn, &original, &["u".into(), "a".into()], None).unwrap();
        let patch = session(
            "edit",
            json!([{"id":"a","role":"assistant","text":"edited"}]),
        );
        let (_, rev) = save(&conn, &patch, &["a".into(), "u".into()], Some(rev)).unwrap();
        assert_eq!(
            get_session(&conn, "edit").unwrap().unwrap().blocks[0]["text"],
            "edited"
        );
        let empty = session("edit", json!([]));
        save(&conn, &empty, &["u".into()], Some(rev)).unwrap();
        assert_eq!(
            get_session(&conn, "edit")
                .unwrap()
                .unwrap()
                .blocks
                .as_array()
                .unwrap()
                .len(),
            1
        );
        upsert_session(&conn, &original).unwrap();
        assert!(!indexed(&conn, "edit").unwrap());
        assert_eq!(
            get_session(&conn, "edit").unwrap().unwrap().blocks,
            original.blocks
        );
        assert!(save(&conn, &empty, &[], Some(rev))
            .unwrap_err()
            .contains("BASELINE_MISMATCH"));
    }
    #[test]
    fn invalid_delta_rolls_back_and_successful_rows_survive_reopen_and_stale_writer() {
        let path = std::env::temp_dir().join(format!("ordinary-delta-{}.db", uuid::Uuid::new_v4()));
        let first = SessionStore::open(path.clone()).unwrap();
        let original = session("durable", json!([{"id":"u","role":"user","text":"hello"}]));
        let rev = {
            let conn = first.conn.lock().unwrap();
            save(&conn, &original, &["u".into()], None).unwrap().1
        };
        let second = SessionStore::open(path.clone()).unwrap();
        let next = {
            let conn = second.conn.lock().unwrap();
            let patch = session("durable", json!([{"id":"u","role":"user","text":"new"}]));
            save(&conn, &patch, &["u".into()], Some(rev)).unwrap().1
        };
        {
            let conn = first.conn.lock().unwrap();
            assert!(save(&conn, &original, &["u".into()], Some(rev))
                .unwrap_err()
                .contains("BASELINE_MISMATCH"));
            let invalid = session("durable", json!([]));
            assert!(save(&conn, &invalid, &["missing".into()], Some(next)).is_err());
            assert_eq!(
                get_session(&conn, "durable").unwrap().unwrap().blocks[0]["text"],
                "new"
            );
        }
        drop(first);
        drop(second);
        let reopened = SessionStore::open(path.clone()).unwrap();
        {
            let conn = reopened.conn.lock().unwrap();
            let value = get_session(&conn, "durable").unwrap().unwrap();
            assert_eq!(value.persistence_revision, Some(next));
            assert_eq!(value.blocks[0]["text"], "new");
            delete_session(&conn, "durable").unwrap();
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM ordinary_blocks", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
        drop(reopened);
        let _ = std::fs::remove_file(path);
    }
    #[test]
    fn deleting_a_lead_keeps_open_normalized_workers_writable_and_detached() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.conn.lock().unwrap();
        let lead = session("lead", json!([{"id":"u","role":"user","text":"lead"}]));
        upsert_session(&conn, &lead).unwrap();
        let worker = session(
            "worker",
            json!([{"id":"u","role":"user","text":"work","orchestrationLeadId":"lead"}]),
        );
        let (_, revision) = save(&conn, &worker, &["u".into()], None).unwrap();
        assert_eq!(
            worker_parent(&conn, "worker").unwrap().as_deref(),
            Some("lead")
        );
        delete_session(&conn, "lead").unwrap();
        assert!(indexed(&conn, "worker").unwrap());
        assert!(worker_parent(&conn, "worker").unwrap().is_none());
        assert!(get_session(&conn, "worker").unwrap().unwrap().blocks[0]
            .get("orchestrationLeadId")
            .is_none());
        let mut late = worker.clone();
        late.blocks[0]["text"] = json!("late edit");
        save(&conn, &late, &["u".into()], Some(revision)).unwrap();
        let restored = get_session(&conn, "worker").unwrap().unwrap();
        assert_eq!(restored.blocks[0]["text"], "late edit");
        assert!(restored.blocks[0].get("orchestrationLeadId").is_none());
        assert!(worker_parent(&conn, "worker").unwrap().is_none());
    }
    #[test]
    fn a_legacy_full_writer_invalidates_an_already_loaded_revision() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.conn.lock().unwrap();
        let old = session("legacy", json!([{"id":"u","role":"user","text":"old"}]));
        upsert_session(&conn, &old).unwrap();
        let baseline = get_session(&conn, "legacy")
            .unwrap()
            .unwrap()
            .persistence_revision;
        let mut newer = old.clone();
        newer.blocks[0]["text"] = json!("newer native write");
        upsert_session(&conn, &newer).unwrap();
        assert!(save(&conn, &old, &["u".into()], baseline)
            .unwrap_err()
            .contains("BASELINE_MISMATCH"));
        assert_eq!(
            get_session(&conn, "legacy").unwrap().unwrap().blocks,
            newer.blocks
        );
    }
    #[test]
    fn search_limits_semantic_hits_instead_of_raw_json_candidates() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.conn.lock().unwrap();
        let mut blocks = vec![json!({"id":"u","role":"user","text":"request"})];
        for index in 0..MAX_MESSAGE_HITS {
            blocks.push(json!({"id":format!("meta{index}"),"role":"tool"}));
        }
        blocks.push(json!({"id":"answer","role":"assistant","text":"use this tool"}));
        let ids = blocks
            .iter()
            .map(|b| b["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        save(&conn, &session("search", Value::Array(blocks)), &ids, None).unwrap();
        let found = search_sessions(
            &conn,
            &SessionSearchOptions {
                query: "tool".into(),
                cwd: None,
                include_archived: false,
                search_owner: "candidate-filter".into(),
            },
        )
        .unwrap();
        assert_eq!(found.hits.iter().filter(|h| h.kind == "message").count(), 1);
        assert!(found
            .hits
            .iter()
            .any(|h| h.block_id.as_deref() == Some("answer")));
        assert!(!found.truncated);
        let (candidates, limited) = matching_blocks(&conn, "search", "%tool%", 8).unwrap();
        assert!(limited);
        assert!(candidates.as_array().unwrap().is_empty());
    }
}
