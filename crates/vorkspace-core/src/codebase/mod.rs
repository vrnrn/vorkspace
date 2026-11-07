//! Optional local code intelligence. One owner controls durable opt-in, private
//! engine clients, and project/worktree scoping for both UI and app CLI callers.
mod engine;
mod query;
mod transport;
use crate::store::Store;
pub use query::tools;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
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
}
impl Worker {
    fn occupies_slot(&self) -> bool {
        self.thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
    }
}
struct State {
    enabled: HashSet<String>,
    workers: HashMap<String, Worker>,
    closed: bool,
}
struct Environment {
    cache: PathBuf,
    runtime: tempfile::TempDir,
}

pub struct Service {
    store: Database,
    directory: PathBuf,
    state: Mutex<State>,
    environment: Arc<Mutex<Option<Arc<Environment>>>>,
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
                environment: Arc::new(Mutex::new(None)),
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
        if let Some(worker) = state.workers.get(&root_key) {
            if worker.project != project_key {
                return Err("This working copy is already owned by another project".into());
            }
            if !reindex
                && !worker.cancel.load(Ordering::Acquire)
                && !worker.thread.as_ref().is_none_or(|t| t.is_finished())
            {
                return Ok(());
            }
        }
        // Draining workers retain their slot until they have actually stopped.
        // A replacement can reuse its predecessor's slot because it joins that
        // predecessor before starting an engine client.
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
        let previous = state.workers.remove(&root_key).and_then(|mut worker| {
            worker.cancel.store(true, Ordering::Release);
            worker.thread.take()
        });
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
        };
        let directory = self.directory.clone();
        let environment = self.environment.clone();
        let changes = self.changes.clone();
        let key = root_key;
        #[cfg(test)]
        let test_engine = self.test_engine.clone();
        worker.thread = Some(
            std::thread::Builder::new()
                .name("codebase".into())
                .spawn(move || {
                    // A replacement waits until its predecessor has released
                    // the request-owned index job and watcher subscription.
                    if let Some(previous) = previous {
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
                        let env = prepare_environment(
                            &environment, &directory, &binary, Path::new(&root), &cancel,
                        )?;
                        let mut command = transport::command(
                            &binary, &env.cache, env.runtime.path(), Path::new(&root),
                        );
                        command.arg("--ui=false");
                        let mut client = transport::Client::connect(command, &cancel)?;
                        publish("indexing", None, None);
                        let indexed = client.call(
                            "index_repository",
                            json!({"repo_path":root,"mode":"full","persistence":false,"async":false}),
                            &cancel,
                            Duration::from_secs(1800),
                        )?;
                        let name = indexed["project"].as_str()
                            .ok_or("Index did not identify its project")?.to_owned();
                        let status = client.call(
                            "index_status",
                            json!({"project":name,"format":"json","diagnostics":"summary"}),
                            &cancel,
                            Duration::from_secs(30),
                        )?;
                        if status["root_path"].as_str().and_then(|p| canonical(p).ok()).as_deref()
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
                                &request.tool, request.arguments, &name, Path::new(&root),
                            ).and_then(|args| client.call(
                                &request.tool, args, &cancel, Duration::from_secs(30),
                            ));
                            let result = engine::check_cancel(&cancel).and(result);
                            let _ = request.reply.try_send(result);
                            if client.broken {
                                return Err("Codebase connection interrupted. Retry the index to reconnect.".into());
                            }
                        }
                        Ok::<(), String>(())
                    })();
                    if let Err(error) = result {
                        publish("error", None, Some(error));
                    }
                })
                .map_err(|e| e.to_string())?,
        );
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
        lock(&self.environment).take();
    }
}

fn prepare_environment(
    shared: &Mutex<Option<Arc<Environment>>>,
    directory: &Path,
    binary: &Path,
    root: &Path,
    cancel: &AtomicBool,
) -> Result<Arc<Environment>, String> {
    let mut shared = loop {
        engine::check_cancel(cancel)?;
        match shared.try_lock() {
            Ok(guard) => break guard,
            Err(std::sync::TryLockError::WouldBlock) => {
                std::thread::sleep(Duration::from_millis(50))
            }
            Err(error) => return Err(error.to_string()),
        }
    };
    if let Some(environment) = shared.as_ref() {
        return Ok(environment.clone());
    }
    let cache = directory.join("indexes");
    engine::private_dir(&cache)?;
    let runtime = tempfile::Builder::new()
        .prefix("vorkspace-cbm-")
        // Darwin's per-user TMPDIR can exceed the Unix socket path limit once
        // the engine adds its endpoint name. tempfile creates this exclusively
        // with mode 0700; the directory remains private even under /tmp.
        .tempdir_in("/tmp")
        .map_err(|e| e.to_string())?;
    // --ui=false alone is applied after daemon startup. Persist this setting
    // before any MCP session so there is never a transient HTTP listener.
    let mut command = transport::command(binary, &cache, runtime.path(), root);
    command.args(["config", "set", "ui_enabled", "false"]);
    transport::Client::configure(command, cancel)?;
    let environment = Arc::new(Environment { cache, runtime });
    *shared = Some(environment.clone());
    Ok(environment)
}

impl Drop for Service {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests;
