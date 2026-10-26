//! Card refreshes own one metadata value, never a projected transcript suffix.
use super::*;
use rusqlite::OptionalExtension;
use serde_json::Value;

impl Store {
    /// Compare the complete displayed card with its durable owner, then change
    /// only that card. An already-saved desired value returns its current row
    /// without writing. None means deletion/replacement or a newer card value.
    /// Unrelated appends and unloaded rich history are deliberately not inputs.
    pub fn refresh_bot_card(
        &self,
        session: i64,
        expected: &Value,
        desired: &Value,
    ) -> Result<Option<Row>, String> {
        let id = expected["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or("Card refresh requires a durable identity")?;
        if !expected.is_object() || !desired.is_object() || desired["id"] != expected["id"] {
            return Err("Card refresh cannot change its identity".into());
        }
        for key in ["type", "sessionId", "documentId", "repo", "number"] {
            if expected.get(key) != desired.get(key) {
                return Err("Card refresh cannot change its reference".into());
            }
        }
        if self.remote_session(session)?.is_some() {
            return Err("Remote transcripts are owned by the host".into());
        }
        if self.shared.is_some() {
            return self.shared_refresh_bot_card(session, id, expected, desired);
        }
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        let saved: Option<(i64, String, bool, Option<i64>)> = tx.query_row(
            "SELECT row_ix,body,first,sent_at FROM native_rows WHERE session_id=?1 AND role='bot-card'
             AND json_valid(body) AND json_extract(body,'$.id')=?2",
            params![session,id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
        ).optional().map_err(|e| e.to_string())?;
        let Some((index, text, first, sent_at)) = saved else {
            return Ok(None);
        };
        let current: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        if current != *expected && current != *desired {
            return Ok(None);
        }
        let row = Row {
            role: "bot-card".into(),
            text: desired.to_string(),
            first,
            sent_at: sent_at.and_then(|at| u64::try_from(at).ok()),
        };
        if current != *desired {
            tx.execute(
                "UPDATE native_rows SET body=?3 WHERE session_id=?1 AND row_ix=?2",
                params![session, index, row.text],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(Some(row))
    }
}
