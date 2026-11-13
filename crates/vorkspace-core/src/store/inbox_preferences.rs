//! Durable Inbox choices saved in the settings sidecar.
use crate::inbox::connectors::KEY;
use crate::inbox::{BotDefaults, Connectors, JiraFilter, PROVIDERS};
use crate::store::Store;
use serde_json::Value;

fn saved(store: &Store, native: &str) -> Option<Value> {
    store
        .setting(native)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str(&raw).ok())
}

impl JiraFilter {
    pub fn load(store: &Store) -> Self {
        let value = saved(store, "native-jira-inbox-filter");
        Self {
            mode: if value.as_ref().and_then(|v| v["mode"].as_str()) == Some("jql") {
                "jql"
            } else {
                "project"
            }
            .into(),
            jql: value
                .as_ref()
                .and_then(|v| v["jql"].as_str())
                .unwrap_or_default()
                .trim()
                .into(),
        }
    }
    pub(crate) fn custom_jql(&self) -> Option<&str> {
        (self.mode == "jql").then_some(self.jql.trim())
    }
    pub fn save(&self, store: &Store) -> Result<(), String> {
        if !matches!(self.mode.as_str(), "project" | "jql") {
            return Err("Invalid Jira filter mode".into());
        }
        if self.mode == "jql" && self.jql.trim().is_empty() {
            return Err("Enter a JQL query.".into());
        }
        let value = Self {
            mode: self.mode.clone(),
            jql: self.jql.trim().into(),
        };
        store.save_setting(
            "native-jira-inbox-filter",
            &serde_json::to_string(&value).map_err(|error| error.to_string())?,
        )
    }
}

impl BotDefaults {
    pub fn load(store: &Store) -> Self {
        let value = saved(store, "native-inbox-bots");
        Self(
            ["jira", "gitlab"]
                .into_iter()
                .filter_map(|source| {
                    let bot = value.as_ref()?.get(source)?.as_str()?.trim();
                    (!bot.is_empty()).then(|| (source.into(), bot.into()))
                })
                .collect(),
        )
    }
    pub fn get(&self, source: &str) -> Option<&str> {
        self.0.get(source).map(String::as_str)
    }
    pub fn set(&mut self, source: &str, bot: &str) -> Result<(), String> {
        if !matches!(source, "jira" | "gitlab") {
            return Err("This source does not support a default Bot".into());
        }
        if bot.trim().is_empty() {
            self.0.remove(source);
        } else {
            self.0.insert(source.into(), bot.trim().into());
        }
        Ok(())
    }
    pub fn save(&self, store: &Store) -> Result<(), String> {
        store.save_setting(
            "native-inbox-bots",
            &serde_json::to_string(&self.0).map_err(|error| error.to_string())?,
        )
    }
}

impl crate::inbox::ViewState {
    pub fn load(store: &Store) -> Self {
        let mut state = store
            .setting("native-inbox-state")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str::<Self>(&s).ok())
            .unwrap_or_default();
        state.connectors = Connectors::load(store);
        state.jira_filter = JiraFilter::load(store);
        state.default_bots = BotDefaults::load(store);
        state.normalize_source();
        state
    }
    pub fn save(&self, store: &Store) -> Result<(), String> {
        store.save_setting(
            "native-inbox-state",
            &serde_json::to_string(self).map_err(|e| e.to_string())?,
        )
    }
}

impl Connectors {
    pub fn load(store: &Store) -> Self {
        let saved = store
            .setting(KEY)
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok());
        let mut connectors = Self::default();
        for provider in PROVIDERS {
            if let Some(enabled) = saved
                .as_ref()
                .and_then(|value| value.get(provider))
                .and_then(Value::as_bool)
            {
                connectors.0.insert(provider.into(), enabled);
            }
        }
        connectors
    }
    pub fn save(&self, store: &Store) -> Result<(), String> {
        store.save_setting(
            KEY,
            &serde_json::to_string(&self.0).map_err(|error| error.to_string())?,
        )
    }
}
