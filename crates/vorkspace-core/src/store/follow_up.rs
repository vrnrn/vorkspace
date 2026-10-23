//! Queue acceptance is strict; delayed presentation saves may skip older revisions.
use super::Store;
use crate::follow_up::State;
impl Store {
    /// Synchronous external admission must prove that this complete queue was
    /// written. Async stale completions may skip; accepted prompts may not.
    pub fn accept_follow_ups(&self, session: i64, state: &State) -> Result<(), String> {
        if self.uses_vorkspace_schema() && session >= 0 {
            return self.shared_accept_follow_ups(session, state);
        }
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )
        .map_err(|e| e.to_string())?;
        let key = format!("follow-up:{session}");
        let previous = self
            .setting(&key)?
            .map(|raw| {
                serde_json::from_str::<State>(&raw)
                    .map_err(|e| format!("Cannot verify the saved queue: {e}"))
            })
            .transpose()?;
        if previous.is_some_and(|previous| previous.revision >= state.revision) {
            return Err("The message queue changed before this prompt could be saved. Reopen it before sending; saved messages were preserved.".into());
        }
        self.save_setting(
            &key,
            &serde_json::to_string(state).map_err(|e| e.to_string())?,
        )?;
        tx.commit().map_err(|e| e.to_string())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::follow_up::Message;
    use std::path::Path;
    #[test]
    fn strict_admission_rejects_another_connections_newer_queue() {
        for open in [
            Store::open as fn(&Path) -> Result<Store, String>,
            Store::open_vorkspace,
        ] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("db");
            let first = open(&path).unwrap();
            let session = first.create("codex", "/project").unwrap().id;
            let stale = first.follow_ups(session).unwrap();
            let second = open(&path).unwrap();
            let mut newer = second.follow_ups(session).unwrap();
            newer.revision = 100;
            newer.messages.push(Message {
                id: "other-owner".into(),
                text: "Keep the newer message".into(),
                ..Default::default()
            });
            second.save_follow_ups(session, &newer).unwrap();
            let mut attempted = stale;
            attempted.revision += 1;
            attempted.messages.push(Message {
                id: "not-admitted".into(),
                text: "Do not falsely accept".into(),
                ..Default::default()
            });
            assert!(first
                .accept_follow_ups(session, &attempted)
                .unwrap_err()
                .contains("queue changed"));
            assert_eq!(first.follow_ups(session).unwrap().messages, newer.messages);
            drop(first);
            drop(second);
            assert_eq!(
                open(&path).unwrap().follow_ups(session).unwrap().messages,
                newer.messages
            );
        }
    }
    #[test]
    fn delayed_async_saves_still_skip_but_synchronous_admission_cannot_skip() {
        for open in [
            Store::open as fn(&Path) -> Result<Store, String>,
            Store::open_vorkspace,
        ] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("db");
            let store = open(&path).unwrap();
            let session = store.create("codex", "/project").unwrap().id;
            let mut state = store.follow_ups(session).unwrap();
            state.revision = 10;
            state.messages.push(Message {
                id: "saved".into(),
                text: "Keep saved".into(),
                ..Default::default()
            });
            store.save_follow_ups(session, &state).unwrap();
            let stale = State {
                revision: 1,
                ..Default::default()
            };
            store.save_follow_ups(session, &stale).unwrap();
            assert!(store.accept_follow_ups(session, &stale).is_err());
            assert!(store.accept_follow_ups(session, &state).is_err());
            assert_eq!(store.follow_ups(session).unwrap().messages, state.messages);
        }
    }
}
