//! Read-only adaptation of original Block.btwThreads. Opening the side surface
//! never rewrites source blocks, provider bindings, or the desktop schema.
use super::{shared, Store};
use crate::transcript::{Row, Transcript};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug)]
pub struct DesktopSideThread {
    pub id: String,
    pub source_turn: usize,
    pub snapshot: String,
    pub provider: String,
    pub model: Option<String>,
    pub model_settings: HashMap<String, String>,
    pub cwd: String,
    pub status: String,
    pub error: Option<String>,
    pub rows: Vec<Row>,
    /// Preserve original message IDs, timestamps, rich blocks, providerThreadId
    /// and unknown metadata without using that provider ID to resume a session.
    pub original: Value,
}

fn normalized(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n").trim().into()
}
fn clipped(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.into();
    }
    let mut text: String = text.chars().take(limit.saturating_sub(1)).collect();
    text.truncate(text.trim_end().len());
    if limit > 0 {
        text.push('…');
    }
    text
}
fn block_text(block: &Value) -> String {
    let role = block["role"].as_str().unwrap_or("");
    let body = normalized(block["text"].as_str().unwrap_or(""));
    let mut lines = Vec::new();
    if role == "tool" {
        let tool = &block["tool"];
        let preview = &tool["preview"];
        for (label, text) in [
            ("Tool", tool["title"].as_str().unwrap_or(&body)),
            ("Kind", tool["kind"].as_str().unwrap_or("")),
            ("Status", tool["status"].as_str().unwrap_or("")),
            ("Detail", tool["detail"].as_str().unwrap_or("")),
            (
                "File",
                preview["path"]
                    .as_str()
                    .or(preview["fileName"].as_str())
                    .unwrap_or(""),
            ),
            ("Query", preview["query"].as_str().unwrap_or("")),
            ("Output", preview["output"].as_str().unwrap_or("")),
        ] {
            let text = normalized(text);
            if !text.is_empty() {
                lines.push(format!("{label}: {text}"));
            }
        }
        for (field, label) in [
            ("startLine", "Start line"),
            ("additions", "Additions"),
            ("deletions", "Deletions"),
        ] {
            if let Some(number) = preview[field].as_i64() {
                lines.push(format!("{label}: {number}"));
            }
        }
        if let Some(entries) = preview["lines"].as_array() {
            let details = entries
                .iter()
                .map(|line| {
                    format!(
                        "{}{}: {}",
                        line["number"]
                            .as_u64()
                            .map_or(String::new(), |number| format!("{number}: ")),
                        line["kind"].as_str().unwrap_or(""),
                        line["text"].as_str().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            if !details.is_empty() {
                lines.push(format!("Lines:\n{details}"));
            }
        }
    } else if role == "image" {
        let caption = [
            block["image"]["name"].as_str().unwrap_or(""),
            block["image"]["alt"].as_str().unwrap_or(""),
        ]
        .into_iter()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" — ");
        lines.push(format!(
            "Image: {}",
            if caption.is_empty() { &body } else { &caption }
        ));
    } else {
        let label = match role {
            "user" => "User",
            "assistant" => "Assistant",
            "tasks" => "Tasks",
            _ => "Plan",
        };
        lines.push(format!("{label}: {body}"));
    }
    for attachment in block["attachments"].as_array().into_iter().flatten() {
        let name = attachment["name"].as_str().unwrap_or("").trim();
        let mime = attachment["mimeType"].as_str().unwrap_or("").trim();
        if !name.is_empty() || !mime.is_empty() {
            lines.push(format!(
                "Attachment: {}{}",
                if name.is_empty() {
                    "unnamed file"
                } else {
                    name
                },
                if mime.is_empty() {
                    String::new()
                } else {
                    format!(" ({mime})")
                }
            ));
        }
    }
    lines.join("\n")
}
fn snapshot(blocks: &[Value], end: usize) -> Option<String> {
    let mut parts = Vec::new();
    let mut remaining = 64_000usize;
    for block in blocks[..=end].iter().rev().filter(|block| {
        block["internal"] != true
            && block["orchestration"] != true
            && !block["orchestration"].is_object()
            && matches!(
                block["role"].as_str(),
                Some("user" | "assistant" | "tool" | "tasks" | "plan" | "image")
            )
    }) {
        let text = block_text(block);
        let bounded = clipped(&text, 8_000);
        let text = clipped(&bounded, remaining);
        if text.is_empty() {
            break;
        }
        remaining = remaining.saturating_sub(text.chars().count() + 2);
        let truncated = text.chars().count() < bounded.chars().count();
        parts.push(text);
        if remaining == 0 || truncated {
            break;
        }
    }
    parts.reverse();
    let text = parts.join("\n\n");
    (!text.is_empty()).then_some(text)
}
fn provider(blocks: &[Value], owner: usize, thread: &Value, fallback: &str) -> Option<String> {
    let kind = thread["harness"]
        .as_str()
        .or(blocks[owner]["turnModel"]["harness"].as_str())
        .or_else(|| {
            blocks[..owner]
                .iter()
                .rev()
                .find_map(|block| block["handoff"]["to"].as_str())
        })
        .or_else(|| {
            blocks[..owner]
                .iter()
                .rev()
                .find_map(|block| block["turnModel"]["harness"].as_str())
        })
        .or_else(|| {
            blocks[owner + 1..]
                .iter()
                .find_map(|block| block["handoff"]["from"].as_str())
        })
        .unwrap_or(fallback);
    // This is a read-only history adapter. Availability belongs to execution
    // admission, so retiring a provider must not hide its saved side threads.
    (!kind.is_empty()).then(|| kind.into())
}

impl Store {
    pub fn desktop_side_threads(&self, id: i64) -> Result<Vec<DesktopSideThread>, String> {
        if self.shared.is_none() {
            return Ok(Vec::new());
        }
        let blocks = self.shared_blocks(id)?;
        let session = self
            .sessions()?
            .into_iter()
            .find(|session| session.id == id)
            .ok_or("Session not found")?;
        let mut imported = Vec::new();
        let mut seen = HashSet::new();
        let mut source_turn = 0;
        for (owner, block) in blocks.iter().enumerate() {
            for thread in block["btwThreads"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|_| block["role"] == "user" && block["internal"] != true)
            {
                let Some(thread_id) = thread["id"].as_str().filter(|id| !id.is_empty()) else {
                    continue;
                };
                let Some(end_id) = thread["sourceEndBlockId"].as_str() else {
                    continue;
                };
                let Some(end) = blocks
                    .iter()
                    .position(|block| block["id"].as_str() == Some(end_id))
                else {
                    continue;
                };
                // A future turn cannot be substituted for the saved source bound.
                if end < owner
                    || blocks[owner + 1..=end].iter().any(|block| {
                        block["role"] == "user"
                            && block["draft"] != true
                            && block["internal"] != true
                    })
                {
                    continue;
                }
                let Some(snapshot) = snapshot(&blocks, end) else {
                    continue;
                };
                let Some(provider) = provider(&blocks, owner, thread, &session.provider) else {
                    continue;
                };
                if !seen.insert(thread_id.to_owned()) {
                    continue;
                }
                let mut rows = Vec::new();
                for message in thread["messages"].as_array().into_iter().flatten() {
                    let Some(role) = message["role"]
                        .as_str()
                        .filter(|role| matches!(*role, "user" | "assistant"))
                    else {
                        continue;
                    };
                    if role == "assistant" {
                        if let Some(activity) = message["blocks"]
                            .as_array()
                            .filter(|blocks| !blocks.is_empty())
                        {
                            rows.extend(shared::blocks::project(activity)?);
                            continue;
                        }
                    }
                    let mut text = Transcript::default();
                    text.begin(role);
                    text.append(message["text"].as_str().unwrap_or(""));
                    text.rows[0].sent_at = message["createdAt"].as_u64();
                    rows.extend(text.rows);
                }
                imported.push(DesktopSideThread {
                    id: thread_id.into(),
                    source_turn,
                    snapshot,
                    provider: provider.clone(),
                    model: thread["model"]
                        .as_str()
                        .and_then(|model| super::native_model(model, &provider)),
                    model_settings: thread["modelSettings"]
                        .as_object()
                        .into_iter()
                        .flatten()
                        .filter_map(|(key, value)| {
                            value.as_str().map(|value| (key.clone(), value.into()))
                        })
                        .collect(),
                    cwd: session.cwd.clone(),
                    status: thread["status"].as_str().unwrap_or("ready").into(),
                    error: thread["error"].as_str().map(str::to_owned),
                    rows,
                    original: thread.clone(),
                });
            }
            source_turn += shared::blocks::project(std::slice::from_ref(block))?.len();
        }
        imported.sort_by_key(|thread| thread.original["createdAt"].as_u64().unwrap_or(0));
        Ok(imported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn historical_provider_identity_is_preserved_without_falling_back() {
        let blocks = [json!({"turnModel":{"harness":"codex"}})];
        for kind in [
            "cursor", "grok", "opencode", "fx", "hermes", "devin", "unknown",
        ] {
            assert_eq!(
                provider(&blocks, 0, &json!({"harness":kind}), "claude").as_deref(),
                Some(kind)
            );
        }
        assert_eq!(
            provider(&blocks, 0, &json!({}), "claude").as_deref(),
            Some("codex")
        );
    }

    #[test]
    fn unsupported_side_history_remains_readable_without_rewriting_its_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.db");
        let store = Store::open_vorkspace(&path).unwrap();
        let session = store.create("codex", "/project").unwrap();
        let original = store.persisted_session_id(session.id).unwrap();
        let threads = [
            "cursor", "grok", "opencode", "fx", "hermes", "devin", "unknown",
        ]
        .map(|kind| {
            json!({
                "id":kind,"harness":kind,"model":format!("{kind}:saved-model"),
                "sourceEndBlockId":"answer","providerThreadId":"preserved-provider-id",
                "status":"error","messages":[{"role":"assistant","text":"Retained side answer"}]
            })
        });
        let blocks = json!([
            {"id":"question","role":"user","text":"Question","btwThreads":threads},
            {"id":"answer","role":"assistant","text":"Main answer"}
        ]);
        store
            .conn
            .execute(
                "UPDATE sessions SET blocks_json=?1 WHERE id=?2",
                rusqlite::params![blocks.to_string(), original],
            )
            .unwrap();
        let imported = store.desktop_side_threads(session.id).unwrap();
        assert_eq!(imported.len(), threads.len());
        for thread in &imported {
            assert_eq!(thread.id, thread.provider);
            assert_eq!(thread.model.as_deref(), Some("saved-model"));
            assert_eq!(thread.original["providerThreadId"], "preserved-provider-id");
            assert!(thread
                .rows
                .iter()
                .any(|row| row.text == "Retained side answer"));
        }
        store.title(session.id, "Edited main title").unwrap();
        drop(store);

        let reopened = Store::open_vorkspace(&path).unwrap();
        assert_eq!(
            reopened.desktop_side_threads(session.id).unwrap().len(),
            threads.len()
        );
        let after: String = reopened
            .conn
            .query_row(
                "SELECT blocks_json FROM sessions WHERE id=?1",
                [&original],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&after).unwrap(), blocks);
    }
}
