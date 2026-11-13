//! Bot transcripts live in bot_transcripts, bot_blocks and bot_turns. A
//! session belongs to them when it has a bot_transcripts root.
use super::*;
#[derive(Clone, Debug)]
pub(in crate::store) struct BotTables {
    pub blocks: String,
    pub turns: String,
}
impl BotTables {
    fn current() -> Self {
        Self {
            blocks: "bot_blocks".into(),
            turns: "bot_turns".into(),
        }
    }
}
impl Store {
    pub(super) fn current_bot_owner_predicate(&self, session: &str) -> Result<String, String> {
        if !has_table(&self.conn, "bot_transcripts")? {
            return Ok("0".into());
        }
        Ok(format!(
            "EXISTS(SELECT 1 FROM bot_transcripts WHERE session_id={session})"
        ))
    }
    fn current_bot_owner(&self, original: &str) -> Result<bool, String> {
        let predicate = self.current_bot_owner_predicate("?1")?;
        if predicate == "0" {
            return Ok(false);
        }
        sql(self
            .conn
            .query_row(&format!("SELECT {predicate}"), [original], |r| r.get(0)))
    }
    pub(in crate::store) fn shared_bot_tables(
        &self,
        original: &str,
    ) -> Result<Option<BotTables>, String> {
        if self.current_bot_owner(original)? {
            return Ok(Some(BotTables::current()));
        }
        Ok(None)
    }
}
