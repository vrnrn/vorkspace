//! A local provider and its saved thread binding change in one transaction.
use super::Store;
use crate::{
    agent::{Handoff, ProviderSwitch, Settings},
    provider::Kind,
};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior};

impl Store {
    pub fn configure_agent(
        &self,
        id: i64,
        provider: &str,
        settings: &Settings,
    ) -> Result<(), String> {
        if let Some(remote) = self.remote_session(id)? {
            if !self.agent_configuration_is_newer(id, settings)? {
                self.change_remote_provider(&remote, provider)?;
                self.save_agent_settings(id, settings)?;
            }
            return Ok(());
        }
        if self.shared.is_some() && !settings.turns.is_empty() {
            if self.agent_configuration_is_newer(id, settings)? {
                return Ok(());
            }
            self.shared_save_turns(id, &settings.turns, None)?;
        }
        let tx = self.agent_configuration_transaction()?;
        if !self.agent_configuration_is_newer(id, settings)? {
            self.change_provider(id, provider)?;
            self.save_agent_preferences(id, settings)?;
        }
        tx.commit().map_err(|error| error.to_string())
    }

    /// Select the next provider without losing the admitted provider's thread.
    /// Consult the saved binding inside the transaction so an earlier view
    /// cannot discard a thread ID the old worker has just learned.
    pub fn select_local_provider(
        &self,
        id: i64,
        expected_provider: &str,
        provider: Kind,
        mut next: Settings,
    ) -> Result<(Settings, Option<String>), String> {
        if id < 0 || self.remote_session(id)?.is_some() {
            return Err("Remote provider choices belong to the Host".into());
        }
        let tx = self.agent_configuration_transaction()?;
        let (current_provider, current_binding) = self.local_agent_binding(id)?;
        let saved = self.agent_preferences(id)?.unwrap_or_default();
        if current_provider != expected_provider
            || current_provider == provider.id()
            || saved.revision >= next.revision
        {
            return Err("The saved provider changed. Reopen the conversation to retry.".into());
        }
        let pending = saved.pending_provider_switch;
        let binding = match pending {
            Some(pending) if pending.from == provider.id() => {
                next.account = pending.account;
                next.pending_provider_switch = None;
                next.handoff = None;
                pending.provider_id
            }
            Some(pending) => {
                next.pending_provider_switch = Some(pending);
                None
            }
            None => {
                if let Some(pending) = next.pending_provider_switch.as_mut() {
                    pending.provider_id = current_binding;
                }
                None
            }
        };
        self.change_provider(id, provider.id())?;
        if let Some(binding) = &binding {
            self.provider_id(id, binding)?;
        }
        self.save_agent_preferences(id, &next)?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok((next, binding))
    }

    pub fn prepare_local_provider_handoff(
        &self,
        id: i64,
        provider: &str,
        account: &str,
        source: &ProviderSwitch,
        prepared: &Handoff,
    ) -> Result<(), String> {
        let tx = self.agent_configuration_transaction()?;
        let mut settings = self.agent_preferences(id)?.unwrap_or_default();
        if self.local_agent_binding(id)?.0 != provider
            || settings.account.id != account
            || settings
                .pending_provider_switch
                .as_ref()
                .is_none_or(|current| {
                    current.from != source.from
                        || current.cwd != source.cwd
                        || current.account.id != source.account.id
                        || current.provider_id != source.provider_id
                })
        {
            return Err("The conversation's provider changed while preparing the handoff".into());
        }
        self.reset_provider_id(id)?;
        settings.pending_provider_switch = None;
        settings.handoff = Some(prepared.clone());
        self.save_agent_preferences(id, &settings)?;
        tx.commit().map_err(|error| error.to_string())
    }

    /// A completed or interrupted turn may settle only the packet it prepared.
    pub fn settle_local_provider_handoff(
        &self,
        id: i64,
        provider: &str,
        account: &str,
        prepared: &Handoff,
        retained: Option<&Handoff>,
    ) -> Result<bool, String> {
        let tx = self.agent_configuration_transaction()?;
        let mut settings = self.agent_preferences(id)?.unwrap_or_default();
        if self.local_agent_binding(id)?.0 != provider
            || settings.account.id != account
            || settings.pending_provider_switch.is_some()
            || settings.handoff.as_ref() != Some(prepared)
        {
            return Ok(false);
        }
        settings.handoff = retained.cloned();
        self.save_agent_preferences(id, &settings)?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok(true)
    }

    fn agent_configuration_transaction(&self) -> Result<Transaction<'_>, String> {
        Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())
    }

    fn agent_configuration_is_newer(&self, id: i64, settings: &Settings) -> Result<bool, String> {
        Ok(self
            .agent_preferences(id)?
            .is_some_and(|saved| saved.revision > settings.revision))
    }

    fn local_agent_binding(&self, id: i64) -> Result<(String, Option<String>), String> {
        let binding = if self.shared.is_some() {
            self.conn.query_row(
                "SELECT harness,provider_session_id FROM sessions WHERE id=?1",
                [self.persisted_session_id(id)?],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
        } else {
            self.conn.query_row(
                "SELECT provider,provider_id FROM native_sessions WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
        };
        binding
            .optional()
            .map_err(|error| error.to_string())?
            .ok_or("Conversation no longer exists".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection(from: Kind, revision: u64) -> Settings {
        Settings {
            revision,
            model: Some("chosen-model".into()),
            pending_provider_switch: Some(ProviderSwitch {
                from: from.id().into(),
                cwd: "/project".into(),
                provider_id: Some("stale-view-thread".into()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn selection_commits_model_and_exact_outgoing_binding_or_rolls_back_both() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.db");
        let store = Store::open_vorkspace(&path).unwrap();
        let id = store.create("codex", "/project").unwrap().id;
        store.provider_id(id, "latest-outgoing-thread").unwrap();
        let original = Settings {
            revision: 1,
            worktree_base: Some("main".into()),
            ..Default::default()
        };
        store
            .save_setting(
                &format!("agent:{id}"),
                &serde_json::to_string(&original).unwrap(),
            )
            .unwrap();
        assert_eq!(
            store
                .agent_preferences(id)
                .unwrap()
                .unwrap()
                .worktree_base
                .as_deref(),
            Some("main")
        );
        store.shared_history_schema().unwrap();
        store.conn.execute_batch("CREATE TRIGGER reject_configuration BEFORE INSERT ON native_agent_settings BEGIN SELECT RAISE(ABORT,'forced failure'); END;").unwrap();

        assert!(store
            .select_local_provider(id, "codex", Kind::Claude, selection(Kind::Codex, 2))
            .is_err());
        let peer = Store::read_vorkspace(&path).unwrap();
        assert_eq!(
            peer.local_agent_binding(id).unwrap(),
            ("codex".into(), Some("latest-outgoing-thread".into()))
        );
        assert_eq!(peer.agent_preferences(id).unwrap().unwrap().revision, 1);
        store
            .conn
            .execute_batch("DROP TRIGGER reject_configuration")
            .unwrap();

        let (selected, binding) = store
            .select_local_provider(id, "codex", Kind::Claude, selection(Kind::Codex, 2))
            .unwrap();
        assert!(binding.is_none());
        assert_eq!(
            selected
                .pending_provider_switch
                .as_ref()
                .unwrap()
                .provider_id
                .as_deref(),
            Some("latest-outgoing-thread")
        );
        store.configure_agent(id, "codex", &original).unwrap();
        assert_eq!(
            peer.local_agent_binding(id).unwrap(),
            ("claude".into(), None)
        );
        let saved = peer.agent_preferences(id).unwrap().unwrap();
        assert_eq!(saved.revision, 2);
        assert_eq!(saved.model.as_deref(), Some("chosen-model"));
        assert_eq!(saved.pending_provider_switch.unwrap().from, "codex");

        let (reverted, binding) = store
            .select_local_provider(id, "claude", Kind::Codex, selection(Kind::Claude, 3))
            .unwrap();
        assert_eq!(binding.as_deref(), Some("latest-outgoing-thread"));
        assert!(reverted.pending_provider_switch.is_none());
        assert_eq!(
            peer.local_agent_binding(id).unwrap(),
            ("codex".into(), binding)
        );
    }

    #[test]
    fn delayed_handoff_settlement_cannot_clear_a_new_provider_selection() {
        let store = Store::open_vorkspace(std::path::Path::new(":memory:")).unwrap();
        let id = store.create("codex", "/project").unwrap().id;
        let (selected, _) = store
            .select_local_provider(id, "codex", Kind::Claude, selection(Kind::Codex, 1))
            .unwrap();
        let source = selected.pending_provider_switch.unwrap();
        let ready = Handoff {
            from: "codex".into(),
            brief: "A prepared recap".into(),
        };
        store
            .prepare_local_provider_handoff(id, "claude", &selected.account.id, &source, &ready)
            .unwrap();
        assert_eq!(
            store
                .agent_preferences(id)
                .unwrap()
                .unwrap()
                .handoff
                .as_ref(),
            Some(&ready)
        );
        store
            .select_local_provider(id, "claude", Kind::Pi, selection(Kind::Claude, 2))
            .unwrap();
        assert!(!store
            .settle_local_provider_handoff(id, "claude", &selected.account.id, &ready, None)
            .unwrap());
        let successor = store.agent_preferences(id).unwrap().unwrap();
        assert_eq!(successor.revision, 2);
        assert_eq!(successor.pending_provider_switch.unwrap().from, "claude");
    }
}
