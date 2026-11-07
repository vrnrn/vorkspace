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
    for name in ["alpha", "beta"] {
        let project = root.path().join(name);
        fs::create_dir(&project).unwrap();
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
    let query = |project: &str, tool: &str, args: Value| {
        service
            .query(project, project, tool, args)
            .unwrap()
            .recv_blocking()
            .unwrap()
            .unwrap()
    };
    let found = query(
        &projects[0],
        "search_graph",
        json!({"query":"alpha_greet","format":"json"}),
    );
    assert!(found.to_string().contains("alpha_greet"), "{found}");
    let absent = query(
        &projects[0],
        "search_graph",
        json!({"query":"beta_greet","format":"json"}),
    );
    assert_eq!(absent["returned"], 0, "{absent}");
    let source = query(
        &projects[0],
        "get_code_snippet",
        json!({"qualified_name":"alpha_greet","max_lines":20}),
    );
    assert!(source["source"]
        .as_str()
        .unwrap()
        .contains("return 'Hello '"));
    service.configure(&projects[0], false).unwrap();
    assert!(service
        .query(&projects[0], &projects[0], "index_status", json!({}))
        .is_err());
    assert_eq!(
        query(&projects[1], "index_status", json!({}))["status"],
        "ready"
    );
    service.shutdown();
    assert!(lock(&service.state).workers.is_empty());
}
