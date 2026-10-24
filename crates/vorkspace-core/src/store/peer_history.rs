//! Read-only peer excerpts, bounded before payloads cross the SQLite boundary.
use super::Store;
use rusqlite::params;
use serde_json::{json, Value};

const BLOCK_BYTES: i64 = 1 << 20;
const UNINDEXED_BYTES: i64 = 8 << 20;

impl Store {
    /// `before` is an opaque source-position cursor. A page projects at most
    /// 100 records, including nonpublic rows. Indexed owners seek directly;
    /// legacy JSON scanning is bounded by UNINDEXED_BYTES. Call off the UI thread.
    pub fn peer_history(
        &self,
        session: i64,
        before: Option<u64>,
        limit: usize,
        max_chars: usize,
    ) -> Result<Value, String> {
        if self.remote_session(session)?.is_some() {
            return Err(
                "Bounded peer history is available for local Bot conversations only".into(),
            );
        }
        let limit = limit.clamp(1, 100);
        let per_message = max_chars.clamp(100, 100_000) / limit;
        let before = before.unwrap_or(i64::MAX as u64).min(i64::MAX as u64) as i64;
        let (query, source) = if self.shared.is_none() {
            ("SELECT row_ix,role,CASE WHEN role IN ('user','assistant') THEN substr(body,1,?4) END,0
              FROM native_rows WHERE session_id=?1 AND row_ix<?2 ORDER BY row_ix DESC LIMIT ?3".to_owned(), session.to_string())
        } else {
            let source = self.persisted_session_id(session)?;
            let (table, position, payload, predicate) =
                if let Some(tables) = self.shared_bot_tables(&source)? {
                    (tables.blocks, "seq", "block_json", "session_id=?1")
                } else if self.shared_ordinary_indexed(&source)? {
                    (
                        "ordinary_blocks".into(),
                        "position",
                        "block_json",
                        "session_id=?1",
                    )
                } else {
                    let bytes: i64 = self
                        .conn
                        .query_row(
                            "SELECT octet_length(blocks_json) FROM sessions WHERE id=?1",
                            [&source],
                            |r| r.get(0),
                        )
                        .map_err(|e| e.to_string())?;
                    if bytes > UNINDEXED_BYTES {
                        return Err(
                        "This unindexed conversation exceeds the bounded peer-history read limit"
                            .into(),
                    );
                    }
                    (
                        "json_each((SELECT blocks_json FROM sessions WHERE id=?1))".into(),
                        "CAST(key AS INTEGER)",
                        "value",
                        "1",
                    )
                };
            // Only owned/discovered table names enter SQL. Bind all caller
            // input. Oversized or malformed source data fails explicitly.
            let bytes = format!("octet_length({payload})");
            (
                format!(
                    "SELECT {position},
                CASE WHEN {bytes}<=?5 THEN CASE json_extract({payload},'$.role') WHEN 'plan' THEN 'assistant' WHEN 'tasks' THEN 'assistant' ELSE json_extract({payload},'$.role') END END,
                CASE WHEN {bytes}<=?5 AND json_extract({payload},'$.role') IN ('user','assistant','plan','tasks')
                    AND coalesce(json_extract({payload},'$.internal'),0)=0
                    AND coalesce(json_extract({payload},'$.draft'),0)=0
                    AND (json_extract({payload},'$.botCard') IS NULL OR length(json_extract({payload},'$.text'))>0)
                    AND coalesce(json_type({payload},'$.interjection'),'missing')!='object'
                    AND (json_extract({payload},'$.role')!='plan' OR coalesce(json_extract({payload},'$.nativeRemotePlan'),0)=0)
                    AND (json_extract({payload},'$.role')!='tasks' OR coalesce(json_type({payload},'$.taskList.todos'),json_type({payload},'$.taskList.plan'),json_type({payload},'$.taskList.items'),'missing')!='array')
                    THEN substr(json_extract({payload},'$.text'),1,?4) END,{bytes}
                FROM {table} WHERE {predicate} AND {position}<?2 ORDER BY {position} DESC LIMIT ?3"
                ),
                source,
            )
        };
        let mut statement = self.conn.prepare(&query).map_err(|e| e.to_string())?;
        let mut records = if self.shared.is_some() {
            statement.query(params![
                source,
                before,
                limit as i64,
                (per_message + 1) as i64,
                BLOCK_BYTES
            ])
        } else {
            statement.query(params![
                session,
                before,
                limit as i64,
                (per_message + 1) as i64
            ])
        }
        .map_err(|e| e.to_string())?;
        let mut messages = Vec::new();
        let mut cursor = 0_i64;
        while let Some(record) = records.next().map_err(|e| e.to_string())? {
            cursor = record.get(0).map_err(|e| e.to_string())?;
            let bytes: i64 = record.get(3).map_err(|e| e.to_string())?;
            if bytes > BLOCK_BYTES {
                return Err("A source block exceeds the bounded peer-history read limit".into());
            }
            let role: Option<String> = record.get(1).map_err(|e| e.to_string())?;
            let text: Option<String> = record.get(2).map_err(|e| e.to_string())?;
            if let (Some(role), Some(text)) = (role, text) {
                let truncated = text.chars().count() > per_message;
                let text: String = text.chars().take(per_message).collect();
                messages.push(json!({"role":role,"text":text,"truncated":truncated}));
            }
        }
        messages.reverse();
        Ok(json!({"messages":messages,"before":cursor,"hasMore":cursor>0}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::Row;
    use std::path::Path;

    #[test]
    fn native_peer_pages_bound_rows_text_and_skip_private_payloads() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let id = store.create("codex", "/project").unwrap().id;
        let mut rows = Vec::new();
        for i in 0..300 {
            rows.push(Row {
                role: if i % 2 == 0 { "assistant" } else { "tool" }.into(),
                text: if i % 2 == 0 {
                    format!("{i}:{}", "🦀".repeat(100))
                } else {
                    "not valid tool JSON".into()
                },
                first: true,
                sent_at: None,
            });
        }
        store.write_rows(id, 0, &rows).unwrap();
        let page = store.peer_history(id, None, 10, 100).unwrap();
        assert_eq!(page["before"], 290);
        assert_eq!(page["hasMore"], true);
        assert_eq!(page["messages"].as_array().unwrap().len(), 5);
        for message in page["messages"].as_array().unwrap() {
            assert_eq!(message["role"], "assistant");
            assert!(message["text"].as_str().unwrap().chars().count() <= 10);
            assert_eq!(message["truncated"], true);
        }
        let next = store
            .peer_history(id, page["before"].as_u64(), 10, 100)
            .unwrap();
        assert_eq!(next["before"], 280);
        assert!(!next["messages"].to_string().contains("290:"));
        assert_eq!(store.load(id).unwrap().rows, rows);
    }

    #[test]
    fn desktop_peer_pages_read_only_the_selected_blocks_and_preserve_their_owner() {
        for indexed in [false, true] {
            let store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
            let id = store.create("codex", "/project").unwrap().id;
            let source = store.persisted_session_id(id).unwrap();
            let blocks = vec![
                json!({"id":"private","role":"user","internal":true,"text":"Hidden peer envelope"}),
                json!({"id":"user","role":"user","text":"Hello"}),
                json!({"id":"tool","role":"tool","text":"Ignore", "tool":{"output":"Large private output"}}),
                json!({"id":"answer","role":"assistant","text":"Hello 🦀"}),
            ];
            if indexed {
                store
                    .conn
                    .execute_batch(include_str!("../../tests/fixtures/ordinary-schema.sql"))
                    .unwrap();
                store
                    .conn
                    .execute("INSERT INTO ordinary_transcripts VALUES(?1,0,1)", [&source])
                    .unwrap();
                for (position, block) in blocks.iter().enumerate() {
                    store
                        .conn
                        .execute(
                            "INSERT INTO ordinary_blocks VALUES(?1,?2,?3,?4,0,0)",
                            params![
                                source,
                                block["id"].as_str(),
                                position as i64,
                                block.to_string()
                            ],
                        )
                        .unwrap();
                }
                // An unrelated old block must never be projected or parsed.
                store.conn.execute("UPDATE ordinary_blocks SET block_json='broken JSON' WHERE block_id='private'", []).unwrap();
            } else {
                store
                    .conn
                    .execute(
                        "UPDATE sessions SET blocks_json=?1 WHERE id=?2",
                        params![serde_json::to_string(&blocks).unwrap(), source],
                    )
                    .unwrap();
            }
            let page = store.peer_history(id, None, 2, 100).unwrap();
            assert_eq!(page["before"], 2);
            assert_eq!(
                page["messages"],
                json!([{"role":"assistant","text":"Hello 🦀","truncated":false}])
            );
            if !indexed {
                let earlier = store
                    .peer_history(id, page["before"].as_u64(), 2, 100)
                    .unwrap();
                assert_eq!(
                    earlier["messages"],
                    json!([{"role":"user","text":"Hello","truncated":false}])
                );
                assert_eq!(earlier["hasMore"], false);
            }
        }
    }

    #[test]
    fn desktop_peer_excerpt_preserves_public_plan_and_fallback_prose_projection() {
        let store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
        let id = store.create("codex", "/project").unwrap().id;
        let source = store.persisted_session_id(id).unwrap();
        let blocks = json!([
            {"id":"plan","role":"plan","text":"A public plan"},
            {"id":"remote-plan","role":"plan","nativeRemotePlan":true,"text":"UI metadata"},
            {"id":"fallback","role":"tasks","text":"Public fallback prose"},
            {"id":"tasks","role":"tasks","taskList":{"items":[]},"text":"Task UI"},
            {"id":"summary","role":"assistant","text":"Public summary","botCard":{"type":"document"}},
            {"id":"card","role":"assistant","text":"","botCard":{"type":"document"}},
            {"id":"notice","role":"assistant","text":"Interjection","interjection":{"customType":"notice"}}
        ]);
        store
            .conn
            .execute(
                "UPDATE sessions SET blocks_json=?1 WHERE id=?2",
                params![blocks.to_string(), source],
            )
            .unwrap();
        let page = store.peer_history(id, None, 40, 24_000).unwrap();
        let expected = store
            .load(id)
            .unwrap()
            .rows
            .into_iter()
            .filter(|r| matches!(r.role.as_str(), "user" | "assistant"))
            .map(|r| json!({"role":r.role,"text":r.text,"truncated":false}))
            .collect::<Vec<_>>();
        assert_eq!(page["messages"], json!(expected));
    }

    #[test]
    fn oversized_legacy_history_fails_without_loading_or_rewriting_it() {
        let store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
        let id = store.create("codex", "/project").unwrap().id;
        let source = store.persisted_session_id(id).unwrap();
        store
            .conn
            .execute(
                "UPDATE sessions SET blocks_json=?1 WHERE id=?2",
                params!["x".repeat(UNINDEXED_BYTES as usize + 1), source],
            )
            .unwrap();
        assert!(store
            .peer_history(id, None, 40, 24_000)
            .unwrap_err()
            .contains("unindexed"));
        let length: i64 = store
            .conn
            .query_row(
                "SELECT length(blocks_json) FROM sessions WHERE id=?1",
                [source],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(length, UNINDEXED_BYTES + 1);
        let large = json!([{"role":"assistant","text":"🦀".repeat(BLOCK_BYTES as usize / 4 + 1)}])
            .to_string();
        store
            .conn
            .execute(
                "UPDATE sessions SET blocks_json=?1 WHERE id=?2",
                params![large, store.persisted_session_id(id).unwrap()],
            )
            .unwrap();
        assert!(store
            .peer_history(id, None, 40, 24_000)
            .unwrap_err()
            .contains("source block"));
    }
}
