use super::*;
use crate::agent::TurnInfo;
use crate::native_history::{Binding, Turn};

impl Store {
    pub(in crate::store) fn shared_native_turn_owners(
        &self,
        id: i64,
        binding: &Binding,
        turns: &[Turn],
        settings: &Settings,
    ) -> Result<Vec<TurnInfo>, String> {
        let complete = || {
            self.agent_settings(id)
                .map(|settings| settings.unwrap_or_default().turns.into_values().collect())
        };
        let original = self.persisted_session_id(id)?;
        let (table, user) = if let Some(tables) = self.shared_bot_tables(&original)? {
            (tables.blocks, "is_user")
        } else if self.shared_ordinary_indexed(&original)? {
            ("ordinary_blocks".into(), "has_user")
        } else {
            return complete();
        };
        let ids: HashSet<_> = turns.iter().map(|turn| turn.id.as_str()).collect();
        let starts: Vec<_> = if binding.provider == "claude" {
            turns.iter().filter_map(|turn| turn.started_at).collect()
        } else {
            Vec::new()
        };
        // Read only prompt blocks that could own an incoming turn. The
        // indexed user flags exclude unrelated tool and assistant payloads.
        let mut query = sql(self.conn.prepare(&format!(
            "SELECT block_json FROM {table} WHERE session_id=?1 AND {user}=1 AND (
                json_extract(block_json,'$.providerTurnId') IN (SELECT value FROM json_each(?2))
                OR (json_type(block_json,'$.providerTurnId') IS NOT 'text' AND EXISTS(
                    SELECT 1 FROM json_each(?3) AS incoming
                    WHERE incoming.value BETWEEN json_extract(block_json,'$.startedAt')
                    AND json_extract(block_json,'$.startedAt') +
                        CASE WHEN json_type(block_json,'$.durationMs')='integer'
                             THEN MAX(0,json_extract(block_json,'$.durationMs')) ELSE 0 END
                ))
            )"
        )))?;
        let mut owners = Vec::new();
        for raw in sql(query
            .query_map(params![original, encode(&ids)?, encode(&starts)?], |row| {
                row.get::<_, String>(0)
            }))?
        {
            let mut projected = HashMap::new();
            project_turn(&parse_block(&sql(raw)?)?, 0, &mut projected)?;
            owners.extend(projected.into_values());
        }
        // Older desktop settings can carry turn metadata keyed by projected
        // row. Merge those offsets only when they could affect this batch.
        let projected: HashSet<_> = owners
            .iter()
            .filter_map(|turn| turn.provider_turn_id.as_deref())
            .collect();
        if (!settings.turns.is_empty() && owners.iter().any(|turn| turn.provider_turn_id.is_none()))
            || settings
                .turns
                .values()
                .any(|turn| match &turn.provider_turn_id {
                    Some(native) => {
                        ids.contains(native.as_str()) && !projected.contains(native.as_str())
                    }
                    None => turn
                        .started_at
                        .zip(turn.completed_at)
                        .is_some_and(|(start, end)| {
                            starts.iter().any(|at| (start..=end).contains(at))
                        }),
                })
        {
            return complete();
        }
        Ok(owners)
    }

    /// Called inside the import's immediate transaction. Rich existing blocks
    /// are retained verbatim, including invisible drafts, cards and attachments.
    pub(in crate::store) fn shared_append_native(
        &self,
        id: i64,
        binding: &Binding,
        turns: &[Turn],
    ) -> Result<Option<incremental::HistoryAppend>, String> {
        self.shared_writable()?;
        let mut blocks = Vec::new();
        let binding_key = binding.key();
        for turn in turns {
            for (index, message) in turn.messages.iter().enumerate() {
                let suffix = message
                    .receipt()
                    .unwrap_or_else(|| format!("{}-{index}", turn.id));
                let mut block = json!({"id":format!("native-{binding_key}-{suffix}"),"role":message.role,"text":message.text,"sentAt":message.at,
                    "nativeHistorySource":{"provider":binding.provider,"thread":binding.thread,"turn":turn.id}});
                if message.role == "user" {
                    block["providerTurnId"] = json!(turn.id);
                    block["turnModel"] = json!({"harness":binding.provider,"id":turn.model});
                    block["startedAt"] = json!(turn.started_at.or(message.at));
                    if let Some((end, start)) = turn.completed_at.zip(turn.started_at) {
                        block["durationMs"] = json!(end.saturating_sub(start));
                    }
                }
                blocks.push(block);
            }
        }
        let original = self.persisted_session_id(id)?;
        let append = if let Some(kind) = self.shared_history_index_kind(&original)? {
            self.shared_append_indexed_blocks(id, &original, kind, &blocks)?
        } else {
            // An older unnormalized desktop blob needs a one-time conversion.
            let mut previous = self.shared_blocks(id)?;
            previous.extend(blocks);
            self.shared_store_blocks(id, &previous)?;
            None
        };
        self.shared_patch(id, &[("updated_at", json!(crate::time::now_ms()))])?;
        Ok(append)
    }
    pub(in crate::store) fn shared_native_published(
        &self,
        id: i64,
        append: Option<incremental::HistoryAppend>,
    ) {
        let shared = self.shared.as_ref().unwrap();
        shared
            .snapshots
            .borrow_mut()
            .insert(id, HistoryRevision::Invalidated);
        self.shared_publish_indexed_append(id, append);
        shared.card_revisions.borrow_mut().remove(&id);
    }
}
