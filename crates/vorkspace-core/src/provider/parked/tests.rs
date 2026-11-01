
use super::*;
use std::os::unix::fs::PermissionsExt;

fn scripted_wire(script: &str, budget: frames::Budget) -> Wire {
    let request = Request {
        kind: Kind::Codex,
        binary: "python3".into(),
        cwd: std::env::temp_dir(),
        prompt: String::new(),
        provider_id: None,
        model: None,
        settings: Settings::default(),
    };
    let mut command = Command::new("python3");
    command
        .args(["-u", "-c", script])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    process::isolate(&mut command);
    Wire::spawn_with_budget(&mut command, &request, budget).unwrap()
}

#[test]
fn overflow_reports_error_and_drop_reaps_provider() {
    let wire = scripted_wire(
        "import os,time; os.write(1,b'x'*1025); time.sleep(60)",
        frames::Budget::with_limits(1024, 2048),
    );
    let pid = wire.child.id();
    let error = wire
        .rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap_err();
    assert!(error.contains("1024 byte limit"));
    drop(wire);
    assert_eq!(
        unsafe { libc::kill(pid as i32, 0) },
        -1,
        "overflow cleanup must reap the provider"
    );
}

#[test]
fn dropping_wire_cancels_a_reader_blocked_on_byte_admission() {
    let budget = frames::Budget::with_limits(1024, 2048);
    let wire = scripted_wire("import os,time; os.write(1,b'x'*511+b'\\n'+b'y'*511+b'\\n'+b'z'*511+b'\\n'); time.sleep(60)", budget.clone());
    let pid = wire.child.id();
    budget.await_waiter();
    let (done, completed) = mpsc::channel();
    let cleanup = thread::spawn(move || {
        drop(wire);
        done.send(()).unwrap();
    });
    completed
        .recv_timeout(Duration::from_secs(5))
        .expect("Drop must wake byte admission before joining the reader");
    cleanup.join().unwrap();
    assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
}

#[test]
fn retiring_leases_releases_the_pool_before_running_destructors() {
    struct Lease {
        id: usize,
        pool: std::sync::Weak<Mutex<Vec<Lease>>>,
        retired: Arc<Mutex<Vec<usize>>>,
    }
    impl Drop for Lease {
        fn drop(&mut self) {
            let pool = self.pool.upgrade().unwrap();
            assert!(
                pool.try_lock().is_ok(),
                "retirement must not lock out reuse"
            );
            self.retired.lock().unwrap().push(self.id);
        }
    }
    let pool = Arc::new(Mutex::new(Vec::new()));
    let retired = Arc::new(Mutex::new(Vec::new()));
    for id in 0..3 {
        pool.lock().unwrap().push(Lease {
            id,
            pool: Arc::downgrade(&pool),
            retired: retired.clone(),
        });
    }
    let removed = remove_matching(&pool, |entry| entry.id != 1);
    assert!(retired.lock().unwrap().is_empty());
    assert_eq!(pool.lock().unwrap()[0].id, 1);
    drop(removed);
    assert_eq!(*retired.lock().unwrap(), [0, 2]);
    drop(remove_matching(&pool, |_| true));
    assert_eq!(*retired.lock().unwrap(), [0, 2, 1]);
}

#[test]
fn parked_keys_separate_bot_identity_and_private_storage_from_regular_sessions() {
    let mut request = Request {
        kind: Kind::Codex,
        binary: PathBuf::from("codex"),
        cwd: PathBuf::from("/project"),
        prompt: "hello".into(),
        provider_id: Some("saved-thread".into()),
        model: None,
        settings: Settings::default(),
    };
    let regular = key(&request);
    request.settings.bot = Some(crate::agent::BotContext {
        bot: crate::bot::Bot {
            id: "first-bot".into(),
            ..Default::default()
        },
        root: PathBuf::from("/desktop/bots"),
        executable: PathBuf::from("vorkspace"),
    });
    let first = key(&request);
    assert_ne!(regular, first);
    request.settings.bot.as_mut().unwrap().bot.id = "other-bot".into();
    assert_ne!(first, key(&request));
    request.settings.bot.as_mut().unwrap().bot.id = "first-bot".into();
    request.settings.bot.as_mut().unwrap().root = PathBuf::from("/isolated/bots");
    assert_ne!(first, key(&request));
}

#[test]
fn reuses_only_the_same_conversation_and_launch_configuration() {
    const CHILD: &str = "VORKSPACE_PARKED_REUSE_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        // The idle limit and pool are process-wide. Other provider tests
        // must not park competing children while this test checks reuse.
        let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "provider::parked::tests::reuses_only_the_same_conversation_and_launch_configuration",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("provider");
    let trace = dir.path().join("starts");
    std::fs::write(
        &binary,
        format!(
            r#"#!/usr/bin/env python3
import json,os,sys
with open({:?},'a') as f: f.write(str(os.getpid())+'\n')
def send(v): print(json.dumps(v),flush=True)
for line in sys.stdin:
 v=json.loads(line)
 if v.get('id')==1: send({{'id':1,'result':{{}}}})
 elif v.get('id')==2: send({{'id':2,'result':{{'thread':{{'id':'settings-thread'}}}}}})
 elif v.get('id')==3:
  send({{'method':'item/agentMessage/delta','params':{{'itemId':'answer','delta':'ok'}}}})
  send({{'method':'turn/completed','params':{{'turn':{{'status':'completed'}}}}}})
"#,
            trace.to_str().unwrap()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    configure(1);
    let mut request = Request {
        kind: Kind::Codex,
        binary,
        cwd: dir.path().into(),
        prompt: "hello".into(),
        provider_id: None,
        model: None,
        settings: Settings::default(),
    };
    run(
        request.clone(),
        Arc::new(AtomicBool::new(false)),
        |_| Ok(()),
    )
    .unwrap();
    request.provider_id = Some("settings-thread".into());
    run(
        request.clone(),
        Arc::new(AtomicBool::new(false)),
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(std::fs::read_to_string(&trace).unwrap().lines().count(), 1);
    request.model = Some("different-model".into());
    run(
        request.clone(),
        Arc::new(AtomicBool::new(false)),
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(std::fs::read_to_string(&trace).unwrap().lines().count(), 2);
    let original = key(&request);
    request.settings.account.id = "isolated-account".into();
    assert_ne!(original, key(&request));
    request.settings.account = Default::default();
    request.cwd = dir.path().join("other");
    assert_ne!(original, key(&request));

    request.cwd = dir.path().into();
    for discard_tree in [true, false] {
        let parked = take(&original).unwrap();
        let binding = Binding::request(&parked.protocol.request);
        let epoch = *epochs().lock().unwrap().entry(binding.clone()).or_default();
        if discard_tree {
            discard_worktree(&request.cwd);
        } else {
            discard_session(&request);
        }
        // Deterministically deliver the old cancellation's terminal frame
        // after invalidation. It must never publish its lease again.
        park_cancelled(parked.wire, parked.protocol, &binding, epoch);
        assert!(take(&original).is_none());
        run(
            request.clone(),
            Arc::new(AtomicBool::new(false)),
            |_| Ok(()),
        )
        .unwrap();
    }
    assert_eq!(std::fs::read_to_string(&trace).unwrap().lines().count(), 4);
    configure(0);
    drop(remove_matching(idle_pool(), |_| true));
}
