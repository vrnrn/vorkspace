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
mod tests;
