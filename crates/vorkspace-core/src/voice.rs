mod audio;
mod display;
pub mod runtime;
// Vox's durable identity is independent of window, selected chat and call.
use crate::{
    bot::{self, memory},
    store::Store,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
const KEY: &str = "vorkspace.vox.conversation.v1";
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub revision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}
pub fn conversation(store: &Store) -> Result<Conversation, String> {
    let raw = store.setting(KEY)?.or_else(|| {
        store
            .desktop_preferences()
            .and_then(|p| p.get(KEY))
            .map(str::to_owned)
    });
    match raw {
        None => Ok(Conversation {
            revision: "initial".into(),
            thread_id: None,
        }),
        Some(raw) => serde_json::from_str(&raw)
            .map_err(|_| "Vox conversation is unreadable; reset it explicitly in Settings".into()),
    }
}
pub fn save(store: &Store, expected: &Conversation, thread: &str) -> Result<(), String> {
    if thread.is_empty()
        || thread.len() > 128
        || !thread
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
    {
        return Err("Invalid Vox thread identity".into());
    }
    let current = conversation(store)?;
    if &current != expected {
        return Err("Vox conversation changed; start the call again".into());
    }
    store.save_setting(
        KEY,
        &serde_json::to_string(&Conversation {
            revision: current.revision,
            thread_id: Some(thread.into()),
        })
        .map_err(|e| e.to_string())?,
    )
}
pub fn reset(store: &Store) -> Result<(), String> {
    store.save_setting(KEY, &json!({"revision":bot::new_id()}).to_string())
}
pub fn memory_action(store: &Store, action: &str, input: &Value) -> Result<Value, String> {
    bot::api::validate(action, input)?;
    let root = crate::store::bot_root(store);
    let topic = input["topic"].as_str();
    match action {
        "memory.read" => Ok(json!(memory::read(&root, "vox", topic)?)),
        "memory.search" => Ok(json!(memory::search(
            &root,
            "vox",
            input["query"].as_str().unwrap_or_default(),
            input["since"].as_str()
        )?)),
        "memory.add" | "memory.replace" | "memory.remove" => {
            let entry = if action == "memory.remove" {
                None
            } else {
                Some(memory::entry(
                    input["fact"].as_str().ok_or("Provide fact")?,
                    &memory::date(),
                    input["until"].as_str(),
                )?)
            };
            let hash = memory::edit(&root, "vox", topic, |old| {
                if action == "memory.add" {
                    Ok(memory::add(old, entry.as_deref().unwrap()))
                } else {
                    memory::replace(
                        old,
                        input["find"].as_str().ok_or("Provide find")?,
                        entry.as_deref(),
                        &memory::date(),
                    )
                }
            })?;
            Ok(json!({"hash":hash}))
        }
        _ => Err("Unknown Vox memory action".into()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_call_cannot_undo_reset_and_memory_survives() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vorkspace.db");
        let store = Store::open_vorkspace(&path).unwrap();
        let original = conversation(&store).unwrap();
        save(&store, &original, "thread-one").unwrap();
        memory_action(&store, "memory.add", &json!({"fact":"use concise replies"})).unwrap();
        let saved = conversation(&store).unwrap();
        reset(&store).unwrap();
        assert!(save(&store, &saved, "thread-one").is_err());
        drop(store);
        let store = Store::open_vorkspace(&path).unwrap();
        assert!(conversation(&store).unwrap().thread_id.is_none());
        assert!(
            memory_action(&store, "memory.read", &json!({})).unwrap()["text"]
                .as_str()
                .unwrap()
                .contains("concise replies")
        );
    }
}
