use super::*;
use habits::{Habit, Schedule};
use std::sync::atomic::{AtomicBool, Ordering};

fn named(roster: &Roster, name: &str, projects: Vec<String>) -> Bot {
    let mut bot = roster.draft(projects);
    bot.name = name.into();
    bot
}

#[test]
fn setup_does_not_create_an_identity_until_a_nonblank_name_is_confirmed() {
    let mut roster = Roster::default();
    let mut draft = roster.draft(vec!["/project".into()]);
    assert!(roster.bots.is_empty());
    assert!(roster.create(draft.clone()).is_err());
    draft.name = " \n\t ".into();
    assert!(roster.create(draft.clone()).is_err());
    assert!(roster.bots.is_empty());
    draft.name = "  Ada 🧭  ".into();
    let id = draft.id.clone();
    assert_eq!(roster.create(draft.clone()).unwrap().name, "Ada 🧭");
    assert!(roster.create(draft).is_err());
    assert_eq!(roster.bots.len(), 1);
    assert_eq!(roster.bots[0].id, id);
}

#[test]
fn identity_projects_order_and_hidden_sessions_survive_restart() {
    let temp = tempfile::tempdir().unwrap();
    let db = Store::open(&temp.path().join("native.sqlite")).unwrap();
    let mut roster = Roster::default();
    let a = roster
        .create(named(
            &roster,
            "Reviewer",
            vec!["/one/app".into(), "/two/app".into()],
        ))
        .unwrap()
        .id
        .clone();
    let other = roster
        .create(named(&roster, "Writer", vec!["/else".into()]))
        .unwrap()
        .id
        .clone();
    let b = roster
        .create(named(&roster, "Builder", vec!["/one/app".into()]))
        .unwrap()
        .id
        .clone();
    assert!(roster.get(&a).unwrap().project(Some("app")).is_err());
    assert!(roster.get(&a).unwrap().project(Some("/outside")).is_err());
    assert!(roster.get(&a).unwrap().project(None).is_err());
    assert_eq!(roster.get(&b).unwrap().project(None).unwrap(), "/one/app");
    roster.reorder(&[b.clone(), a.clone()]);
    roster.hidden_sessions.push(17);
    roster.save(&db).unwrap();
    let saved = Roster::load(&db).unwrap();
    assert_eq!(
        saved.bots.iter().map(|m| &m.id).collect::<Vec<_>>(),
        vec![&b, &other, &a]
    );
    assert_eq!(saved.hidden_sessions, vec![17]);
    assert_ne!(saved.bots[0].mascot, saved.bots[1].mascot);
}
#[test]
fn memory_supersedes_uniquely_redacts_and_archives_before_prompt_budget() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let id = "memory-test";
    let secret = "sk-1234567890abcdefghijklmnop";
    let entry = memory::entry(&format!("Use Rust. token={secret}"), "2026-10-19", None).unwrap();
    assert!(!entry.contains(secret));
    let first = memory::add("# User notes\n", &entry);
    assert_eq!(memory::add(&first, &entry), first);
    let new = memory::entry("Use GPUI", "2026-10-19", None).unwrap();
    let replaced = memory::replace(&first, "Use Rust", Some(&new), "2026-10-19").unwrap();
    assert!(replaced.contains("~~"));
    assert!(memory::replace(&replaced, "not present", None, "2026-10-19").is_err());
    let mut many = "# User notes\nDo not drop this user-written line.\n".to_string();
    for n in 0..205 {
        many.push_str(&memory::entry(&format!("fact number {n}"), "2026-10-18", None).unwrap());
        many.push('\n');
    }
    memory::edit(root, id, None, |_| Ok(many.clone())).unwrap();
    let file = memory::read(root, id, None).unwrap().text.unwrap();
    assert!(file.lines().count() <= 200);
    assert!(file.contains("Do not drop"));
    assert!(file.contains("fact number 204"));
    assert!(files::read(root, id, None, "memory/archive.md")
        .unwrap()
        .text
        .unwrap()
        .contains("fact number 0"));
    assert_eq!(
        memory::search(root, id, "fact number 204", None)
            .unwrap()
            .len(),
        1
    );
    assert!(memory::topic("../secret").is_err());
}
#[test]
fn habit_updates_retry_conflicts_without_losing_other_edits() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let id = "habit-test";
    let now = 1_792_411_200_000;
    let a = Habit::new(
        "A".into(),
        "Report changes".into(),
        Schedule::default(),
        now,
    )
    .unwrap();
    let b = Habit::new("B".into(), "Check builds".into(), Schedule::default(), now).unwrap();
    let inject = AtomicBool::new(true);
    habits::update(root, id, |all| {
        if inject.swap(false, Ordering::SeqCst) {
            habits::update(root, id, |other| {
                other.push(b.clone());
                Ok(())
            })?;
        }
        all.push(a.clone());
        Ok(())
    })
    .unwrap();
    let all = habits::load(root, id).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].id, b.id);
    assert_eq!(all[1].id, a.id);
    let mut h = a;
    h.enabled = false;
    h.next_run_at = now;
    assert!(!h.due(now));
    h.run_requested = true;
    assert!(h.due(now));
    h.running_since = Some(now);
    assert!(!h.due(now));
    assert!(habits::report("`NOTHING_TO_REPORT`").is_none());
    assert!(habits::report("The build failed.\nNOTHING_TO_REPORT")
        .unwrap()
        .contains("build failed"));
}
#[test]
fn schedules_are_strictly_future_and_reject_invalid_fields() {
    let now = 1_792_411_200_000;
    for kind in ["hourly", "daily", "weekdays", "weekly"] {
        let s = Schedule {
            schedule_kind: kind.into(),
            ..Default::default()
        };
        let next = s.next(now).unwrap();
        assert!(next > now);
        assert!(next - now <= 8 * 86400000);
        assert!(s.next(next).unwrap() > next);
    }
    assert!(Schedule {
        time: "25:00".into(),
        ..Default::default()
    }
    .validate()
    .is_err());
    assert!(Schedule {
        minute: 60,
        ..Default::default()
    }
    .validate()
    .is_err());
    let s: Schedule = serde_json::from_str(r#"{"kind":"hourly","minute":15}"#).unwrap();
    assert_eq!(s.minute, 15);
}
#[test]
fn delivery_receipt_is_saved_with_the_user_message() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("native.sqlite");
    let mut db = Store::open(&path).unwrap();
    let session = db.create("codex", "/project").unwrap();
    let row = Row {
        role: "user".into(),
        text: "hello".into(),
        first: true,
        sent_at: None,
    };
    db.write_rows_with_message(session.id, 0, std::slice::from_ref(&row), Some("message-1"))
        .unwrap();
    drop(db);
    let db = Store::open(&path).unwrap();
    assert_eq!(db.load(session.id).unwrap().rows, vec![row]);
    assert_eq!(
        db.setting(&format!("bot-delivered:{}:message-1", session.id))
            .unwrap()
            .as_deref(),
        Some("1")
    );
}

#[test]
fn confirmed_setup_configuration_survives_reopen_until_a_session_takes_ownership() {
    let directory = tempfile::tempdir().unwrap();
    let db = Store::open(&directory.path().join("native.sqlite")).unwrap();
    let mut roster = Roster::default();
    let mut bot = named(&roster, "Ada", Vec::new());
    bot.initial_configuration = Some(Box::new(InitialConfiguration {
        provider: "claude".into(),
        settings: crate::agent::Settings {
            model: Some("selected-model".into()),
            effort: Some("high".into()),
            access: crate::agent::AccessMode::ReadOnly,
            ..Default::default()
        },
        soul: Some("# Soul\nKeep replies concise.\n".into()),
    }));
    let id = bot.id.clone();
    roster.create(bot).unwrap();
    roster.save(&db).unwrap();
    drop(db);
    let db = Store::open(&directory.path().join("native.sqlite")).unwrap();
    let roster = Roster::load(&db).unwrap();
    let bot = roster.get(&id).unwrap();
    assert_eq!(bot.name, "Ada");
    assert_eq!(bot.session_id, None);
    let config = bot.initial_configuration.as_ref().unwrap();
    assert_eq!(config.provider, "claude");
    assert_eq!(config.settings.model.as_deref(), Some("selected-model"));
    assert_eq!(config.settings.effort.as_deref(), Some("high"));
    assert_eq!(config.settings.access, crate::agent::AccessMode::ReadOnly);
    let files = load_files(directory.path(), bot).unwrap();
    assert_eq!(files.soul.as_deref(), config.soul.as_deref());
    // Reopening while setup is pending preserves already-published file edits.
    files::save(
        directory.path(),
        &id,
        None,
        "SOUL.md",
        "Newer soul",
        Some(&files.soul_hash),
    )
    .unwrap();
    files::save(
        directory.path(),
        &id,
        None,
        "MEMORY.md",
        "Newer memory",
        Some(&files.memory_hash),
    )
    .unwrap();
    let files = load_files(directory.path(), bot).unwrap();
    assert_eq!(files.soul.as_deref(), Some("Newer soul"));
    assert_eq!(files.memory, "Newer memory");
}

#[test]
fn sidecar_and_roster_merge_stale_writes_without_erasing_other_settings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.db");
    let first = Store::open_vorkspace(&path).unwrap();
    let mut roster = Roster::default();
    roster.bots.push(Bot {
        id: "bot".into(),
        name: "Bot".into(),
        ..Default::default()
    });
    roster.save(&first).unwrap();
    let second = Store::open_vorkspace(&path).unwrap();
    let mut a = Roster::load(&first).unwrap();
    let mut b = Roster::load(&second).unwrap();
    first.save_setting("first-only", "preserve").unwrap();
    second.save_setting("second-only", "preserve").unwrap();
    a.get_mut("bot").unwrap().outbox.push(Message {
        id: "first".into(),
        text: "one".into(),
        peer: None,
        sent_at: None,
        internal: false,
        error: None,
        attachments: vec![],
    });
    b.get_mut("bot").unwrap().outbox.push(Message {
        id: "second".into(),
        text: "two".into(),
        peer: None,
        sent_at: None,
        internal: false,
        error: None,
        attachments: vec![],
    });
    a.held_messages.insert("first".into());
    a.message_modes.insert("first".into(), "plan".into());
    b.held_messages.insert("second".into());
    b.message_modes.insert("second".into(), "operator".into());
    a.save(&first).unwrap();
    b.save(&second).unwrap();
    assert_eq!(b.get("bot").unwrap().outbox[0].id, "second");
    let reopened = Store::open_vorkspace(&path).unwrap();
    assert_eq!(
        reopened.setting("first-only").unwrap().as_deref(),
        Some("preserve")
    );
    assert_eq!(
        reopened.setting("second-only").unwrap().as_deref(),
        Some("preserve")
    );
    let saved = Roster::load(&reopened).unwrap();
    assert_eq!(
        saved
            .get("bot")
            .unwrap()
            .outbox
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "second"]
    );
    // A later save of the stale local view must not interpret the merged
    // message it never observed as a deliberate deletion.
    b.get_mut("bot").unwrap().name = "Renamed".into();
    b.save(&second).unwrap();
    let saved = Roster::load(&reopened).unwrap();
    assert_eq!(saved.get("bot").unwrap().outbox.len(), 2);
    assert!(saved.held_messages.contains("first") && saved.held_messages.contains("second"));
    assert_eq!(
        saved.message_modes.get("first").map(String::as_str),
        Some("plan")
    );
    assert_eq!(
        saved.message_modes.get("second").map(String::as_str),
        Some("operator")
    );
    assert_eq!(saved.get("bot").unwrap().name, "Renamed");
}

fn release_process_race(gate: &Path, one: &Path, two: &Path) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !(one.exists() && two.exists()) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    // Release even on a failed setup so a child is never left waiting.
    let ready = one.exists() && two.exists();
    std::fs::write(gate, "go").unwrap();
    assert!(
        ready,
        "both child processes must load before the race starts"
    );
}

#[test]
fn dispatch_claim_process_worker() {
    let Some(path) = std::env::var_os("VORKSPACE_CLAIM_TEST_DB") else {
        return;
    };
    let gate = std::env::var_os("VORKSPACE_CLAIM_TEST_GATE").unwrap();
    let store = Store::open_vorkspace(Path::new(&path)).unwrap();
    let roster = Roster::load(&store).unwrap();
    let message = roster.get("bot").unwrap().outbox[0].clone();
    std::fs::write(
        std::env::var_os("VORKSPACE_CLAIM_TEST_READY").unwrap(),
        "ready",
    )
    .unwrap();
    while !Path::new(&gate).exists() {
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let claimed = store.claim_bot_message("bot", &message, &new_id()).unwrap();
    println!("CLAIM_RESULT={claimed}");
}

#[test]
fn dispatch_claim_is_unique_across_processes_and_survives_restart() {
    use std::process::{Command, Stdio};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.db");
    let gate = dir.path().join("gate");
    let store = Store::open_vorkspace(&path).unwrap();
    let message = Message {
        id: "message".into(),
        text: "hello".into(),
        peer: None,
        sent_at: None,
        internal: false,
        error: None,
        attachments: vec![],
    };
    let mut roster = Roster::default();
    roster.bots.push(Bot {
        id: "bot".into(),
        name: "Bot".into(),
        outbox: vec![message.clone()],
        ..Default::default()
    });
    roster.save(&store).unwrap();
    let ready_one = dir.path().join("ready-one");
    let ready_two = dir.path().join("ready-two");
    let spawn = |ready: &Path| {
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "bot::tests::dispatch_claim_process_worker",
                "--nocapture",
            ])
            .env("VORKSPACE_CLAIM_TEST_DB", &path)
            .env("VORKSPACE_CLAIM_TEST_GATE", &gate)
            .env("VORKSPACE_CLAIM_TEST_READY", ready)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let one = spawn(&ready_one);
    let two = spawn(&ready_two);
    release_process_race(&gate, &ready_one, &ready_two);
    let results: Vec<_> = [one, two]
        .into_iter()
        .map(|child| child.wait_with_output().unwrap())
        .collect();
    for result in &results {
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
    }
    let output = results
        .iter()
        .map(|result| String::from_utf8_lossy(&result.stdout).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(output.matches("CLAIM_RESULT=true").count(), 1, "{output}");
    assert_eq!(output.matches("CLAIM_RESULT=false").count(), 1, "{output}");
    drop(store);
    let reopened = Store::open_vorkspace(&path).unwrap();
    assert!(!reopened
        .claim_bot_message("bot", &message, "new-owner")
        .unwrap());
    reopened
        .release_unsubmitted_bot_claim("bot", "message", "wrong-owner")
        .unwrap();
    assert!(!reopened
        .claim_bot_message("bot", &message, "new-owner")
        .unwrap());
}

#[test]
fn claim_rejects_changed_or_missing_outbox_and_rolls_back_on_failure() {
    let dir = tempfile::tempdir().unwrap();
    for open in [
        Store::open as fn(&Path) -> Result<Store, String>,
        Store::open_vorkspace,
    ] {
        let path = dir.path().join(new_id());
        let store = open(&path).unwrap();
        let message = Message {
            id: "one".into(),
            text: "original".into(),
            peer: None,
            sent_at: None,
            internal: false,
            error: None,
            attachments: vec![],
        };
        let mut roster = Roster::default();
        roster.bots.push(Bot {
            id: "bot".into(),
            name: "Bot".into(),
            outbox: vec![message.clone()],
            ..Default::default()
        });
        roster.save(&store).unwrap();
        let mut changed = message.clone();
        changed.text = "changed".into();
        assert!(store.claim_bot_message("bot", &changed, "token").is_err());
        assert!(store
            .claim_bot_message("missing", &message, "token")
            .is_err());
        assert!(store.claim_bot_message("bot", &message, "token").unwrap());
        store
            .release_unsubmitted_bot_claim("bot", &message.id, "token")
            .unwrap();
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TRIGGER fail_claim BEFORE INSERT ON native_bot_dispatch_claims BEGIN SELECT RAISE(FAIL, 'injected'); END;").unwrap();
        assert!(store.claim_bot_message("bot", &message, "token").is_err());
        drop(store);
        conn.execute_batch("DROP TRIGGER fail_claim").unwrap();
        drop(conn);
        let reopened = open(&path).unwrap();
        assert!(reopened.claim_bot_message("bot", &message, "next").unwrap());
    }
}

#[test]
fn sidecar_write_process_worker() {
    let Some(path) = std::env::var_os("VORKSPACE_SIDECAR_TEST_DB") else {
        return;
    };
    let gate = std::env::var_os("VORKSPACE_SIDECAR_TEST_GATE").unwrap();
    let key = std::env::var("VORKSPACE_SIDECAR_TEST_KEY").unwrap();
    let store = Store::open_vorkspace(Path::new(&path)).unwrap();
    std::fs::write(
        std::env::var_os("VORKSPACE_SIDECAR_TEST_READY").unwrap(),
        "ready",
    )
    .unwrap();
    while !Path::new(&gate).exists() {
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    store.save_setting(&key, "preserved").unwrap();
}

#[test]
fn sidecar_updates_from_two_processes_survive_the_same_publication_race() {
    use std::process::Command;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.db");
    let gate = dir.path().join("gate");
    let store = Store::open_vorkspace(&path).unwrap();
    let ready_one = dir.path().join("ready-one");
    let ready_two = dir.path().join("ready-two");
    let spawn = |key, ready: &Path| {
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "bot::tests::sidecar_write_process_worker"])
            .env("VORKSPACE_SIDECAR_TEST_DB", &path)
            .env("VORKSPACE_SIDECAR_TEST_GATE", &gate)
            .env("VORKSPACE_SIDECAR_TEST_KEY", key)
            .env("VORKSPACE_SIDECAR_TEST_READY", ready)
            .spawn()
            .unwrap()
    };
    let mut one = spawn("writer-one", &ready_one);
    let mut two = spawn("writer-two", &ready_two);
    release_process_race(&gate, &ready_one, &ready_two);
    assert!(one.wait().unwrap().success());
    assert!(two.wait().unwrap().success());
    assert_eq!(
        store.setting("writer-one").unwrap().as_deref(),
        Some("preserved")
    );
    assert_eq!(
        store.setting("writer-two").unwrap().as_deref(),
        Some("preserved")
    );
}

#[test]
fn concurrent_edits_to_the_same_queued_message_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.db");
    let first = Store::open_vorkspace(&path).unwrap();
    let message = Message {
        id: "same".into(),
        text: "original".into(),
        peer: None,
        sent_at: None,
        internal: false,
        error: None,
        attachments: vec![],
    };
    let mut initial = Roster::default();
    initial.bots.push(Bot {
        id: "bot".into(),
        name: "Bot".into(),
        outbox: vec![message],
        ..Default::default()
    });
    initial.save(&first).unwrap();
    let second = Store::open_vorkspace(&path).unwrap();
    let mut a = Roster::load(&first).unwrap();
    let mut b = Roster::load(&second).unwrap();
    a.get_mut("bot").unwrap().outbox[0].text = "first edit".into();
    b.get_mut("bot").unwrap().outbox[0].text = "second edit".into();
    a.save(&first).unwrap();
    assert!(b.save(&second).unwrap_err().contains("another window"));
    assert_eq!(b.get("bot").unwrap().outbox[0].text, "second edit");
    drop(first);
    drop(second);
    let reopened = Store::open_vorkspace(&path).unwrap();
    assert_eq!(
        Roster::load(&reopened).unwrap().get("bot").unwrap().outbox[0].text,
        "first edit"
    );
}

#[test]
fn roster_edits_retain_unknown_fields_in_existing_messages_and_bots() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_vorkspace(&dir.path().join("db")).unwrap();
    let message = Message {
        id: "one".into(),
        text: "original".into(),
        peer: None,
        sent_at: None,
        internal: false,
        error: None,
        attachments: vec![],
    };
    let mut roster = Roster::default();
    roster.bots.push(Bot {
        id: "bot".into(),
        name: "Bot".into(),
        outbox: vec![message],
        ..Default::default()
    });
    let mut raw = serde_json::to_value(&roster).unwrap();
    raw["futureRoot"] = serde_json::json!({"opaque":1});
    raw["bots"][0]["futureBot"] = serde_json::json!({"opaque":2});
    raw["bots"][0]["outbox"][0]["futureMessage"] = serde_json::json!({"opaque":3});
    store.save_setting("bot-roster", &raw.to_string()).unwrap();
    let mut loaded = Roster::load(&store).unwrap();
    loaded.get_mut("bot").unwrap().outbox[0].text = "edited".into();
    loaded.save(&store).unwrap();
    let saved: serde_json::Value =
        serde_json::from_str(&store.setting("bot-roster").unwrap().unwrap()).unwrap();
    assert_eq!(saved["futureRoot"]["opaque"], 1);
    assert_eq!(saved["bots"][0]["futureBot"]["opaque"], 2);
    assert_eq!(saved["bots"][0]["outbox"][0]["futureMessage"]["opaque"], 3);
    assert_eq!(saved["bots"][0]["outbox"][0]["text"], "edited");
}
