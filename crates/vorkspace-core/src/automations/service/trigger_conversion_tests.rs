//! Definitions saved before the trigger list are converted once, after a backup.
use super::*;
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;

const LEGACY_EVENT: &str = r#"{"id":"legacy-event","name":"Review PRs","prompt":"Review the change.","harness":"codex","model":"gpt-5","cwd":"/repo","workspaceMode":"local","reuseSession":false,"runtimeMode":"default","triggerKind":"github","triggerEvent":"pull_request_opened","scheduleKind":"weekdays","minute":0,"time":"09:00","dayOfWeek":1,"missedRunGraceMinutes":30,"enabled":true,"nextRunAt":4102444800000,"createdAt":1,"updatedAt":2,"savedByLaterBuild":{"kept":true}}"#;
const LEGACY_TIME: &str = r#"{"id":"legacy-time","name":"Morning","prompt":"Plan the day.","harness":"claude","model":"opus","cwd":"/repo","workspaceMode":"worktree","reuseSession":true,"runtimeMode":"default","scheduleKind":"weekly","minute":15,"time":"08:30","dayOfWeek":3,"triggers":null,"missedRunGraceMinutes":60,"enabled":false,"nextRunAt":4102444800000,"createdAt":3,"updatedAt":4}"#;
const CURRENT: &str = r#"{"id":"current","name":"Current","prompt":"Keep going.","harness":"codex","model":"gpt-5","cwd":"/repo","workspaceMode":"local","reuseSession":false,"runtimeMode":"default","triggerKind":"time","triggerEvent":"","scheduleKind":"daily","minute":0,"time":"10:00","dayOfWeek":1,"triggers":[{"id":"t1","kind":"time","event":"daily","scheduleKind":"daily","minute":0,"time":"10:00","dayOfWeek":1,"repos":[],"repo":"","branch":"","actor":"anyone"}],"missedRunGraceMinutes":30,"enabled":true,"nextRunAt":4102444800000,"createdAt":5,"updatedAt":6}"#;

/// What the pre-conversion reader produced in memory for each legacy row.
fn old_reader_output() -> Vec<(&'static str, Value)> {
    vec![
        (
            "legacy-event",
            json!({"id":"legacy-event","name":"Review PRs","prompt":"Review the change.","harness":"codex","model":"gpt-5","modelSettings":{},"cwd":"/repo","workspaceMode":"local","worktreeCwd":"","sessionFolderId":"","reuseSession":false,"runtimeMode":"default","triggerKind":"github","triggerEvent":"pull_request_opened","scheduleKind":"weekdays","minute":0,"time":"09:00","dayOfWeek":1,"triggers":[{"id":"legacy-event:legacy","kind":"github","event":"pull_request_opened","scheduleKind":"weekdays","minute":0,"time":"09:00","dayOfWeek":1,"repos":[],"repo":"","branch":"","actor":"anyone"}],"missedRunGraceMinutes":30,"enabled":true,"nextRunAt":4102444800000_i64,"createdAt":1,"updatedAt":2}),
        ),
        (
            "legacy-time",
            json!({"id":"legacy-time","name":"Morning","prompt":"Plan the day.","harness":"claude","model":"opus","modelSettings":{},"cwd":"/repo","workspaceMode":"worktree","worktreeCwd":"","sessionFolderId":"","reuseSession":true,"runtimeMode":"default","triggerKind":"time","triggerEvent":"","scheduleKind":"weekly","minute":15,"time":"08:30","dayOfWeek":3,"triggers":[{"id":"legacy-time:legacy","kind":"time","event":"weekly","scheduleKind":"weekly","minute":15,"time":"08:30","dayOfWeek":3,"repos":[],"repo":"","branch":"","actor":"anyone"}],"missedRunGraceMinutes":60,"enabled":false,"nextRunAt":4102444800000_i64,"createdAt":3,"updatedAt":4}),
        ),
    ]
}

fn fixture(rows: &[&str]) -> (tempfile::TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("vorkspace.db");
    let conn = Connection::open(&path).unwrap();
    ensure_schema(&conn).unwrap();
    for raw in rows {
        let id = serde_json::from_str::<Value>(raw).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        conn.execute(
            "INSERT INTO automations (id, definition_json, enabled, next_run_at, updated_at)
             VALUES (?1, ?2, 1, 4102444800000, 0)",
            params![id, raw],
        )
        .unwrap();
    }
    (directory, path)
}

fn saved(path: &Path) -> Vec<(String, String)> {
    let conn =
        Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut statement = conn
        .prepare("SELECT id, definition_json FROM automations ORDER BY id")
        .unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn backups(directory: &Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory.join("backups")) else {
        return Vec::new();
    };
    let mut folders: Vec<_> = entries.map(|entry| entry.unwrap().path()).collect();
    folders.sort();
    folders
}

fn assert_loads_like_the_old_reader(service: &Service) {
    let loaded: Vec<Value> = service
        .list()
        .unwrap()
        .iter()
        .map(|automation| serde_json::to_value(automation).unwrap())
        .collect();
    for (id, expected) in old_reader_output() {
        let found = loaded.iter().find(|value| value["id"] == id).unwrap();
        assert_eq!(
            found, &expected,
            "{id} must load exactly as before conversion"
        );
    }
}

#[test]
fn legacy_definitions_convert_once_after_a_verified_backup() {
    for (id, raw) in [("legacy-event", LEGACY_EVENT), ("legacy-time", LEGACY_TIME)] {
        let expected = old_reader_output()
            .into_iter()
            .find(|(name, _)| *name == id)
            .unwrap()
            .1;
        let old = parse_automation(raw.to_owned()).unwrap();
        assert_eq!(serde_json::to_value(old).unwrap(), expected);
    }
    let (directory, path) = fixture(&[LEGACY_EVENT, LEGACY_TIME, CURRENT]);
    let before = saved(&path);

    let service = Service::open(&path).unwrap();
    assert!(triggers_converted(&service.conn).unwrap());
    assert_loads_like_the_old_reader(&service);
    let after = saved(&path);
    assert_eq!(after[0], before[0], "current-shape data is untouched");
    for (id, raw) in &after[1..] {
        let value: Value = serde_json::from_str(raw).unwrap();
        assert!(value["triggers"].is_array(), "{id} stores its trigger list");
    }
    let event: Value = serde_json::from_str(&after[1].1).unwrap();
    assert_eq!(event["savedByLaterBuild"], json!({"kept": true}));

    let folders = backups(directory.path());
    assert_eq!(folders.len(), 1);
    let folder = &folders[0];
    assert!(folder
        .file_name()
        .unwrap()
        .to_string_lossy()
        .ends_with("-definition-triggers-v1"));
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&directory.path().join("backups")), 0o700);
    assert_eq!(mode(folder), 0o700);
    let copy = folder.join("vorkspace.db");
    assert_eq!(mode(&copy), 0o600);
    assert_eq!(
        saved(&copy),
        before,
        "the backup holds the unconverted rows"
    );
    drop(service);

    let service = Service::open(&path).unwrap();
    assert_eq!(saved(&path), after, "a second run changes nothing");
    assert_eq!(
        backups(directory.path()),
        folders,
        "a second run takes no backup"
    );
    assert_loads_like_the_old_reader(&service);
}

#[test]
fn an_interrupted_conversion_is_redone_by_the_next_open() {
    let (directory, path) = fixture(&[LEGACY_EVENT, LEGACY_TIME]);
    let before = saved(&path);
    Connection::open(&path)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER reject_marker BEFORE INSERT ON automation_migrations
             WHEN NEW.name = 'definition-triggers-v1'
             BEGIN SELECT RAISE(ABORT, 'interrupted conversion'); END;",
        )
        .unwrap();
    let error = Service::open(&path).err().unwrap();
    assert!(error.contains("interrupted conversion"), "{error}");
    assert_eq!(
        saved(&path),
        before,
        "nothing is published before the marker"
    );
    assert_eq!(backups(directory.path()).len(), 1);

    Connection::open(&path)
        .unwrap()
        .execute_batch("DROP TRIGGER reject_marker;")
        .unwrap();
    let service = Service::open(&path).unwrap();
    assert!(triggers_converted(&service.conn).unwrap());
    assert_loads_like_the_old_reader(&service);
}

#[test]
fn a_failed_backup_refuses_to_convert() {
    let (directory, path) = fixture(&[LEGACY_EVENT]);
    let before = saved(&path);
    std::fs::write(directory.path().join("backups"), b"not a folder").unwrap();
    let error = Service::open(&path).err().unwrap();
    assert!(error.contains("backup"), "{error}");
    assert_eq!(saved(&path), before);
    let conn = Connection::open(&path).unwrap();
    assert!(!triggers_converted(&conn).unwrap());
}

#[test]
fn current_definitions_are_marked_without_a_backup() {
    let (directory, path) = fixture(&[CURRENT]);
    let before = saved(&path);
    let service = Service::open(&path).unwrap();
    assert!(triggers_converted(&service.conn).unwrap());
    assert_eq!(saved(&path), before);
    assert!(backups(directory.path()).is_empty());
}
