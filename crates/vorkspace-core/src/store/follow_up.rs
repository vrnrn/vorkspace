//! Queue acceptance is strict; delayed presentation saves may skip older revisions.
use super::{ComposerOrigin, Store};
use crate::follow_up::State;
use rusqlite::{Transaction, TransactionBehavior};

#[derive(Clone, Copy)]
pub(super) enum FollowUpWrite<'a> {
    Deferred,
    Accept,
    BotDraft {
        revision: u64,
        draft_id: &'a str,
    },
    Composer {
        origin: &'a ComposerOrigin,
        message_id: &'a str,
    },
}

impl FollowUpWrite<'_> {
    pub(super) fn check_revision(self, previous: u64, next: u64) -> Result<bool, String> {
        if !matches!(self, Self::Deferred) && previous >= next {
            return Err("The message queue changed before this prompt could be saved. Reopen it before sending; saved messages were preserved.".into());
        }
        Ok(previous <= next)
    }
}

impl Store {
    /// Publish a Bot draft and its exact composer origin in the same transaction.
    pub fn accept_bot_draft(
        &self,
        session: i64,
        state: &State,
        revision: u64,
        draft_id: &str,
    ) -> Result<(), String> {
        self.write_follow_ups(
            session,
            state,
            FollowUpWrite::BotDraft { revision, draft_id },
        )
    }

    pub fn accept_composer_message(
        &self,
        session: i64,
        state: &State,
        origin: &ComposerOrigin,
        message_id: &str,
    ) -> Result<(), String> {
        self.write_follow_ups(
            session,
            state,
            FollowUpWrite::Composer { origin, message_id },
        )
    }

    /// Synchronous external admission must prove that this complete queue was
    /// written. Async stale completions may skip; accepted prompts may not.
    pub fn accept_follow_ups(&self, session: i64, state: &State) -> Result<(), String> {
        self.write_follow_ups(session, state, FollowUpWrite::Accept)
    }

    pub fn save_follow_ups(&self, session: i64, state: &State) -> Result<(), String> {
        self.write_follow_ups(session, state, FollowUpWrite::Deferred)
    }

    fn write_follow_ups(
        &self,
        session: i64,
        state: &State,
        write: FollowUpWrite<'_>,
    ) -> Result<(), String> {
        if self.uses_vorkspace_schema() && session >= 0 {
            return self.shared_write_follow_ups(session, state, write);
        }
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let key = format!("follow-up:{session}");
        let previous = self
            .setting(&key)?
            .map(|raw| {
                serde_json::from_str::<State>(&raw)
                    .map_err(|error| format!("Cannot verify the saved queue: {error}"))
            })
            .transpose()?;
        if let Some(previous) = previous {
            if !write.check_revision(previous.revision, state.revision)? {
                return Ok(());
            }
        }
        self.transfer_follow_up(&tx, session, state, write)?;
        self.save_setting(
            &key,
            &serde_json::to_string(state).map_err(|error| error.to_string())?,
        )?;
        tx.commit().map_err(|error| error.to_string())
    }

    pub(super) fn transfer_follow_up(
        &self,
        tx: &Transaction<'_>,
        session: i64,
        state: &State,
        write: FollowUpWrite<'_>,
    ) -> Result<(), String> {
        match write {
            FollowUpWrite::Deferred | FollowUpWrite::Accept => Ok(()),
            FollowUpWrite::Composer { origin, message_id } => {
                if !state
                    .messages
                    .last()
                    .into_iter()
                    .chain(state.draft.as_ref())
                    .any(|message| message.id == message_id)
                {
                    return Err("The saved message does not own this draft version.".into());
                }
                self.transfer_composer_origin(tx, session, origin)
            }
            FollowUpWrite::BotDraft { revision, draft_id } => {
                let current = self.composer_draft(session)?;
                if current.as_ref().map_or(0, |draft| draft.revision) != revision
                    || state
                        .draft
                        .as_ref()
                        .is_none_or(|draft| draft.id != draft_id)
                {
                    return Err(
                        "Composer changed in another window; your input was preserved".into(),
                    );
                }
                // Bot draft editing still uses its original receipt during delivery.
                self.save_setting(
                    &format!("bot-draft-origin:{session}"),
                    &serde_json::json!({"id":draft_id,"revision":revision}).to_string(),
                )
            }
        }
    }

    pub fn follow_ups(&self, session: i64) -> Result<State, String> {
        if self.uses_vorkspace_schema() && session >= 0 {
            return self.shared_follow_ups(session);
        }
        let mut state: State = self
            .setting(&format!("follow-up:{session}"))?
            .map(|text| serde_json::from_str(&text).map_err(|e| e.to_string()))
            .transpose()?
            .unwrap_or_default();
        let mut remaining = Vec::new();
        for message in state.messages {
            if self
                .setting(&format!("bot-delivered:{session}:{}", message.id))?
                .is_none()
            {
                remaining.push(message);
            }
        }
        state.messages = remaining;
        if let Some(draft) = &state.draft {
            if self
                .setting(&format!("bot-delivered:{session}:{}", draft.id))?
                .is_some()
            {
                state.draft = None;
            }
        }
        // Recovery is always explicit, as in React's deserializeSession.
        state.paused = !state.messages.is_empty();
        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::follow_up::Message;
    use std::path::Path;
    #[test]
    fn bot_draft_origin_queue_and_revision_commit_together() {
        use serde_json::json;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.db");
        let first = Store::open_vorkspace(&path).unwrap();
        let id = first.create("codex", "/project").unwrap().id;
        let second = Store::open_vorkspace(&path).unwrap();
        let payload = json!({"cwd":"/project","text":"original","selectionStart":0,"selectionEnd":0,"attachments":[],"mcpTags":[]});
        first.save_composer_draft(id, 0, &payload).unwrap();
        let mut state = first.follow_ups(id).unwrap();
        state.revision += 1;
        state.draft = Some(Message {
            id: "draft-one".into(),
            text: "original".into(),
            ..Default::default()
        });
        first.accept_bot_draft(id, &state, 1, "draft-one").unwrap();
        let key = format!("bot-draft-origin:{id}");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&second.setting(&key).unwrap().unwrap())
                .unwrap()["id"],
            "draft-one"
        );
        let mut changed = payload.clone();
        changed["text"] = json!("other input");
        second.save_composer_draft(id, 1, &changed).unwrap();
        let mut next = first.follow_ups(id).unwrap();
        next.revision += 1;
        next.draft = Some(Message {
            id: "draft-two".into(),
            text: "stale".into(),
            ..Default::default()
        });
        assert!(first.accept_bot_draft(id, &next, 1, "draft-two").is_err());
        assert_eq!(first.follow_ups(id).unwrap().draft.unwrap().id, "draft-one");
        first.conn.execute_batch("CREATE TRIGGER fail_origin BEFORE UPDATE ON native_bot_draft_origins BEGIN SELECT RAISE(FAIL, 'injected'); END;").unwrap();
        assert!(first.accept_bot_draft(id, &next, 2, "draft-two").is_err());
        drop(first);
        drop(second);
        let reopened = Store::open_vorkspace(&path).unwrap();
        assert_eq!(
            reopened.composer_draft(id).unwrap().unwrap().payload["text"],
            "other input"
        );
        assert_eq!(
            reopened.follow_ups(id).unwrap().draft.unwrap().id,
            "draft-one"
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&reopened.setting(&key).unwrap().unwrap())
                .unwrap()["id"],
            "draft-one"
        );
    }
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
