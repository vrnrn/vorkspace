//! Durable Inbox choices that also read the desktop preferences in place.
use crate::inbox::connectors::KEY;
use crate::inbox::{BotDefaults, Connectors, JiraFilter, Seen, PROVIDERS};
use crate::store::Store;
use serde_json::Value;

fn saved(store: &Store, native: &str, desktop: &str) -> Option<Value> {
    store
        .setting(native)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .or_else(|| {
            store
                .desktop_preferences()
                .and_then(|prefs| prefs.json(desktop))
        })
}

impl JiraFilter {
    pub fn load(store: &Store) -> Self {
        let value = saved(
            store,
            "native-jira-inbox-filter",
            "vorkspace.jiraInboxFilter",
        );
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
    pub fn custom_jql(&self) -> Option<&str> {
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
        let value = saved(store, "native-inbox-bots", "vorkspace.inboxBots.v1");
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
        if let Some(mut state) = store
            .setting("native-inbox-state")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str::<Self>(&s).ok())
        {
            state.connectors = Connectors::load(store);
            state.jira_filter = JiraFilter::load(store);
            state.default_bots = BotDefaults::load(store);
            state.normalize_source();
            return state;
        }
        let desktop = store.desktop_preferences().cloned().unwrap_or_default();
        let mut state = Self::default();
        if let Some(source) = desktop
            .get("vorkspace.inboxSource")
            .filter(|s| PROVIDERS.contains(s))
        {
            state.source = source.into();
        }
        if let Some(filters) = desktop
            .json("vorkspace.inboxFilters")
            .and_then(|v| serde_json::from_value(v).ok())
        {
            state.filters = filters;
        }
        if let Some(seen) = desktop.json("vorkspace.inboxSeen") {
            state.seen = serde_json::from_value(seen.clone()).unwrap_or_else(|_| Seen {
                seeded: true,
                items: serde_json::from_value(seen).unwrap_or_default(),
            });
        }
        state.hidden_jira_projects = desktop
            .json("vorkspace.jiraHiddenProjects")
            .and_then(|v| serde_json::from_value(v).ok())
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
        let native = store
            .setting(KEY)
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok());
        let saved = native.or_else(|| {
            store
                .desktop_preferences()
                .and_then(|preferences| preferences.json("vorkspace.inboxConnectors.v1"))
        });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop_preferences::DesktopPreferences;
    use serde_json::json;
    use std::sync::Arc;

    #[test]
    fn old_jira_queries_and_bot_defaults_survive_native_settings_and_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sessions.sqlite");
        let preferences = Arc::new(DesktopPreferences::from_values(&[
            (
                "vorkspace.jiraInboxFilter",
                json!({"mode":"jql","jql":" filter = 12345 "}),
            ),
            (
                "vorkspace.inboxBots.v1",
                json!({"jira":" jira-bot ","gitlab":"gitlab-bot","linear":"removed-bot"}),
            ),
        ]));
        let store =
            Store::open_vorkspace_with_preferences_for_test(&path, preferences.clone()).unwrap();
        store
            .save_setting("native-inbox-state", r#"{"source":"jira"}"#)
            .unwrap();
        let state = crate::inbox::ViewState::load(&store);
        assert_eq!(state.jira_filter.custom_jql(), Some("filter = 12345"));
        assert_eq!(state.default_bots.get("jira"), Some("jira-bot"));
        assert_eq!(state.default_bots.get("gitlab"), Some("gitlab-bot"));
        assert_eq!(state.default_bots.get("linear"), None);
        let mut filter = state.jira_filter;
        filter.mode = "project".into();
        filter.save(&store).unwrap();
        let mut bots = state.default_bots;
        bots.set("jira", "").unwrap();
        bots.save(&store).unwrap();
        drop(store);
        let store = Store::open_vorkspace_with_preferences_for_test(&path, preferences).unwrap();
        let state = crate::inbox::ViewState::load(&store);
        assert_eq!(state.jira_filter.mode, "project");
        assert_eq!(state.jira_filter.jql, "filter = 12345");
        assert_eq!(state.jira_filter.custom_jql(), None);
        assert_eq!(state.default_bots.get("jira"), None);
        assert_eq!(state.default_bots.get("gitlab"), Some("gitlab-bot"));
    }
}
