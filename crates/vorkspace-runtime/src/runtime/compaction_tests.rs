use super::*;
use serde_json::{Value, json};
use std::{
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};
use vorkspace_core::{
    agent::BotContext,
    bot::{self, CompactionMode, MinionMetadata, Roster},
    context::Usage,
    tool::{ToolCall, ToolState},
};

fn fixture(root: &Path) -> (Database, Session, Settings) {
    let store = Store::open_vorkspace(&root.join("sessions.db")).unwrap();
    let mut summary = store.create("codex", root.to_str().unwrap()).unwrap();
    store.provider_id(summary.id, "durable-bot-thread").unwrap();
    summary.provider_id = Some("durable-bot-thread".into());
    let bot = Roster::default().draft(vec![summary.cwd.clone()]);
    bot::files::save(
        root,
        &bot.id,
        None,
        "SOUL.md",
        "Respect the assigned scope.",
        None,
    )
    .unwrap();
    bot::files::save(
        root,
        &bot.id,
        None,
        "MEMORY.md",
        "Prefer small patches.",
        None,
    )
    .unwrap();
    let settings = Settings {
        bot: Some(BotContext {
            bot,
            root: root.into(),
            executable: root.join("vorkspace"),
        }),
        text_only: true,
        ..Default::default()
    };
    (Arc::new(Mutex::new(store)), summary, settings)
}

fn native_fixture(root: &Path) -> (Database, Session, Settings, PathBuf) {
    let (store, summary, mut settings) = fixture(root);
    settings.text_only = false;
    {
        let store = locked(&store).unwrap();
        let mut accounts = store.provider_accounts().unwrap();
        let account = accounts.add(Kind::Codex, "Native history fixture").unwrap();
        store.save_provider_accounts(&accounts).unwrap();
        settings.account = vorkspace_core::accounts::Profile::new(&account.id, root);
        store.save_agent_settings(summary.id, &settings).unwrap();
    }
    let home = settings.account.home(Kind::Codex).unwrap();
    std::fs::create_dir_all(home.join("sessions")).unwrap();
    let source = home.join("sessions/rollout-2026-11-06-durable-bot-thread.jsonl");
    append_native(
        &source,
        &[json!({"type":"session_meta","payload":{"id":"durable-bot-thread"}})],
    );
    (store, summary, settings, source)
}

fn append_native(source: &Path, records: &[Value]) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(source)
        .unwrap();
    for record in records {
        writeln!(file, "{record}").unwrap();
    }
}

fn external_turn(id: &str, prompt: &str, answer: &str) -> [Value; 4] {
    [
        json!({"type":"event_msg","payload":{"type":"task_started","turn_id":id}}),
        json!({"type":"event_msg","payload":{"type":"user_message","message":prompt}}),
        json!({"type":"event_msg","payload":{"type":"agent_message","message":answer}}),
        json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":id}}),
    ]
}

#[test]
fn external_native_messages_are_in_the_admitted_chat_before_the_next_managed_turn() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let (store, summary, settings, source) = native_fixture(root);
    append_native(
        &source,
        &external_turn("outside", "From the terminal", "Terminal answer"),
    );
    let binary = root.join("mock-provider");
    std::fs::write(&binary,r#"#!/usr/bin/env python3
import json, sys
for line in sys.stdin:
    value=json.loads(line)
    method=value.get('method')
    if method=='initialize':
        print(json.dumps({'id':value['id'],'result':{}}),flush=True)
    elif method in ('thread/start','thread/resume'):
        assert method=='thread/resume', value
        assert value['params']['threadId']=='durable-bot-thread', value
        print(json.dumps({'id':value['id'],'result':{'thread':{'id':'durable-bot-thread'}}}),flush=True)
    elif method=='turn/start':
        print(json.dumps({'id':value['id'],'result':{'turn':{'id':'managed'}}}),flush=True)
        print(json.dumps({'method':'item/agentMessage/delta','params':{'itemId':'answer','delta':'Managed answer'}}),flush=True)
        print(json.dumps({'method':'turn/completed','params':{'turn':{'status':'completed'}}}),flush=True)
        break
"#).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let (tx, rx) = async_channel::bounded(64);
    let (_control, controls) = std::sync::mpsc::channel();
    run_turn_using(
        TurnInput {
            composer_origin: None,
            summary: summary.clone(),
            prompt: "Back in Vorkspace".into(),
            settings,
        },
        controls,
        store.clone(),
        Arc::new(AtomicBool::new(false)),
        tx,
        |_| Some(binary),
    );
    let updates: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    let finished = updates.last().unwrap();
    assert_eq!(finished.status, "Ready");
    assert!(matches!(
        &finished.transcript,
        TranscriptPatch::BotSnapshot { turn_start: 2, .. }
    ));
    assert_eq!(finished.provider_turn_id.as_deref(), Some("managed"));
    let first = updates
        .iter()
        .find(|u| !u.transcript.rows().is_empty())
        .unwrap();
    assert!(matches!(
        &first.transcript,
        TranscriptPatch::BotSnapshot { turn_start: 2, .. }
    ));
    assert_eq!(first.transcript.rows()[0].text, "From the terminal");
    let store = locked(&store).unwrap();
    assert_eq!(
        store.bot_session(summary.id).unwrap().unwrap().provider_id,
        summary.provider_id
    );
    let rows = store.load(summary.id).unwrap().rows;
    assert_eq!(
        rows.iter()
            .filter(|r| !r.text.is_empty())
            .map(|r| r.text.as_str())
            .collect::<Vec<_>>(),
        [
            "From the terminal",
            "Terminal answer",
            "Back in Vorkspace",
            "Managed answer"
        ]
    );
}

#[test]
fn bot_final_snapshot_includes_imports_when_initial_publication_is_stopped_or_skipped() {
    for stopped in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let (store, summary, mut settings, source) = native_fixture(directory.path());
        let id = summary.id;
        let message = "managed-message";
        settings.bot_message_id = Some(message.into());
        let mut visible = Transcript::default();
        visible.begin("user");
        visible.append("Previous prompt");
        visible.begin("assistant");
        visible.append("Previous answer");
        let draft = json!({"text":"Unsent draft","cwd":summary.cwd,"selectionStart":0,"selectionEnd":0,"attachments":[],"mcpTags":[]});
        let queued = vorkspace_core::follow_up::Message {
            id: "queued-later".into(),
            text: "Retained queued input".into(),
            ..Default::default()
        };
        {
            let mut owner = locked(&store).unwrap();
            owner.write_rows(id, 0, &visible.rows).unwrap();
            owner.save_composer_draft(id, 0, &draft).unwrap();
            owner
                .save_follow_ups(
                    id,
                    &vorkspace_core::follow_up::State {
                        messages: vec![queued.clone()],
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        append_native(
            &source,
            &external_turn("outside", "Terminal prompt", "Terminal answer"),
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = async_channel::bounded(if stopped { 1 } else { 8 });
        if stopped {
            // Keep the channel full until the worker has processed Stop. The
            // first Bot snapshot cannot reach a receiver in this ordering.
            tx.try_send(Update {
                status: "Hold the channel".into(),
                ..Default::default()
            })
            .unwrap();
        }
        let worker = std::thread::spawn({
            let store = store.clone();
            let cancel = cancel.clone();
            let summary = summary.clone();
            move || {
                let (_control, controls) = std::sync::mpsc::channel();
                run_turn_using(
                    TurnInput {
                        composer_origin: None,
                        summary,
                        prompt: "Managed prompt".into(),
                        settings,
                    },
                    controls,
                    store,
                    cancel,
                    tx,
                    |_| {
                        assert!(
                            !stopped,
                            "Stop must reject the first publication before provider resolution"
                        );
                        None
                    },
                );
            }
        });
        if stopped {
            let delivered = format!("bot-delivered:{id}:{message}");
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                {
                    let owner = locked(&store).unwrap();
                    if owner.setting(&delivered).unwrap().is_some() {
                        cancel.store(true, Ordering::Relaxed);
                    }
                    if owner
                        .local_submissions(id)
                        .unwrap()
                        .iter()
                        .any(|receipt| receipt.phase == "settled")
                    {
                        break;
                    }
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "The blocked worker must settle after Stop"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(rx.try_recv().unwrap().status, "Hold the channel");
        }
        worker.join().unwrap();
        let first = rx.try_recv().unwrap();
        let finished = if stopped {
            first
        } else {
            assert!(!first.finished());
            rx.try_recv().unwrap()
        };
        assert!(finished.finished() && rx.is_empty());
        assert!(matches!(
            &finished.transcript,
            TranscriptPatch::BotSnapshot { turn_start: 4, .. }
        ));
        assert_eq!(finished.provider_id, summary.provider_id);
        assert_eq!(finished.delivered.as_deref(), Some(message));
        assert_eq!(
            finished.transcript.rows()[..visible.rows.len()],
            visible.rows
        );
        assert_eq!(finished.transcript.rows()[2].text, "Terminal prompt");
        assert_eq!(finished.transcript.rows()[3].text, "Terminal answer");
        assert_eq!(finished.transcript.rows()[4].text, "Managed prompt");
        if stopped {
            assert_eq!(
                finished.settlement().unwrap().outcome,
                TurnOutcome::Cancelled
            );
            assert_eq!(finished.status, "Ready");
        } else {
            assert_eq!(finished.settlement().unwrap().outcome, TurnOutcome::Failed);
            assert!(finished.status.contains("CLI not found"));
            assert_eq!(finished.transcript.rows().last().unwrap().role, "error");
        }
        let owner = locked(&store).unwrap();
        assert_eq!(owner.composer_draft(id).unwrap().unwrap().payload, draft);
        assert_eq!(owner.follow_ups(id).unwrap().messages, vec![queued]);
        assert_eq!(
            owner.local_submissions(id).unwrap()[0].outcome.as_deref(),
            Some("failed")
        );
    }
}

#[test]
fn manual_compaction_resumes_external_history_and_excludes_a_concurrent_import() {
    struct IdlePool;
    impl Drop for IdlePool {
        fn drop(&mut self) {
            provider::configure_idle_agents(0);
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let (store, summary, settings, source) = native_fixture(root);
    let binary = root.join("mock-provider");
    std::fs::write(&binary, r#"#!/usr/bin/env python3
import json, os, pathlib, sys
def send(message):
    print(json.dumps(message), flush=True)
source = pathlib.Path(os.environ['CODEX_HOME']) / 'sessions/rollout-2026-11-06-durable-bot-thread.jsonl'
fresh = 'From the terminal' in source.read_text()
with pathlib.Path('launches.txt').open('a') as launches:
    launches.write(str(os.getpid()) + '\n')
turn = 0
for line in sys.stdin:
    value = json.loads(line)
    method = value.get('method')
    if method == 'initialize':
        send({'id': value['id'], 'result': {}})
    elif method == 'thread/resume':
        assert value['params']['threadId'] == 'durable-bot-thread', value
        send({'id': value['id'], 'result': {'thread': {'id': 'durable-bot-thread'}}})
    elif method == 'turn/start':
        turn += 1
        identity = 'warm-' + str(turn)
        send({'id': value['id'], 'result': {'turn': {'id': identity}}})
        send({'method': 'turn/completed', 'params': {'turn': {'id': identity, 'status': 'completed'}}})
    elif method == 'thread/compact/start':
        assert fresh, 'Compaction used a process parked before the external turn'
        assert value['params']['threadId'] == 'durable-bot-thread', value
        send({'id': value['id'], 'result': {}})
        send({'method': 'turn/started', 'params': {'turn': {'id': 'compact'}}})
        send({'method': 'thread/tokenUsage/updated', 'params': {'tokenUsage': {
            'last': {'totalTokens': 2000}, 'modelContextWindow': 100000}}})
        if not pathlib.Path('cancel-mode').exists():
            send({'method': 'turn/completed', 'params': {'turn': {'id': 'compact', 'status': 'completed'}}})
    elif method == 'turn/interrupt':
        send({'id': value['id'], 'result': {}})
    elif method == 'thread/start':
        raise AssertionError('Compaction discarded the existing thread')
"#).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    provider::configure_idle_agents(5);
    let _idle_pool = IdlePool;
    let cancel = Arc::new(AtomicBool::new(false));
    vorkspace_core::process::owned_by(summary.id, || {
        let request = Request {
            kind: Kind::Codex,
            binary: binary.clone(),
            cwd: root.into(),
            prompt: "Warm the existing thread".into(),
            provider_id: summary.provider_id.clone(),
            model: settings.model.clone(),
            settings: settings.clone(),
        };
        for _ in 0..2 {
            provider::run(request.clone(), cancel.clone(), |_| Ok(())).unwrap();
        }
    });
    let launches = || {
        std::fs::read_to_string(root.join("launches.txt"))
            .unwrap()
            .lines()
            .count()
    };
    assert_eq!(
        launches(),
        1,
        "The fixture must actually reuse a parked process"
    );
    append_native(
        &source,
        &external_turn("outside", "From the terminal", "Terminal answer"),
    );

    let id = summary.id;
    let other = std::cell::RefCell::new(Store::open_vorkspace(&root.join("sessions.db")).unwrap());
    let pending = std::cell::RefCell::new(None);
    let result = manual_compaction::run_using(
        summary.clone(),
        settings.clone(),
        store.clone(),
        cancel,
        |started| {
            assert_eq!(started.rows[0].text, "From the terminal");
            assert_eq!(started.rows[1].text, "Terminal answer");
            assert_eq!(started.rows[2].role, "tool");
            assert_eq!(
                other.borrow().local_submissions(id).unwrap()[0].phase,
                "reserved"
            );
            append_native(
                &source,
                &external_turn("later", "Later terminal prompt", "Later answer"),
            );
            let request = other
                .borrow()
                .native_history_request(id, root)
                .unwrap()
                .unwrap();
            let batch =
                vorkspace_core::native_history::read(&request.binding, request.cursor.as_ref())
                    .unwrap()
                    .unwrap();
            assert!(batch.update.is_some());
            let receipt = vorkspace_core::store::SubmissionInput {
                attempt_id: "other-turn".into(),
                user_turn_id: "other-turn".into(),
                session_id: String::new(),
                payload_json: "{}".into(),
                user_turn: Some(json!({"id":"other-turn","role":"user","text":"another window"})),
            };
            assert!(
                other
                    .borrow()
                    .reserve_local_submission(id, receipt)
                    .is_err()
            );
            *pending.borrow_mut() = Some((request, batch));
        },
        |_| Some(binary.clone()),
        |request, cancel, emit| {
            provider::compact(request, cancel, |events| {
                if events
                    .iter()
                    .any(|event| matches!(event, Event::Context { .. }))
                {
                    let mut other = other.borrow_mut();
                    assert_eq!(other.local_submissions(id).unwrap()[0].phase, "dispatching");
                    let (mut request, batch) = pending.borrow_mut().take().unwrap();
                    assert!(
                        other
                            .import_native_history(&mut request, batch)
                            .unwrap()
                            .superseded
                    );
                    assert_eq!(other.load(id).unwrap().rows.len(), 3);
                }
                emit(events)
            })
        },
    );
    result.result.unwrap();
    assert_eq!(launches(), 2);
    assert!(pending.borrow().is_none());
    assert_eq!(
        result.context,
        Some(Usage {
            used: 2000,
            window: Some(100_000)
        })
    );
    let owner = locked(&store).unwrap();
    assert_eq!(
        owner.bot_session(id).unwrap().unwrap().provider_id,
        summary.provider_id
    );
    let rows = owner.load(id).unwrap().rows;
    let tool: ToolCall = serde_json::from_str(&rows[2].text).unwrap();
    assert_eq!(tool.state, ToolState::Completed);
    let receipts = owner.local_submissions(id).unwrap();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].attempt_id, tool.id);
    assert_eq!(receipts[0].outcome.as_deref(), Some("completed"));
    assert!(receipts[0].user_turn.is_none());
    drop(owner);
    assert_eq!(native_history::sync(&store, id, root).unwrap().imported, 1);
    let rows = locked(&store).unwrap().load(id).unwrap().rows;
    assert_eq!(rows[3].text, "Later terminal prompt");
    assert_eq!(rows[4].text, "Later answer");

    // A Stop can interrupt a live compaction without a terminal frame. The
    // child must be gone while this attempt still owns its durable claim.
    std::fs::write(root.join("cancel-mode"), "").unwrap();
    let stopped = manual_compaction::run_using(
        summary,
        settings,
        store.clone(),
        Arc::new(AtomicBool::new(false)),
        |_| {},
        |_| Some(binary),
        |request, cancel, emit| {
            let stop = cancel.clone();
            let mut process = None;
            let result = provider::compact(request, cancel, |events| {
                if events
                    .iter()
                    .any(|event| matches!(event, Event::Context { .. }))
                {
                    let pids = std::fs::read_to_string(root.join("launches.txt")).unwrap();
                    let pid = pids.lines().last().unwrap().parse().unwrap();
                    let identity = vorkspace_core::process::identity(pid).unwrap();
                    process = Some((pid, identity));
                    stop.store(true, Ordering::Relaxed);
                }
                emit(events)
            });
            let (pid, identity) = process.unwrap();
            assert_ne!(vorkspace_core::process::identity(pid), Some(identity));
            assert!(
                other
                    .borrow()
                    .local_submissions(id)
                    .unwrap()
                    .iter()
                    .any(|r| r.phase == "dispatching")
            );
            result
        },
    );
    assert_eq!(stopped.result.unwrap_err(), "Stopped");
    let owner = locked(&store).unwrap();
    let rows = owner.load(id).unwrap().rows;
    let tool: ToolCall = serde_json::from_str(&rows[5].text).unwrap();
    assert_eq!(tool.state, ToolState::Cancelled);
    let receipt = owner.latest_unconfirmed_submission(id).unwrap().unwrap();
    assert_eq!(receipt.attempt_id, tool.id);
    assert!(receipt.user_turn.is_none());
}

#[test]
fn native_cursor_advance_blocks_stale_bot_and_compaction_admission() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let (store, summary, settings, source) = native_fixture(root);
    let cancel = Arc::new(AtomicBool::new(false));
    let checked = native_history::before_turn(&store, &summary, &settings, &cancel)
        .unwrap()
        .unwrap();
    append_native(
        &source,
        &external_turn("unfinished", "New external prompt", "Reply")[..2],
    );
    let other = Arc::new(Mutex::new(
        Store::open_vorkspace(&root.join("sessions.db")).unwrap(),
    ));
    assert!(
        native_history::sync(&other, summary.id, root)
            .unwrap()
            .active
    );
    let mut submissions = submissions::Submissions::new(store.clone(), summary.id);
    assert!(
        submissions
            .reserve_initial("Stale prompt", &settings, Some(&checked), None)
            .is_err()
    );
    assert!(
        submissions
            .reserve_compaction("stale-compaction", Some(&checked))
            .is_err()
    );
    let rejected = manual_compaction::run_using(
        summary.clone(),
        settings,
        store.clone(),
        cancel,
        |_| panic!("An unfinished external turn cannot start compaction"),
        |_| panic!("An unfinished external turn cannot resolve a provider"),
        |_, _, _| panic!("An unfinished external turn cannot reach the provider"),
    );
    assert!(rejected.result.unwrap_err().contains("unfinished turn"));
    let owner = locked(&store).unwrap();
    assert!(owner.local_submissions(summary.id).unwrap().is_empty());
    assert!(owner.load(summary.id).unwrap().rows.is_empty());
}

#[test]
fn manual_compaction_does_not_publish_usage_after_its_binding_changes() {
    for change_thread in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let (store, summary, settings) = fixture(root);
        let replacement_usage = Usage {
            used: 50_000,
            window: Some(100_000),
        };
        let id = summary.id;
        let outcome = manual_compaction::run_using(
            summary.clone(),
            settings,
            store.clone(),
            Arc::new(AtomicBool::new(false)),
            |_| {},
            |_| Some("mock-provider".into()),
            |_, _, emit| {
                let other = Store::open_vorkspace(&root.join("sessions.db")).unwrap();
                if change_thread {
                    other.provider_id(id, "replacement-thread").unwrap();
                } else {
                    let mut changed = other.agent_settings(id).unwrap().unwrap();
                    changed.worktree_cwd = Some(root.join("new-worktree").to_str().unwrap().into());
                    other.save_agent_settings(id, &changed).unwrap();
                }
                other.save_context_usage(id, &replacement_usage).unwrap();
                emit(vec![Event::Context {
                    used: Some(2000),
                    window: Some(100_000),
                }])
            },
        );
        assert!(
            outcome
                .result
                .unwrap_err()
                .contains("conversation changed during compaction")
        );
        assert!(outcome.context.is_none());
        let owner = locked(&store).unwrap();
        assert_eq!(owner.context_usage(id).unwrap(), Some(replacement_usage));
        assert_eq!(
            owner.local_submissions(id).unwrap()[0].outcome.as_deref(),
            Some("completed")
        );
    }
}

fn prepare(
    store: &Database,
    summary: &Session,
    settings: &Settings,
) -> (String, BotContextReceipt) {
    let history = locked(store).unwrap().load(summary.id).unwrap();
    let mut receipt = None;
    let prompt = turn_prompt(
        "Continue the work.".into(),
        settings,
        TurnHistory {
            summary,
            earlier: &history.rows,
        },
        summary.provider_id.as_deref(),
        &mut receipt,
        store,
    )
    .unwrap();
    (prompt, receipt.unwrap())
}

fn remember(store: &Database, summary: &Session, receipt: &BotContextReceipt) {
    locked(store)
        .unwrap()
        .save_setting(
            &format!("bot-context:{}", summary.id),
            &receipt.encode(summary.provider_id.as_deref()),
        )
        .unwrap();
}

#[test]
fn rollover_resets_context_before_provider_start_and_keeps_it_reset_on_failure() {
    for already_detached in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let (store, mut summary, settings) = fixture(root);
        assert_eq!(
            settings.bot.as_ref().unwrap().bot.compaction_mode,
            CompactionMode::SessionRollover
        );
        {
            let store = locked(&store).unwrap();
            store
                .save_context_usage(
                    summary.id,
                    &Usage {
                        used: 95_000,
                        window: Some(100_000),
                    },
                )
                .unwrap();
            if already_detached {
                // Older versions could clear just the ID, leaving a red ring
                // for a provider thread the app would never resume.
                store.reset_provider_id(summary.id).unwrap();
                summary.provider_id = None;
            }
        }
        let (tx, rx) = async_channel::bounded(64);
        let (_controls, control_rx) = std::sync::mpsc::channel();
        run_turn_using(
            TurnInput {
                composer_origin: None,
                summary: summary.clone(),
                prompt: "Try to continue".into(),
                settings,
            },
            control_rx,
            store.clone(),
            Arc::new(AtomicBool::new(false)),
            tx,
            |_| {
                let reset = std::iter::from_fn(|| rx.try_recv().ok())
                    .find(|update| update.provider_reset)
                    .expect("reset must reach the view before provider startup");
                assert!(!reset.finished());
                assert_eq!(reset.provider_id, None);
                assert_eq!(reset.context, Some(Usage::default()));
                let store = locked(&store).unwrap();
                let saved = store.bot_session(summary.id).unwrap().unwrap();
                assert_eq!(saved.provider_id, None);
                assert_eq!(store.context_usage(summary.id).unwrap(), None);
                None
            },
        );
        let finished = std::iter::from_fn(|| rx.try_recv().ok()).last().unwrap();
        assert!(finished.finished() && finished.provider_reset);
        assert!(
            finished.status.contains("CLI not found"),
            "{}",
            finished.status
        );
        assert_eq!(finished.provider_id, None);
        assert_eq!(finished.context, Some(Usage::default()));
        drop(store);
        let reopened = Store::open_vorkspace(&root.join("sessions.db")).unwrap();
        let saved = reopened.bot_session(summary.id).unwrap().unwrap();
        assert_eq!(saved.provider_id, None);
        assert_eq!(reopened.context_usage(summary.id).unwrap(), None);
    }
}

#[test]
fn fresh_session_rollover_at_eighty_percent_and_provider_replacement_clear_old_usage() {
    for (used, expected_method, report_usage) in [
        (80_000, "thread/start", true),
        (79_999, "thread/resume", false),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let (store, summary, settings) = fixture(root);
        let mut prior = Transcript::default();
        for exchange in 0..13 {
            prior.begin(if exchange == 12 { "bot-system" } else { "user" });
            prior.append(&format!("Prior prompt {exchange}."));
            prior.begin("assistant");
            prior.append(&format!("Prior answer {exchange}."));
            if exchange == 12 {
                prior.append(&"🧭".repeat(4000));
                prior.append("Omitted reply suffix.");
            }
        }
        let saved_rows = {
            let mut store = locked(&store).unwrap();
            store.write_rows(summary.id, 0, &prior.rows).unwrap();
            store
                .save_context_usage(
                    summary.id,
                    &Usage {
                        used,
                        window: Some(100_000),
                    },
                )
                .unwrap();
            store.load(summary.id).unwrap().rows
        };
        let binary = root.join("mock-provider");
        let script = r#"#!/usr/bin/env python3
import json, pathlib, sys
def send(value):
    print(json.dumps(value), flush=True)
for line in sys.stdin:
    value = json.loads(line)
    method = value.get('method')
    if method == 'initialize':
        send({'id': value['id'], 'result': {}})
    elif method in ('thread/start', 'thread/resume'):
        assert method == '__EXPECTED_METHOD__', value
        send({'id': value['id'], 'result': {'thread': {'id': 'replacement-thread'}}})
    elif method == 'turn/start':
        assert value['params']['threadId'] == 'replacement-thread'
        pathlib.Path('received-prompt.txt').write_text(value['params']['input'][0]['text'])
        send({'id': value['id'], 'result': {'turn': {'id': 'turn'}}})
        if __REPORT_USAGE__:
            send({'method': 'thread/tokenUsage/updated', 'params': {'tokenUsage': {
                'last': {'totalTokens': 4200}, 'modelContextWindow': 272000}}})
        send({'method': 'item/agentMessage/delta', 'params': {'itemId': 'answer', 'delta': 'Continuity kept.'}})
        send({'method': 'turn/completed', 'params': {'turn': {'status': 'completed'}}})
        break
"#
        .replace("__EXPECTED_METHOD__", expected_method)
        .replace("__REPORT_USAGE__", if report_usage { "True" } else { "False" });
        std::fs::write(&binary, script).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (tx, rx) = async_channel::bounded(64);
        let (_controls, control_rx) = std::sync::mpsc::channel();
        run_turn_using(
            TurnInput {
                composer_origin: None,
                summary: summary.clone(),
                prompt: "Keep working.".into(),
                settings,
            },
            control_rx,
            store.clone(),
            Arc::new(AtomicBool::new(false)),
            tx,
            |_| Some(binary),
        );
        let updates: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        let finished = updates.last().unwrap();
        assert!(finished.finished() && finished.provider_reset);
        assert_eq!(finished.status, "Ready");
        assert_eq!(finished.provider_id.as_deref(), Some("replacement-thread"));
        let new_usage = report_usage.then_some(Usage {
            used: 4200,
            window: Some(272_000),
        });
        assert_eq!(finished.context, Some(new_usage.unwrap_or_default()));
        assert!(
            updates[..updates.len() - 1].iter().any(|update| {
                update.provider_reset && update.context == Some(Usage::default())
            })
        );

        let prompt = std::fs::read_to_string(root.join("received-prompt.txt")).unwrap();
        assert!(prompt.contains("Respect the assigned scope."));
        assert!(prompt.ends_with("Keep working."));
        if report_usage {
            assert!(prompt.contains("Your provider context was rotated"));
            assert!(prompt.contains("Prior prompt 1."));
            assert!(prompt.contains("Prior prompt 12."));
            assert!(!prompt.contains("Prior prompt 0."));
            assert!(!prompt.contains("Omitted reply suffix."));
        } else {
            assert!(!prompt.contains("Your provider context was rotated"));
        }
        let store = locked(&store).unwrap();
        assert_eq!(store.context_usage(summary.id).unwrap(), new_usage);
        let saved = store.bot_session(summary.id).unwrap().unwrap();
        assert_eq!(saved.provider_id.as_deref(), Some("replacement-thread"));
        let rows = store.load(summary.id).unwrap().rows;
        assert_eq!(&rows[..saved_rows.len()], saved_rows.as_slice());
        assert_eq!(rows.last().unwrap().text, "Continuity kept.");
    }
}

#[test]
fn native_compaction_preserves_the_thread_and_refreshes_context_after_restart() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let (store, summary, mut settings) = fixture(root);
    settings.bot.as_mut().unwrap().bot.compaction_mode = CompactionMode::Native;
    let bot_context = settings.bot.as_ref().unwrap();
    let mut initial_pointer = summary.clone();
    initial_pointer.provider_id = None;
    bot::home::write_session(root, &bot_context.bot, &initial_pointer, &settings.account).unwrap();
    locked(&store)
        .unwrap()
        .save_context_usage(
            summary.id,
            &vorkspace_core::context::Usage {
                used: 95_000,
                window: Some(100_000),
            },
        )
        .unwrap();
    let (initial, receipt) = prepare(&store, &summary, &settings);
    assert!(initial.contains("Prefer small patches."));
    remember(&store, &summary, &receipt);
    let binary = root.join("mock-provider");
    std::fs::write(&binary, r#"#!/usr/bin/env python3
import json, pathlib, sys
def send(value):
    print(json.dumps(value), flush=True)
for line in sys.stdin:
    value = json.loads(line)
    method = value.get('method')
    if method == 'initialize':
        send({'id': value['id'], 'result': {}})
    elif method == 'thread/start':
        raise AssertionError('Existing Bot context was discarded')
    elif method == 'thread/resume':
        assert value['params']['threadId'] == 'durable-bot-thread'
        send({'id': value['id'], 'result': {'thread': {'id': 'durable-bot-thread'}}})
    elif method == 'turn/start':
        path = pathlib.Path('received-prompts.jsonl')
        first = not path.exists()
        with path.open('a') as out:
            out.write(json.dumps(value['params']['input']) + '\n')
        send({'id': value['id'], 'result': {'turn': {'id': 'turn'}}})
        if first:
            for phase, status in [('started', 'inProgress'), ('completed', 'completed')]:
                send({'method': 'item/' + phase, 'params': {'item': {
                    'id': 'compact-1', 'type': 'contextCompaction', 'status': status}}})
        send({'method': 'item/agentMessage/delta', 'params': {'itemId': 'answer', 'delta': 'Work preserved.'}})
        send({'method': 'turn/completed', 'params': {'turn': {'status': 'completed'}}})
        break
"#).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut saved_rows = Vec::new();
    for turn in 0..4 {
        let pointer_path = root.join(&bot_context.bot.id).join("SESSION.md");
        if turn == 2 {
            // A session-pointer write failure must not turn successful work
            // into a failed Bot delivery that could be retried.
            std::fs::remove_file(&pointer_path).unwrap();
            std::fs::create_dir(&pointer_path).unwrap();
        }
        if turn == 1 {
            // A process restart must retain both the compaction and its stale receipt.
            *locked(&store).unwrap() = Store::open_vorkspace(&root.join("sessions.db")).unwrap();
        }
        if turn == 3 {
            bot::files::save(
                root,
                &settings.bot.as_ref().unwrap().bot.id,
                None,
                "MEMORY.md",
                "Use the new release branch. Source: project decision.",
                None,
            )
            .unwrap();
        }
        let (tx, rx) = async_channel::bounded(64);
        let (_controls, control_rx) = std::sync::mpsc::channel();
        run_turn_using(
            TurnInput {
                composer_origin: None,
                summary: summary.clone(),
                prompt: format!("Work {turn}"),
                settings: settings.clone(),
            },
            control_rx,
            store.clone(),
            Arc::new(AtomicBool::new(false)),
            tx,
            |_| Some(binary.clone()),
        );
        let updates: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        let finished = updates.last().unwrap();
        assert!(finished.finished());
        assert_eq!(finished.status, "Ready");
        assert_eq!(finished.provider_id, summary.provider_id);
        if turn == 2 {
            std::fs::remove_dir(&pointer_path).unwrap();
            bot::home::write_session(root, &bot_context.bot, &summary, &settings.account).unwrap();
        }
        let pointer = std::fs::read_to_string(&pointer_path).unwrap();
        assert!(pointer.contains("-- durable-bot-thread"));
        let store = locked(&store).unwrap();
        assert_eq!(
            store.sessions().unwrap()[0].provider_id,
            summary.provider_id
        );
        let rows = store.load(summary.id).unwrap().rows;
        assert_eq!(&rows[..saved_rows.len()], saved_rows.as_slice());
        saved_rows = rows;
    }
    let prompts: Vec<serde_json::Value> =
        std::fs::read_to_string(root.join("received-prompts.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    let text = |index: usize| prompts[index][0]["text"].as_str().unwrap();
    assert!(
        !text(0).contains("<soul>"),
        "95% usage must not rotate or reintroduce context"
    );
    assert!(text(1).contains("Respect the assigned scope."));
    assert!(text(1).contains("Prefer small patches."));
    assert!(
        !text(2).contains("<soul>"),
        "Unchanged context should not repeat on every turn"
    );
    assert!(text(3).contains("Use the new release branch."));
    assert!(!text(3).contains("Prefer small patches."));
    assert!(saved_rows.iter().any(|row| row.text.contains("compact-1")));
    assert_eq!(
        saved_rows
            .iter()
            .filter(|row| row.text == "Work preserved.")
            .count(),
        4
    );
}

#[test]
fn manual_and_uncertain_compactions_refresh_bot_and_minion_context_without_consuming_input() {
    for minion in [false, true] {
        for state in [
            ToolState::Completed,
            ToolState::Failed,
            ToolState::Cancelled,
            ToolState::Running,
        ] {
            let directory = tempfile::tempdir().unwrap();
            let (store, summary, mut settings) = fixture(directory.path());
            settings.bot.as_mut().unwrap().bot.compaction_mode = CompactionMode::Native;
            if minion {
                let actor = &mut settings.bot.as_mut().unwrap().bot;
                let parent_id = std::mem::replace(&mut actor.id, "helper".into());
                actor.name = "Helper".into();
                actor.minion = Some(MinionMetadata {
                    parent_id,
                    parent_name: "Parent".into(),
                    role: "Reviewer".into(),
                    ..Default::default()
                });
            }
            let (_, receipt) = prepare(&store, &summary, &settings);
            remember(&store, &summary, &receipt);
            let pending = vorkspace_core::follow_up::Message {
                id: "queued".into(),
                text: "Pending user work".into(),
                ..Default::default()
            };
            let draft = vorkspace_core::follow_up::Message {
                id: "draft".into(),
                text: "Unsent text".into(),
                ..Default::default()
            };
            let mut tool = ToolCall::new("manual-compaction-1", "contextCompaction");
            tool.state = state;
            let row = Row {
                role: "tool".into(),
                text: serde_json::to_string(&tool).unwrap(),
                first: true,
                sent_at: None,
            };
            {
                let mut store = locked(&store).unwrap();
                store
                    .write_rows(summary.id, 0, std::slice::from_ref(&row))
                    .unwrap();
                store
                    .save_follow_ups(
                        summary.id,
                        &vorkspace_core::follow_up::State {
                            messages: vec![pending.clone()],
                            draft: Some(draft.clone()),
                            ..Default::default()
                        },
                    )
                    .unwrap();
            }
            // Shared storage enriches tool presentation on save. Compare the
            // durable rows before and after preparation, including that metadata.
            let saved = locked(&store).unwrap().load(summary.id).unwrap().rows;
            let (prompt, receipt) = prepare(&store, &summary, &settings);
            assert!(prompt.contains("Respect the assigned scope."));
            assert!(prompt.contains("Prefer small patches."));
            assert!(prompt.ends_with("Continue the work."));
            remember(&store, &summary, &receipt);
            assert_eq!(prepare(&store, &summary, &settings).0, "Continue the work.");
            let store = locked(&store).unwrap();
            assert_eq!(
                store.sessions().unwrap()[0].provider_id,
                summary.provider_id
            );
            assert_eq!(store.load(summary.id).unwrap().rows, saved);
            let queued = store.follow_ups(summary.id).unwrap();
            assert_eq!(queued.messages, vec![pending]);
            assert_eq!(queued.draft, Some(draft));
        }
    }
}
