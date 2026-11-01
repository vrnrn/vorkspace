
use super::*;
use crate::bot::{channels::Channels, peers};

fn roster() -> Roster {
    Roster {
        bots: vec![
            Bot {
                id: "parent".into(),
                name: "Parent".into(),
                ..Default::default()
            },
            Bot {
                id: "other".into(),
                name: "Other".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}
#[test]
fn a_hire_is_persistent_without_a_project_and_repeated_tasks_keep_identity_and_history() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("teams.db");
    let db = Store::open(&path).unwrap();
    let mut r = roster();
    let id = r
        .hire_minion(
            "parent",
            "Scout",
            "Repository analyst",
            vec!["Rust".into()],
            None,
        )
        .unwrap();
    assert!(r.get(&id).unwrap().session_id.is_none());
    assert!(db.sessions().unwrap().is_empty());
    let summary = db.create("codex", "/project").unwrap();
    r.get_mut(&id).unwrap().session_id = Some(summary.id);
    let appearance = (
        r.get(&id).unwrap().seed,
        r.get(&id).unwrap().mascot.clone(),
        r.get(&id).unwrap().shape.clone(),
    );
    peers::enqueue(&mut r, "parent", &id, "First task", "first", 0, true).unwrap();
    peers::enqueue(&mut r, "parent", &id, "Second task", "second", 0, true).unwrap();
    r.save(&db).unwrap();
    drop(db);
    let db = Store::open(&path).unwrap();
    let r = Roster::load(&db).unwrap();
    let member = r.get(&id).unwrap();
    assert_eq!(member.session_id, Some(summary.id));
    assert_eq!(member.outbox.len(), 2);
    assert_eq!(
        (member.seed, member.mascot.clone(), member.shape.clone()),
        appearance
    );
    assert_eq!(member.storage_id(), "parent");
    assert_eq!(r.active_minions("parent").count(), 1);
}
#[test]
fn legacy_migration_is_idempotent_and_preserves_the_existing_session() {
    let directory = tempfile::tempdir().unwrap();
    let mut db = Store::open(&directory.path().join("teams.db")).unwrap();
    let session = db.create("codex", "/old").unwrap().id;
    let history = vec![
        Row {
            role: "user".into(),
            text: "Legacy task".into(),
            first: true,
            sent_at: None,
        },
        Row {
            role: "assistant".into(),
            text: "Preserved result".into(),
            first: true,
            sent_at: None,
        },
    ];
    db.write_rows(session, 0, &history).unwrap();
    let mut old = roster();
    old.get_mut("parent").unwrap().minions.push(Minion {
        name: "Legacy".into(),
        session_id: session,
        project: "/old".into(),
        created_at: 7,
    });
    old.save(&db).unwrap();
    let migrated = Roster::load(&db).unwrap();
    let member = migrated.active_minions("parent").next().unwrap();
    assert_eq!(member.session_id, Some(session));
    assert_eq!(member.minion.as_ref().unwrap().created_at, 7);
    let id = member.id.clone();
    assert!(migrated.get("parent").unwrap().minions.is_empty());
    assert!(migrated.hidden_sessions.contains(&session));
    migrated.save(&db).unwrap();
    let reopened = Roster::load(&db).unwrap();
    assert_eq!(reopened.active_minions("parent").count(), 1);
    assert_eq!(reopened.get(&id).unwrap().session_id, Some(session));
    assert_eq!(
        db.load(session)
            .unwrap()
            .rows
            .iter()
            .map(|r| r.text.as_str())
            .collect::<Vec<_>>(),
        vec!["Legacy task", "Preserved result"]
    );
}
#[test]
fn peer_admission_is_bidirectional_and_retirement_cannot_resurrect_queued_work() {
    let directory = tempfile::tempdir().unwrap();
    let mut db = Store::open(&directory.path().join("teams.db")).unwrap();
    let mut r = roster();
    let a = r
        .hire_minion("parent", "A", "Review", vec![], None)
        .unwrap();
    let b = r.hire_minion("parent", "B", "Test", vec![], None).unwrap();
    let session = db.create("codex", "/project").unwrap().id;
    r.get_mut(&a).unwrap().session_id = Some(session);
    db.write_rows(
        session,
        0,
        &[Row {
            role: "assistant".into(),
            text: "Saved work before retirement".into(),
            first: true,
            sent_at: None,
        }],
    )
    .unwrap();
    let outsider = r.hire_minion("other", "C", "Build", vec![], None).unwrap();
    let denied: [(&str, &str); 5] = [
        (&a, "other"),
        ("other", &a),
        (&a, &outsider),
        (&outsider, &a),
        ("parent", &outsider),
    ];
    for (from, to) in denied {
        assert!(peers::enqueue(&mut r, from, to, "forbidden", "bad", 0, true).is_err());
        assert!(!r.can_read_peer(from, to));
    }
    peers::enqueue(&mut r, &a, &b, "Team work", "team", 0, true).unwrap();
    assert_eq!(r.get(&b).unwrap().outbox[0].peer.as_ref().unwrap().from, a);
    peers::enqueue(&mut r, &a, "parent", "Report", "report", 1, false).unwrap();
    peers::enqueue(
        &mut r,
        "parent",
        "other",
        "Ordinary peer",
        "ordinary",
        0,
        true,
    )
    .unwrap();
    peers::enqueue(&mut r, "parent", &a, "Task", "task", 0, true).unwrap();
    assert!(r.retire_minion("other", &a).is_err());
    r.retire_minion("parent", &a).unwrap();
    assert!(!r.is_active(&a));
    assert!(r.get(&a).unwrap().outbox.is_empty());
    assert!(r.get(&b).unwrap().outbox[0].error.is_some());
    assert!(r.get("parent").unwrap().outbox[0].error.is_some());
    assert!(r.can_read_peer("parent", &a));
    assert!(peers::enqueue(&mut r, "parent", &a, "Again", "again", 0, true).is_err());
    r.save(&db).unwrap();
    let reopened = Roster::load(&db).unwrap();
    assert!(!reopened.is_active(&a));
    assert!(reopened.get(&a).unwrap().outbox.is_empty());
    assert!(reopened.get(&b).unwrap().outbox[0].error.is_some());
    assert!(reopened.get("other").unwrap().outbox[0].error.is_none());
    assert_eq!(reopened.get(&a).unwrap().session_id, Some(session));
    assert_eq!(
        db.load(session).unwrap().rows[0].text,
        "Saved work before retirement"
    );
}
#[test]
fn team_channels_reject_users_outsiders_reuse_and_stale_deliveries() {
    let mut r = roster();
    let a = r
        .hire_minion("parent", "A", "Review", vec![], None)
        .unwrap();
    let b = r.hire_minion("parent", "B", "Test", vec![], None).unwrap();
    let mut channels = Channels::default();
    assert!(channels
        .create("mixed", "", vec![a.clone(), "other".into()], &r)
        .is_err());
    let normal = channels
        .create("normal", "", vec!["parent".into(), "other".into()], &r)
        .unwrap();
    assert!(channels.create("normal", "", vec![a.clone()], &r).is_err());
    let team = channels
        .create("team", "", vec!["parent".into(), a.clone(), b.clone()], &r)
        .unwrap();
    assert!(channels.get(&team).unwrap().user_read_only(&r));
    assert!(!channels.get(&team).unwrap().can_access(Some("other"), &r));
    assert!(!channels.get(&normal).unwrap().can_access(Some(&a), &r));
    assert!(channels
        .post(&team, None, "user input", "user", 0, &[], &r)
        .is_err());
    assert!(channels
        .post(
            &team,
            r.get("other"),
            "foreign input",
            "foreign",
            0,
            &[],
            &r
        )
        .is_err());
    channels
        .post(
            &team,
            r.get(&a),
            "Review",
            "allowed",
            0,
            std::slice::from_ref(&b),
            &r,
        )
        .unwrap();
    let delivery = channels.get(&team).unwrap().deliveries[0].clone();
    assert!(channels.get(&team).unwrap().delivery_allowed(&delivery, &r));
    r.retire_minion("parent", &a).unwrap();
    assert!(!channels.get(&team).unwrap().delivery_allowed(&delivery, &r));
    assert!(channels
        .post(&team, r.get(&a), "Late reply", "late", 1, &[], &r)
        .is_err());
    assert_eq!(channels.get(&team).unwrap().messages.len(), 1);
}
#[test]
fn execution_scope_and_shared_context_follow_the_live_parent_without_changing_actor() {
    let directory = tempfile::tempdir().unwrap();
    let mut r = roster();
    let a = r
        .hire_minion("parent", "Scout", "Analysis", vec!["Rust".into()], None)
        .unwrap();
    let b = r
        .hire_minion("parent", "Tester", "Validation", vec![], None)
        .unwrap();
    r.get_mut("parent").unwrap().projects = vec!["/new-scope".into()];
    r.get_mut("parent").unwrap().color = 0x123456;
    r.get_mut("parent").unwrap().fg_color = Some(0xfedcba);
    let actor = r.effective_bot(&a).unwrap();
    assert_eq!(actor.id, a);
    assert_eq!(actor.color, 0x123456);
    assert_eq!(actor.resolved_fg_color(), 0xfedcba);
    assert_eq!(actor.projects, vec!["/new-scope"]);
    let files = load_files(directory.path(), &actor).unwrap();
    assert_eq!(files.id, "parent");
    let prompt = super::super::context(&actor, &files, Path::new("/bin/v"));
    for required in [
        "Analysis",
        "Rust",
        "Tester",
        "/new-scope",
        "Users cannot message you",
    ] {
        assert!(prompt.contains(required), "{required}");
    }
    r.retire_minion("parent", &b).unwrap();
    assert!(!r
        .effective_bot(&a)
        .unwrap()
        .minion
        .unwrap()
        .team
        .iter()
        .any(|s| s.contains("Tester")));
}
