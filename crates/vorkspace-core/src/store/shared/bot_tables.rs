//! Select one existing transcript owner for each Bot. Earlier tables remain the
//! write authority when their current-name counterpart is an empty placeholder.
use super::*;
#[derive(Clone, Debug)]
pub(in crate::store) struct BotTables {
    pub transcripts: String,
    pub blocks: String,
    pub turns: String,
}
impl BotTables {
    fn current() -> Self {
        Self {
            transcripts: "bot_transcripts".into(),
            blocks: "bot_blocks".into(),
            turns: "bot_turns".into(),
        }
    }
    pub(super) fn discover(conn: &Connection) -> Result<Option<Self>, String> {
        let names = sql(
            sql(conn.prepare("SELECT name FROM sqlite_master WHERE type='table'"))?
                .query_map([], |r| r.get::<_, String>(0)),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
        let find = |identity: &str| {
            names
                .iter()
                .find(|name| {
                    format!("{:x}", Sha256::digest(name.as_bytes())) == identity
                        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                })
                .cloned()
        };
        let found = (
            find("051a98fb8bd88e2e2123e04e90867a12950688f43671bc1c13797c640ca8d912"),
            find("6638365007fdf1dd4c71b4383c4787b859d7758d21152182d6391cbde709fb93"),
            find("6ddfe9c708f2d3020f7fc08cd235b6657ff8c5765b89f2183cca8f51f7dd7d46"),
        );
        let (transcripts,blocks,turns)=match found {
            (None,None,None) => return Ok(None),
            (Some(roots),Some(blocks),Some(turns)) => (roots,blocks,turns),
            _ => return Err("Earlier Bot transcript storage is incomplete. Its data was preserved; restore the missing tables before opening this database.".into()),
        };
        Ok(Some(Self {
            transcripts,
            blocks,
            turns,
        }))
    }
}
impl Store {
    pub(super) fn current_bot_owner_predicate(&self, session: &str) -> Result<String, String> {
        if !has_table(&self.conn, "bot_transcripts")? {
            return Ok("0".into());
        }
        let root = format!("EXISTS(SELECT 1 FROM bot_transcripts WHERE session_id={session})");
        if self.shared.as_ref().unwrap().earlier_bot_tables.is_none() {
            return Ok(root);
        }
        let finalized = has_table(&self.conn, "schema_migrations")?
            && sql(self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version=22)",
                [],
                |r| r.get::<_, bool>(0),
            ))?;
        if finalized {
            return Ok(root);
        }
        Ok(format!(
            "{root} AND (EXISTS(SELECT 1 FROM bot_blocks WHERE session_id={session}) OR EXISTS(SELECT 1 FROM bot_turns WHERE session_id={session}))"
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
    fn bot_owner_exists(&self, tables: &BotTables, original: &str) -> Result<bool, String> {
        sql(self.conn.query_row(
            &format!(
                "SELECT EXISTS(SELECT 1 FROM {} WHERE session_id=?1)",
                tables.transcripts
            ),
            [original],
            |r| r.get(0),
        ))
    }
    pub(in crate::store) fn shared_bot_tables(
        &self,
        original: &str,
    ) -> Result<Option<BotTables>, String> {
        if self.current_bot_owner(original)? {
            return Ok(Some(BotTables::current()));
        }
        if let Some(earlier) = &self.shared.as_ref().unwrap().earlier_bot_tables {
            if self.bot_owner_exists(earlier, original)? {
                return Ok(Some(earlier.clone()));
            }
        }
        if has_table(&self.conn, "bot_transcripts")?
            && self.bot_owner_exists(&BotTables::current(), original)?
        {
            return Ok(Some(BotTables::current()));
        }
        Ok(None)
    }
    pub(in crate::store) fn shared_earlier_bot_field(&self, current: &str) -> Option<String> {
        let tables = self.shared.as_ref()?.earlier_bot_tables.as_ref()?;
        let prefix = tables.blocks.strip_suffix("_blocks")?;
        Some(format!("{prefix}{}", current.strip_prefix("bot")?))
    }
}

#[cfg(test)]
mod tests;
