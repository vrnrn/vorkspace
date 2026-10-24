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
