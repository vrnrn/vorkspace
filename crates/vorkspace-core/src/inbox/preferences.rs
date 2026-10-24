//! Durable Inbox choices that also read the desktop preferences in place.
use crate::store::Store;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct JiraFilter {
    pub mode: String,
    pub jql: String,
}
impl Default for JiraFilter {
    fn default() -> Self {
        Self {
            mode: "project".into(),
            jql: String::new(),
        }
    }
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BotDefaults(BTreeMap<String, String>);
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
        let state = super::super::ViewState::load(&store);
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
        let state = super::super::ViewState::load(&store);
        assert_eq!(state.jira_filter.mode, "project");
        assert_eq!(state.jira_filter.jql, "filter = 12345");
        assert_eq!(state.jira_filter.custom_jql(), None);
        assert_eq!(state.default_bots.get("jira"), None);
        assert_eq!(state.default_bots.get("gitlab"), Some("gitlab-bot"));
    }
}
