//! A local provider turn owns its write baseline independently of view loads.
use super::*;
use rusqlite::OptionalExtension;
use sha2::{Digest, Sha256};

pub struct TurnHistory {
    session: i64,
    pub(super) version: Option<(i64, i64)>,
    rows: Vec<[u8; 32]>,
}

impl TurnHistory {
    pub(super) fn fingerprints(rows: &[Row]) -> impl Iterator<Item = [u8; 32]> + '_ {
        rows.iter().map(|row| {
            let mut hash = Sha256::new();
            hash.update((row.role.len() as u64).to_le_bytes());
            hash.update(row.role.as_bytes());
            hash.update([u8::from(row.first), u8::from(row.sent_at.is_some())]);
            hash.update(row.sent_at.unwrap_or_default().to_le_bytes());
            hash.update(row.text.as_bytes());
            hash.finalize().into()
        })
    }

    pub(super) fn matches(&self, rows: &[Row]) -> bool {
        self.rows.iter().copied().eq(Self::fingerprints(rows))
    }

    pub(super) fn committed(
        &mut self,
        version: Option<(i64, i64)>,
        from: usize,
        rows: Vec<[u8; 32]>,
    ) {
        self.version = version;
        self.rows.truncate(from);
        self.rows.extend(rows);
    }
}

#[derive(Debug, Clone)]
pub struct RecoveredTurn {
    pub id: String,
    pub created_at: u64,
}

pub(super) fn ensure_recovery_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS conversation_turn_recoveries(
            session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
            recovery_id TEXT NOT NULL, rows_json TEXT NOT NULL, created_at INTEGER NOT NULL,
            PRIMARY KEY(session_id,recovery_id)
        );",
    )
    .map_err(|e| e.to_string())
}

impl Store {
    pub fn load_turn_history(&self, id: i64) -> Result<(Transcript, Option<TurnHistory>), String> {
        if self.shared.is_none() {
            return Ok((self.load(id)?, None));
        }
        self.shared_history_schema()?;
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        let version = self.shared_turn_version(id)?;
        let transcript = self.shared_load(id)?;
        tx.commit().map_err(|e| e.to_string())?;
        let writer = TurnHistory {
            session: id,
            version,
            rows: TurnHistory::fingerprints(&transcript.rows).collect(),
        };
        Ok((transcript, Some(writer)))
    }

    /// False means the turn's baseline was superseded. A caller can preserve
    /// its output separately, but must never retry its numeric offsets.
    pub fn write_turn_rows(
        &mut self,
        id: i64,
        start: usize,
        rows: &[Row],
        message: Option<&str>,
        writer: Option<&mut TurnHistory>,
    ) -> Result<bool, String> {
        if let Some(writer) = writer {
            if writer.session != id {
                return Err("Turn history belongs to another conversation".into());
            }
            self.shared_write_rows(id, start, rows, message, Some(writer))
        } else {
            self.write_rows_with_message(id, start, rows, message)?;
            Ok(true)
        }
    }

    pub fn save_turn_info(
        &self,
        id: i64,
        at: usize,
        info: crate::agent::TurnInfo,
        writer: &mut TurnHistory,
    ) -> Result<bool, String> {
        if writer.session != id {
            return Err("Turn history belongs to another conversation".into());
        }
        self.shared_save_turns(id, &[(at, info)].into(), Some(writer))
    }

    /// Publish the competing version without changing the winner's transcript,
    /// writer cursor, receipts, or composer draft. Retrying the same intent
    /// cannot create another copy or replace a different saved version.
    pub fn preserve_turn_overlap(
        &self,
        id: i64,
        attempt: &str,
        rows: &[Row],
        abandoned_binding: Option<&str>,
    ) -> Result<(), String> {
        self.shared_writable()?;
        let original = self.persisted_session_id(id)?;
        let raw = serde_json::to_string(rows).map_err(|e| e.to_string())?;
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )
        .map_err(|e| e.to_string())?;
        ensure_recovery_schema(&tx)?;
        let inserted = tx.execute(
            "INSERT INTO conversation_turn_recoveries(session_id,recovery_id,rows_json,created_at)
             VALUES(?1,?2,?3,?4) ON CONFLICT(session_id,recovery_id) DO NOTHING",
            params![original, attempt, raw, crate::time::now_ms().min(i64::MAX as u64) as i64],
        ).map_err(|e| e.to_string())?;
        if inserted == 0 {
            let saved: String = tx.query_row(
                "SELECT rows_json FROM conversation_turn_recoveries WHERE session_id=?1 AND recovery_id=?2",
                params![original, attempt], |row| row.get(0),
            ).map_err(|e| e.to_string())?;
            if saved != raw {
                return Err("Recovery identifier already holds a different turn".into());
            }
        }
        if let Some(binding) = abandoned_binding.filter(|_| inserted != 0) {
            // A subsequent explicit send starts fresh if the losing
            // provider thread still owns the binding. A successor wins.
            tx.execute(
                "UPDATE sessions SET provider_session_id=NULL WHERE id=?1 AND provider_session_id=?2",
                params![original, binding],
            ).map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }

    pub fn recovered_turns(&self, id: i64) -> Result<Vec<RecoveredTurn>, String> {
        if self.shared.is_none() || !shared::has_table(&self.conn, "conversation_turn_recoveries")?
        {
            return Ok(Vec::new());
        }
        let mut query = self.conn.prepare(
            "SELECT recovery_id,created_at FROM conversation_turn_recoveries WHERE session_id=?1 ORDER BY created_at DESC,recovery_id",
        ).map_err(|e| e.to_string())?;
        let rows = query
            .query_map([self.persisted_session_id(id)?], |row| {
                Ok(RecoveredTurn {
                    id: row.get(0)?,
                    created_at: row.get::<_, i64>(1)?.max(0) as u64,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<rusqlite::Result<_>>()
            .map_err(|e| e.to_string())
    }

    pub fn read_recovered_turn(&self, id: i64, attempt: &str) -> Result<Option<Vec<Row>>, String> {
        if self.shared.is_none() || !shared::has_table(&self.conn, "conversation_turn_recoveries")?
        {
            return Ok(None);
        }
        let raw: Option<String> = self.conn.query_row(
            "SELECT rows_json FROM conversation_turn_recoveries WHERE session_id=?1 AND recovery_id=?2",
            params![self.persisted_session_id(id)?, attempt], |row| row.get(0),
        ).optional().map_err(|e| e.to_string())?;
        raw.map(|raw| serde_json::from_str(&raw).map_err(|e| e.to_string()))
            .transpose()
    }
}

pub fn recovery_text(rows: &[Row]) -> String {
    let mut text =
        String::from("This version was saved separately after the conversation changed.\n");
    for row in rows {
        if row.first {
            let label = match row.role.as_str() {
                "user" | "bot-system" => "Prompt",
                "assistant" => "Response",
                "tool" => "Tool",
                crate::transcript::attachments::ROLE => "Attachments",
                role => role,
            };
            text.push_str("\n\n");
            text.push_str(label);
            text.push('\n');
        }
        text.push_str(&row.text);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_view_reload_cannot_bless_an_overlapping_turn_and_both_versions_survive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.db");
        let mut losing = Store::open_vorkspace(&path).unwrap();
        let id = losing.create("codex", "/project").unwrap().id;
        let mut history = Transcript::default();
        history.begin("user");
        history.append("Question");
        history.begin("assistant");
        history.append("Starting answer");
        losing.write_rows(id, 0, &history.rows).unwrap();
        let (mut left, mut author) = losing.load_turn_history(id).unwrap();
        left.rows[1].text = "Competing response".into();
        let draft = serde_json::json!({"cwd":"/project","text":"Keep my unsent draft","attachments":[],"mcpTags":[],"selectionStart":0,"selectionEnd":0});
        losing.save_composer_draft(id, 0, &draft).unwrap();

        let mut winner = Store::open_vorkspace(&path).unwrap();
        let (mut right, mut active) = winner.load_turn_history(id).unwrap();
        right.rows[1].text = "Canonical response".into();
        assert!(winner
            .write_turn_rows(id, 1, &right.rows[1..], None, active.as_mut())
            .unwrap());
        assert_eq!(losing.load(id).unwrap().rows, right.rows);
        assert!(!losing
            .write_turn_rows(id, 1, &left.rows[1..], None, author.as_mut())
            .unwrap());

        let revision = winner.session_history_revision(id).unwrap();
        for _ in 0..2 {
            losing
                .preserve_turn_overlap(id, "same-attempt", &left.rows, None)
                .unwrap();
        }
        assert_eq!(winner.session_history_revision(id).unwrap(), revision);
        right.rows[1].text.push_str(" continues");
        assert!(winner
            .write_turn_rows(id, 1, &right.rows[1..], None, active.as_mut())
            .unwrap());
        assert!(losing
            .preserve_turn_overlap(id, "same-attempt", &right.rows, None)
            .is_err());

        let reopened = Store::read_vorkspace(&path).unwrap();
        assert_eq!(reopened.load(id).unwrap().rows, right.rows);
        let recovered = reopened.recovered_turns(id).unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(
            reopened.read_recovered_turn(id, &recovered[0].id).unwrap(),
            Some(left.rows)
        );
        assert_eq!(reopened.composer_draft(id).unwrap().unwrap().payload, draft);
    }
}
