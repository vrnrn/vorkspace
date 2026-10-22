//! Vorkspace's normalized ordinary history remains authoritative after a native
//! edit. Never fall back to its intentionally empty sessions.blocks_json cache.
use super::*;

impl Store {
    pub(in crate::store) fn shared_ordinary_indexed(&self, original: &str) -> Result<bool, String> {
        if !has_table(&self.conn, "ordinary_transcripts")? {
            return Ok(false);
        }
        sql(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM ordinary_transcripts WHERE session_id=?1 AND normalized=1)",
            [original],
            |row| row.get(0),
        ))
    }

    pub(super) fn shared_ordinary_blocks(&self, original: &str) -> Result<Vec<Value>, String> {
        let mut query = sql(self.conn.prepare(
            "SELECT block_json FROM ordinary_blocks WHERE session_id=?1 ORDER BY position",
        ))?;
        let blocks = sql(query.query_map([original], |row| row.get::<_, String>(0)))?
            .map(|raw| parse(&sql(raw)?))
            .collect();
        blocks
    }

    // The caller owns the transaction and validates its loaded history revision.
    // Advance the shared revision too, so another storage client cannot apply
    // a stale delta over this transaction's changes.
    pub(super) fn shared_store_ordinary_blocks(
        &self,
        original: &str,
        blocks: &[Value],
    ) -> Result<(), String> {
        let ids: HashSet<_> = blocks.iter().filter_map(|b| b["id"].as_str()).collect();
        if ids.len() != blocks.len() {
            return Err("Indexed conversation contains missing or duplicate block IDs".into());
        }
        let mut query = sql(self
            .conn
            .prepare("SELECT block_id FROM ordinary_blocks WHERE session_id=?1"))?;
        let old = sql(
            sql(query.query_map([original], |row| row.get::<_, String>(0)))?
                .collect::<rusqlite::Result<Vec<_>>>(),
        )?;
        drop(query);
        for id in old {
            if !ids.contains(id.as_str()) {
                sql(self.conn.execute(
                    "DELETE FROM ordinary_blocks WHERE session_id=?1 AND block_id=?2",
                    params![original, id],
                ))?;
            }
        }
        for (position, block) in blocks.iter().enumerate() {
            sql(self.conn.execute(
                "INSERT INTO ordinary_blocks(session_id,block_id,position,block_json,has_user,is_draft)
                 VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(session_id,block_id) DO UPDATE SET
                 position=excluded.position,block_json=excluded.block_json,has_user=excluded.has_user,is_draft=excluded.is_draft
                 WHERE ordinary_blocks.block_json != excluded.block_json OR ordinary_blocks.position != excluded.position",
                params![original, block["id"].as_str().unwrap(), position as i64, encode(block)?,
                    block["role"] == "user", block["role"] == "user" && block["draft"] == true],
            ))?;
        }
        sql(self.conn.execute(
            "UPDATE ordinary_transcripts SET revision=CASE WHEN revision>=9007199254740990 THEN 0 ELSE revision+1 END WHERE session_id=?1",
            [original],
        ))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_history_round_trips_without_overwriting_unloaded_payloads() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vorkspace.db");
        let mut store = Store::open_vorkspace(&path).unwrap();
        // Frozen desktop storage contract, independent of the old UI source.
        store
            .conn
            .execute_batch(include_str!("../../../tests/fixtures/ordinary-schema.sql"))
            .unwrap();
        let session = store.create("codex", "/project").unwrap();
        let original = store.persisted_session_id(session.id).unwrap();
        store
            .conn
            .execute(
                "INSERT INTO ordinary_transcripts VALUES(?1,41,1)",
                [&original],
            )
            .unwrap();
        let block = json!({"id":"saved", "role":"assistant", "text":"Retained history", "tool":{"detail":"large durable output"}, "future":{"keep":true}});
        store
            .conn
            .execute(
                "INSERT INTO ordinary_blocks VALUES(?1,'saved',0,?2,0,0)",
                params![original, block.to_string()],
            )
            .unwrap();
        let mut transcript = store.load(session.id).unwrap();
        let start = transcript.begin("user");
        transcript.append("Continue");
        store
            .write_rows(session.id, start, &transcript.rows[start..])
            .unwrap();
        assert_eq!(store.shared_blocks(session.id).unwrap()[0], block);
        assert_eq!(
            store
                .conn
                .query_row("SELECT revision FROM ordinary_transcripts", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            42
        );
        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT blocks_json FROM sessions WHERE id=?1",
                    [&original],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "[]"
        );
        // Another desktop writer changes history after the native snapshot.
        store.conn.execute("UPDATE ordinary_blocks SET block_json=json_set(block_json,'$.text','Desktop edit') WHERE block_id='saved'", []).unwrap();
        assert!(store
            .write_rows(session.id, start, &transcript.rows[start..])
            .unwrap_err()
            .contains("changed in another window"));
        drop(store);
        let reopened = Store::open_vorkspace(&path).unwrap();
        let id = reopened.resolve_session_id(&original).unwrap();
        let history = reopened.load(id).unwrap();
        assert!(history.rows.iter().any(|row| row.text == "Desktop edit"));
        assert!(history.rows.iter().any(|row| row.text == "Continue"));
    }
}
