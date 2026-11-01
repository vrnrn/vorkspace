//! Bounded, expiring provider processes. Each parked process belongs to one
//! conversation, account, directory and launch configuration.
use super::*;
use std::sync::{atomic::AtomicUsize, OnceLock};
type WriteRequest = (Vec<u8>, mpsc::Sender<std::io::Result<()>>);
pub(super) struct Input {
    sender: Option<mpsc::Sender<WriteRequest>>,
}
impl Write for Input {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let (reply, result) = mpsc::channel();
        self.sender
            .as_ref()
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::BrokenPipe, "Provider input closed")
            })?
            .send((bytes.to_vec(), reply))
            .map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::BrokenPipe, "Provider input closed")
            })?;
        result
            .recv_timeout(Duration::from_secs(15))
            .map_err(|error| {
                std::io::Error::new(
                    if error == mpsc::RecvTimeoutError::Timeout {
                        std::io::ErrorKind::TimedOut
                    } else {
                        std::io::ErrorKind::BrokenPipe
                    },
                    "Provider stdin write failed or timed out",
                )
            })??;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub(super) struct Wire {
    pub child: std::process::Child,
    pub stdin: Input,
    pub rx: frames::Receiver<Result<frames::Frame, String>>,
    pub tail: Arc<Mutex<Vec<u8>>>,
    reader: Option<thread::JoinHandle<()>>,
    stderr_reader: Option<thread::JoinHandle<()>>,
    writer: Option<thread::JoinHandle<()>>,
    _registration: process::Registration,
    _account: crate::accounts::Tracked,
    _copilot_gateway: Option<super::copilot::Gateway>,
}
impl Wire {
    pub fn spawn(command: &mut Command, request: &Request) -> Result<Self, String> {
        Self::spawn_with_budget(command, request, frames::Budget::new())
    }
    fn spawn_with_budget(
        command: &mut Command,
        request: &Request,
        budget: frames::Budget,
    ) -> Result<Self, String> {
        let gateway = super::copilot::configure(command, request)?;
        let mut child = command.spawn().map_err(|e| {
            format!(
                "Cannot start {} at {}: {e}",
                request.kind.label(),
                request.binary.display()
            )
        })?;
        let registration = process::register(child.id());
        let account = crate::accounts::track(child.id(), request.kind, &request.settings.account);
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        // Provider input has a 15s write deadline. A wedged child
        // must not leave a worker stuck forever in a full stdin pipe.
        let (writes, requests) = mpsc::channel::<WriteRequest>();
        let writer = thread::spawn(move || {
            while let Ok((bytes, reply)) = requests.recv() {
                let result = stdin.write_all(&bytes).and_then(|_| stdin.flush());
                let failed = result.is_err();
                let _ = reply.send(result);
                if failed {
                    break;
                }
            }
        });
        let (tx, rx) = frames::channel(budget.clone());
        let reader = thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match frames::read_line(&mut reader, &budget) {
                    Ok(None) => break,
                    Ok(Some(bytes)) => {
                        if tx.send(Ok(bytes)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = tx.send(Err(error));
                        break;
                    }
                }
            }
        });
        let tail = Arc::new(Mutex::new(Vec::new()));
        let stderr_tail = tail.clone();
        let stderr_reader = thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut chunk = [0; 4096];
            while let Ok(n) = reader.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                let mut tail = stderr_tail.lock().unwrap();
                tail.extend_from_slice(&chunk[..n]);
                let excess = tail.len().saturating_sub(8192);
                tail.drain(..excess);
            }
        });
        Ok(Self {
            child,
            stdin: Input {
                sender: Some(writes),
            },
            rx,
            tail,
            reader: Some(reader),
            stderr_reader: Some(stderr_reader),
            writer: Some(writer),
            _registration: registration,
            _account: account,
            _copilot_gateway: gateway,
        })
    }
}
impl Drop for Wire {
    fn drop(&mut self) {
        // Wake both byte-admission and count-admission waiters before joining.
        self.rx.close();
        process::terminate(&mut self.child);
        self.stdin.sender.take();
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        if let Some(t) = self.reader.take() {
            let _ = t.join();
        }
        if let Some(t) = self.stderr_reader.take() {
            let _ = t.join();
        }
    }
}
pub(super) struct Parked {
    pub wire: Wire,
    pub protocol: Protocol,
    pub key: String,
    at: Instant,
    owner: Option<i64>,
}
static LIMIT: AtomicUsize = AtomicUsize::new(0);
fn selected_sessions() -> &'static Mutex<std::collections::HashSet<i64>> {
    static SELECTED: OnceLock<Mutex<std::collections::HashSet<i64>>> = OnceLock::new();
    SELECTED.get_or_init(Default::default)
}
/// Snapshot the conversations currently shown, including split panes.
pub fn keep(ids: Vec<i64>) {
    let next = ids.into_iter().collect();
    let mut selected = selected_sessions()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if *selected == next {
        return;
    }
    *selected = next;
    drop(selected);
    // Selection runs on the UI thread; child reaping belongs to a worker.
    thread::spawn(enforce_idle_limit);
}
fn idle_pool() -> &'static Mutex<Vec<Parked>> {
    static POOL: OnceLock<Mutex<Vec<Parked>>> = OnceLock::new();
    POOL.get_or_init(Default::default)
}

/// Removing a lease transfers its ownership to the caller. Reaping a child
/// and joining its I/O threads must happen after the pool lock is released.
fn remove_matching<T>(pool: &Mutex<Vec<T>>, remove: impl FnMut(&mut T) -> bool) -> Vec<T> {
    pool.lock().unwrap().extract_if(.., remove).collect()
}
#[derive(Clone, PartialEq, Eq, Hash)]
struct Binding {
    kind: Kind,
    session: Option<String>,
    account: String,
    cwd: PathBuf,
}
impl Binding {
    fn request(request: &Request) -> Self {
        Self {
            kind: request.kind,
            session: request.provider_id.clone(),
            account: request.settings.account.id.clone(),
            cwd: request.cwd.clone(),
        }
    }
}
fn epochs() -> &'static Mutex<HashMap<Binding, u64>> {
    static EPOCHS: OnceLock<Mutex<HashMap<Binding, u64>>> = OnceLock::new();
    EPOCHS.get_or_init(Default::default)
}
pub fn configure(limit: usize) {
    LIMIT.store(limit.min(5), Ordering::Relaxed);
    thread::spawn(enforce_idle_limit);
}

/// The desktop registry applies one idle-child budget across every provider.
/// Serialize eviction decisions, but never hold either pool while reaping.
pub(super) fn enforce_idle_limit() {
    static EVICT: OnceLock<Mutex<()>> = OnceLock::new();
    let _evict = EVICT.get_or_init(Default::default).lock().unwrap();
    let mut entries: Vec<_> = idle_pool()
        .lock()
        .unwrap()
        .iter()
        .map(|entry| (entry.wire.child.id(), entry.at, entry.owner))
        .collect();
    entries.sort_by_key(|(_, at, _)| *at);
    let limit = LIMIT.load(Ordering::Relaxed);
    let excess = entries.len().saturating_sub(limit);
    let selected = selected_sessions()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let retire: std::collections::HashSet<_> = entries
        .into_iter()
        .filter(|(_, _, owner)| limit == 0 || owner.is_none_or(|owner| !selected.contains(&owner)))
        .take(excess)
        .map(|(pid, _, _)| pid)
        .collect();
    if retire.is_empty() {
        return;
    }
    let retired = remove_matching(idle_pool(), |entry| retire.contains(&entry.wire.child.id()));
    drop(retired);
}
pub(super) fn key(request: &Request) -> String {
    let mut settings = request.settings.clone();
    settings.turns.clear();
    settings.attachments.clear();
    settings.revision = 0;
    settings.handoff = None;
    settings.pending_provider_switch = None;
    json!([
        process::owner(),
        request.kind.id(),
        request.binary,
        request.cwd,
        request.provider_id,
        request.model,
        settings,
        request.settings.environment,
        request
            .settings
            .bot
            .as_ref()
            .map(|bot| (&bot.root, &bot.bot.id)),
        request.settings.disable_claude_hooks
    ])
    .to_string()
}
pub(super) fn take(key: &str) -> Option<Parked> {
    if LIMIT.load(Ordering::Relaxed) == 0 {
        return None;
    }
    let mut parked = {
        let mut pool = idle_pool().lock().unwrap();
        let ix = pool.iter().position(|p| p.key == key)?;
        pool.remove(ix)
    };
    if parked.at.elapsed() >= Duration::from_secs(180)
        || parked.wire.child.try_wait().ok().flatten().is_some()
    {
        return None;
    }
    process::mark_idle(parked.wire.child.id(), false);
    Some(parked)
}
pub(super) fn put(wire: Wire, protocol: Protocol) {
    put_if_current(wire, protocol, None);
}

fn put_if_current(wire: Wire, mut protocol: Protocol, expected: Option<(&Binding, u64)>) {
    if LIMIT.load(Ordering::Relaxed) == 0 || protocol.request.provider_id.is_none() {
        return;
    }
    let key = key(&protocol.request);
    protocol.emitted.clear();
    protocol.tools.clear();
    protocol.active_compaction = None;
    protocol.claude_tools.clear();
    protocol.request.prompt.clear();
    protocol.request.settings.attachments.clear();
    protocol.request.settings.turns.clear();
    process::mark_idle(wire.child.id(), true);
    {
        // Serialize a cancelled lease's publication with binding invalidation,
        // but release both locks before eviction can reap another process.
        let epochs = expected.map(|_| epochs().lock().unwrap());
        if expected.is_some_and(|(binding, epoch)| {
            epochs
                .as_ref()
                .unwrap()
                .get(binding)
                .copied()
                .unwrap_or_default()
                != epoch
        }) {
            return;
        }
        idle_pool().lock().unwrap().push(Parked {
            wire,
            protocol,
            key,
            at: Instant::now(),
            owner: process::owner(),
        });
    }
    enforce_idle_limit();
    static WATCH: OnceLock<()> = OnceLock::new();
    WATCH.get_or_init(|| {
        thread::spawn(|| loop {
            thread::sleep(Duration::from_secs(1));
            let limit = LIMIT.load(Ordering::Relaxed);
            let retired = remove_matching(idle_pool(), |entry| {
                limit == 0
                    || entry.at.elapsed() >= Duration::from_secs(180)
                    || entry.wire.child.try_wait().ok().flatten().is_some()
            });
            drop(retired);
            enforce_idle_limit();
        });
    });
}

/// Stop settles after the provider interrupt is written/acknowledged. Keep the
/// old pipe muted until its terminal frame before making it reusable; otherwise
/// that frame could complete a new user prompt on the same process.
pub(super) fn settle_cancelled(mut wire: Wire, mut protocol: Protocol) {
    if LIMIT.load(Ordering::Relaxed) == 0
        || (protocol.request.kind == Kind::Codex && protocol.active_turn.is_none())
        || (protocol.request.kind == Kind::Claude && !protocol.claude_prompt_sent)
    {
        return;
    }
    let owner = process::owner();
    let binding = Binding::request(&protocol.request);
    let epoch = *epochs().lock().unwrap().entry(binding.clone()).or_default();
    process::mark_idle(wire.child.id(), true);
    thread::spawn(move || {
        let settle = move || {
            let started = Instant::now();
            // This is the existing idle-child retention period after Stop,
            // not a deadline on an active provider conversation.
            while started.elapsed() < Duration::from_secs(180)
                && LIMIT.load(Ordering::Relaxed) > 0
                && epochs()
                    .lock()
                    .unwrap()
                    .get(&binding)
                    .copied()
                    .unwrap_or_default()
                    == epoch
            {
                match wire.rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(Ok(bytes)) => {
                        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                            continue;
                        };
                        let kind = protocol.request.kind;
                        let turn = value["params"]["turnId"]
                            .as_str()
                            .or_else(|| value["params"]["turn"]["id"].as_str());
                        let thread = value["params"]["threadId"].as_str();
                        let codex_terminal = kind == Kind::Codex
                            && matches!(
                                value["method"].as_str(),
                                Some("turn/completed" | "turn/aborted")
                            )
                            && turn.is_none_or(|id| Some(id) == protocol.active_turn.as_deref())
                            && thread.is_none_or(|id| {
                                Some(id) == protocol.request.provider_id.as_deref()
                            });
                        let claude_terminal = kind == Kind::Claude && value["type"] == "result";
                        if codex_terminal || claude_terminal {
                            // Discard already-buffered trailing frames too.
                            while wire.rx.try_recv().is_ok() {}
                            protocol.done = true;
                            park_cancelled(wire, protocol, &binding, epoch);
                            return;
                        }
                        if kind == Kind::Codex
                            && matches!(
                                value["method"].as_str(),
                                Some("turn/completed" | "turn/aborted")
                            )
                        {
                            continue;
                        }
                        // A stopped tool may still ask for input while it
                        // unwinds. Resolve it without resurfacing a UI card.
                        if let Ok(events) = protocol.frame(value, &mut wire.stdin) {
                            for event in events {
                                let id = match event {
                                    Event::Approval(approval) => Some(approval.id),
                                    Event::Question(question) => Some(question.id),
                                    _ => None,
                                };
                                if let Some(id) = id {
                                    let _ = protocol.respond(
                                        Control {
                                            id,
                                            decision: Decision::Deny,
                                        },
                                        &mut wire.stdin,
                                    );
                                }
                            }
                        }
                        if protocol.request.kind.is_pi() {
                            let _ = protocol.pi_tick(&mut wire.stdin);
                        }
                        if protocol.done {
                            while wire.rx.try_recv().is_ok() {}
                            park_cancelled(wire, protocol, &binding, epoch);
                            return;
                        }
                    }
                    Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if !matches!(wire.child.try_wait(), Ok(None)) {
                            return;
                        }
                        if protocol.request.kind.is_pi() {
                            let _ = protocol.pi_tick(&mut wire.stdin);
                            if protocol.done {
                                park_cancelled(wire, protocol, &binding, epoch);
                                return;
                            }
                        }
                    }
                }
            }
        };
        if let Some(owner) = owner {
            process::owned_by(owner, settle)
        } else {
            settle()
        }
    });
}

fn park_cancelled(wire: Wire, protocol: Protocol, binding: &Binding, epoch: u64) {
    put_if_current(wire, protocol, Some((binding, epoch)));
}

pub fn discard_worktree(path: &Path) {
    let root = path.to_string_lossy();
    let retired = {
        let mut epochs = epochs().lock().unwrap();
        for (binding, epoch) in epochs.iter_mut() {
            if crate::files::is_equal_or_inside(&binding.cwd.to_string_lossy(), &root) {
                *epoch += 1;
            }
        }
        remove_matching(idle_pool(), |entry| {
            crate::files::is_equal_or_inside(&entry.protocol.request.cwd.to_string_lossy(), &root)
        })
    };
    drop(retired);
}

pub(super) fn forget_session(kind: Kind, session: &str, account: &str, cwd: &Path) {
    let binding = Binding {
        kind,
        session: Some(session.into()),
        account: account.into(),
        cwd: cwd.into(),
    };
    let retired = {
        let mut epochs = epochs().lock().unwrap();
        *epochs.entry(binding.clone()).or_default() += 1;
        remove_matching(idle_pool(), |entry| {
            Binding::request(&entry.protocol.request) == binding
        })
    };
    if !retired.is_empty() {
        thread::spawn(move || drop(retired));
    }
}

/// A rewind changes provider context independently of the next launch options.
/// Evict every warm configuration for that binding, including an older model
/// or permission mode which the exact launch-key lookup would miss.
pub(super) fn discard_session(request: &Request) {
    let binding = Binding::request(request);
    let retired = {
        let mut epochs = epochs().lock().unwrap();
        *epochs.entry(binding.clone()).or_default() += 1;
        remove_matching(idle_pool(), |entry| {
            Binding::request(&entry.protocol.request) == binding
        })
    };
    drop(retired);
}

#[cfg(test)]
mod tests {
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
}
