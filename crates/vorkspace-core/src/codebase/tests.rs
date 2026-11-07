use super::*;
use std::{fs, os::unix::fs::PermissionsExt, time::Instant};

fn fixture() -> (tempfile::TempDir, Arc<Service>) {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(Mutex::new(
        Store::open(&root.path().join("native.sqlite")).unwrap(),
    ));
    let (service, _) = Service::new(store, root.path().join("codebase")).unwrap();
    (root, service)
}

#[test]
fn disabled_is_durable_and_does_no_engine_work() {
    let (root, service) = fixture();
    let project = root.path().to_str().unwrap();
    assert!(!service.enabled(project));
    assert!(service.activate(project, project, false).is_err());
    assert!(service
        .query(project, project, "index_status", json!({}))
        .is_err());
    assert!(!root.path().join("codebase").exists());
    service.configure(project, true).unwrap();
    assert!(service.enabled(project));
    assert!(!root.path().join("codebase").exists());
    let (reopened, _) = Service::new(service.store.clone(), root.path().join("codebase")).unwrap();
    assert!(reopened.enabled(project));
    service.configure(project, false).unwrap();
    assert_eq!(service.snapshot(project, project).phase, "disabled");
}

#[test]
fn query_scope_and_budgets_are_owned_by_the_app() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    for (tool, args) in [
        ("list_projects", json!({})),
        ("delete_project", json!({})),
        ("search_graph", json!({"project":"other"})),
        (
            "query_graph",
            json!({"query":"MATCH (n) RETURN n", "max_rows":0}),
        ),
        ("trace_path", json!({"function_name":"a", "depth":15})),
        ("get_file_outline", json!({"file_path":"../secret"})),
        ("get_architecture", json!({"path":"/etc"})),
        ("detect_changes", json!({"config_path":"/etc/config"})),
        ("detect_changes", json!({"fixpoint":true})),
    ] {
        assert!(
            query::arguments(tool, args, "mine", &path).is_err(),
            "{tool}"
        );
    }
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), path.join("escape")).unwrap();
    assert!(query::arguments(
        "check_index_coverage",
        json!({"paths":["escape/absent.rs"]}),
        "mine",
        &path
    )
    .is_err());
    let args = query::arguments(
        "query_graph",
        json!({"query":"MATCH (n) RETURN n"}),
        "mine",
        &path,
    )
    .unwrap();
    assert_eq!(args["project"], "mine");
    assert_eq!(args["max_rows"], 200);
    assert_eq!(args["max_output_tokens"], 4000);
}

#[test]
fn unrelated_working_copy_cannot_start_a_client() {
    let (root, service) = fixture();
    let other = tempfile::tempdir().unwrap();
    service
        .configure(root.path().to_str().unwrap(), true)
        .unwrap();
    assert!(service
        .activate(
            root.path().to_str().unwrap(),
            other.path().to_str().unwrap(),
            false
        )
        .is_err());
    assert!(!root.path().join("codebase").exists());
}

#[test]
fn inactive_entries_cannot_bypass_capacity_and_draining_workers_keep_their_slots() {
    let (root, mut service) = fixture();
    Arc::get_mut(&mut service).unwrap().test_engine = Some("/usr/bin/false".into());
    let project = root.path().to_str().unwrap();
    service.configure(project, true).unwrap();
    // Dropping these senders releases every barrier, including on assertion
    // failure, before Service::drop joins the fixture workers.
    let mut releases = Vec::new();
    for index in 0..8 {
        let (release, barrier) = mpsc::channel::<()>();
        releases.push(release);
        let (sender, _) = mpsc::sync_channel(2);
        lock(&service.state).workers.insert(
            format!("slot-{index}"),
            Worker {
                project: "other".into(),
                cancel: Arc::new(AtomicBool::new(false)),
                snapshot: Arc::new(Mutex::new(Snapshot::default())),
                sender,
                thread: Some(std::thread::spawn(move || {
                    let _ = barrier.recv();
                })),
                retirement: Arc::default(),
            },
        );
    }
    let (sender, _) = mpsc::sync_channel(2);
    lock(&service.state).workers.insert(
        crate::files::path_key(project),
        Worker {
            project: crate::files::path_key(project),
            cancel: Arc::new(AtomicBool::new(true)),
            snapshot: Arc::new(Mutex::new(Snapshot {
                phase: "indexing".into(),
                ..Default::default()
            })),
            sender,
            thread: None,
            retirement: Arc::default(),
        },
    );
    assert_eq!(service.snapshot(project, project).phase, "idle");
    assert!(service
        .activate(project, project, false)
        .unwrap_err()
        .contains("Eight"));
    for worker in lock(&service.state).workers.values() {
        worker.cancel.store(true, Ordering::Release);
    }
    assert!(service
        .activate(project, project, false)
        .unwrap_err()
        .contains("Eight"));
    drop(releases);
    service.shutdown();
}

#[test]
fn runtime_locks_survive_client_replacement_and_cache_aliases() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let cancel = AtomicBool::new(false);
    let cache = root.join("codebase");
    let first = prepare_environment(&cache, Path::new("/usr/bin/true"), &root, &cancel).unwrap();
    let runtime = first.runtime.clone();
    let lock_path = runtime.join("fixture-lifetime.lock");
    let held = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&lock_path)
        .unwrap();
    held.lock().unwrap();
    drop(first);
    let alias = root.join("cache-alias");
    std::os::unix::fs::symlink(&cache, &alias).unwrap();
    let second = prepare_environment(&alias, Path::new("/usr/bin/true"), &root, &cancel).unwrap();
    assert_eq!(second.runtime, runtime);
    let successor = fs::OpenOptions::new().write(true).open(&lock_path).unwrap();
    assert!(matches!(
        successor.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    let other = prepare_environment(
        &root.join("other-app-data"),
        Path::new("/usr/bin/true"),
        &root,
        &cancel,
    )
    .unwrap();
    assert_ne!(other.runtime, runtime);
    // Only this fixture uses /usr/bin/true, so no daemon can own these paths.
    drop((held, successor));
    fs::remove_dir_all(&runtime).unwrap();
    fs::remove_dir_all(&other.runtime).unwrap();
}

#[test]
fn detached_cleanup_keeps_capacity_after_frontend_exit() {
    let (root, mut service) = fixture();
    Arc::get_mut(&mut service).unwrap().test_engine = Some("/usr/bin/false".into());
    let project = root.path().to_str().unwrap();
    service.configure(project, true).unwrap();
    let runtime = root.path().join("runtime");
    let directory = runtime.join(format!("cbm-daemon-{}", unsafe { libc::geteuid() }));
    engine::private_dir(&directory).unwrap();
    let daemon = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(directory.join("cbm-version-cohort-lifetime-v1.lock"))
        .unwrap();
    daemon.lock_shared().unwrap();
    let mut retirement = Retirement::Unconfirmed;
    retirement.observe(&runtime).unwrap();
    // The frontend is authoritatively joined, while detached cleanup is held
    // behind the same flock barrier used by the pinned engine.
    std::thread::spawn(|| {}).join().unwrap();
    let mut releases = Vec::new();
    for index in 0..8 {
        let (sender, _) = mpsc::sync_channel(2);
        let (thread, retirement) = if index == 0 {
            (None, std::mem::take(&mut retirement))
        } else {
            let (release, barrier) = mpsc::channel::<()>();
            releases.push(release);
            (
                Some(std::thread::spawn(move || {
                    let _ = barrier.recv();
                })),
                Retirement::Clear,
            )
        };
        lock(&service.state).workers.insert(
            format!("slot-{index}"),
            Worker {
                project: "other".into(),
                cancel: Arc::new(AtomicBool::new(true)),
                snapshot: Arc::default(),
                sender,
                thread,
                retirement: Arc::new(Mutex::new(retirement)),
            },
        );
    }
    assert!(service
        .activate(project, project, false)
        .unwrap_err()
        .contains("Eight"));
    daemon.unlock().unwrap();
    drop(daemon);
    service.activate(project, project, false).unwrap();
    drop(releases);
    service.shutdown();
}

#[test]
fn uncertain_startup_reservation_survives_a_failed_retry() {
    let (root, mut service) = fixture();
    Arc::get_mut(&mut service).unwrap().test_engine = Some("/usr/bin/false".into());
    let project = root.path().to_str().unwrap();
    service.configure(project, true).unwrap();
    let retirement = Arc::new(Mutex::new(Retirement::Unconfirmed));
    assert!(lock(&retirement).observe(root.path()).is_err());
    assert!(lock(&retirement).occupies_slot());
    let key = crate::files::path_key(project);
    let (sender, _) = mpsc::sync_channel(2);
    lock(&service.state).workers.insert(
        key.clone(),
        Worker {
            project: key.clone(),
            cancel: Arc::new(AtomicBool::new(true)),
            snapshot: Arc::new(Mutex::new(Snapshot {
                root: canonical(project).unwrap(),
                ..Default::default()
            })),
            sender,
            thread: None,
            retirement: retirement.clone(),
        },
    );
    service.activate(project, project, false).unwrap();
    let thread = lock(&service.state)
        .workers
        .get_mut(&key)
        .unwrap()
        .thread
        .take()
        .unwrap();
    thread.join().unwrap();
    let state = lock(&service.state);
    assert!(Arc::ptr_eq(&state.workers[&key].retirement, &retirement));
    assert!(state.workers[&key].occupies_slot());
}

#[test]
fn retargeted_path_cannot_reuse_another_canonical_roots_reservation() {
    let (root, mut service) = fixture();
    Arc::get_mut(&mut service).unwrap().test_engine = Some("/usr/bin/false".into());
    let original = root.path().join("original");
    let successor = root.path().join("successor");
    fs::create_dir(&original).unwrap();
    fs::create_dir(&successor).unwrap();
    let link = root.path().join("project");
    std::os::unix::fs::symlink(&original, &link).unwrap();
    let project = link.to_str().unwrap();
    service.configure(project, true).unwrap();
    let key = crate::files::path_key(project);
    let (sender, _) = mpsc::sync_channel(2);
    lock(&service.state).workers.insert(
        key.clone(),
        Worker {
            project: key.clone(),
            cancel: Arc::new(AtomicBool::new(true)),
            snapshot: Arc::new(Mutex::new(Snapshot {
                root: canonical(project).unwrap(),
                ..Default::default()
            })),
            sender,
            thread: None,
            retirement: Arc::new(Mutex::new(Retirement::Unconfirmed)),
        },
    );
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&successor, &link).unwrap();
    assert!(service
        .query(project, project, "index_status", json!({}))
        .unwrap_err()
        .contains("different working copy"));
    for reindex in [false, true] {
        assert!(service
            .activate(project, project, reindex)
            .unwrap_err()
            .contains("different working copy"));
    }
    *lock(&lock(&service.state).workers[&key].retirement) = Retirement::Clear;
    service.activate(project, project, false).unwrap();
    assert_eq!(
        service.snapshot(project, project).root,
        canonical(project).unwrap()
    );
    service.shutdown();
}

#[test]
fn stopped_worker_can_transfer_project_ownership() {
    let (root, mut service) = fixture();
    Arc::get_mut(&mut service).unwrap().test_engine = Some("/usr/bin/false".into());
    let project = root.path().to_str().unwrap();
    service.configure(project, true).unwrap();
    let (release, barrier) = mpsc::channel::<()>();
    let (sender, _) = mpsc::sync_channel(2);
    let key = crate::files::path_key(project);
    lock(&service.state).workers.insert(
        key.clone(),
        Worker {
            project: "previous-project".into(),
            cancel: Arc::new(AtomicBool::new(true)),
            snapshot: Arc::new(Mutex::new(Snapshot {
                root: canonical(project).unwrap(),
                ..Snapshot::default()
            })),
            sender,
            thread: Some(std::thread::spawn(move || {
                let _ = barrier.recv();
            })),
            retirement: Arc::default(),
        },
    );
    assert!(service
        .activate(project, project, false)
        .unwrap_err()
        .contains("owned by another project"));
    let previous = lock(&service.state)
        .workers
        .get_mut(&key)
        .unwrap()
        .thread
        .take()
        .unwrap();
    drop(release);
    previous.join().unwrap();
    // The retained entry still belongs to the old project, but has released its
    // physical client. A new owner can now replace it without restarting the app.
    service.activate(project, project, false).unwrap();
    assert_eq!(lock(&service.state).workers[&key].project, key);
    service.shutdown();
}

#[test]
fn disable_during_preparation_cannot_publish_a_ready_successor() {
    let (root, mut service) = fixture();
    let script = root.path().join("fixture-engine");
    let entered = root.path().join("entered");
    // The config command is held behind a real subprocess barrier. Disable has
    // its own cancellation path, independent of the worker's request queue.
    fs::write(
        &script,
        format!(
            "#!/bin/sh\ntouch '{}'\nexec /bin/sleep 60\n",
            entered.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    Arc::get_mut(&mut service).unwrap().test_engine = Some(script);
    let project = root.path().to_str().unwrap();
    service.configure(project, true).unwrap();
    service.activate(project, project, false).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !entered.exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    service.configure(project, false).unwrap();
    service.shutdown();
    assert_eq!(service.snapshot(project, project).phase, "disabled");
    assert!(lock(&service.state).workers.is_empty());
}

#[test]
#[ignore = "Requires an explicitly supplied pinned VORKSPACE_CODEBASE_TEST_ENGINE"]
fn pinned_engine_indexes_isolated_roots_and_stops_cleanly() {
    let binary = PathBuf::from(std::env::var_os("VORKSPACE_CODEBASE_TEST_ENGINE").unwrap());
    let (root, mut service) = fixture();
    Arc::get_mut(&mut service).unwrap().test_engine = Some(binary);
    let mut projects = Vec::new();
    // These roots collide under the upstream default path-to-name mapping.
    for (path, name) in [("repo-copy", "alpha"), ("repo/copy", "beta")] {
        let project = root.path().join(path);
        fs::create_dir_all(&project).unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "--quiet", "--initial-branch=main"])
            .arg(&project)
            .status()
            .unwrap()
            .success());
        fs::write(project.join("sample.py"),format!("def {name}_greet(name):\n    return 'Hello ' + name\n\ndef main():\n    return {name}_greet('world')\n")).unwrap();
        let project = project.to_str().unwrap().to_owned();
        service.configure(&project, true).unwrap();
        service.activate(&project, &project, false).unwrap();
        projects.push(project);
    }
    let deadline = Instant::now() + Duration::from_secs(60);
    for project in &projects {
        loop {
            let snapshot = service.snapshot(project, project);
            assert_ne!(snapshot.phase, "error", "{:?}", snapshot.error);
            if snapshot.phase == "ready" {
                assert_eq!(snapshot.index.unwrap()["watch"]["watched"], true);
                break;
            }
            assert!(Instant::now() < deadline, "index did not finish");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    let alias = root.path().join("alias");
    std::os::unix::fs::symlink(&projects[0], &alias).unwrap();
    let alias = alias.to_str().unwrap();
    service.configure(alias, true).unwrap();
    assert!(service
        .activate(alias, alias, false)
        .unwrap_err()
        .contains("another path"));
    let query = |project: &str, tool: &str, args: Value| {
        service
            .query(project, project, tool, args)
            .unwrap()
            .recv_blocking()
            .unwrap()
            .unwrap()
    };
    // Check both directions after initial indexing and after replacing each
    // graph. Reading the same qualified name must still use the caller's root.
    for reindex in [None, Some(0), Some(1)] {
        if let Some(index) = reindex {
            let project = &projects[index];
            service.activate(project, project, true).unwrap();
            loop {
                let snapshot = service.snapshot(project, project);
                assert_ne!(snapshot.phase, "error", "{:?}", snapshot.error);
                if snapshot.phase == "ready" {
                    break;
                }
                assert!(Instant::now() < deadline, "reindex did not finish");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        for (project, own, foreign) in [
            (&projects[0], "alpha_greet", "beta_greet"),
            (&projects[1], "beta_greet", "alpha_greet"),
        ] {
            let found = query(project, "search_graph", json!({"query":own}));
            assert!(found.to_string().contains(own), "{found}");
            let absent = query(project, "search_graph", json!({"query":foreign}));
            assert_eq!(absent["returned"], 0, "{absent}");
            let source = query(
                project,
                "get_code_snippet",
                json!({"qualified_name":"main","max_lines":20}),
            );
            let source = source["source"].as_str().unwrap();
            assert!(source.contains(own), "{source}");
            assert!(!source.contains(foreign), "{source}");
        }
    }
    service.configure(&projects[0], false).unwrap();
    assert!(service
        .query(&projects[0], &projects[0], "index_status", json!({}))
        .is_err());
    assert_eq!(
        query(&projects[1], "index_status", json!({}))["status"],
        "ready"
    );
    service.configure(&projects[0], true).unwrap();
    assert_eq!(service.snapshot(&projects[0], &projects[0]).phase, "idle");
    service.activate(&projects[0], &projects[0], false).unwrap();
    loop {
        let snapshot = service.snapshot(&projects[0], &projects[0]);
        assert_ne!(snapshot.phase, "error", "{:?}", snapshot.error);
        if snapshot.phase == "ready" {
            break;
        }
        assert!(Instant::now() < deadline, "reactivation did not finish");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(query(
        &projects[0],
        "get_code_snippet",
        json!({"qualified_name":"alpha_greet"})
    )["source"]
        .as_str()
        .unwrap()
        .contains("return 'Hello '"));
    service.shutdown();
    assert!(lock(&service.state).workers.is_empty());
}
