//! Inbox visibility is independent of authentication. Disabling a connector
//! keeps credentials and read state while removing its work and UI surfaces.
use super::{Store, PROVIDERS};
use serde_json::Value;
use std::collections::BTreeMap;

const KEY: &str = "native-inbox-connectors";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connectors(BTreeMap<String, bool>);

impl Default for Connectors {
    fn default() -> Self {
        Self(
            PROVIDERS
                .into_iter()
                .map(|provider| (provider.into(), true))
                .collect(),
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

    pub fn enabled(&self, provider: &str) -> bool {
        self.0.get(provider).copied().unwrap_or(false)
    }

    pub fn enabled_providers(&self) -> impl Iterator<Item = &'static str> + '_ {
        PROVIDERS
            .into_iter()
            .filter(|provider| self.enabled(provider))
    }

    pub fn set(&mut self, provider: &str, enabled: bool) -> Result<(), String> {
        let value = self
            .0
            .get_mut(provider)
            .ok_or("Unsupported inbox provider")?;
        *value = enabled;
        Ok(())
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
    use crate::{
        desktop_preferences::DesktopPreferences,
        inbox::{Item, Service, ViewState},
    };
    use serde_json::json;
    use std::{path::Path, sync::Arc};

    #[test]
    fn desktop_switches_survive_native_view_state_and_saved_native_edits() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("data.sqlite");
        let preferences = Arc::new(DesktopPreferences::from_values(&[(
            "vorkspace.inboxConnectors.v1",
            json!({"github":false,"gitlab":true,"jira":false,"linear":true,"azuredevops":true}),
        )]));
        let store = Store::open_vorkspace_with_preferences_for_test(&database, preferences.clone())
            .unwrap();
        // A native 1.0 view-state file must not mask the older connector switches.
        store
            .save_setting(
                "native-inbox-state",
                r#"{"source":"linear","seen":{"seeded":true}}"#,
            )
            .unwrap();
        let mut state = ViewState::load(&store);
        assert_eq!(
            state.connectors.enabled_providers().collect::<Vec<_>>(),
            ["gitlab"]
        );
        assert_eq!(state.source, "gitlab");
        let credential = directory.path().join("gitlab-config.json");
        std::fs::write(&credential, "saved private connection").unwrap();
        state.connectors.set("gitlab", false).unwrap();
        state.connectors.save(&store).unwrap();
        drop(store);
        let reopened =
            Store::open_vorkspace_with_preferences_for_test(&database, preferences).unwrap();
        let state = ViewState::load(&reopened);
        assert!(state.connectors.enabled_providers().next().is_none());
        assert_eq!(state.source, "");
        assert!(state.seen.seeded);
        assert_eq!(
            std::fs::read_to_string(credential).unwrap(),
            "saved private connection"
        );
    }

    #[test]
    fn disabled_connectors_do_not_create_polling_or_connection_results() {
        let mut state = ViewState::default();
        for provider in PROVIDERS {
            state.connectors.set(provider, false).unwrap();
        }
        let service = Service {
            data_dir: Path::new("/nonexistent-data").into(),
            cwd: Path::new("/nonexistent-project").into(),
        };
        let snapshot = service.list(&[], &state);
        assert!(snapshot.items.is_empty());
        assert!(snapshot.connections.is_empty());
        assert!(snapshot.errors.is_empty());
    }

    #[test]
    fn removed_and_unknown_connectors_fail_before_any_adapter_is_used() {
        assert_eq!(PROVIDERS, ["github", "gitlab", "jira"]);
        let service = Service {
            data_dir: Path::new("/nonexistent-data").into(),
            cwd: Path::new("/nonexistent-project").into(),
        };
        for provider in ["linear", "azuredevops", "unknown"] {
            let item = Item {
                provider: provider.into(),
                ..Item::default()
            };
            assert_eq!(
                service.status(provider).unwrap_err(),
                "Unsupported inbox provider"
            );
            assert_eq!(
                service
                    .connect(provider, String::new(), String::new(), String::new())
                    .unwrap_err(),
                "Unsupported inbox provider"
            );
            assert_eq!(
                service.details(&item).unwrap_err(),
                "Unsupported inbox provider"
            );
            assert_eq!(
                service.thread(&item).unwrap_err(),
                "Unsupported inbox provider"
            );
            assert_eq!(
                service
                    .comment(&item, "comment".into(), String::new())
                    .unwrap_err(),
                "Unsupported inbox provider"
            );
            assert!(service.checks(&item).is_err());
            assert!(service.action(&item, "merge").is_err());
            assert!(Connectors::default().set(provider, true).is_err());
        }
    }
}
