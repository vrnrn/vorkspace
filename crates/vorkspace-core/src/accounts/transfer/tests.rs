
use super::*;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "vorkspace-session-transfer-{}",
            rand::random::<u128>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn from(&self) -> PathBuf {
        self.0.join("from")
    }
    fn to(&self) -> PathBuf {
        self.0.join("to")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn carries_a_claude_transcript_with_its_subagents_and_checkpoints() {
    let fixture = Fixture::new();
    let project = fixture.from().join("projects").join("-Users-me-app");
    std::fs::create_dir_all(project.join("abc").join("subagents")).unwrap();
    std::fs::write(project.join("abc.jsonl"), "{\"turn\":2}\n").unwrap();
    std::fs::write(
        project.join("abc").join("subagents").join("agent-1.jsonl"),
        "x",
    )
    .unwrap();
    std::fs::create_dir_all(fixture.from().join("file-history").join("abc")).unwrap();
    std::fs::write(
        fixture.from().join("file-history").join("abc").join("v1"),
        "y",
    )
    .unwrap();
    std::fs::write(project.join("other.jsonl"), "unrelated").unwrap();
    // An older copy from an earlier switch is replaced by the newer one.
    let dest = fixture.to().join("projects").join("-Users-me-app");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("abc.jsonl"), "{\"turn\":1}\n").unwrap();

    transfer_claude(&fixture.from(), &fixture.to(), "abc").unwrap();

    assert_eq!(
        std::fs::read_to_string(dest.join("abc.jsonl")).unwrap(),
        "{\"turn\":2}\n"
    );
    assert!(dest
        .join("abc")
        .join("subagents")
        .join("agent-1.jsonl")
        .is_file());
    assert!(fixture
        .to()
        .join("file-history")
        .join("abc")
        .join("v1")
        .is_file());
    assert!(!dest.join("other.jsonl").exists());
}

#[test]
fn reports_a_missing_claude_transcript() {
    let fixture = Fixture::new();
    std::fs::create_dir_all(fixture.from().join("projects")).unwrap();
    assert!(transfer_claude(&fixture.from(), &fixture.to(), "missing").is_err());
}

#[test]
fn carries_a_codex_rollout_and_its_inherited_history() {
    let fixture = Fixture::new();
    let write = |dir: &str, id: &str, base: Option<&str>| {
        let dir = fixture.from().join(dir).join("2026").join("10").join("08");
        std::fs::create_dir_all(&dir).unwrap();
        let meta = serde_json::json!({"type":"session_meta","payload":{"id":id,"history_base":base.map(|id| serde_json::json!({"thread_id":id}))}});
        std::fs::write(
            dir.join(format!("rollout-2026-10-08T10-00-00-{id}.jsonl")),
            format!("{meta}\n"),
        )
        .unwrap();
    };
    write("sessions", "root-1", Some("base-1"));
    write("archived_sessions", "base-1", None);
    write("sessions", "unrelated-1", None);

    transfer_codex(&fixture.from(), &fixture.to(), "root-1").unwrap();

    let day = fixture
        .to()
        .join("sessions")
        .join("2026")
        .join("10")
        .join("08");
    assert!(day
        .join("rollout-2026-10-08T10-00-00-root-1.jsonl")
        .is_file());
    assert!(day
        .join("rollout-2026-10-08T10-00-00-base-1.jsonl")
        .is_file());
    assert!(!day
        .join("rollout-2026-10-08T10-00-00-unrelated-1.jsonl")
        .exists());
}
#[test]
fn refuses_to_publish_a_partial_codex_transfer_without_closed_agent_state() {
    let fixture = Fixture::new();
    let sessions = fixture.from().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join("rollout-2026-10-18T10-00-00-root.jsonl"),
        "root",
    )
    .unwrap();
    let db = rusqlite::Connection::open(fixture.from().join("state_5.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE thread_spawn_edges(parent_thread_id TEXT NOT NULL, child_thread_id TEXT PRIMARY KEY, status TEXT NOT NULL); INSERT INTO thread_spawn_edges VALUES ('root','child','closed');").unwrap();
    let error = transfer_codex(&fixture.from(), &fixture.to(), "root").unwrap_err();
    assert!(error.contains("initialized"), "{error}");
    assert!(!fixture.to().join("sessions").exists());
}
#[test]
fn refuses_a_missing_required_history_base_before_publishing_the_root() {
    let fixture = Fixture::new();
    let sessions = fixture.from().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let meta = serde_json::json!({"type":"session_meta","payload":{"id":"root","history_base":{"thread_id":"missing-base"}}});
    std::fs::write(
        sessions.join("rollout-2026-10-18T10-00-00-root.jsonl"),
        format!("{meta}\n"),
    )
    .unwrap();
    let error = transfer_codex(&fixture.from(), &fixture.to(), "root").unwrap_err();
    assert!(
        error.contains("required Codex history base missing-base"),
        "{error}"
    );
    assert!(!fixture.to().join("sessions").exists());
}

#[test]
fn allows_a_spawned_descendant_that_never_wrote_a_rollout() {
    let fixture = Fixture::new();
    let sessions = fixture.from().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join("rollout-2026-10-18T10-00-00-root.jsonl"),
        "{}\n",
    )
    .unwrap();
    let db = rusqlite::Connection::open(fixture.from().join("state_5.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE thread_spawn_edges(parent_thread_id TEXT NOT NULL, child_thread_id TEXT PRIMARY KEY, status TEXT NOT NULL); INSERT INTO thread_spawn_edges VALUES ('root','never-written','open');").unwrap();
    transfer_codex(&fixture.from(), &fixture.to(), "root").unwrap();
    assert!(fixture
        .to()
        .join("sessions")
        .join("2026")
        .join("10")
        .join("18")
        .join("rollout-2026-10-18T10-00-00-root.jsonl")
        .is_file());
}

#[test]
fn retries_closure_failures_without_publishing_or_replacing_the_root() {
    let fixture = Fixture::new();
    let sessions = fixture.from().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join("rollout-2026-10-18T10-00-00-root.jsonl"),
        "{}\n",
    )
    .unwrap();
    let source = rusqlite::Connection::open(fixture.from().join("state_5.sqlite")).unwrap();
    source.execute_batch("CREATE TABLE thread_spawn_edges(parent_thread_id TEXT NOT NULL, child_thread_id TEXT PRIMARY KEY, status TEXT NOT NULL); INSERT INTO thread_spawn_edges VALUES ('root','child','closed');").unwrap();
    let dest_sessions = fixture
        .to()
        .join("sessions")
        .join("2026")
        .join("10")
        .join("18");
    std::fs::create_dir_all(&dest_sessions).unwrap();
    let dest_root = dest_sessions.join("rollout-2026-10-18T10-00-00-root.jsonl");
    std::fs::write(&dest_root, "previous complete transcript").unwrap();
    let dest = rusqlite::Connection::open(fixture.to().join("state_5.sqlite")).unwrap();
    dest.execute_batch("CREATE TABLE unrelated(value TEXT);")
        .unwrap();
    for _ in 0..2 {
        assert!(transfer_codex(&fixture.from(), &fixture.to(), "root").is_err());
        assert_eq!(
            std::fs::read_to_string(&dest_root).unwrap(),
            "previous complete transcript"
        );
    }
    dest.execute_batch("CREATE TABLE thread_spawn_edges(parent_thread_id TEXT NOT NULL, child_thread_id TEXT PRIMARY KEY, status TEXT NOT NULL);").unwrap();
    transfer_codex(&fixture.from(), &fixture.to(), "root").unwrap();
    assert_eq!(std::fs::read_to_string(&dest_root).unwrap(), "{}\n");
    let status: String = dest
        .query_row(
            "SELECT status FROM thread_spawn_edges WHERE child_thread_id='child'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "closed");
}
