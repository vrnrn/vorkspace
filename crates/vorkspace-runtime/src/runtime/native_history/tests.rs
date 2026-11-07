use super::*;
use serde_json::json;
use std::io::Write;
use vorkspace_core::{agent::TurnInfo, store::SubmissionInput};

fn append(source: &Path, records: &[serde_json::Value]) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(source)
        .unwrap();
    for record in records {
        writeln!(file, "{record}").unwrap();
    }
}

#[derive(Clone, Copy)]
enum Log {
    CodexEvents,
    CodexItems,
    Claude,
}

impl Log {
    fn kind(self) -> Kind {
        match self {
            Self::CodexEvents | Self::CodexItems => Kind::Codex,
            Self::Claude => Kind::Claude,
        }
    }

    fn prompt(self, turn: &str, text: &str) -> Vec<serde_json::Value> {
        if matches!(self, Self::Claude) {
            return vec![
                json!({"type":"user","uuid":turn,"sessionId":"thread","message":{"content":text}}),
            ];
        }
        let input = match self {
            Self::CodexEvents => json!({"type":"user_message","message":text}),
            Self::CodexItems => {
                json!({"type":"item_completed","turn_id":turn,"item":{"type":"UserMessage","id":format!("{turn}-item"),"text":text}})
            }
            Self::Claude => unreachable!(),
        };
        vec![
            json!({"type":"event_msg","payload":{"type":"task_started","turn_id":turn}}),
            json!({"type":"event_msg","payload":input}),
        ]
    }

    fn answer(self, turn: &str, id: &str, text: &str, complete: bool) -> Vec<serde_json::Value> {
        if matches!(self, Self::Claude) {
            return vec![
                json!({"type":"assistant","uuid":id,"sessionId":"thread","message":{"stop_reason":if complete { "end_turn" } else { "tool_use" },"content":[{"type":"text","text":text}]}}),
            ];
        }
        let output = match self {
            Self::CodexEvents => json!({"type":"agent_message","message":text}),
            Self::CodexItems => {
                json!({"type":"item_completed","turn_id":turn,"item":{"type":"AgentMessage","id":id,"text":text}})
            }
            Self::Claude => unreachable!(),
        };
        let mut records = vec![json!({"type":"event_msg","payload":output})];
        if complete {
            records.push(
                json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":turn}}),
            );
        }
        records
    }
}

fn saved_text(database: &Database, session: i64) -> Vec<String> {
    locked(database)
        .unwrap()
        .load(session)
        .unwrap()
        .rows
        .into_iter()
        .map(|row| row.text)
        .collect()
}

#[test]
fn unfinished_external_turn_recovery_publishes_saved_text_before_managed_history() {
    for shared in [false, true] {
        for log in [Log::CodexEvents, Log::CodexItems, Log::Claude] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path();
            let path = root.join("sessions.db");
            let open = || {
                if shared {
                    Store::open_vorkspace(&path)
                } else {
                    Store::open(&path)
                }
                .unwrap()
            };
            let owner = open();
            let kind = log.kind();
            let session = owner.create(kind.id(), root.to_str().unwrap()).unwrap();
            owner.provider_id(session.id, "thread").unwrap();
            let settings = Settings {
                account: vorkspace_core::accounts::Profile::new("fixture", root),
                ..Default::default()
            };
            owner.save_agent_settings(session.id, &settings).unwrap();
            let draft = json!({"text":"My unsent draft","cwd":session.cwd,"selectionStart":0,"selectionEnd":0,"attachments":[],"mcpTags":[]});
            owner.save_composer_draft(session.id, 0, &draft).unwrap();
            let home = settings.account.home(kind).unwrap();
            let source = if kind == Kind::Codex {
                home.join("sessions/rollout-2026-11-06-thread.jsonl")
            } else {
                home.join("projects/fixture/thread.jsonl")
            };
            std::fs::create_dir_all(source.parent().unwrap()).unwrap();
            if kind == Kind::Codex {
                append(
                    &source,
                    &[json!({"type":"session_meta","payload":{"id":"thread"}})],
                );
            }
            append(&source, &log.prompt("outside", "External prompt"));
            // A stopped Codex writer can have persisted the event before the
            // completed item for the same answer reaches the log.
            let first_answer = if matches!(log, Log::CodexItems) {
                Log::CodexEvents
            } else {
                log
            };
            append(
                &source,
                &first_answer.answer("outside", "first", "Partial answer", false),
            );
            let database = Arc::new(std::sync::Mutex::new(owner));
            let first = sync(&database, session.id, root).unwrap();
            assert!(first.active);
            let token = first.recovery.unwrap();
            assert!(recover(&database, session.id, root, "wrong snapshot").is_err());
            assert!(sync(&database, session.id, root).unwrap().active);
            {
                let owner = locked(&database).unwrap();
                owner
                    .reserve_local_submission(
                        session.id,
                        SubmissionInput {
                            attempt_id: "local-compaction".into(),
                            user_turn_id: "local-compaction".into(),
                            session_id: String::new(),
                            payload_json: "{}".into(),
                            user_turn: None,
                        },
                    )
                    .unwrap();
            }
            assert!(sync(&database, session.id, root).unwrap().superseded);
            assert!(recover(&database, session.id, root, &token).is_err());
            locked(&database)
                .unwrap()
                .settle_local_submission("local-compaction", "failed", None)
                .unwrap();
            recover(&database, session.id, root, &token).unwrap();
            assert!(!sync(&database, session.id, root).unwrap().active);
            assert_eq!(
                saved_text(&database, session.id),
                ["External prompt", "Partial answer"]
            );
            assert!(recover(&database, session.id, root, &token).is_err());
            drop(database);

            let database = Arc::new(std::sync::Mutex::new(open()));
            assert!(!sync(&database, session.id, root).unwrap().active);
            {
                let owner = locked(&database).unwrap();
                assert_eq!(
                    owner
                        .bot_session(session.id)
                        .unwrap()
                        .unwrap()
                        .provider_id
                        .as_deref(),
                    Some("thread")
                );
                assert_eq!(
                    owner.composer_draft(session.id).unwrap().unwrap().payload,
                    draft
                );
            }
            assert_eq!(
                saved_text(&database, session.id),
                ["External prompt", "Partial answer"]
            );
            // A resumed external writer revokes the acknowledgement, even without
            // task_complete. The old UI prompt cannot release this newer state.
            if matches!(log, Log::CodexItems) {
                append(
                    &source,
                    &log.answer("outside", "first", "Partial answer", false),
                );
            }
            append(
                &source,
                &log.answer("outside", "second", "More external work", false),
            );
            let changed = sync(&database, session.id, root).unwrap();
            assert!(changed.active);
            let changed_token = changed.recovery.unwrap();
            assert_ne!(changed_token, token);
            assert!(recover(&database, session.id, root, &token).is_err());
            recover(&database, session.id, root, &changed_token).unwrap();
            let (ready, request) = sync_checked(&database, session.id, root).unwrap();
            assert!(!ready.active);
            {
                // Exercise managed admission and storage with a disposable
                // exchange. Recovery and the fixture make no provider request.
                let mut owner = locked(&database).unwrap();
                let reservation = owner
                    .reserve_local_submission_after_native_history(
                        session.id,
                        SubmissionInput {
                            attempt_id: "managed".into(),
                            user_turn_id: "managed".into(),
                            session_id: String::new(),
                            payload_json: "{}".into(),
                            user_turn: None,
                        },
                        request.as_ref(),
                    )
                    .unwrap();
                assert!(reservation.created);
                let mut transcript = owner.load(session.id).unwrap();
                let start = transcript.begin("user");
                transcript.append("Managed prompt");
                transcript.begin("assistant");
                transcript.append("Managed answer");
                owner
                    .write_rows(session.id, start, &transcript.rows[start..])
                    .unwrap();
                let mut settings = owner.agent_settings(session.id).unwrap().unwrap();
                settings.turns.insert(
                    start + 1,
                    TurnInfo {
                        provider_turn_id: Some("managed".into()),
                        provider: kind.id().into(),
                        ..Default::default()
                    },
                );
                owner.save_agent_settings(session.id, &settings).unwrap();
                owner
                    .settle_local_submission("managed", "completed", None)
                    .unwrap();
            }
            // The managed native boundary also closes the earlier unfinished turn.
            append(&source, &log.prompt("managed", "Managed prompt"));
            append(
                &source,
                &log.answer("managed", "managed-answer", "Managed answer", true),
            );
            let settled = sync(&database, session.id, root).unwrap();
            assert!(!settled.active);
            assert!(!settled.reload_provider);
            assert_eq!(settled.imported, 0);
            let expected = [
                "External prompt",
                "Partial answer",
                "More external work",
                "Managed prompt",
                "Managed answer",
            ];
            assert_eq!(saved_text(&database, session.id), expected);
            drop(database);

            let replacement = source.with_extension("replacement");
            std::fs::copy(&source, &replacement).unwrap();
            std::fs::rename(replacement, &source).unwrap();
            let database = Arc::new(std::sync::Mutex::new(open()));
            assert_eq!(sync(&database, session.id, root).unwrap().imported, 0);
            assert_eq!(saved_text(&database, session.id), expected);
        }
    }
}
