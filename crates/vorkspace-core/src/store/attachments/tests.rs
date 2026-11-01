use super::*;
struct Fixture {
    directory: tempfile::TempDir,
    conn: Connection,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let conn = Connection::open(directory.path().join("vorkspace.db")).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE composer_attachment_processes(owner_instance TEXT PRIMARY KEY,owner_pid INTEGER NOT NULL);").unwrap();
        Self { directory, conn }
    }
    fn file(&self, name: &str, old: bool) -> PathBuf {
        let path = cache_attachment(self.directory.path(), name, b"private bytes").unwrap();
        if old {
            std::fs::File::open(&path)
                .unwrap()
                .set_times(
                    std::fs::FileTimes::new()
                        .set_modified(SystemTime::now() - Duration::from_secs(7200)),
                )
                .unwrap();
        }
        path
    }
    fn collect(&self) -> usize {
        let owner = uuid::Uuid::new_v4().to_string();
        assert!(register_attachment_process(&self.conn, &owner, 1, |_| false).unwrap());
        collect_cached_attachments(
            &self.conn,
            self.directory.path(),
            SystemTime::now(),
            &owner,
            &AtomicBool::new(false),
            |_| false,
        )
        .unwrap()
    }
}
#[test]
fn pasted_bytes_reopen_privately_and_names_cannot_escape_the_owned_directory() {
    let fixture = Fixture::new();
    let path = fixture.file("../private.png", false);
    assert_eq!(std::fs::read(&path).unwrap(), b"private bytes");
    assert_eq!(
        path.parent().unwrap(),
        fixture.directory.path().join(ATTACHMENT_DIRECTORY)
    );
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}
#[test]
fn collector_scans_all_sql_owners_and_raw_sidecar_then_removes_only_old_orphans() {
    let fixture = Fixture::new();
    let mut kept = Vec::new();
    for table in [
        "composer_drafts",
        "composer_draft_recoveries",
        "ordinary_blocks",
        "bot_blocks",
        "local_submissions",
        "sessions",
        "notes",
        "future_owner",
    ] {
        fixture
            .conn
            .execute_batch(&format!("CREATE TABLE {table}(payload_json TEXT);"))
            .unwrap();
        let file = fixture.file(table, true);
        fixture
            .conn
            .execute(
                &format!("INSERT INTO {table} VALUES(?1)"),
                [format!("malformed raw reference {}", file.display())],
            )
            .unwrap();
        kept.push(file);
    }
    let sidecar = fixture.file("native-sidecar", true);
    std::fs::write(
        fixture.directory.path().join("vorkspace.db.gpui.json"),
        format!("{{broken:{}", sidecar.display()),
    )
    .unwrap();
    kept.push(sidecar);
    let orphan = fixture.file("orphan", true);
    let young = fixture.file("not-published-yet", false);
    let unknown = fixture
        .directory
        .path()
        .join(ATTACHMENT_DIRECTORY)
        .join("unknown-user-file");
    std::fs::write(&unknown, b"keep").unwrap();
    assert_eq!(fixture.collect(), 1);
    assert!(!orphan.exists());
    assert!(young.exists());
    assert!(unknown.exists());
    assert!(kept.iter().all(|p| p.exists()));
}
#[test]
fn read_failure_expired_budget_and_live_owners_preserve_every_candidate() {
    let fixture = Fixture::new();
    let orphan = fixture.file("orphan", true);
    assert!(scan_attachment_collection(
        &fixture.conn,
        fixture.directory.path(),
        SystemTime::now(),
        Duration::ZERO
    )
    .is_err());
    assert!(orphan.exists());
    fixture
        .conn
        .execute_batch("CREATE TABLE malformed(value TEXT); INSERT INTO malformed VALUES(x'FF');")
        .unwrap();
    assert_eq!(fixture.collect(), 0);
    assert!(orphan.exists());
    fixture.conn.execute_batch("DROP TABLE malformed").unwrap();
    assert!(register_attachment_process(&fixture.conn, "live", 99, |_| false).unwrap());
    assert!(!register_attachment_process(&fixture.conn, "second", 100, |pid| pid == 99).unwrap());
    assert_eq!(
        collect_cached_attachments(
            &fixture.conn,
            fixture.directory.path(),
            SystemTime::now(),
            "second",
            &AtomicBool::new(false),
            |pid| pid == 99
        )
        .unwrap(),
        0
    );
    assert!(orphan.exists());
}
#[test]
fn foreground_sidecar_publication_from_another_store_cancels_collection() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("vorkspace.db");
    let first = Store::open_vorkspace(&path).unwrap();
    let second = Store::open_vorkspace(&path).unwrap();
    let activity = Arc::new(AtomicBool::new(false));
    activity_registry()
        .lock()
        .unwrap()
        .insert(activity_key(&path), Arc::downgrade(&activity));
    second.save_ui_state("{\"project\":\"/saved\"}").unwrap();
    assert!(activity.load(Ordering::SeqCst));
    let file = cache_attachment(directory.path(), "old", b"private").unwrap();
    std::fs::File::open(&file)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(7200)),
        )
        .unwrap();
    assert_eq!(
        collect_cached_attachments(
            &first.conn,
            directory.path(),
            SystemTime::now(),
            "not-admitted",
            &activity,
            |_| false
        )
        .unwrap(),
        0
    );
    assert!(file.exists());
}
#[test]
fn symlink_files_and_a_symlink_cache_root_are_never_collected_or_written_through() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let outside = fixture.directory.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    let target = outside.join("target");
    std::fs::write(&target, b"private").unwrap();
    let normal = fixture.file("normal", true);
    let link = normal
        .parent()
        .unwrap()
        .join(format!("{}-link", uuid::Uuid::new_v4()));
    symlink(&target, &link).unwrap();
    assert_eq!(fixture.collect(), 1);
    assert!(link.is_symlink());
    assert!(target.exists());
    let cache = fixture.directory.path().join(ATTACHMENT_DIRECTORY);
    std::fs::remove_file(&link).unwrap();
    std::fs::remove_dir(&cache).unwrap();
    symlink(&outside, &cache).unwrap();
    assert!(cache_attachment(fixture.directory.path(), "refuse", b"bytes").is_err());
    assert_eq!(fixture.collect(), 0);
    assert_eq!(std::fs::read(&target).unwrap(), b"private");
}
#[test]
fn simultaneous_openers_admit_one_cold_owner_and_retain_both_live_leases() {
    let fixture = Fixture::new();
    fixture
        .conn
        .execute(
            "INSERT INTO composer_attachment_processes VALUES('dead',999)",
            [],
        )
        .unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let workers: Vec<_> = (0..2)
        .map(|i| {
            let database = fixture.directory.path().join("vorkspace.db");
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let conn = Connection::open(database).unwrap();
                conn.busy_timeout(Duration::from_secs(5)).unwrap();
                barrier.wait();
                register_attachment_process(&conn, &format!("owner-{i}"), i + 1, |pid| pid != 999)
                    .unwrap()
            })
        })
        .collect();
    assert_eq!(
        workers
            .into_iter()
            .map(|w| w.join().unwrap())
            .filter(|cold| *cold)
            .count(),
        1
    );
    assert_eq!(
        fixture
            .conn
            .query_row(
                "SELECT count(*) FROM composer_attachment_processes",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
}
#[test]
fn composer_attachment_gc_read_scan_allows_publication_and_rechecks_data_version() {
    use rusqlite::hooks::{AuthAction, Authorization};
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("publication.db");
    let conn = Connection::open(&database).unwrap();
    conn.execute_batch("PRAGMA journal_mode = WAL; CREATE TABLE durable_roots(reference TEXT);")
        .unwrap();
    conn.busy_timeout(Duration::from_millis(100)).unwrap();
    conn.execute_batch("CREATE TABLE composer_attachment_processes(owner_instance TEXT PRIMARY KEY,owner_pid INTEGER NOT NULL)").unwrap();
    assert!(register_attachment_process(&conn, "collector", 1, |_| false).unwrap());
    let file = cache_attachment(directory.path(), "publishing", b"private").unwrap();
    std::fs::File::open(&file)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(7200)),
        )
        .unwrap();
    let (entered, scanning) = std::sync::mpsc::sync_channel(0);
    let (resume, resumed) = std::sync::mpsc::sync_channel(0);
    let root = directory.path().to_path_buf();
    let worker = std::thread::spawn(move || {
        let conn = Connection::open(database).unwrap();
        let mut paused = false;
        conn.authorizer(Some(move |context: rusqlite::hooks::AuthContext<'_>| {
            if !paused
                && matches!(
                    context.action,
                    AuthAction::Read {
                        table_name: "durable_roots",
                        column_name: "reference"
                    }
                )
            {
                paused = true;
                entered.send(()).unwrap();
                resumed.recv().unwrap();
            }
            Authorization::Allow
        }))
        .unwrap();
        collect_cached_attachments(
            &conn,
            &root,
            SystemTime::now(),
            "collector",
            &AtomicBool::new(false),
            |_| false,
        )
        .unwrap()
    });
    scanning.recv_timeout(Duration::from_secs(5)).unwrap();
    let published = conn.execute(
        "INSERT INTO durable_roots(reference) VALUES (?1)",
        [file.to_string_lossy().as_ref()],
    );
    // Always release the scanner before asserting, including on a regression.
    resume.send(()).unwrap();
    let removed = worker.join().unwrap();
    assert!(
        published.is_ok(),
        "A background reference scan reserved the writer: {published:?}"
    );
    assert_eq!(removed, 0);
    assert!(file.exists());
}
