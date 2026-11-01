//! Durable Inbox choices. The store reads and writes them.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BotDefaults(pub(crate) BTreeMap<String, String>);
