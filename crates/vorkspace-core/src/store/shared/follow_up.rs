//! The desktop schema keeps queued messages and saved drafts in separate rows.
use super::*;
use crate::follow_up::{Message, State};
use crate::store::follow_up::FollowUpWrite;

#[derive(serde::Serialize, serde::Deserialize)]
struct QueueRevision {
    #[serde(default)]
    revision: u64,
}

impl Store {
    fn shared_follow_up_revision(&self, id: i64, source: &str) -> Result<u64, String> {
        let mut raw = None;
        if has_table(&self.conn, "native_follow_up_metadata")? {
            raw = sql(self
                .conn
                .query_row(
                    "SELECT state_json FROM native_follow_up_metadata WHERE session_id=?1",
                    [source],
                    |row| row.get(0),
                )
                .optional())?;
        }
        if raw.is_none() {
            raw = self.shared_setting(&format!("follow-up:{id}"))?;
        }
        raw.map(|raw| parse::<QueueRevision>(&raw))
            .transpose()
            .map(|metadata| metadata.map_or(0, |metadata| metadata.revision))
    }

    fn shared_has_saved_draft(&self, source: &str) -> Result<bool, String> {
        let query = if let Some(tables) = self.shared_bot_tables(source)? {
            format!(
                "SELECT EXISTS(SELECT 1 FROM {} WHERE session_id=?1 AND is_draft=1)",
                tables.blocks
            )
        } else if self.shared_ordinary_indexed(source)? {
            "SELECT EXISTS(SELECT 1 FROM ordinary_blocks WHERE session_id=?1 AND has_user=1 AND is_draft=1)".into()
        } else {
            // A previous desktop can update the inline transcript without
            // maintaining its summary flag. Inspect its actual draft field.
            "SELECT CASE WHEN json_type(blocks_json)='array' THEN EXISTS(
                SELECT 1 FROM json_each(blocks_json)
                WHERE CASE WHEN type='object' THEN json_type(value,'$.draft')='true' END
             ) ELSE 1 END FROM sessions WHERE id=?1"
                .into()
        };
        sql(self.conn.query_row(&query, [source], |row| row.get(0)))
    }

    pub(crate) fn shared_follow_ups(&self, id: i64) -> Result<State, String> {
        let transaction = self
            .conn
            .is_autocommit()
            .then(|| self.conn.unchecked_transaction())
            .transpose()
            .map_err(|error| error.to_string())?;
        let source = self.persisted_session_id(id)?;
        let fields = format!(
            "{},{}",
            self.shared_field("queued_messages_json", "'[]'"),
            self.shared_field("queue_status", "NULL")
        );
        let (raw, status) = sql(self.conn.query_row(
            &format!("SELECT {fields} FROM sessions WHERE id=?1"),
            [&source],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
        ))?;
        let revision = self.shared_follow_up_revision(id, &source)?;
        let messages: Vec<_> = parse::<Vec<Value>>(&raw)?.iter().map(follow_up).collect();
        let mut draft = None;
        if self.shared_has_saved_draft(&source)? {
            let blocks = self.shared_blocks(id)?;
            if let Some((at, block)) = blocks.iter().enumerate().find(|(_, b)| b["draft"] == true) {
                let mut message = follow_up(block);
                message.at = blocks::project(&blocks[..at])?.len();
                draft = Some(message);
            }
        }
        if let Some(transaction) = transaction {
            sql(transaction.commit())?;
        }
        self.shared
            .as_ref()
            .unwrap()
            .queues
            .borrow_mut()
            .insert(id, raw);
        Ok(State {
            revision,
            paused: status.as_deref() == Some("paused") || !messages.is_empty(),
            messages,
            draft,
        })
    }
    pub(in crate::store) fn shared_write_follow_ups(
        &self,
        id: i64,
        state: &State,
        write: FollowUpWrite<'_>,
    ) -> Result<(), String> {
        self.shared_writable()?;
        let tx = sql(rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        ))?;
        incremental::ensure_bookkeeping_schema(&tx)?;
        let source = self.persisted_session_id(id)?;
        let previous = self.shared_follow_up_revision(id, &source)?;
        if !write.check_revision(previous, state.revision)? {
            return Ok(());
        }
        self.transfer_follow_up(&tx, id, state, write)?;
        let fields = self.shared_field("queued_messages_json", "'[]'");
        let raw: String = sql(self.conn.query_row(
            &format!("SELECT {fields} FROM sessions WHERE id=?1"),
            [&source],
            |r| r.get(0),
        ))?;
        if self
            .shared
            .as_ref()
            .unwrap()
            .queues
            .borrow()
            .get(&id)
            .is_some_and(|expected| expected != &raw)
        {
            return Err("The message queue changed in another window. Reopen it before saving; its data was preserved.".into());
        }
        let old: Vec<Value> = parse(&raw)?;
        let messages = state
            .messages
            .iter()
            .map(|m| {
                let mut value = old
                    .iter()
                    .find(|v| v["id"].as_str() == Some(&m.id))
                    .cloned()
                    .unwrap_or(json!({}));
                update_follow_up(&mut value, m);
                value
            })
            .collect::<Vec<_>>();
        let raw = encode(&messages)?;
        self.shared_patch(
            id,
            &[
                ("queued_messages_json", json!(raw)),
                (
                    "queue_status",
                    json!(if state.paused { "paused" } else { "active" }),
                ),
            ],
        )?;
        let write_draft = state.draft.is_some() || self.shared_has_saved_draft(&source)?;
        let history = if write_draft {
            Some(self.shared_write_saved_draft(id, state.draft.as_ref())?)
        } else {
            None
        };
        let metadata = QueueRevision {
            revision: state.revision,
        };
        sql(tx.execute("INSERT INTO native_follow_up_metadata(session_id,state_json) VALUES(?1,?2) ON CONFLICT(session_id) DO UPDATE SET state_json=excluded.state_json", params![source, encode(&metadata)?]))?;
        sql(tx.commit())?;
        let shared = self.shared.as_ref().unwrap();
        if let Some(history) = history {
            shared.snapshots.borrow_mut().insert(id, history);
        }
        shared.queues.borrow_mut().insert(id, raw);
        Ok(())
    }

    fn shared_write_saved_draft(
        &self,
        id: i64,
        draft: Option<&Message>,
    ) -> Result<HistoryRevision, String> {
        let mut blocks = self.shared_blocks(id)?;
        self.shared_validate_history_or_fast_forward(id, &blocks)?;

        let old_draft = blocks.iter().find(|b| b["draft"] == true).cloned();
        blocks.retain(|b| b["draft"] != true);
        if let Some(draft) = draft {
            let mut value = old_draft
                .filter(|v| {
                    v["id"].as_str() == Some(&draft.id)
                        || v["id"].as_str() == draft.replaces_draft_id.as_deref()
                })
                .unwrap_or(json!({"id":draft.id,"role":"user","draft":true}));
            update_follow_up(&mut value, draft);
            let mut at = blocks.len();
            let mut row = 0;
            for (ix, block) in blocks.iter().enumerate() {
                if row >= draft.at {
                    at = ix;
                    break;
                }
                row += blocks::project(std::slice::from_ref(block))?.len();
            }
            blocks.insert(at, value);
        }
        self.shared_store_blocks(id, &blocks)?;
        Ok(HistoryRevision::of(&blocks))
    }
}

fn follow_up(value: &Value) -> Message {
    Message {
        id: value["id"].as_str().unwrap_or("").into(),
        text: value["text"].as_str().unwrap_or("").into(),
        prepared_prompt: value["preparedPrompt"].as_str().map(str::to_owned),
        attachments: value["attachments"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| a["path"].as_str())
            .map(PathBuf::from)
            .collect(),
        mode: value["intent"].as_str().map(str::to_owned),
        error: value["error"].as_str().map(str::to_owned),
        ..Default::default()
    }
}
fn update_follow_up(value: &mut Value, message: &Message) {
    value["id"] = json!(message.id);
    value["text"] = json!(message.text);
    value["preparedPrompt"] = json!(message.prepared_prompt);
    if let Some(mode) = &message.mode {
        value["intent"] = json!(mode);
    }
    value["error"] = json!(message.error);
    if message.replaces_draft_id.is_some() {
        if let Some(files) = value["attachments"].as_array_mut() {
            if files.len() == message.attachments.len() {
                for (file, path) in files.iter_mut().zip(&message.attachments) {
                    file["path"] = json!(path);
                }
            }
        }
    }
    if value["attachments"].is_null() {
        value["attachments"]=json!(message.attachments.iter().map(|p|json!({"id":p.to_string_lossy(),"path":p,"name":p.file_name().unwrap_or_default().to_string_lossy(),
            "mimeType":"application/octet-stream","kind":"file","size":std::fs::metadata(p).map(|m|m.len()).unwrap_or(0)})).collect::<Vec<_>>());
    }
}
