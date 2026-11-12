//! Inbox visibility is independent of authentication. Disabling a connector
//! keeps credentials and read state while removing its work and UI surfaces.
use super::PROVIDERS;
use std::collections::BTreeMap;

pub(crate) const KEY: &str = "native-inbox-connectors";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connectors(pub(crate) BTreeMap<String, bool>);

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
    pub fn enabled(&self, provider: &str) -> bool {
        self.0.get(provider).copied().unwrap_or(false)
    }

    pub fn enabled_providers(&self) -> impl Iterator<Item = &'static str> + '_ {
        PROVIDERS
            .into_iter()
            .filter(|provider| self.enabled(provider))
    }

    pub fn set(&mut self, provider: &str, enabled: bool) -> Result<(), String> {
        let value = self.0.get_mut(provider).ok_or("Unsupported connector")?;
        *value = enabled;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inbox::{Item, Service, ViewState};
    use std::path::Path;

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
                "Unsupported connector"
            );
            assert_eq!(
                service
                    .connect(provider, String::new(), String::new(), String::new())
                    .unwrap_err(),
                "Unsupported connector"
            );
            assert!(
                matches!(service.discussion(&item), Err(error) if error == "Unsupported connector")
            );
            assert_eq!(
                service
                    .comment(&item, "comment".into(), String::new())
                    .unwrap_err(),
                "Unsupported connector"
            );
            assert!(service.checks(&item).is_err());
            assert!(service.action(&item, "merge").is_err());
            assert!(Connectors::default().set(provider, true).is_err());
        }
    }
}
