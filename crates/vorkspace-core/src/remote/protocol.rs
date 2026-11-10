//! vorkspace Host v1 session sync. Values retain provider-specific block fields.
use super::Connections;
use serde_json::{json, Value};
use std::collections::HashMap;

pub const HOST_PROTOCOL_VERSION: u64 = 1;

pub fn supported_providers() -> &'static [&'static str] {
    &["claude", "codex", "pi", "omp", "antigravity"]
}

/// The native common controls use the same provider-specific keys as the
/// headless host adapters. Other advertised controls retain their exact IDs.
pub fn wire_model_settings(
    provider: &str,
    settings: &crate::agent::Settings,
) -> HashMap<String, String> {
    let mut values = settings.model_settings.clone();
    if let Some(effort) = &settings.effort {
        values.insert(
            if provider == "codex" {
                "reasoningEffort"
            } else {
                "effort"
            }
            .into(),
            effort.clone(),
        );
    }
    if provider == "codex" {
        if let Some(tier) = &settings.service_tier {
            values.insert("serviceTier".into(), tier.clone());
        } else if settings.fast {
            values.insert("serviceTier".into(), "fast".into());
        }
    } else {
        values.insert("fast".into(), settings.fast.to_string());
    }
    if let Some(thinking) = settings.thinking {
        values.insert("thinking".into(), thinking.to_string());
    }
    values
}

/// Canonical configuration used to distinguish a local unsent choice from a
/// host update made by another desktop.
pub fn settings_configuration(provider: &str, settings: &crate::agent::Settings) -> Value {
    let model = settings.model.as_deref().map(|model| {
        if model.starts_with(&format!("{provider}:")) {
            model.to_owned()
        } else {
            format!("{provider}:{model}")
        }
    });
    json!({"model":model,"modelSettings":wire_model_settings(provider, settings),"runtimeMode":settings.access.id()})
}

/// Host attachment references require RFC 4122 UUIDs, unlike command IDs.
pub fn new_attachment_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
/// A transfer can exceed the per-response RPC cap while retaining a memory bound.
pub const MAX_SESSION_SYNC_UNITS: usize = 256 * 1024 * 1024;
const MAX_SESSION_SYNC_BYTES: usize = 512 * 1024 * 1024;
const MAX_SESSION_SYNC_CHUNKS: usize = 4096;

/// One owned wire snapshot. A changed poll returns the first block position
/// whose projection may differ, including a deletion at the end.
#[derive(Default)]
pub struct SessionObservation {
    snapshot: Option<Value>,
    machine_id: Option<String>,
}

impl SessionObservation {
    pub fn snapshot(&self) -> Option<&Value> {
        self.snapshot.as_ref()
    }

    pub fn revision(&self) -> Option<u64> {
        self.snapshot.as_ref()?.get("revision")?.as_u64()
    }

    pub fn apply(&mut self, mut sync: Value) -> Result<Option<usize>, String> {
        match sync["kind"].as_str() {
            Some("unchanged")
                if self.revision().is_some() && self.revision() == sync["revision"].as_u64() =>
            {
                Ok(None)
            }
            Some("snapshot") => {
                let next = sync["value"].take();
                let blocks = session_blocks(&next)?;
                if self.revision().is_some_and(|before| {
                    next["revision"]
                        .as_u64()
                        .is_none_or(|revision| revision < before)
                }) {
                    return Err("Host returned an older session snapshot".into());
                }
                let first = self
                    .snapshot
                    .as_ref()
                    .and_then(|before| session_blocks(before).ok())
                    .map_or(0, |before| {
                        before
                            .iter()
                            .zip(blocks)
                            .take_while(|(a, b)| a == b)
                            .count()
                    });
                self.snapshot = Some(next);
                Ok(Some(first))
            }
            Some("delta")
                if self.revision().is_some() && self.revision() == sync["base"].as_u64() =>
            {
                self.apply_delta(sync).map(Some)
            }
            _ => Err("Invalid session sync response or revision".into()),
        }
    }

    fn apply_delta(&mut self, mut sync: Value) -> Result<usize, String> {
        let mut next = sync["value"].take();
        if next["revision"].as_u64() <= self.revision() {
            return Err("Session delta did not advance its revision".into());
        }
        let session = next
            .get_mut("session")
            .and_then(Value::as_object_mut)
            .ok_or("Session sync is missing its session")?;
        let mut changed = take_array(&mut sync["blocks"])?;
        let ids = sync["blockIds"]
            .as_array()
            .ok_or("Session sync is missing its block order")?;
        let known = self.snapshot.as_mut().expect("delta has an exact base");
        let previous = known
            .pointer_mut("/session/blocks")
            .and_then(Value::as_array_mut)
            .ok_or("Session sync is missing its blocks")?;

        // Validate the complete order before moving anything from the base.
        // A rejected delta leaves the original available for a full retry.
        let mut positions = HashMap::with_capacity(previous.len() + changed.len());
        for (index, block) in previous.iter().enumerate() {
            positions.insert(block_id(block)?, BlockSource::Previous(index));
        }
        for (index, block) in changed.iter().enumerate() {
            positions.insert(block_id(block)?, BlockSource::Changed(index));
        }
        let order = ids
            .iter()
            .map(|id| {
                let id = id.as_str().ok_or("Session sync has an invalid block ID")?;
                positions
                    .remove(id)
                    .ok_or("Session sync has a missing or duplicate block")
            })
            .collect::<Result<Vec<_>, _>>()?;
        drop(positions);

        let first = order
            .iter()
            .enumerate()
            .take_while(|(at, entry)| **entry == BlockSource::Previous(*at))
            .count();
        let blocks = order
            .into_iter()
            .map(|source| match source {
                BlockSource::Previous(index) => previous[index].take(),
                BlockSource::Changed(index) => changed[index].take(),
            })
            .collect();
        session.insert("blocks".into(), Value::Array(blocks));
        self.snapshot = Some(next);
        Ok(first)
    }
}

#[derive(PartialEq, Eq)]
enum BlockSource {
    Previous(usize),
    Changed(usize),
}

fn session_blocks(snapshot: &Value) -> Result<&[Value], String> {
    snapshot
        .pointer("/session/blocks")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| "Session sync is missing its blocks".into())
}

fn take_array(value: &mut Value) -> Result<Vec<Value>, String> {
    match value.take() {
        Value::Array(values) => Ok(values),
        _ => Err("Session sync is missing its blocks".into()),
    }
}

fn block_id(block: &Value) -> Result<&str, String> {
    block["id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "Session sync has an invalid block".into())
}

/// Fetches one immutable sync transfer. Offsets and the reported length count
/// UTF-16 units, as in the JavaScript host, rather than UTF-8 bytes or characters.
pub fn read_session_sync(
    session_id: &str,
    revision: Option<u64>,
    mut request: impl FnMut(&str, Value) -> Result<Value, String>,
) -> Result<Value, String> {
    let response = request(
        "sessions.sync",
        json!({ "sessionId": session_id, "revision": revision }),
    )?;
    if response.get("kind").and_then(Value::as_str) != Some("chunked") {
        return Ok(response);
    }
    let transfer = response
        .get("transfer")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or("Invalid session transfer")?;
    let length = response
        .get("length")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .filter(|length| *length > 0 && *length <= MAX_SESSION_SYNC_UNITS)
        .ok_or("Session transfer has an invalid length")?;
    let mut offset = 0;
    let mut text = String::with_capacity(length.min(4 * 1024 * 1024));
    let mut chunks = 0;
    while offset < length {
        chunks += 1;
        if chunks > MAX_SESSION_SYNC_CHUNKS {
            return Err("Session transfer has too many pieces".into());
        }
        let chunk = request(
            "sessions.syncChunk",
            json!({
                "sessionId": session_id, "transfer": transfer, "offset": offset,
            }),
        )?;
        let data = chunk
            .get("data")
            .and_then(Value::as_str)
            .filter(|data| !data.is_empty())
            .ok_or("Session transfer ended early")?;
        offset = offset
            .checked_add(data.encode_utf16().count())
            .ok_or("Session transfer has an unexpected length")?;
        if offset > length {
            return Err("Session transfer has an unexpected length".into());
        }
        if text.len().saturating_add(data.len()) > MAX_SESSION_SYNC_BYTES {
            return Err("Session transfer is too large".into());
        }
        text.push_str(data);
    }
    serde_json::from_str(&text).map_err(|_| "Invalid session transfer data".into())
}

impl Connections {
    pub fn load_session(&self, machine_id: &str, session_id: &str) -> Result<Value, String> {
        let mut observation = SessionObservation::default();
        self.poll_session(machine_id, session_id, &mut observation)?;
        observation
            .snapshot
            .ok_or_else(|| "Session sync is missing its snapshot".into())
    }

    /// An unchanged revision keeps the same snapshot and projection. A
    /// rejected delta retries with a full snapshot against the retained base.
    pub fn poll_session(
        &self,
        machine_id: &str,
        session_id: &str,
        observation: &mut SessionObservation,
    ) -> Result<Option<usize>, String> {
        if observation.snapshot().is_some_and(|snapshot| {
            observation.machine_id.as_deref() != Some(machine_id)
                || snapshot["session"]["id"].as_str() != Some(session_id)
        }) {
            *observation = SessionObservation::default();
        }
        let sync = |revision| {
            read_session_sync(session_id, revision, |method, params| {
                self.request(machine_id, method, params)
            })
        };
        let apply = |observation: &mut SessionObservation, response: Value| {
            if matches!(response["kind"].as_str(), Some("snapshot" | "delta"))
                && response["value"]["session"]["id"].as_str() != Some(session_id)
            {
                return Err("Host returned a snapshot for another session".into());
            }
            observation.apply(response)
        };
        let response = sync(observation.revision())?;
        let changed = match apply(observation, response) {
            Ok(changed) => Ok(changed),
            Err(_) => apply(observation, sync(None)?),
        }?;
        if observation.machine_id.is_none() {
            observation.machine_id = Some(machine_id.into());
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> Value {
        json!({ "projectId": "project", "revision": 4, "status": "running", "updatedAt": 0,
            "session": { "id": "session", "busy": true, "blocks": [
                { "id": "user", "role": "user", "text": "Do it", "custom": true },
                { "id": "reply", "role": "assistant", "text": "Work", "streaming": true }
            ] }
        })
    }

    #[test]
    fn delta_keeps_unchanged_blocks_and_host_order() {
        let known = known();
        let sync = json!({ "kind": "delta", "base": 4,
            "value": { "revision": 6, "status": "idle", "session": { "id": "session", "busy": false } },
            "blockIds": ["user", "reply", "done"],
            "blocks": [ { "id": "reply", "role": "assistant", "text": "Work done" },
                { "id": "done", "role": "system", "text": "Finished" } ]
        });
        let mut observation = SessionObservation {
            snapshot: Some(known.clone()),
            ..Default::default()
        };
        assert_eq!(observation.apply(sync).unwrap(), Some(1));
        let next = observation.snapshot().unwrap();
        assert_eq!(next["revision"], 6);
        assert_eq!(next["session"]["busy"], false);
        assert_eq!(next["session"]["blocks"][0], known["session"]["blocks"][0]);
        assert_eq!(next["session"]["blocks"][1]["text"], "Work done");
        assert_eq!(next["session"]["blocks"][2]["id"], "done");
    }

    #[test]
    fn incompatible_deltas_and_unchanged_revisions_require_snapshot() {
        let known = known();
        let mut observation = SessionObservation::default();
        assert!(observation
            .apply(json!({ "kind": "unchanged", "revision": 4 }))
            .is_err());
        assert_eq!(
            observation
                .apply(json!({ "kind": "snapshot", "value": known }))
                .unwrap(),
            Some(0)
        );
        assert_eq!(
            observation
                .apply(json!({ "kind": "unchanged", "revision": 4 }))
                .unwrap(),
            None
        );
        assert!(observation
            .apply(json!({ "kind": "unchanged", "revision": 3 }))
            .is_err());
        for order in [json!(["unknown"]), json!(["user", "user"])] {
            assert!(observation
                .apply(json!({ "kind": "delta", "base": 4,
                "value": { "revision": 5, "session": {} }, "blockIds": order, "blocks": [] }))
                .is_err());
        }
        assert_eq!(observation.snapshot(), Some(&known));
    }

    #[test]
    fn delta_moves_existing_blocks_and_reports_deletions_and_reorders() {
        let mut observation = SessionObservation {
            snapshot: Some(known()),
            ..Default::default()
        };
        let text = observation.snapshot().unwrap()["session"]["blocks"][1]["text"]
            .as_str()
            .unwrap()
            .as_ptr();
        assert_eq!(
            observation
                .apply(json!({ "kind": "delta", "base": 4,
                "value": { "revision": 5, "session": { "id": "session" } },
                "blockIds": ["reply", "user"], "blocks": [] }))
                .unwrap(),
            Some(0)
        );
        assert_eq!(
            observation.snapshot().unwrap()["session"]["blocks"][0]["text"]
                .as_str()
                .unwrap()
                .as_ptr(),
            text
        );
        assert_eq!(
            observation
                .apply(json!({ "kind": "delta", "base": 5,
                "value": { "revision": 6, "session": { "id": "session" } },
                "blockIds": ["reply"], "blocks": [] }))
                .unwrap(),
            Some(1)
        );
        assert_eq!(
            observation.snapshot().unwrap()["session"]["blocks"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn transfer_offsets_count_utf16_and_preserve_non_ascii() {
        let data = json!({ "kind": "snapshot", "value": { "text": "日本語 🖥 𝄞" } }).to_string();
        let pieces = [
            &data[..data.find('🖥').unwrap()],
            "🖥",
            &data[data.find('🖥').unwrap() + '🖥'.len_utf8()..],
        ];
        let mut index = 0;
        let mut units = 0;
        let sync = read_session_sync("session", Some(8), |method, params| {
            if method == "sessions.sync" {
                assert_eq!(params["revision"], 8);
                return Ok(json!({ "kind": "chunked", "transfer": "transfer", "length": data.encode_utf16().count() }));
            }
            assert_eq!(params["offset"], units);
            assert_eq!(params["transfer"], "transfer");
            let piece = pieces[index];
            units += piece.encode_utf16().count();
            index += 1;
            Ok(json!({ "data": piece }))
        }).unwrap();
        assert_eq!(sync["value"]["text"], "日本語 🖥 𝄞");
        assert_eq!(index, 3);
    }

    #[test]
    fn transfers_reject_empty_oversized_and_inconsistent_data() {
        for (length, data) in [
            (4_u64, ""),
            (1, "🖥"),
            (0, ""),
            (MAX_SESSION_SYNC_UNITS as u64 + 1, ""),
        ] {
            let mut called = 0;
            assert!(read_session_sync("session", None, |_, _| {
                called += 1;
                if called == 1 {
                    Ok(json!({ "kind": "chunked", "transfer": "t", "length": length }))
                } else {
                    Ok(json!({ "data": data }))
                }
            })
            .is_err());
        }
    }
}
