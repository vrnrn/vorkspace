use super::*;
use gpui::{
    TestAppContext,
    profiler::{
        ActionTiming, SerializedLocation,
        journal::{
            ForegroundEvent, ForegroundJournalEntry, FrameSnapshot, IntervalBoundary,
            IntervalSealer,
        },
    },
};
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::Path,
};

fn temporary() -> tempfile::TempDir {
    // macOS /var is a symlink; exercise the actual, canonical private path.
    tempfile::tempdir_in("/private/tmp").unwrap()
}
fn private_file(path: &Path, data: &[u8]) {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(data).unwrap();
}
fn log_path(root: &tempfile::TempDir) -> PathBuf {
    root.path().join("diagnostics")
}
fn snapshot(cx: &TestAppContext, millis: u64) -> FrameSnapshot {
    let start = cx.executor().now();
    let end = start + Duration::from_millis(millis);
    let mut sealer = IntervalSealer::new(start);
    // Real GPUI event -> interval sealer -> detector -> serializer; no sleeping.
    sealer
        .push_entries([
            ForegroundJournalEntry::Event(ForegroundEvent::Action(ActionTiming {
                name: "diagnostic_fixture::ForegroundWork",
                start,
                end,
            })),
            ForegroundJournalEntry::Boundary(IntervalBoundary::Idle { ended_at: end }),
        ])
        .pop()
        .unwrap()
}
fn incident(cx: &TestAppContext, millis: u64) -> HangIncident {
    HangIncident::detect(snapshot(cx, millis), THRESHOLD, FRAME_BUDGET).unwrap()
}

#[gpui::test]
fn threshold_filters_frame_budget_noise_and_includes_exact_boundary(cx: &mut TestAppContext) {
    assert!(HangIncident::detect(snapshot(cx, 31), THRESHOLD, FRAME_BUDGET).is_none());
    assert!(!qualifies(&incident(cx, 32)));
    assert!(!qualifies(&incident(cx, 499)));
    assert!(qualifies(&incident(cx, 500)));
    assert!(qualifies(&incident(cx, 10_000)));
    let mut wrong_trigger = incident(cx, 10_000);
    wrong_trigger.trigger = HangTrigger::LateFrame;
    assert!(!qualifies(&wrong_trigger));
}

#[gpui::test]
fn synthetic_ten_second_foreground_stall_persists_attribution(cx: &mut TestAppContext) {
    let root = temporary();
    let path = log_path(&root);
    let startup = cx.executor().now();
    let incident = incident(cx, 10_000);
    let timing =
        SerializedHangIncident::convert(startup, &incident, MAX_CONTRIBUTORS, Some(startup));
    let run = Run::new();
    let mut sink = Sink::new(path.clone());
    sink.record(&Record::new(&run, timing, 0));
    sink.flush();
    assert!(!sink.disabled);
    drop(sink);
    let data = fs::read_to_string(path.join("hang-0.jsonl")).unwrap();
    assert_eq!(data.lines().count(), 1);
    let record: serde_json::Value = serde_json::from_str(data.trim()).unwrap();
    assert_eq!(record["stall_ms"], 10_000.0);
    assert_eq!(record["phase"], "steady");
    assert_eq!(record["contributors"][0]["kind"], "action");
    assert_eq!(
        record["contributors"][0]["name"],
        "diagnostic_fixture::ForegroundWork"
    );
    assert_eq!(record["contributors"][0]["duration_ms"], 10_000.0);
    println!(
        "synthetic foreground event -> GPUI sealer/detector -> persisted JSONL: stall_ms=10000, kind=action, type=diagnostic_fixture::ForegroundWork, phase=steady, bytes={}",
        data.len()
    );
    assert_eq!(
        fs::metadata(path.join("hang-0.jsonl")).unwrap().mode() & 0o777,
        0o600
    );
}

#[gpui::test]
fn startup_power_boundary_and_discontinuity_remain_explicit(cx: &mut TestAppContext) {
    let startup = cx.executor().now();
    let mut incident = incident(cx, 700);
    incident.snapshot.boundary = IntervalBoundary::PowerTransition {
        ended_at: startup + Duration::from_millis(700),
    };
    incident.snapshot.dropped_events = 17;
    incident.snapshot.journal_discontinuous = true;
    let timing = SerializedHangIncident::convert(startup, &incident, MAX_CONTRIBUTORS, None);
    let run = Run::new();
    let record = Record::new(&run, timing, 0);
    assert_eq!(record.phase, "startup");
    assert_eq!(record.sealed_by, "power_transition");
    assert!(!record.during_active_use);
    assert_eq!(record.dropped_events, 17);
    assert!(record.journal_discontinuous);
}

#[gpui::test]
fn record_is_allowlisted_and_bounded_even_with_escaped_type_names(cx: &mut TestAppContext) {
    let startup = cx.executor().now();
    let mut timing =
        SerializedHangIncident::convert(startup, &incident(cx, 900), MAX_CONTRIBUTORS, None);
    const LONG_NAME: &str = concat!(
        "\u{0001}\u{0001}\u{0001}\u{0001}long_static_fixture_name_abcdefghijklmnopqrstuvwxyz_abcdefghijklmnopqrstuvwxyz_abcdefghijklmnopqrstuvwxyz_",
        "long_static_fixture_name_abcdefghijklmnopqrstuvwxyz_abcdefghijklmnopqrstuvwxyz_abcdefghijklmnopqrstuvwxyz_"
    );
    timing.contributors = (0..100)
        .map(|_| SerializedHangContributor::View {
            name: LONG_NAME,
            phase: "paint",
            start_ms: 0.0,
            duration_ms: 900.0,
            self_ms: 900.0,
            depth: 0,
        })
        .collect();
    let run = Run::new();
    let record = Record::new(&run, timing, 92);
    let bytes = encode(&record).unwrap();
    assert!(bytes.len() <= MAX_RECORD_BYTES);
    assert_eq!(record.contributors.len(), MAX_CONTRIBUTORS);
    assert_eq!(record.contributors_elided, 92);
    assert_eq!(record.incidents_elided_in_poll, 92);
    assert!(
        record
            .contributors
            .iter()
            .all(|c| c.name.as_ref().unwrap().len() <= MAX_STRING_BYTES)
    );
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let mut keys: Vec<_> = value["contributors"][0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "depth",
            "duration_ms",
            "kind",
            "name",
            "phase",
            "self_ms",
            "start_ms"
        ]
    );
    assert_eq!(bounded(&"🙂".repeat(100)).len(), MAX_STRING_BYTES);
    let mut buffer = BoundedBuffer(Vec::new());
    assert!(buffer.write_all(&vec![b'x'; MAX_RECORD_BYTES]).is_err());
    assert!(buffer.0.is_empty());
}

#[test]
fn contributor_sources_remove_build_prefix_and_inputs_are_only_variants() {
    let contributor = Contributor::from(SerializedHangContributor::TaskPoll {
        location: SerializedLocation {
            file: "/build-machine/private/crates/vorkspace-gpui/src/fixture.rs".into(),
            line: 42,
            column: 9,
        },
        start_ms: 0.0,
        duration_ms: 600.0,
        depth: 2,
    });
    let source = contributor.source.unwrap();
    assert_eq!(source.file, "vorkspace-gpui/src/fixture.rs");
    assert_eq!((source.line, source.column), (42, 9));
    assert_eq!(
        source_file("crates/vorkspace-gpui/src/fixture.rs"),
        "vorkspace-gpui/src/fixture.rs"
    );
    assert_eq!(
        source_file("vendor/gpui-pre/src/fixture.rs"),
        "gpui-pre/src/fixture.rs"
    );
    assert_eq!(
        source_file("/other-build-root/src/fixture.rs"),
        "fixture.rs"
    );
    assert_eq!(safe_input_kind("unknown_fixture_input"), "other");
    assert_eq!(safe_input_kind("key_down"), "key_down");
}

#[test]
fn rotation_bounds_real_bytes_and_reopens_without_truncation() {
    let root = temporary();
    let path = log_path(&root);
    let mut log = storage::Log::open(&path).unwrap();
    let mut data = vec![b'x'; MAX_RECORD_BYTES];
    *data.last_mut().unwrap() = b'\n';
    for _ in 0..120 {
        log.append(&data).unwrap();
        let total: u64 = (0..3)
            .map(|i| {
                fs::metadata(path.join(format!("hang-{i}.jsonl")))
                    .map(|m| {
                        assert!(m.len() <= storage::FILE_BYTES);
                        m.len()
                    })
                    .unwrap_or(0)
            })
            .sum();
        assert!(total <= storage::FILE_BYTES * 3);
    }
    let before = fs::read(path.join("hang-0.jsonl")).unwrap();
    drop(log);
    let mut log = storage::Log::open(&path).unwrap();
    log.append(b"{}\n").unwrap();
    let after = fs::read(path.join("hang-0.jsonl")).unwrap();
    assert!(after.starts_with(&before));
    assert!(after.ends_with(b"{}\n"));
    assert_eq!(fs::read_dir(&path).unwrap().count(), 4);
}

#[test]
fn interrupted_tail_is_preserved_as_archive() {
    let root = temporary();
    let path = log_path(&root);
    let log = storage::Log::open(&path).unwrap();
    drop(log);
    fs::write(path.join("hang-0.jsonl"), b"{partial").unwrap();
    let mut log = storage::Log::open(&path).unwrap();
    log.append(b"{}\n").unwrap();
    assert_eq!(fs::read(path.join("hang-1.jsonl")).unwrap(), b"{partial");
    assert_eq!(fs::read(path.join("hang-0.jsonl")).unwrap(), b"{}\n");
}

#[test]
fn empty_polls_and_flush_do_not_create_or_grow_files() {
    let root = temporary();
    let path = log_path(&root);
    let mut sink = Sink::new(path.clone());
    for _ in 0..1000 {
        sink.flush();
    }
    assert!(!path.exists());
    let mut log = storage::Log::open(&path).unwrap();
    log.append(b"{}\n").unwrap();
    sink.writer = Some(log);
    for _ in 0..1000 {
        sink.flush();
    }
    assert_eq!(fs::metadata(path.join("hang-0.jsonl")).unwrap().len(), 3);
}

#[test]
fn concurrent_writer_fails_open_and_restart_releases_lock() {
    let root = temporary();
    let path = log_path(&root);
    let first = storage::Log::open(&path).unwrap();
    assert!(storage::Log::open(&path).is_err());
    drop(first);
    assert!(storage::Log::open(&path).is_ok());
}

#[test]
fn unsafe_destinations_are_rejected_without_touching_targets() {
    for name in ["hang-0.jsonl", "hang-1.jsonl", "hang-2.jsonl", "hang.lock"] {
        let root = temporary();
        let path = log_path(&root);
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let sentinel = root.path().join("sentinel");
        private_file(&sentinel, b"fixture sentinel");
        symlink(&sentinel, path.join(name)).unwrap();
        assert!(storage::Log::open(&path).is_err());
        assert_eq!(fs::read(&sentinel).unwrap(), b"fixture sentinel");
    }
    let root = temporary();
    let real = root.path().join("real");
    fs::create_dir(&real).unwrap();
    symlink(&real, log_path(&root)).unwrap();
    assert!(storage::Log::open(&log_path(&root)).is_err());
    assert_eq!(fs::read_dir(&real).unwrap().count(), 0);
    let alias = root.path().join("alias");
    symlink(root.path(), &alias).unwrap();
    assert!(storage::Log::open(&alias.join("diagnostics")).is_err());
    assert!(storage::Log::open(Path::new("relative/diagnostics")).is_err());
    assert!(storage::Log::open(&root.path().join("../diagnostics")).is_err());
}

#[test]
fn hard_links_special_files_oversize_and_shared_permissions_are_rejected() {
    for case in ["hardlink", "directory", "oversize", "shared", "fifo"] {
        let root = temporary();
        let path = log_path(&root);
        let log = storage::Log::open(&path).unwrap();
        drop(log);
        let active = path.join("hang-0.jsonl");
        match case {
            "hardlink" => fs::hard_link(&active, root.path().join("alias")).unwrap(),
            "directory" => {
                fs::remove_file(&active).unwrap();
                fs::create_dir(&active).unwrap();
            }
            "oversize" => fs::OpenOptions::new()
                .write(true)
                .open(&active)
                .unwrap()
                .set_len(storage::FILE_BYTES + 1)
                .unwrap(),
            "shared" => fs::set_permissions(&active, fs::Permissions::from_mode(0o644)).unwrap(),
            "fifo" => {
                use std::os::unix::ffi::OsStrExt;
                fs::remove_file(&active).unwrap();
                let name = std::ffi::CString::new(active.as_os_str().as_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            }
            _ => unreachable!(),
        }
        assert!(storage::Log::open(&path).is_err(), "{case}");
    }
}

#[gpui::test]
fn failure_disables_until_next_run_and_recovery_preserves_files(cx: &mut TestAppContext) {
    let root = temporary();
    let path = log_path(&root);
    private_file(&path, b"fixture sentinel");
    let startup = cx.executor().now();
    let run = Run::new();
    let record = Record::new(
        &run,
        SerializedHangIncident::convert(startup, &incident(cx, 800), MAX_CONTRIBUTORS, None),
        0,
    );
    let mut sink = Sink::new(path.clone());
    sink.record(&record);
    assert!(sink.disabled);
    assert_eq!(fs::read(&path).unwrap(), b"fixture sentinel");
    fs::remove_file(&path).unwrap();
    sink.record(&record);
    assert!(!path.exists()); // No recurring retry after the destination changes.
    let mut next_run = Sink::new(path.clone());
    next_run.record(&record);
    assert!(!next_run.disabled);
    assert!(path.join("hang-0.jsonl").exists());
    symlink(root.path().join("sentinel"), path.join("hang-2.jsonl")).unwrap();
    next_run.record(&record);
    assert!(next_run.disabled); // Rotation/append validation failures also stop capture.
}

#[test]
fn unwritable_parent_and_directory_permissions_fail_without_cleanup() {
    let root = temporary();
    let path = log_path(&root);
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o500)).unwrap();
    let result = storage::Log::open(&path);
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
    assert!(!path.exists());
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(storage::Log::open(&path).is_err());
    assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
}

#[gpui::test]
fn setup_and_shutdown_keep_empty_capture_lazy(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = temporary();
    let path = log_path(&root);
    cx.update(|app| start(app, path.clone()));
    cx.update(|app| app.shutdown());
    assert!(!path.exists());
    let config = config();
    assert_eq!(config.threshold, Duration::from_millis(500));
    assert_eq!(config.interval, Duration::from_secs(1));
}

#[gpui::test]
fn callback_filters_caps_and_preserves_background_boundary_clues(cx: &mut TestAppContext) {
    use gpui::profiler::journal::{FrameSkipReason, FrameSkipped};
    let root = temporary();
    let path = log_path(&root);
    let mut callback = cx.update(|app| callback(app, path.clone()));
    let startup = cx.executor().now();
    let poll = |incidents| HangMonitorPoll {
        incidents,
        first_present_at: Some(startup),
        active_time: Duration::ZERO,
        reason: HangMonitorPollReason::Interval,
    };
    callback(poll(vec![]));
    callback(poll(vec![incident(cx, 32), incident(cx, 499)]));
    assert!(!path.exists());
    let mut inactive = incident(cx, 10_000);
    inactive.snapshot.boundary = IntervalBoundary::FrameSkipped(FrameSkipped {
        window_id: gpui::WindowId::from(1),
        at: startup + Duration::from_secs(10),
        reason: FrameSkipReason::InactiveFrameRateLimit,
    });
    callback(poll(vec![inactive; MAX_RECORDS_PER_POLL + 3]));
    let before = fs::read_to_string(path.join("hang-0.jsonl")).unwrap();
    assert_eq!(before.lines().count(), MAX_RECORDS_PER_POLL);
    for line in before.lines() {
        let record: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(record["frame_skip_reason"], "inactive_frame_rate_limit");
        assert_eq!(record["incidents_elided_in_poll"], 3);
        assert_eq!(record["stall_ms"], 10_000.0);
    }
    for _ in 0..100 {
        callback(poll(vec![]));
    }
    callback(HangMonitorPoll {
        reason: HangMonitorPollReason::Flush,
        ..poll(vec![])
    });
    assert_eq!(
        fs::read_to_string(path.join("hang-0.jsonl")).unwrap(),
        before
    );
    drop(callback);
    assert!(storage::Log::open(&path).is_ok());
}

#[gpui::test]
fn rotation_failure_disables_capture_and_leaves_unrelated_files(cx: &mut TestAppContext) {
    let root = temporary();
    let path = log_path(&root);
    let log = storage::Log::open(&path).unwrap();
    let active = path.join("hang-0.jsonl");
    fs::OpenOptions::new()
        .write(true)
        .open(&active)
        .unwrap()
        .set_len(storage::FILE_BYTES)
        .unwrap();
    private_file(&path.join("unrelated.fixture"), b"fixture sentinel");
    let startup = cx.executor().now();
    let run = Run::new();
    let record = Record::new(
        &run,
        SerializedHangIncident::convert(startup, &incident(cx, 800), MAX_CONTRIBUTORS, None),
        0,
    );
    let mut sink = Sink::new(path.clone());
    sink.writer = Some(log);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o500)).unwrap();
    sink.record(&record);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(sink.disabled);
    assert_eq!(fs::metadata(&active).unwrap().len(), storage::FILE_BYTES);
    assert_eq!(
        fs::read(path.join("unrelated.fixture")).unwrap(),
        b"fixture sentinel"
    );
    let mut next_run = Sink::new(path.clone());
    next_run.record(&record);
    assert!(!next_run.disabled);
    assert_eq!(
        fs::read(path.join("unrelated.fixture")).unwrap(),
        b"fixture sentinel"
    );
}
