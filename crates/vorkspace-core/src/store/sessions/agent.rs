//! Transactional local agent preferences. Session columns keep their stored format.
use super::*;

pub(super) fn ensure_schema(conn: &Connection) -> Result<(), String> {
    sql(conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS native_agent_settings(
            session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
            state_json TEXT NOT NULL);",
    ))
}

impl Store {
    pub fn agent_settings(&self, id: i64) -> Result<Option<Settings>, String> {
        let Some(mut settings) = self.agent_preferences(id)? else {
            return Ok(None);
        };
        // Desktop blocks are authoritative. Native-only legacy metadata remains
        // available, but cannot override a saved desktop turn's identity/time.
        for (turn, source) in self.shared_project_turns(id)? {
            merge_turn_info(settings.turns.entry(turn).or_default(), source);
        }
        Ok(Some(settings))
    }
    /// Scalar preferences and native settings; historical receipts are added
    /// only for full agent reads, never while saving composer preferences.
    /// Read preferences without projecting saved turn metadata from the transcript.
    pub fn agent_preferences(&self, id: i64) -> Result<Option<Settings>, String> {
        self.shared_agent_configuration(id)
            .map(|configuration| configuration.map(|(_, settings)| settings))
    }

    pub(in crate::store) fn shared_agent_configuration(
        &self,
        id: i64,
    ) -> Result<Option<(LocalAgentBinding, Settings)>, String> {
        let tx = self
            .conn
            .is_autocommit()
            .then(|| sql(self.conn.unchecked_transaction()))
            .transpose()?;
        let native = if has_table(&self.conn, "native_agent_settings")? {
            "(SELECT state_json FROM native_agent_settings WHERE session_id=sessions.id)"
        } else {
            "NULL"
        };
        let fields = ["harness", "model", "model_settings", "runtime_mode"]
            .into_iter()
            .map(str::to_owned)
            .chain(
                ["worktree_cwd", "worktree_removed", "provider_account_id"]
                    .into_iter()
                    .map(|name| self.shared_field(name, "NULL")),
            )
            .chain(["provider_session_id".to_owned(), native.to_owned()])
            .collect::<Vec<_>>()
            .join(",");
        let raw = sql(self
            .conn
            .query_row(
                &format!("SELECT {fields} FROM sessions WHERE id=?1"),
                [self.persisted_session_id(id)?],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, Option<bool>>(5)?,
                        r.get::<_, Option<String>>(6)?,
                        r.get::<_, Option<String>>(7)?,
                        r.get::<_, Option<String>>(8)?,
                    ))
                },
            )
            .optional())?;
        let Some((provider, model, controls, access, cwd, removed, account, provider_id, native)) =
            raw
        else {
            return Ok(None);
        };
        let controls: HashMap<String, String> = parse(&controls)?;
        let native = match native {
            Some(raw) => Some(raw),
            None => self.setting(&format!("agent:{id}"))?,
        };
        let mut settings: Settings = native
            .map(|raw| parse(&raw))
            .transpose()?
            .unwrap_or_default();
        settings.model = crate::session::native_model(&model, &provider);
        settings.access = crate::agent::AccessMode::from_id(&access);
        settings.effort = [
            "effort",
            "reasoning",
            "reasoningEffort",
            "reasoning_effort",
            "variant",
        ]
        .into_iter()
        .find_map(|key| controls.get(key).cloned());
        settings.fast = controls.get("fast").is_some_and(|v| v == "true");
        settings.service_tier = controls.get("serviceTier").cloned();
        settings.thinking = controls.get("thinking").and_then(|s| s.parse().ok());
        settings.model_settings = controls;
        settings.worktree_cwd = cwd;
        settings.worktree_removed = removed.unwrap_or(false);
        settings.account.id = account.unwrap_or_else(|| crate::accounts::DEFAULT.into());
        if let Some(tx) = tx {
            sql(tx.commit())?;
        }
        Ok(Some((
            LocalAgentBinding {
                provider,
                provider_id,
            },
            settings,
        )))
    }
    pub fn save_agent_settings(&self, id: i64, settings: &Settings) -> Result<(), String> {
        self.shared_save_turns(id, &settings.turns, None)?;
        self.save_agent_preferences(id, settings)
    }
    /// Preference-only updates for validated operations which already preserve
    /// their source transcript inside a surrounding transaction (rewind and
    /// worktree cleanup). These must not re-save or re-index turn receipts.
    /// Save an edit to `agent_preferences` without rewriting transcript receipts.
    pub fn save_agent_preferences(&self, id: i64, settings: &Settings) -> Result<(), String> {
        self.shared_writable()?;
        let tx = self
            .conn
            .is_autocommit()
            .then(|| {
                sql(rusqlite::Transaction::new_unchecked(
                    &self.conn,
                    rusqlite::TransactionBehavior::Immediate,
                ))
            })
            .transpose()?;
        let current = self.agent_preferences(id)?.unwrap_or_default();
        let mut controls = current.model_settings;
        controls.extend(settings.model_settings.clone());
        let effort_key = [
            "effort",
            "reasoning",
            "reasoningEffort",
            "reasoning_effort",
            "variant",
        ]
        .into_iter()
        .find(|key| controls.contains_key(*key))
        .unwrap_or("effort");
        if let Some(effort) = &settings.effort {
            controls.insert(effort_key.into(), effort.clone());
        } else {
            controls.remove(effort_key);
        }
        controls.insert("fast".into(), settings.fast.to_string());
        if let Some(tier) = &settings.service_tier {
            controls.insert("serviceTier".into(), tier.clone());
        }
        if let Some(thinking) = settings.thinking {
            controls.insert("thinking".into(), thinking.to_string());
        }
        let mut fields = vec![
            ("model_settings", json!(encode(&controls)?)),
            ("runtime_mode", json!(settings.access.id())),
            ("worktree_cwd", json!(settings.worktree_cwd)),
            ("worktree_removed", json!(settings.worktree_removed)),
            ("provider_account_id", json!(settings.account.id)),
        ];
        let (provider, saved_model): (String, String) = sql(self.conn.query_row(
            "SELECT harness,model FROM sessions WHERE id=?1",
            [self.persisted_session_id(id)?],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ))?;
        if settings.model != crate::session::native_model(&saved_model, &provider) {
            let model = settings.model.as_deref().unwrap_or("default");
            let model = if provider == "claude" {
                model.strip_prefix("claude-").unwrap_or(model)
            } else {
                model
            };
            fields.push(("model", json!(format!("{provider}:{model}"))));
        }
        self.shared_patch(id, &fields)?;
        #[cfg(test)]
        tests::pause_preference_save();
        ensure_schema(&self.conn)?;
        sql(self.conn.execute(
            "INSERT INTO native_agent_settings(session_id,state_json) VALUES(?1,?2)
             ON CONFLICT(session_id) DO UPDATE SET state_json=excluded.state_json",
            params![self.persisted_session_id(id)?, encode(settings)?],
        ))?;
        if let Some(tx) = tx {
            sql(tx.commit())?;
        }
        Ok(())
    }
}
