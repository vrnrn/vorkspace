use super::*;
use crate::store::{EditClaim, EditSnapshot};

impl Store {
    pub(in crate::store) fn shared_validate_edit_rows(
        &self,
        id: i64,
        expected: &[Value],
    ) -> Result<(), String> {
        let current = self.shared_blocks(id)?;
        if current != expected {
            return Err("Conversation metadata changed; its history was preserved".into());
        }
        self.shared_edit_queue_empty(id, &current)
    }
    fn shared_edit_queue_empty(&self, id: i64, blocks: &[Value]) -> Result<(), String> {
        // Validation must not refresh the optimistic queue/history revision:
        // a failed edit leaves the visible revision old and still guarded.
        let field = self.shared_field("queued_messages_json", "'[]'");
        let raw: String = sql(self.conn.query_row(
            &format!("SELECT {field} FROM sessions WHERE id=?1"),
            [self.persisted_session_id(id)?],
            |row| row.get(0),
        ))?;
        let queued: Vec<Value> = parse(&raw)?;
        if !queued.is_empty() || blocks.iter().any(|block| block["draft"] == true) {
            return Err(
                "Clear queued messages and saved drafts before editing the last message".into(),
            );
        }
        Ok(())
    }
    /// `expected` is the visible transcript; never recall an unloaded revision.
    pub fn prepare_edit_last_turn(
        &self,
        id: i64,
        expected: &[Row],
        turn_id: Option<&str>,
    ) -> Result<EditSnapshot, String> {
        if let Some(reason) = self.edit_recovery_reason(id)? {
            return Err(reason);
        }

        self.shared_writable()?;
        let session = self
            .bot_session(id)?
            .ok_or("Conversation no longer exists")?;
        if !crate::kind::supports_rewind(crate::kind::Kind::from_id(&session.provider)) {
            return Err("This provider does not support editing the last turn".into());
        }
        let blocks = self.shared_blocks(id)?;
        self.shared_edit_queue_empty(id, &blocks)?;
        if blocks.iter().any(|b| b["role"] == "handoff") {
            return Err("Editing a turn across a provider handoff is unavailable".into());
        }
        let rows = blocks::project(&blocks)?;
        if rows != expected {
            return Err("Conversation changed before the edit could be prepared".into());
        }
        let latest = blocks
            .iter()
            .rposition(|b| b["role"] == "user" && b["internal"] != true && b["draft"] != true)
            .ok_or("No user message to edit")?;
        let block = &blocks[latest];
        if ["secondOpinion", "noteCard", "ciContext"]
            .iter()
            .any(|field| !block[*field].is_null())
        {
            return Err("This contextual user message cannot be edited".into());
        }
        let provider_turn_id = block["providerTurnId"]
            .as_str()
            .or(turn_id)
            .filter(|id| !id.trim().is_empty())
            .map(str::to_owned);
        let settings = self.agent_settings(id)?.unwrap_or_default();
        let mut first = latest;
        if session.provider == "codex" {
            let exact = provider_turn_id
                .as_deref()
                .ok_or("Codex did not expose an exact user turn to edit")?;
            for index in (0..latest).rev() {
                let prior = &blocks[index];
                if prior["role"] == "user" {
                    if prior["providerTurnId"].as_str() != Some(exact) {
                        break;
                    }
                    if prior["internal"] != true && prior["draft"] != true {
                        first = index;
                    }
                }
            }
            if block["providerTurnId"].is_null() {
                if let Some(row_start) =
                    super::super::edit::codex_turn_start(&rows, &settings, exact)
                {
                    let mut offset = 0;
                    for (index, candidate) in blocks.iter().enumerate().take(latest + 1) {
                        if offset == row_start
                            && candidate["role"] == "user"
                            && candidate["internal"] != true
                        {
                            first = index;
                            break;
                        }
                        offset += blocks::project(std::slice::from_ref(candidate))?.len();
                    }
                } else if blocks
                    .iter()
                    .filter(|block| {
                        block["role"] == "user"
                            && block["internal"] != true
                            && block["draft"] != true
                    })
                    .count()
                    > 1
                {
                    return Err(
                        "Codex user-turn boundaries are unavailable; history was preserved".into(),
                    );
                }
            }
        }
        let start = blocks::project(&blocks[..first])?.len();
        let mut prompt = block["text"].as_str().unwrap_or("").to_owned();
        if crate::historical::application_block(block) {
            prompt = format!("/operator {prompt}");
        }
        let attachments = block["attachments"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|a| {
                a["path"]
                    .as_str()
                    .map(PathBuf::from)
                    .ok_or("An original attachment cannot be restored safely".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        if attachments.is_empty() && super::super::edit::untracked_attachment_tail(&prompt) {
            return Err("This message has attachment links without a saved attachment manifest and cannot be edited safely".into());
        }
        if prompt.trim().is_empty() && attachments.is_empty() {
            return Err("No editable prompt text or attachments".into());
        }
        Ok(EditSnapshot {
            start,
            prompt,
            attachments,
            provider_turn_id,
            provider_id: session.provider_id.ok_or("No provider session to edit")?,
            account_id: settings.account.id,
            cwd: settings.worktree_cwd.unwrap_or(session.cwd),
            rows,
            blocks,
            block_start: first,
            provider: session.provider,
        })
    }
    /// Call only after the provider confirms. This is a compare-and-swap,
    /// preserving an independently changed conversation instead of deleting it.
    pub fn finish_edit_last_turn(
        &self,
        claim: &mut EditClaim,
        provider_id: &str,
    ) -> Result<(), String> {
        let id = claim.session;
        let edit = &claim.snapshot;
        let tx = self.edit_transaction()?;
        self.validate_edit_last_turn(id, edit)?;
        claim.confirm(&tx)?;
        let next = &edit.blocks[..edit.block_start];
        self.shared_store_blocks(id, next)?;
        self.shared_patch(
            id,
            &[
                ("provider_session_id", json!(provider_id)),
                ("updated_at", json!(crate::time::now_ms())),
                (
                    "has_user_message",
                    json!(next.iter().any(|b| b["role"] == "user"
                        && b["internal"] != true
                        && b["draft"] != true)),
                ),
            ],
        )?;
        if let Some(mut settings) = self.agent_settings(id)? {
            settings.turns.retain(|&row, _| row < edit.start);
            // The validated prefix already retains its original rich turn
            // receipts. Save only the trimmed sidecar/preferences in this tx.
            self.save_agent_preferences(id, &settings)?;
        }
        self.save_context_usage(id, &Default::default())?;
        sql(tx.commit())?;
        self.shared
            .snapshots
            .borrow_mut()
            .insert(id, HistoryRevision::of(next));
        claim.close();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::edit::dispatched_edit;

    fn fixture(blocks: Value) -> (Store, i64) {
        let store = Store::test(Path::new(":memory:")).unwrap();
        let session = store.create("codex", "/project").unwrap();
        store.provider_id(session.id, "thread").unwrap();
        store
            .shared_store_blocks(session.id, blocks.as_array().unwrap())
            .unwrap();
        (store, session.id)
    }
    #[test]
    fn rich_prefix_and_hidden_metadata_survive_grouped_codex_turn_replacement() {
        let blocks = json!([
            {"id":"initial","role":"user","text":"Earlier","providerTurnId":"old","future":{"keep":true},
             "startedAt":100,"durationMs":5000,"turnModel":{"harness":"codex","id":"codex:old-model","name":"Earlier Model"}},
            {"id":"answer","role":"assistant","text":"Kept","citations":["source"]},
            {"id":"hidden","role":"system","internal":true,"text":"Prior metadata","future":7},
            {"id":"original","role":"user","text":"Original turn","providerTurnId":"turn"},
            {"id":"work","role":"assistant","text":"Working"},
            {"id":"steer","role":"user","text":"Edited follow-up","providerTurnId":"turn","attachments":[{"id":"image","path":"/tmp/image.png","future":true}]},
            {"id":"replaced","role":"assistant","text":"Old answer"}
        ]);
        let (store, id) = fixture(blocks.clone());
        let rows = store.load(id).unwrap().rows;
        let edit = store.prepare_edit_last_turn(id, &rows, None).unwrap();
        assert_eq!(edit.start, 2);
        assert_eq!(edit.prompt, "Edited follow-up");
        assert_eq!(edit.attachments, vec![PathBuf::from("/tmp/image.png")]);
        assert_eq!(
            store.shared_blocks(id).unwrap(),
            blocks.as_array().unwrap().to_vec()
        );
        let mut claim = dispatched_edit(&store, id, edit);
        store.finish_edit_last_turn(&mut claim, "thread").unwrap();
        assert_eq!(
            store.shared_blocks(id).unwrap(),
            blocks.as_array().unwrap()[..3]
        );
        assert_eq!(store.load(id).unwrap().rows, rows[..2]);
        let retained = store.agent_settings(id).unwrap().unwrap();
        assert_eq!(retained.turns.len(), 1);
        assert_eq!(retained.turns[&1].model, "Earlier Model");
        assert_eq!(retained.turns[&1].duration_ms, 5000);
    }
    #[test]
    fn hidden_metadata_changes_during_provider_rewind_preserve_every_source_block() {
        let (store, id) = fixture(json!([
            {"id":"user","role":"user","text":"Prompt","providerTurnId":"turn"},
            {"id":"answer","role":"assistant","text":"Answer","future":"first"}
        ]));
        let rows = store.load(id).unwrap().rows;
        let edit = store.prepare_edit_last_turn(id, &rows, None).unwrap();
        let mut claim = dispatched_edit(&store, id, edit);
        let mut newer = store.shared_blocks(id).unwrap();
        newer[1]["future"] = json!("another window");
        store.shared_store_blocks(id, &newer).unwrap();
        assert!(store.finish_edit_last_turn(&mut claim, "fork").is_err());
        assert_eq!(store.shared_blocks(id).unwrap(), newer);
        assert_eq!(
            store.sessions().unwrap()[0].provider_id.as_deref(),
            Some("thread")
        );
    }
    #[test]
    fn saved_drafts_queues_handoffs_and_missing_provider_ids_do_not_allow_rewind() {
        for blocks in [
            json!([{"role":"user","id":"u","text":"Prompt"}]),
            json!([{"role":"handoff","id":"h","text":"Handoff"},{"role":"user","id":"u","text":"Prompt","providerTurnId":"turn"}]),
            json!([{"role":"user","id":"u","text":"Prompt","providerTurnId":"turn"},{"role":"user","id":"draft","draft":true,"text":"Saved"}]),
            json!([{"role":"user","id":"u","text":"Prompt","providerTurnId":"turn","noteCard":{"title":"Context"}}]),
        ] {
            let (store, id) = fixture(blocks.clone());
            let rows = store.load(id).unwrap().rows;
            assert!(store.prepare_edit_last_turn(id, &rows, None).is_err());
            assert_eq!(
                store.shared_blocks(id).unwrap(),
                blocks.as_array().unwrap().to_vec()
            );
        }
    }
    #[test]
    fn a_queue_added_after_preparation_is_never_removed_by_confirmed_rewind() {
        let (store, id) =
            fixture(json!([{"role":"user","id":"u","text":"Prompt","providerTurnId":"turn"}]));
        let rows = store.load(id).unwrap().rows;
        let edit = store.prepare_edit_last_turn(id, &rows, None).unwrap();
        let mut claim = dispatched_edit(&store, id, edit);
        let queue = crate::follow_up::State {
            revision: 1,
            messages: vec![crate::follow_up::Message {
                id: "queued".into(),
                text: "New queued prompt".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        store.save_follow_ups(id, &queue).unwrap();
        assert!(store.finish_edit_last_turn(&mut claim, "fork").is_err());
        assert_eq!(store.load(id).unwrap().rows, rows);
        assert_eq!(
            store.follow_ups(id).unwrap().messages[0].text,
            "New queued prompt"
        );
    }
}
