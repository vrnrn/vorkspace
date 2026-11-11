//! A local provider and its saved thread binding change in one transaction.
use super::{LocalAgentBinding, Store};
use crate::{
    agent::{ConfigurationPatch, Handoff, ProviderSwitch, Settings},
    provider::Kind,
};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior};

/// Identity of the configuration admitted by a local turn.
pub struct ProviderSource<'a> {
    pub session: i64,
    pub provider: &'a str,
    pub account: &'a str,
    pub provider_id: Option<&'a str>,
    pub revision: u64,
}

impl Store {
    /// Bind a provider's newly observed thread only to its admitted source.
    /// The read, ownership check and write share the SQLite writer transaction.
    pub fn record_provider_session(
        &self,
        source: ProviderSource<'_>,
        next_id: &str,
        reset_context: bool,
    ) -> Result<bool, String> {
        let tx = self.agent_configuration_transaction()?;
        let Some(binding) = self.local_agent_binding(source.session)? else {
            return Ok(false);
        };
        let mut settings = self.agent_preferences(source.session)?.unwrap_or_default();
        if let Some(pending) = settings.pending_provider_switch.as_mut().filter(|pending| {
            pending.from == source.provider
                && pending.account.id == source.account
                && pending.provider_id.as_deref() == source.provider_id
        }) {
            pending.provider_id = Some(next_id.into());
            self.save_agent_preferences(source.session, &settings)?;
        } else if binding.provider == source.provider
            && binding.provider_id.as_deref() == source.provider_id
            && settings.account.id == source.account
        {
            if reset_context {
                if !self.replace_bot_provider_session(
                    source.session,
                    source.provider,
                    source.account,
                    source.provider_id,
                    Some(next_id),
                )? {
                    return Ok(false);
                }
            } else {
                self.provider_id(source.session, next_id)?;
            }
        } else {
            return Ok(false);
        }
        tx.commit().map_err(|error| error.to_string())?;
        Ok(true)
    }

    /// Provider feedback can update its selected or outgoing binding, never a newer choice.
    pub fn record_provider_configuration(
        &self,
        source: ProviderSource<'_>,
        patch: &ConfigurationPatch,
    ) -> Result<bool, String> {
        let tx = self.agent_configuration_transaction()?;
        let Some(binding) = self.local_agent_binding(source.session)? else {
            return Ok(false);
        };
        let mut settings = self.agent_preferences(source.session)?.unwrap_or_default();
        if let Some(pending) = settings.pending_provider_switch.as_mut().filter(|pending| {
            pending.from == source.provider
                && pending.account.id == source.account
                && pending.provider_id.as_deref() == source.provider_id
        }) {
            patch.apply_to_switch(pending);
        } else if binding.provider == source.provider
            && binding.provider_id.as_deref() == source.provider_id
            && settings.account.id == source.account
            && settings.revision == source.revision
        {
            patch.apply(&mut settings);
        } else {
            return Ok(false);
        }
        self.save_agent_preferences(source.session, &settings)?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok(true)
    }

    pub fn configure_agent(
        &self,
        id: i64,
        provider: &str,
        settings: &Settings,
    ) -> Result<(), String> {
        if !settings.turns.is_empty() {
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
        let tx = self.agent_configuration_transaction()?;
        let current = self
            .local_agent_binding(id)?
            .ok_or("Conversation no longer exists")?;
        let saved = self.agent_preferences(id)?.unwrap_or_default();
        if current.provider != expected_provider
            || current.provider == provider.id()
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
                    pending.provider_id = current.provider_id;
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
        if self
            .local_agent_binding(id)?
            .is_none_or(|binding| binding.provider != provider)
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
        if self
            .local_agent_binding(id)?
            .is_none_or(|binding| binding.provider != provider)
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

    pub(super) fn agent_configuration_transaction(&self) -> Result<Transaction<'_>, String> {
        Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())
    }

    fn agent_configuration_is_newer(&self, id: i64, settings: &Settings) -> Result<bool, String> {
        Ok(self
            .agent_preferences(id)?
            .is_some_and(|saved| saved.revision > settings.revision))
    }

    pub(super) fn local_agent_binding(&self, id: i64) -> Result<Option<LocalAgentBinding>, String> {
        let read = |row: &rusqlite::Row<'_>| {
            Ok(LocalAgentBinding {
                provider: row.get(0)?,
                provider_id: row.get(1)?,
            })
        };
        let binding = self.conn.query_row(
            "SELECT harness,provider_session_id FROM sessions WHERE id=?1",
            [self.persisted_session_id(id)?],
            read,
        );
        binding.optional().map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observed_threads_preserve_replacements_and_follow_only_the_exact_outgoing_source() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("agent.db");
        let open = { Store::test };
        let store = open(&path).unwrap();
        let id = store.create("omp", "/project").unwrap().id;
        let peer = open(&path).unwrap();
        let settings = Settings {
            revision: 3,
            ..Default::default()
        };
        store.save_agent_preferences(id, &settings).unwrap();
        store.provider_id(id, "source-thread").unwrap();
        let source = |provider_id| ProviderSource {
            session: id,
            provider: "omp",
            account: crate::accounts::DEFAULT,
            provider_id,
            revision: 3,
        };
        store
            .save_context_usage(
                id,
                &crate::context::Usage {
                    used: 9000,
                    window: Some(10000),
                },
            )
            .unwrap();
        assert!(store
            .record_provider_session(source(Some("source-thread")), "observed-thread", true)
            .unwrap());
        assert_eq!(
            peer.local_agent_binding(id)
                .unwrap()
                .unwrap()
                .provider_id
                .as_deref(),
            Some("observed-thread")
        );
        assert_eq!(peer.context_usage(id).unwrap(), None);
        assert_eq!(peer.agent_preferences(id).unwrap().unwrap().revision, 3);

        peer.provider_id(id, "replacement-thread").unwrap();
        assert!(!store
            .record_provider_session(source(Some("observed-thread")), "stale-thread", true)
            .unwrap());
        assert_eq!(
            peer.local_agent_binding(id)
                .unwrap()
                .unwrap()
                .provider_id
                .as_deref(),
            Some("replacement-thread")
        );
        let mut replacement = settings.clone();
        replacement.account.id = "replacement-account".into();
        peer.save_agent_preferences(id, &replacement).unwrap();
        assert!(!store
            .record_provider_session(
                source(Some("replacement-thread")),
                "wrong-account-thread",
                false
            )
            .unwrap());
        peer.save_agent_preferences(id, &settings).unwrap();
        peer.select_local_provider(id, "omp", Kind::Claude, selection(Kind::Omp, 4))
            .unwrap();
        assert!(!store
            .record_provider_session(
                source(Some("observed-thread")),
                "stale-outgoing-thread",
                false
            )
            .unwrap());
        assert!(store
            .record_provider_session(source(Some("replacement-thread")), "outgoing-thread", false)
            .unwrap());
        let selected = peer.local_agent_binding(id).unwrap().unwrap();
        assert_eq!(selected.provider, "claude");
        assert_eq!(selected.provider_id, None);
        let saved = peer.agent_preferences(id).unwrap().unwrap();
        assert_eq!(saved.revision, 4);
        assert_eq!(
            saved
                .pending_provider_switch
                .unwrap()
                .provider_id
                .as_deref(),
            Some("outgoing-thread")
        );
    }

    #[test]
    fn provider_feedback_preserves_new_choices_and_updates_the_exact_outgoing_source() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("agent.db");
        let open = { Store::test };
        let store = open(&path).unwrap();
        let id = store.create("omp", "/project").unwrap().id;
        store.provider_id(id, "source-thread").unwrap();
        let settings = Settings {
            revision: 3,
            thinking: Some(false),
            ..Default::default()
        };
        store.save_agent_preferences(id, &settings).unwrap();
        let source = || ProviderSource {
            session: id,
            provider: "omp",
            account: crate::accounts::DEFAULT,
            provider_id: Some("source-thread"),
            revision: 3,
        };
        let patch = ConfigurationPatch {
            model: Some("fixture/observed".into()),
            model_settings: [
                ("fast".into(), "true".into()),
                ("thinking".into(), "high".into()),
                ("reasoningEffort".into(), "medium".into()),
                ("serviceTier".into(), "fast".into()),
            ]
            .into(),
        };
        assert!(store
            .record_provider_configuration(source(), &patch)
            .unwrap());
        let peer = open(&path).unwrap();
        let observed = peer.agent_preferences(id).unwrap().unwrap();
        assert_eq!(observed.revision, 3);
        assert_eq!(observed.model.as_deref(), Some("fixture/observed"));
        assert!(observed.fast);
        assert_eq!(observed.model_settings["fast"], "true");
        assert_eq!(observed.model_settings["thinking"], "high");
        assert_eq!(observed.effort.as_deref(), Some("medium"));
        assert_eq!(observed.service_tier.as_deref(), Some("fast"));
        assert!(observed.thinking.is_none());

        let mut newer = observed;
        newer.revision = 4;
        newer.model = Some("fixture/deliberate".into());
        peer.save_agent_preferences(id, &newer).unwrap();
        assert!(!store
            .record_provider_configuration(source(), &patch)
            .unwrap());
        assert_eq!(
            peer.agent_preferences(id).unwrap().unwrap().model,
            newer.model
        );
        let changed_account = ProviderSource {
            account: "replacement",
            revision: 4,
            ..source()
        };
        assert!(!store
            .record_provider_configuration(changed_account, &patch)
            .unwrap());
        let changed_thread = ProviderSource {
            provider_id: Some("replacement-thread"),
            revision: 4,
            ..source()
        };
        assert!(!store
            .record_provider_configuration(changed_thread, &patch)
            .unwrap());

        let next = Settings {
            revision: 5,
            model: Some("next-model".into()),
            pending_provider_switch: Some(ProviderSwitch {
                from: "omp".into(),
                account: newer.account.clone(),
                ..Default::default()
            }),
            ..Default::default()
        };
        peer.select_local_provider(id, "omp", Kind::Claude, next)
            .unwrap();
        assert!(store
            .record_provider_configuration(source(), &patch)
            .unwrap());
        let selected = peer.agent_preferences(id).unwrap().unwrap();
        assert_eq!(selected.model.as_deref(), Some("next-model"));
        assert!(!selected.fast);
        assert_eq!(selected.revision, 5);
        let pending = selected.pending_provider_switch.unwrap();
        assert_eq!(pending.provider_id.as_deref(), Some("source-thread"));
        assert_eq!(pending.model.as_deref(), Some("fixture/observed"));
        assert!(pending.fast);
        assert_eq!(pending.model_settings["thinking"], "high");
    }

    #[test]
    fn a_failed_provider_observation_rolls_back_scalar_and_native_preferences() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("agent.db");
        let store = Store::open_vorkspace(&path).unwrap();
        let id = store.create("omp", "/project").unwrap().id;
        let settings = Settings {
            revision: 3,
            ..Default::default()
        };
        store.save_agent_preferences(id, &settings).unwrap();
        store.conn.execute_batch("CREATE TRIGGER fail_observation BEFORE INSERT ON native_agent_settings BEGIN SELECT RAISE(ABORT, 'fixture failure'); END;").unwrap();
        let patch = ConfigurationPatch {
            model: Some("fixture/rejected".into()),
            model_settings: [("fast".into(), "true".into())].into(),
        };
        assert!(store
            .record_provider_configuration(
                ProviderSource {
                    session: id,
                    provider: "omp",
                    account: crate::accounts::DEFAULT,
                    provider_id: None,
                    revision: 3,
                },
                &patch
            )
            .is_err());
        let peer = Store::open_vorkspace(&path).unwrap();
        let saved = peer.agent_preferences(id).unwrap().unwrap();
        assert_eq!(saved.model, settings.model);
        assert_eq!(saved.fast, settings.fast);
        assert_eq!(saved.revision, settings.revision);
    }

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
            Some(LocalAgentBinding {
                provider: "codex".into(),
                provider_id: Some("latest-outgoing-thread".into()),
            })
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
            Some(LocalAgentBinding {
                provider: "claude".into(),
                provider_id: None
            })
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
            Some(LocalAgentBinding {
                provider: "codex".into(),
                provider_id: binding
            })
        );
    }

    #[test]
    fn delayed_handoff_settlement_cannot_clear_a_new_provider_selection() {
        let store = Store::test(std::path::Path::new(":memory:")).unwrap();
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
