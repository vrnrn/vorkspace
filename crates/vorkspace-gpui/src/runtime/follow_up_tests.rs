use super::*;
use std::{os::unix::fs::PermissionsExt, path::PathBuf};
use vorkspace_core::{agent::Decision, follow_up::Message};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn accepted_steer_publishes_its_working_receipt_before_provider_output() {
    let fixture = Fixture(std::env::temp_dir().join(format!(
        "vorkspace-runtime-steer-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )));
    std::fs::create_dir_all(&fixture.0).unwrap();
    let binary = fixture.0.join("mock-provider");
    std::fs::write(
        &binary,
        r#"#!/usr/bin/env python3
import json, os, sys, time
def send(value):
    print(json.dumps(value), flush=True)
for line in sys.stdin:
    value = json.loads(line)
    method = value.get('method')
    if method == 'initialize':
        send({'id': value['id'], 'result': {}})
    elif method == 'thread/start':
        send({'id': value['id'], 'result': {'thread': {'id': 'thread'}}})
    elif method == 'turn/start':
        send({'id': value['id'], 'result': {'turn': {'id': 'turn'}}})
        send({'method': 'item/started', 'params': {'item': {'id': 'tool', 'type': 'commandExecution', 'command': 'cargo check', 'status': 'inProgress'}}})
        send({'method': 'item/completed', 'params': {'item': {'id': 'tool', 'type': 'commandExecution', 'command': 'cargo check', 'status': 'completed', 'aggregatedOutput': 'Checked'}}})
        send({'method': 'item/agentMessage/delta', 'params': {'itemId': 'before', 'delta': 'Before'}})
    elif method == 'turn/steer':
        send({'id': value['id'], 'result': {}})
        # Keep the provider silent until the UI has received the receipt.
        deadline = time.monotonic() + 4
        while not os.path.exists('receipt-observed'):
            assert time.monotonic() < deadline, 'Working receipt never arrived'
            time.sleep(0.01)
        send({'method': 'item/agentMessage/delta', 'params': {'itemId': 'after', 'delta': 'After'}})
        send({'method': 'turn/completed', 'params': {'turn': {'status': 'completed'}}})
        break
"#,
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = Store::open_vorkspace(&fixture.0.join("sessions.db")).unwrap();
    let summary = store.create("codex", fixture.0.to_str().unwrap()).unwrap();
    let session = summary.id;
    let message = Message {
        id: "steer".into(),
        text: "Change direction\nwith more detail".into(),
        ..Default::default()
    };
    store
        .save_follow_ups(
            session,
            &vorkspace_core::follow_up::State {
                messages: vec![message.clone()],
                ..Default::default()
            },
        )
        .unwrap();
    let store = Arc::new(Mutex::new(store));
    let worker_store = store.clone();
    let (updates, receive) = async_channel::bounded(64);
    let (controls, control_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        run_turn_using(
            TurnInput {
                summary,
                prompt: "Start working".into(),
                settings: Settings {
                    text_only: true,
                    turn_model: Some(vorkspace_core::agent::TurnInfo {
                        provider: "codex".into(),
                        model: "Chosen Model".into(),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            },
            control_rx,
            worker_store,
            Arc::new(AtomicBool::new(false)),
            updates,
            |_| Some(binary),
        );
    });
    let mut receipt_seen = false;
    let mut tool_seen = false;
    let final_status = loop {
        let update = receive
            .recv_blocking()
            .expect("Runtime closed before its final update");
        if update.rows.iter().any(|row| row.role == "tool") && !receipt_seen {
            assert_eq!(update.activity.as_deref(), Some("Run cargo check"));
            tool_seen = true;
        }
        if update.turn_ready {
            controls
                .send(Control {
                    id: message.id.clone(),
                    decision: Decision::Steer(message.clone()),
                })
                .unwrap();
        }
        if !update.finished && update.follow_up_delivered.contains(&message.id) {
            assert!(
                update.activity.is_none(),
                "a delivered steer resets latest tool activity"
            );
            let prompt = update.rows.iter().position(Row::is_prompt).unwrap();
            let response = update.rows.last().unwrap();
            assert_eq!(response.role, "assistant");
            assert!(response.first && response.text.is_empty());
            assert!(prompt < update.rows.len() - 1);
            let store = locked(&store).unwrap();
            assert!(store.follow_ups(session).unwrap().messages.is_empty());
            let receipts = store.local_submissions(session).unwrap();
            assert_eq!(receipts.len(), 2);
            let steer = receipts
                .iter()
                .find(|r| r.user_turn_id == message.id)
                .unwrap();
            assert_eq!(steer.outcome.as_deref(), Some("completed"));
            assert!(receipts.iter().any(|r| r.phase == "dispatching"));
            let saved = store.load(session).unwrap();
            assert_eq!(saved.rows.last().unwrap(), response);
            let turn = saved.rows.len() - 1;
            let settings = store.agent_settings(session).unwrap().unwrap();
            assert_eq!(settings.turns[&turn].provider, "codex");
            assert_eq!(settings.turns[&turn].model, "Chosen Model");
            assert!(!settings.turns[&turn].has_duration());
            receipt_seen = true;
            std::fs::write(fixture.0.join("receipt-observed"), "ready").unwrap();
        }
        if update.finished {
            break update.status;
        }
    };
    worker.join().unwrap();
    assert_eq!(final_status, "Ready");
    assert!(tool_seen);
    assert!(receipt_seen);
    assert!(
        locked(&store)
            .unwrap()
            .local_submissions(session)
            .unwrap()
            .iter()
            .all(|r| r.outcome.as_deref() == Some("completed"))
    );
    let saved = locked(&store).unwrap().load(session).unwrap();
    let after = saved.rows.iter().find(|row| row.text == "After").unwrap();
    assert_eq!(after.role, "assistant");
    assert_eq!(saved.rows.last(), Some(after));
}

#[test]
fn provider_disconnect_keeps_an_uncertain_receipt_and_blocks_queue_replay_before_spawn() {
    let directory = tempfile::tempdir().unwrap();
    let binary = directory.path().join("disconnect-provider");
    std::fs::write(&binary,r#"#!/usr/bin/env python3
import json,sys
for line in sys.stdin:
    value=json.loads(line)
    if value.get('method')=='initialize':
        print(json.dumps({'id':value['id'],'result':{}}),flush=True)
    elif value.get('method')=='thread/start':
        print(json.dumps({'id':value['id'],'result':{'thread':{'id':'disconnect-thread'}}}),flush=True)
    elif value.get('method')=='turn/start':
        open('provider-received','w').write('accepted input')
        break
"#).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = Store::open_vorkspace(&directory.path().join("sessions.db")).unwrap();
    let summary = store
        .create("codex", directory.path().to_str().unwrap())
        .unwrap();
    let id = summary.id;
    let db = Arc::new(Mutex::new(store));
    for retry in [false, true] {
        let (tx, rx) = async_channel::bounded(64);
        let (_controls, control_rx) = std::sync::mpsc::channel();
        let binary = binary.clone();
        run_turn_using(
            TurnInput {
                summary: summary.clone(),
                prompt: "Saved request".into(),
                settings: Settings {
                    bot_message_id: Some("stable-queued-message".into()),
                    text_only: true,
                    ..Default::default()
                },
            },
            control_rx,
            db.clone(),
            Arc::new(AtomicBool::new(false)),
            tx,
            |_| Some(binary),
        );
        let updates: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(updates.last().unwrap().finished);
        if retry {
            assert!(updates.last().unwrap().status.contains("not replayed"));
            assert!(!directory.path().join("provider-received").exists());
        } else {
            assert!(directory.path().join("provider-received").exists());
            std::fs::remove_file(directory.path().join("provider-received")).unwrap();
        }
        let store = locked(&db).unwrap();
        let receipt = store.latest_unconfirmed_submission(id).unwrap().unwrap();
        assert_eq!(receipt.user_turn.unwrap()["text"], "Saved request");
        assert_eq!(store.local_submissions(id).unwrap().len(), 1);
    }
}
