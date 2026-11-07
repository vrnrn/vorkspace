use super::*;
use std::{os::unix::fs::PermissionsExt, path::Path};
use vorkspace_core::{
    agent::{BotContext, MinionContext},
    bot::{self, CompactionMode, Roster},
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

#[test]
fn external_native_messages_are_in_the_admitted_chat_before_the_next_managed_turn() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
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
    let native = [
        serde_json::json!({"type":"session_meta","payload":{"id":"durable-bot-thread"}}),
        serde_json::json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"outside"}}),
        serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":"From the terminal"}}),
        serde_json::json!({"type":"event_msg","payload":{"type":"agent_message","message":"Terminal answer"}}),
        serde_json::json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"outside"}}),
    ];
    std::fs::write(
        source,
        native.iter().map(|v| format!("{v}\n")).collect::<String>(),
    )
    .unwrap();
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
    assert_eq!(updates.last().unwrap().status, "Ready");
    let first = updates.iter().find(|u| !u.rows.is_empty()).unwrap();
    assert_eq!(first.start, 0);
    assert_eq!(first.rows[0].text, "From the terminal");
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
                assert!(!reset.finished);
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
        assert!(finished.finished && finished.provider_reset);
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
        assert!(finished.finished && finished.provider_reset);
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
        assert!(finished.finished);
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
                let bot = settings.bot.take().unwrap();
                settings.minion = Some(MinionContext {
                    bot: bot.bot,
                    name: "Helper".into(),
                    root: bot.root,
                    executable: bot.executable,
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
                store.write_rows(summary.id, 0, &[row.clone()]).unwrap();
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
