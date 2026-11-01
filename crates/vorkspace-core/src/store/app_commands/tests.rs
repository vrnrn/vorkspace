
use super::*;
use serde_json::json;
#[test]
fn admission_survives_reopen_and_cannot_replay_an_unknown_effect_or_changed_payload() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sessions.db");
    let store = Store::open_vorkspace(&path).unwrap();
    let caller = store.create("codex", "/project").unwrap().id;
    let input = json!({"prompt":"hello","modelSettings":{"effort":"high"}});
    assert_eq!(
        store
            .admit_app_command(caller, "pending", "sessions.start", &input)
            .unwrap(),
        AppCommandAdmission::Admitted
    );
    drop(store);
    let store = Store::open_vorkspace(&path).unwrap();
    assert!(store
        .admit_app_command(caller, "pending", "sessions.start", &input)
        .unwrap_err()
        .contains("unconfirmed"));
    assert!(store
        .admit_app_command(
            caller,
            "pending",
            "sessions.start",
            &json!({"prompt":"other"})
        )
        .unwrap_err()
        .contains("different"));
    assert!(store
        .admit_app_command(caller, "pending", "notes.write", &input)
        .unwrap_err()
        .contains("different"));
}
#[test]
fn completed_results_are_durable_beyond_prior_cache_eviction_and_scoped_to_original_caller() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sessions.db");
    let store = Store::open_vorkspace(&path).unwrap();
    let caller = store.create("codex", "/project").unwrap().id;
    let other = store.create("codex", "/project").unwrap().id;
    for i in 0..300 {
        let request = format!("command-{i}");
        let input = json!({"id":i});
        assert_eq!(
            store
                .admit_app_command(caller, &request, "notes.write", &input)
                .unwrap(),
            AppCommandAdmission::Admitted
        );
        store
            .finish_app_command(
                caller,
                &request,
                "notes.write",
                &input,
                &Ok(json!({"saved":i})),
            )
            .unwrap();
    }
    drop(store);
    let store = Store::open_vorkspace(&path).unwrap();
    assert_eq!(
        store
            .admit_app_command(caller, "command-0", "notes.write", &json!({"id":0}))
            .unwrap(),
        AppCommandAdmission::Completed(Ok(json!({"saved":0})))
    );
    assert_eq!(
        store
            .admit_app_command(other, "command-0", "notes.write", &json!({"id":0}))
            .unwrap(),
        AppCommandAdmission::Admitted
    );
}
#[test]
fn failed_admission_never_authorizes_an_effect_and_failed_completion_stays_unconfirmed() {
    let store = Store::open_vorkspace(std::path::Path::new(":memory:")).unwrap();
    let caller = store.create("codex", "/project").unwrap().id;
    ensure(&store.conn).unwrap();
    store.conn.execute_batch("CREATE TRIGGER refuse_admit BEFORE INSERT ON app_command_receipts BEGIN SELECT RAISE(ABORT,'storage failed'); END").unwrap();
    assert!(store
        .admit_app_command(caller, "blocked", "sessions.start", &json!({}))
        .is_err());
    assert_eq!(
        store
            .conn
            .query_row("SELECT count(*) FROM app_command_receipts", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    store
        .conn
        .execute_batch("DROP TRIGGER refuse_admit")
        .unwrap();
    store
        .admit_app_command(caller, "unknown", "sessions.start", &json!({}))
        .unwrap();
    store.conn.execute_batch("CREATE TRIGGER refuse_result BEFORE UPDATE ON app_command_receipts BEGIN SELECT RAISE(ABORT,'storage failed'); END").unwrap();
    assert!(store
        .finish_app_command(
            caller,
            "unknown",
            "sessions.start",
            &json!({}),
            &Ok(json!({"accepted":true}))
        )
        .is_err());
    assert!(store
        .admit_app_command(caller, "unknown", "sessions.start", &json!({}))
        .unwrap_err()
        .contains("unconfirmed"));
}
#[test]
fn only_one_connection_can_admit_the_same_effect() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("db");
    let first = Store::open_vorkspace(&path).unwrap();
    let caller = first.create("codex", "/project").unwrap().id;
    let second = Store::open_vorkspace(&path).unwrap();
    assert_eq!(
        first
            .admit_app_command(caller, "once", "sessions.start", &json!({}))
            .unwrap(),
        AppCommandAdmission::Admitted
    );
    assert!(second
        .admit_app_command(caller, "once", "sessions.start", &json!({}))
        .unwrap_err()
        .contains("unconfirmed"));
    first
        .finish_app_command(caller, "once", "sessions.start", &json!({}), &Ok(json!(12)))
        .unwrap();
    assert_eq!(
        second
            .admit_app_command(caller, "once", "sessions.start", &json!({}))
            .unwrap(),
        AppCommandAdmission::Completed(Ok(json!(12)))
    );
}
#[test]
fn cold_hidden_habit_channel_and_worker_targets_are_protected_at_the_mutation_owner() {
    use crate::bot::{
        channels::{Channel, Channels},
        Roster,
    };
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("db");
    let mut store = Store::open_vorkspace(&path).unwrap();
    let caller = store.create("codex", "/project").unwrap().id;
    let target = store.create("codex", "/project").unwrap().id;
    let outside = store.create("codex", "/outside").unwrap().id;
    assert!(store.check_app_target(caller, caller, "/project").is_err());
    assert!(store.check_app_target(caller, outside, "/project").is_err());
    let mut roster = Roster::default();
    roster.hidden_sessions = vec![target];
    roster.save(&store).unwrap();
    drop(store);
    store = Store::open_vorkspace(&path).unwrap();
    assert!(store
        .archive_app_session(caller, target, "/project")
        .is_err());
    assert!(store
        .delete_app_session(caller, target, "/project")
        .is_err());
    assert!(!store.bot_session(target).unwrap().unwrap().archived);
    roster.hidden_sessions.clear();
    let bot = roster
        .create(crate::bot::Bot {
            name: "Test bot".into(),
            ..roster.draft(vec!["/project".into()])
        })
        .unwrap()
        .id
        .clone();
    roster.get_mut(&bot).unwrap().habit_sessions.push(target);
    roster.save(&store).unwrap();
    assert!(store.check_app_target(caller, target, "/project").is_err());
    roster.get_mut(&bot).unwrap().habit_sessions.clear();
    roster.save(&store).unwrap();
    Channels {
        channels: vec![Channel {
            member_sessions: [(bot, target)].into(),
            ..Default::default()
        }],
    }
    .save(&store)
    .unwrap();
    assert!(store.check_app_target(caller, target, "/project").is_err());
    Channels::default().save(&store).unwrap();
    store
        .save_orchestration_summary(
            caller,
            &crate::orchestration::Summary {
                tasks: vec![crate::orchestration::SummaryTask {
                    session: Some(target),
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .unwrap();
    assert!(store
        .delete_app_session(caller, target, "/project")
        .is_err());
    store
        .save_orchestration_summary(caller, &Default::default())
        .unwrap();
    store
        .archive_app_session(caller, target, "/project")
        .unwrap();
    assert!(store.bot_session(target).unwrap().unwrap().archived);
    store
        .delete_app_session(caller, target, "/project")
        .unwrap();
    assert!(store.bot_session(target).unwrap().is_none());
    assert!(store.bot_session(caller).unwrap().is_some());
}
#[test]
fn ordinary_background_launches_remain_mutable_after_reopen() {
    for open in [
        Store::open as fn(&std::path::Path) -> Result<Store, String>,
        Store::open_vorkspace,
    ] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("db");
        let store = open(&path).unwrap();
        let caller = store.create("codex", "/project").unwrap().id;
        let child = store.create("codex", "/project").unwrap().id;
        let internal = store.create("codex", "/project").unwrap().id;
        let mut roster = crate::bot::Roster::default();
        let bot = roster
            .create(crate::bot::Bot {
                name: "Test bot".into(),
                ..roster.draft(vec!["/project".into()])
            })
            .unwrap()
            .id
            .clone();
        let bot = roster.get_mut(&bot).unwrap();
        bot.session_id = Some(caller);
        bot.show_started_sessions_in_sidebar = false;
        bot.launches.push(crate::bot::Launch {
            session_id: child,
            cwd: "/project".into(),
            title: "Background work".into(),
            harness: "codex".into(),
            model: None,
            origin_turn: 0,
            notify_on_complete: false,
            completed: false,
            dismissed: false,
        });
        roster.hidden_sessions.push(internal);
        roster.save(&store).unwrap();
        if store.shared.is_some() {
            store
                .shared_patch(child, &[("sidebar_hidden", json!(true))])
                .unwrap();
        }
        drop(store);
        let mut store = open(&path).unwrap();
        assert!(store.check_app_target(caller, child, "/project").is_ok());
        assert!(store.check_app_target(caller, caller, "/project").is_err());
        assert!(store
            .check_app_target(caller, internal, "/project")
            .is_err());
        store
            .archive_app_session(caller, child, "/project")
            .unwrap();
        assert!(store.bot_session(child).unwrap().unwrap().archived);
        store.delete_app_session(caller, child, "/project").unwrap();
        assert!(store.bot_session(child).unwrap().is_none());
        assert!(store.bot_session(internal).unwrap().is_some());
    }
}
#[test]
fn legacy_background_launch_identity_is_independent_of_sidebar_visibility() {
    let store = Store::open_vorkspace(std::path::Path::new(":memory:")).unwrap();
    let caller = store.create("codex", "/project").unwrap().id;
    let source = store.persisted_session_id(caller).unwrap();
    let mut roster = crate::bot::Roster::default();
    let bot = roster
        .create(crate::bot::Bot {
            name: "Test bot".into(),
            ..roster.draft(vec!["/project".into(), "/assigned".into()])
        })
        .unwrap()
        .id
        .clone();
    roster.get_mut(&bot).unwrap().session_id = Some(caller);
    roster.hidden_sessions.push(caller);
    roster.save(&store).unwrap();
    for (original, project, allowed) in [
        (format!("app-{source}-background-task"), "/project", true),
        (
            format!("app-{source}-other-assigned-task"),
            "/assigned",
            true,
        ),
        (format!("app-{source}-unrelated-task"), "/unassigned", false),
        ("unclassified-hidden".into(), "/project", false),
    ] {
        store.conn.execute("INSERT INTO sessions(id,cwd,harness,model,model_settings,runtime_mode,title,blocks_json,created_at,updated_at,sidebar_hidden) VALUES(?1,?2,'codex','codex:default','{}','supervised','Background','[]',1,1,1)",params![original,project]).unwrap();
        let target = store.resolve_session_id(&original).unwrap();
        assert_eq!(
            store.check_app_target(caller, target, project).is_ok(),
            allowed,
            "{original}"
        );
        if allowed {
            store.archive_app_session(caller, target, project).unwrap();
            assert!(store.bot_session(target).unwrap().unwrap().archived);
        }
    }
    let internal = format!("app-{source}-internal-chat");
    store.conn.execute("INSERT INTO sessions(id,cwd,harness,model,model_settings,runtime_mode,title,blocks_json,created_at,updated_at,sidebar_hidden) VALUES(?1,'/project','codex','codex:default','{}','supervised','Internal','[]',1,1,1)",[&internal]).unwrap();
    let internal = store.resolve_session_id(&internal).unwrap();
    let other = roster
        .create(crate::bot::Bot {
            name: "Test bot".into(),
            ..roster.draft(vec!["/project".into()])
        })
        .unwrap()
        .id
        .clone();
    roster.get_mut(&other).unwrap().session_id = Some(internal);
    roster.save(&store).unwrap();
    assert!(
        store
            .check_app_target(caller, internal, "/project")
            .is_err(),
        "an app-shaped child identity never overrides a real Bot identity"
    );
    assert!(store.check_app_target(caller, caller, "/project").is_err());
}
#[test]
fn admission_does_not_commit_a_post_insert_corrupted_receipt() {
    let store = Store::open_vorkspace(std::path::Path::new(":memory:")).unwrap();
    let caller = store.create("codex", "/project").unwrap().id;
    ensure(&store.conn).unwrap();
    store.conn.execute_batch("CREATE TRIGGER corrupt_receipt AFTER INSERT ON app_command_receipts BEGIN UPDATE app_command_receipts SET fingerprint='different' WHERE caller_session_id=NEW.caller_session_id AND request_id=NEW.request_id; END").unwrap();
    assert!(store
        .admit_app_command(caller, "corrupt", "sessions.start", &json!({}))
        .is_err());
    assert_eq!(
        store
            .conn
            .query_row("SELECT count(*) FROM app_command_receipts", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
#[test]
fn canonical_object_order_reuses_the_same_fingerprint() {
    let a: Value = serde_json::from_str(r#"{"x":1,"nested":{"a":1,"b":2}}"#).unwrap();
    let b: Value = serde_json::from_str(r#"{"nested":{"b":2,"a":1},"x":1}"#).unwrap();
    assert_eq!(
        fingerprint("notes.write", &a),
        fingerprint("notes.write", &b)
    );
}
