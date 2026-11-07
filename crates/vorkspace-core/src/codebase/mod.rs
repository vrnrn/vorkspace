//! Optional local code intelligence. One owner controls durable opt-in, private
//! engine clients, and project/worktree scoping for both UI and app CLI callers.
mod engine;
mod query;
mod transport;
use crate::store::Store;
pub use query::tools;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::{File, OpenOptions},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::JoinHandle,
    time::Duration,
};

const SETTING: &str = "codebase-enabled-projects";
type Database = Arc<Mutex<Store>>;
pub type Reply = async_channel::Receiver<Result<Value, String>>;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub enabled: bool,
    pub phase: String,
    pub root: String,
    pub error: Option<String>,
    pub index: Option<Value>,
}

struct Request {
    tool: String,
    arguments: Value,
    reply: async_channel::Sender<Result<Value, String>>,
}
struct Worker {
    project: String,
    cancel: Arc<AtomicBool>,
    snapshot: Arc<Mutex<Snapshot>>,
    sender: mpsc::SyncSender<Request>,
    thread: Option<JoinHandle<()>>,
    retirement: Arc<Mutex<Retirement>>,
}
impl Worker {
    fn occupies_slot(&self) -> bool {
        self.thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
            || lock(&self.retirement).occupies_slot()
    }
}

#[derive(Default)]
enum Retirement {
    #[default]
    Clear,
    // A failed handshake cannot prove whether a detached daemon was launched.
    // Preserve this reservation through retries instead of treating it as absent.
    Unconfirmed,
    Observed(File),
}

impl Retirement {
    fn observe(&mut self, runtime: &Path) -> Result<(), String> {
        let uid = unsafe { libc::geteuid() };
        let directory = runtime.join(format!("cbm-daemon-{uid}"));
        let metadata = std::fs::symlink_metadata(&directory).map_err(|e| e.to_string())?;
        if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
            return Err("Codebase lifetime directory is not private".into());
        }
        // Pinned v0.12.1 holds a shared flock through daemon job, watcher and
        // runtime cleanup (src/daemon/host.c). Retain this inode, never unlink it.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(directory.join("cbm-version-cohort-lifetime-v1.lock"))
            .map_err(|e| e.to_string())?;
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file()
            || metadata.uid() != uid
            || metadata.mode() & 0o077 != 0
            || metadata.nlink() != 1
        {
            return Err("Codebase lifetime lock is not private".into());
        }
        *self = Self::Observed(file);
        Ok(())
    }

    fn occupies_slot(&mut self) -> bool {
        match self {
            Self::Clear => false,
            Self::Unconfirmed => true,
            Self::Observed(file) => {
                if file.try_lock().is_err() || file.unlock().is_err() {
                    return true;
                }
                // Exclusive acquisition is the cleanup barrier. Release it
                // immediately, including in any temporarily inherited FD copy.
                *self = Self::Clear;
                false
            }
        }
    }
}
struct State {
    enabled: HashSet<String>,
    workers: HashMap<String, Worker>,
    closed: bool,
}
struct Environment {
    cache: PathBuf,
    runtime: PathBuf,
}

pub struct Service {
    store: Database,
    directory: PathBuf,
    state: Mutex<State>,
    changes: async_channel::Sender<()>,
    #[cfg(test)]
    test_engine: Option<PathBuf>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
fn canonical(path: &str) -> Result<String, String> {
    if crate::files::remote::is_remote(Path::new(path)) {
        return Err("Codebase Memory currently supports local projects".into());
    }
    let path = Path::new(path)
        .canonicalize()
        .map_err(|e| format!("Working copy unavailable: {e}"))?;
    if !path.is_dir() {
        return Err("Choose a project directory".into());
    }
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| "Project path is not UTF-8".into())
}

impl Service {
    /// Loading opt-in state has no filesystem or subprocess side effects.
    pub fn new(
        store: Database,
        directory: PathBuf,
    ) -> Result<(Arc<Self>, async_channel::Receiver<()>), String> {
        let enabled = lock(&store)
            .setting(SETTING)?
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .map_err(|e| format!("Invalid Codebase settings: {e}"))?
            .unwrap_or_default();
        let (changes, receiver) = async_channel::bounded(1);
        Ok((
            Arc::new(Self {
                store,
                directory,
                state: Mutex::new(State {
                    enabled,
                    workers: HashMap::new(),
                    closed: false,
                }),
                changes,
                #[cfg(test)]
                test_engine: None,
            }),
            receiver,
        ))
    }

    pub fn enabled(&self, project: &str) -> bool {
        lock(&self.state)
            .enabled
            .contains(&crate::files::path_key(project))
    }

    pub fn configure(&self, project: &str, enabled: bool) -> Result<(), String> {
        if enabled {
            canonical(project)?;
        }
        let project = crate::files::path_key(project);
        let mut state = lock(&self.state);
        if state.closed {
            return Err("Codebase Memory is shutting down".into());
        }
        let mut next = state.enabled.clone();
        if enabled {
            next.insert(project.clone());
        } else {
            next.remove(&project);
        }
        lock(&self.store).save_setting(
            SETTING,
            &serde_json::to_string(&next).map_err(|e| e.to_string())?,
        )?;
        state.enabled = next;
        if !enabled {
            for worker in state.workers.values().filter(|w| w.project == project) {
                worker.cancel.store(true, Ordering::Release);
            }
        }
        let _ = self.changes.try_send(());
        Ok(())
    }

    pub fn snapshot(&self, project: &str, root: &str) -> Snapshot {
        let state = lock(&self.state);
        let enabled = state.enabled.contains(&crate::files::path_key(project));
        let mut snapshot = state
            .workers
            .get(&crate::files::path_key(root))
            .filter(|w| w.project == crate::files::path_key(project))
            .filter(|w| !w.cancel.load(Ordering::Acquire))
            .map(|w| lock(&w.snapshot).clone())
            .unwrap_or_else(|| Snapshot {
                phase: "idle".into(),
                root: root.into(),
                ..Snapshot::default()
            });
        snapshot.enabled = enabled;
        if !enabled {
            snapshot.phase = "disabled".into();
            snapshot.error = None;
            snapshot.index = None;
        }
        snapshot
    }

    /// Run off the UI thread: validating a linked worktree may invoke Git.
    pub fn activate(&self, project: &str, root: &str, reindex: bool) -> Result<(), String> {
        let project_key = crate::files::path_key(project);
        let root_key = crate::files::path_key(root);
        let project = canonical(project)?;
        let root = canonical(root)?;
        if project != root
            && !crate::git::worktrees(Path::new(&project))?
                .iter()
                .any(|w| canonical(&w.path).is_ok_and(|p| p == root))
        {
            return Err("Working copy does not belong to this project".into());
        }
        let mut state = lock(&self.state);
        if state.closed || !state.enabled.contains(&project_key) {
            return Err("Enable Codebase Memory in this project's settings first".into());
        }
        // Aliases share the canonical cache. Do not let a second lexical path
        // start an independent daemon while that cache's owner is still alive.
        if state.workers.iter().any(|(key, worker)| {
            key != &root_key && worker.occupies_slot() && lock(&worker.snapshot).root == root
        }) {
            return Err("This working copy is active through another path. Disable that project's Codebase Memory and wait for it to stop before retrying.".into());
        }
        if let Some(worker) = state.workers.get(&root_key) {
            if worker.project != project_key && worker.occupies_slot() {
                return Err("This working copy is already owned by another project".into());
            }
            if !reindex
                && !worker.cancel.load(Ordering::Acquire)
                && !worker.thread.as_ref().is_none_or(|t| t.is_finished())
            {
                return Ok(());
            }
        }
        // Retiring daemons retain capacity after their stdio frontend exits.
        // Same-root replacement joins its predecessor and uses the same engine
        // locks, so it can reuse the predecessor's reservation.
        let replacing_live_worker = state
            .workers
            .get(&root_key)
            .is_some_and(Worker::occupies_slot);
        if !replacing_live_worker
            && state
                .workers
                .values()
                .filter(|worker| worker.occupies_slot())
                .count()
                >= 8
        {
            return Err("Eight working copies are still active or stopping. Disable Codebase Memory for an unused project or retry after it stops.".into());
        }
        let mut previous_worker = state.workers.remove(&root_key);
        let retirement = previous_worker
            .as_ref()
            .map(|worker| worker.retirement.clone())
            .unwrap_or_default();
        let previous = Arc::new(Mutex::new(previous_worker.as_mut().and_then(|worker| {
            worker.cancel.store(true, Ordering::Release);
            worker.thread.take()
        })));
        let predecessor = previous.clone();
        let snapshot = Arc::new(Mutex::new(Snapshot {
            enabled: true,
            phase: "preparing".into(),
            root: root.clone(),
            ..Snapshot::default()
        }));
        let cancel = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::sync_channel(2);
        let mut worker = Worker {
            project: project_key,
            cancel: cancel.clone(),
            snapshot: snapshot.clone(),
            sender,
            thread: None,
            retirement: retirement.clone(),
        };
        let directory = self.directory.clone();
        let changes = self.changes.clone();
        let key = root_key;
        #[cfg(test)]
        let test_engine = self.test_engine.clone();
        let spawned = std::thread::Builder::new()
            .name("codebase".into())
            .spawn(move || {
                // Join the previous frontend. Its detached daemon cleanup is
                // tracked separately by the retained retirement reservation.
                if let Some(previous) = lock(&predecessor).take() {
                    let _ = previous.join();
                }
                let publish = |phase: &str, index: Option<Value>, error: Option<String>| {
                    if !cancel.load(Ordering::Acquire) {
                        *lock(&snapshot) = Snapshot {
                            enabled: true,
                            phase: phase.into(),
                            root: root.clone(),
                            index,
                            error,
                        };
                        let _ = changes.try_send(());
                    }
                };
                let result = (|| {
                    engine::check_cancel(&cancel)?;
                    #[cfg(test)]
                    let binary = if let Some(path) = test_engine {
                        path
                    } else {
                        engine::ensure(&directory, &cancel)?
                    };
                    #[cfg(not(test))]
                    let binary = engine::ensure(&directory, &cancel)?;
                    let env = prepare_environment(&directory, &binary, Path::new(&root), &cancel)?;
                    let mut command =
                        transport::command(&binary, &env.cache, &env.runtime, Path::new(&root));
                    command.arg("--ui=false");
                    *lock(&retirement) = Retirement::Unconfirmed;
                    let mut client = transport::Client::connect(command, &cancel)?;
                    lock(&retirement).observe(&env.runtime)?;
                    publish("indexing", None, None);
                    let indexed = client.call(
                        "index_repository",
                        json!({"repo_path":root,"mode":"full","persistence":false,"async":false}),
                        &cancel,
                        Duration::from_secs(1800),
                    )?;
                    let name = indexed["project"]
                        .as_str()
                        .ok_or("Index did not identify its project")?
                        .to_owned();
                    let status = client.call(
                        "index_status",
                        json!({"project":name,"format":"json","diagnostics":"summary"}),
                        &cancel,
                        Duration::from_secs(30),
                    )?;
                    if status["root_path"]
                        .as_str()
                        .and_then(|p| canonical(p).ok())
                        .as_deref()
                        != Some(root.as_str())
                    {
                        return Err("Index belongs to a different working copy".into());
                    }
                    publish("ready", Some(status), None);
                    loop {
                        engine::check_cancel(&cancel)?;
                        let request: Request = match receiver.recv_timeout(Duration::from_secs(1)) {
                            Ok(request) => request,
                            Err(mpsc::RecvTimeoutError::Timeout) => {
                                client.idle(&cancel)?;
                                continue;
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        };
                        let result = query::arguments(
                            &request.tool,
                            request.arguments,
                            &name,
                            Path::new(&root),
                        )
                        .and_then(|args| {
                            client.call(&request.tool, args, &cancel, Duration::from_secs(30))
                        });
                        let result = engine::check_cancel(&cancel).and(result);
                        let _ = request.reply.try_send(result);
                        if client.broken {
                            return Err(
                                "Codebase connection interrupted. Retry the index to reconnect."
                                    .into(),
                            );
                        }
                    }
                    Ok::<(), String>(())
                })();
                if let Err(error) = result {
                    publish("error", None, Some(error));
                }
            });
        worker.thread = Some(match spawned {
            Ok(thread) => thread,
            Err(error) => {
                if let Some(mut old) = previous_worker {
                    old.thread = lock(&previous).take();
                    state.workers.insert(key, old);
                }
                return Err(error.to_string());
            }
        });
        state.workers.insert(key, worker);
        let _ = self.changes.try_send(());
        Ok(())
    }

    pub fn query(
        &self,
        project: &str,
        root: &str,
        tool: &str,
        arguments: Value,
    ) -> Result<Reply, String> {
        let state = lock(&self.state);
        let project = crate::files::path_key(project);
        if state.closed || !state.enabled.contains(&project) {
            return Err("Codebase Memory is disabled for this project".into());
        }
        let worker = state
            .workers
            .get(&crate::files::path_key(root))
            .filter(|w| w.project == project)
            .ok_or("Index this working copy first")?;
        if worker.cancel.load(Ordering::Acquire) || lock(&worker.snapshot).phase != "ready" {
            return Err("Codebase Memory is not ready; check its status".into());
        }
        let (reply, receiver) = async_channel::bounded(1);
        worker
            .sender
            .try_send(Request {
                tool: tool.into(),
                arguments,
                reply,
            })
            .map_err(|_| "Codebase Memory is busy; retry shortly")?;
        Ok(receiver)
    }

    /// Called off the UI thread before the app's general child-process cleanup.
    pub fn shutdown(&self) {
        let workers = {
            let mut state = lock(&self.state);
            state.closed = true;
            std::mem::take(&mut state.workers)
        };
        for worker in workers.values() {
            worker.cancel.store(true, Ordering::Release);
        }
        for (_, mut worker) in workers {
            if let Some(thread) = worker.thread.take() {
                let _ = thread.join();
            }
        }
    }
}

fn prepare_environment(
    directory: &Path,
    binary: &Path,
    root: &Path,
    cancel: &AtomicBool,
) -> Result<Environment, String> {
    engine::check_cancel(cancel)?;
    let indexes = directory.join("indexes");
    engine::private_dir(&indexes)?;
    // Default upstream names flatten path punctuation, and its watcher only
    // follows those names. Isolate the entire cache/daemon namespace by the
    // canonical root rather than overriding the name and losing file watching.
    let key = format!("{:x}", Sha256::digest(root.as_os_str().as_encoded_bytes()));
    let cache = indexes.join(key);
    engine::private_dir(&cache)?;
    // The stdio frontend can exit before its detached daemon finishes draining.
    // Keep one rendezvous for this cache across client AND app restarts so the
    // engine's lifetime/project locks serialize every successor. Never remove
    // it on client drop. A 128-bit digest prefix keeps Darwin socket paths short,
    // including the engine's per-user subdirectory and endpoint suffix.
    let canonical_cache = cache.canonicalize().map_err(|e| e.to_string())?;
    let runtime_key = format!(
        "{:x}",
        Sha256::digest(canonical_cache.as_os_str().as_encoded_bytes())
    );
    let runtime = Path::new("/tmp").join(format!("vcbm-{}", &runtime_key[..32]));
    engine::private_dir(&runtime)?;
    // --ui=false alone is applied after daemon startup. Persist this setting
    // before any MCP session so there is never a transient HTTP listener.
    let mut command = transport::command(binary, &cache, &runtime, root);
    command.args(["config", "set", "ui_enabled", "false"]);
    transport::Client::configure(command, cancel)?;
    Ok(Environment { cache, runtime })
}

impl Drop for Service {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests;
