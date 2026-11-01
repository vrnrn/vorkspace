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
mod tests;
