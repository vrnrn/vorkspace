use super::*;
use crate::native_history::{Binding, Turn};

impl Store {
    /// Called inside the import's immediate transaction. Rich existing blocks
    /// are retained verbatim, including invisible drafts, cards and attachments.
    pub(in crate::store) fn shared_append_native(
        &self,
        id: i64,
        binding: &Binding,
        turns: &[&Turn],
    ) -> Result<(), String> {
        self.shared_writable()?;
        self.shared_history_schema()?;
        let mut blocks = self.shared_blocks(id)?;
        for turn in turns {
            for (index, message) in turn.messages.iter().enumerate() {
                let mut block = json!({"id":format!("native-{}-{}-{index}",binding.key(),turn.receipt()),"role":message.role,"text":message.text,"sentAt":message.at,
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
        self.shared_store_blocks(id, &blocks)?;
        self.shared_build_history_index(id, &blocks)?;
        self.shared_patch(id, &[("updated_at", json!(crate::time::now_ms()))])?;
        Ok(())
    }
    pub(in crate::store) fn shared_native_published(&self, id: i64) {
        let shared = self.shared.as_ref().unwrap();
        shared
            .snapshots
            .borrow_mut()
            .insert(id, HistoryRevision::Invalidated);
        shared.incremental.borrow_mut().remove(&id);
        shared.card_revisions.borrow_mut().remove(&id);
    }
}
