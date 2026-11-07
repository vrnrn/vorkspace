use super::*;
use serde_json::json;
use std::io::Write;

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

#[test]
fn unfinished_external_turn_recovery_is_explicit_durable_and_bound_to_its_source() {
    for shared in [false, true] {
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
        let session = owner.create("codex", root.to_str().unwrap()).unwrap();
        owner.provider_id(session.id, "thread").unwrap();
        let settings = Settings {
            account: vorkspace_core::accounts::Profile::new("fixture", root),
            ..Default::default()
        };
        owner.save_agent_settings(session.id, &settings).unwrap();
        let draft = json!({"text":"My unsent draft","cwd":session.cwd,"selectionStart":0,"selectionEnd":0,"attachments":[],"mcpTags":[]});
        owner.save_composer_draft(session.id, 0, &draft).unwrap();
        let home = settings.account.home(Kind::Codex).unwrap();
        std::fs::create_dir_all(home.join("sessions")).unwrap();
        let source = home.join("sessions/rollout-2026-11-06-thread.jsonl");
        append(
            &source,
            &[
                json!({"type":"session_meta","payload":{"id":"thread"}}),
                json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"outside"}}),
                json!({"type":"event_msg","payload":{"type":"user_message","message":"External prompt"}}),
            ],
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
                    vorkspace_core::store::SubmissionInput {
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
            assert!(
                owner.load(session.id).unwrap().rows.is_empty(),
                "recovery must not replay input"
            );
        }
        // A resumed external writer revokes the acknowledgement, even without
        // task_complete. The old UI prompt cannot release this newer state.
        append(
            &source,
            &[
                json!({"type":"event_msg","payload":{"type":"agent_message","message":"More external work"}}),
            ],
        );
        let changed = sync(&database, session.id, root).unwrap();
        assert!(changed.active);
        let changed_token = changed.recovery.unwrap();
        assert_ne!(changed_token, token);
        assert!(recover(&database, session.id, root, &token).is_err());
        recover(&database, session.id, root, &changed_token).unwrap();
        // The next real turn boundary retains the interrupted exchange once.
        append(
            &source,
            &[
                json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"next"}}),
                json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"next"}}),
            ],
        );
        assert!(!sync(&database, session.id, root).unwrap().active);
        assert!(!sync(&database, session.id, root).unwrap().active);
        let rows = locked(&database).unwrap().load(session.id).unwrap().rows;
        assert_eq!(
            rows.iter()
                .filter(|row| row.text == "External prompt")
                .count(),
            1
        );
        assert_eq!(
            rows.iter()
                .filter(|row| row.text == "More external work")
                .count(),
            1
        );
    }
}
