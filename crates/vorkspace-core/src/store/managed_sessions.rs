//! Desktop metadata for automation-created sessions.
use super::Store;
use crate::{files::path_key, session_groups::WorkItem};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior};
use serde_json::Value;
use std::collections::HashMap;

impl Store {
    /// A prepared run may link only the exact local checkout it inspected.
    pub fn link_automation_session(
        &self,
        id: i64,
        automation: &str,
        provider: &str,
        project: &str,
        worktree: Option<&str>,
        item: Option<&WorkItem>,
    ) -> Result<bool, String> {
        if id < 0 || project.starts_with("remote://") {
            return Ok(false);
        }
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let read = |row: &rusqlite::Row<'_>| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
                row.get::<_, bool>(3)?,
            ))
        };
        let target = if self.shared.is_some() {
            self.conn.query_row(
                &format!(
                    "SELECT cwd,harness,{},{} FROM sessions WHERE id=?1",
                    self.shared_field("archived", "0"),
                    self.shared_field("is_draft", "0"),
                ),
                [self.persisted_session_id(id)?],
                read,
            )
        } else {
            self.conn.query_row(
                "SELECT cwd,provider,archived,0 FROM native_sessions WHERE id=?1",
                [id],
                read,
            )
        };
        let Some((cwd, harness, archived, draft)) =
            target.optional().map_err(|error| error.to_string())?
        else {
            return Ok(false);
        };
        if archived || draft || harness != provider || path_key(&cwd) != path_key(project) {
            return Ok(false);
        }
        let saved = self.agent_preferences(id)?.unwrap_or_default();
        if saved.worktree_removed
            || saved.worktree_cwd.as_deref().map(path_key) != worktree.map(path_key)
        {
            return Ok(false);
        }
        if self.shared.is_some() {
            let mut fields = vec![("automation_id", Value::String(automation.into()))];
            if let Some(item) = item {
                fields.push((
                    "linked_work_item_json",
                    Value::String(super::shared::encode(item)?),
                ));
            }
            self.shared_patch(id, &fields)?;
        } else {
            self.save_setting(&format!("automation:{id}"), automation)?;
            if let Some(item) = item {
                self.set_work_item(id, Some(item))?;
            }
        }
        tx.commit().map_err(|error| error.to_string())?;
        Ok(true)
    }

    pub fn automation_session_ids(&self) -> Result<HashMap<i64, String>, String> {
        if self.shared.is_none() {
            return Ok(self
                .settings_with_prefix("automation:")?
                .into_iter()
                .collect());
        }
        let field = self.shared_field("automation_id", "NULL");
        let mut query = self
            .conn
            .prepare(&format!(
                "SELECT id,{field} FROM sessions WHERE {field} IS NOT NULL"
            ))
            .map_err(|error| error.to_string())?;
        let rows = query
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| error.to_string())?;
        rows.map(|row| {
            let (id, automation) = row.map_err(|error| error.to_string())?;
            Ok((self.shared_register(&id)?, automation))
        })
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn automation_metadata_uses_the_original_desktop_session_identity() {
        let store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
        let session = store.create("codex", "/project").unwrap();
        assert!(store
            .link_automation_session(
                session.id,
                "automation-one",
                "codex",
                "/project",
                None,
                None
            )
            .unwrap());
        assert!(!store
            .link_automation_session(
                session.id,
                "stale-run",
                "codex",
                "/project",
                Some("/different-checkout"),
                None,
            )
            .unwrap());
        assert_eq!(
            store
                .automation_session_ids()
                .unwrap()
                .get(&session.id)
                .map(String::as_str),
            Some("automation-one")
        );
        let persisted = store.persisted_session_id(session.id).unwrap();
        let raw: String = store
            .conn
            .query_row(
                "SELECT automation_id FROM sessions WHERE id=?1",
                [persisted],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(raw, "automation-one");
    }
}
