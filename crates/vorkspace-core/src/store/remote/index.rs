//! Read model over the saved bindings. Retain positions, decode only a hit.
use super::{decode_hidden, RemoteSession, Store, HIDDEN, SESSIONS};
use serde::Deserialize;
use serde_json::value::RawValue;
use std::{
    borrow::Cow,
    collections::{BTreeMap, HashSet},
    ops::Range,
};

pub(in crate::store) struct Index {
    sessions: BTreeMap<i64, Range<usize>>,
    hidden: Result<HashSet<i64>, String>,
}

#[derive(Deserialize)]
struct Binding<'a> {
    #[serde(borrow)]
    project: Project<'a>,
    #[serde(borrow)]
    session_id: Option<Cow<'a, str>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Project<'a> {
    #[serde(borrow)]
    environment_id: Cow<'a, str>,
}

fn decode_error(error: serde_json::Error) -> String {
    format!("Cannot read remote sessions: {error}")
}

impl Index {
    pub(in crate::store) fn build(values: &BTreeMap<String, String>) -> Result<Self, String> {
        let mut sessions = BTreeMap::new();
        if let Some(raw) = values.get(SESSIONS) {
            let records: BTreeMap<i64, &RawValue> =
                serde_json::from_str(raw).map_err(decode_error)?;
            for (id, value) in records {
                let value = value.get();
                // Borrowed RawValues are slices of this same input string.
                let start = value.as_ptr() as usize - raw.as_ptr() as usize;
                sessions.insert(id, start..start + value.len());
            }
        }
        Ok(Self {
            sessions,
            hidden: decode_hidden(values.get(HIDDEN).map(String::as_str)),
        })
    }

    pub(in crate::store) fn same_source(
        left: &BTreeMap<String, String>,
        right: &BTreeMap<String, String>,
    ) -> bool {
        left.get(SESSIONS) == right.get(SESSIONS) && left.get(HIDDEN) == right.get(HIDDEN)
    }

    pub(in crate::store) fn resolve(
        &self,
        values: &BTreeMap<String, String>,
        store: &Store,
        id: i64,
    ) -> Result<Option<RemoteSession>, String> {
        let raw = values.get(SESSIONS).map_or("", String::as_str);
        let exact = self
            .sessions
            .get(&id)
            .map(|range| serde_json::from_str(&raw[range.clone()]).map_err(decode_error))
            .transpose()?;
        store.resolve_remote_session(
            id,
            exact,
            || {
                self.hidden
                    .as_ref()
                    .map(|hidden| hidden.contains(&id))
                    .map_err(Clone::clone)
            },
            |legacy| self.alias(raw, legacy),
        )
    }

    fn alias(&self, raw: &str, legacy: &RemoteSession) -> Result<Option<RemoteSession>, String> {
        let Some(host) = legacy.session_id.as_deref() else {
            return Ok(None);
        };
        for range in self.sessions.values() {
            let value = &raw[range.clone()];
            let binding: Binding<'_> = serde_json::from_str(value).map_err(decode_error)?;
            if binding.project.environment_id == legacy.project.environment_id
                && binding.session_id.as_deref() == Some(host)
            {
                return serde_json::from_str(value).map(Some).map_err(decode_error);
            }
        }
        Ok(None)
    }
}
