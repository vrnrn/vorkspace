//! A project's setup command for new worktrees. Creation only records that setup
//! is due in the app data directory. The first turn working there
//! runs it, so it has one owner, can be stopped, and can tell the agent how it went.
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
            process::{CommandExt, ExitStatusExt},
        },
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(600);
const POLL: Duration = Duration::from_millis(50);
const OUTPUT_LIMIT: usize = 2000;
const MAX_RECORD_BYTES: u64 = 1024 * 1024;
const DUE: &str = "due";
const RUNNING: &str = "running";
const NOTICE: &str = "notice";
struct SetupPolicy {
    data_dir: Option<PathBuf>,
    commands: HashMap<String, String>,
}
static POLICY: Mutex<Option<SetupPolicy>> = Mutex::new(None);

/// Worktree creation uses the current user's per-project setup choices.
pub fn configure(data_dir: Option<&Path>, commands: &HashMap<String, String>) {
    let commands = commands
        .iter()
        .filter(|(_, command)| !command.trim().is_empty())
        .map(|(project, command)| (crate::files::path_key(project), command.clone()))
        .collect();
    *POLICY.lock().unwrap_or_else(|e| e.into_inner()) = Some(SetupPolicy {
        data_dir: data_dir.map(Path::to_owned),
        commands,
    });
}

/// Bind a record to the linked checkout's Git entry. Reusing the path for
/// a different checkout cannot inherit a command. Never read a Git entry as
/// a path or source of commands.
fn marker(data_dir: &Path, worktree: &Path) -> Option<PathBuf> {
    let checkout = fs::metadata(worktree).ok()?;
    let metadata = fs::symlink_metadata(worktree.join(".git")).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let born = metadata
        .created()
        .or_else(|_| metadata.modified())
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    let mut hash = Sha256::new();
    hash.update(checkout.dev().to_le_bytes());
    hash.update(checkout.ino().to_le_bytes());
    hash.update(metadata.dev().to_le_bytes());
    hash.update(metadata.ino().to_le_bytes());
    hash.update(born.as_secs().to_le_bytes());
    hash.update(born.subsec_nanos().to_le_bytes());
    let key = hash.finalize();
    Some(data_dir.join("worktree-setup").join(format!("{key:x}")))
}

fn prepare_directory(marker: &Path) -> io::Result<()> {
    let directory = marker.parent().expect("setup records have a directory");
    match fs::DirBuilder::new().mode(0o700).create(directory) {
        Ok(()) => {
            let root = directory
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            File::open(root)?.sync_all()
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if directory.symlink_metadata()?.is_dir() {
                Ok(())
            } else {
                Err(io::Error::other(
                    "Worktree setup storage is not a directory",
                ))
            }
        }
        Err(error) => Err(error),
    }
}

fn lock_file(marker: &Path) -> io::Result<File> {
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(marker.with_extension("lock"))?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("Invalid worktree setup lock"));
    }
    Ok(file)
}

fn try_lock(lock: &File) -> io::Result<bool> {
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    match error.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted => Ok(false),
        _ => Err(error),
    }
}

fn write(marker: &Path, state: &str, content: &str) -> io::Result<()> {
    crate::private_file::replace(marker, |file| write!(file, "{state}\n{content}"))
}

fn open_marker(marker: &Path) -> io::Result<File> {
    let file = File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(marker)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_RECORD_BYTES {
        return Err(io::Error::other("Invalid worktree setup record"));
    }
    Ok(file)
}

/// Keeps the command chosen when the worktree was made, whatever changes later.
pub fn mark_due(projects: &[&Path], worktree: &Path) {
    let pending = {
        let policy = POLICY.lock().unwrap_or_else(|e| e.into_inner());
        policy.as_ref().and_then(|policy| {
            let data_dir = policy.data_dir.as_ref()?;
            let command = projects.iter().find_map(|project| {
                let key = crate::files::path_key(&project.to_string_lossy());
                policy.commands.get(&key)
            })?;
            Some((data_dir.clone(), command.clone()))
        })
    };
    let Some((data_dir, command)) = pending else {
        return;
    };
    let Some(marker) = marker(&data_dir, worktree) else {
        return;
    };
    if prepare_directory(&marker).is_err() {
        return;
    }
    let Ok(lock) = lock_file(&marker) else {
        return;
    };
    if try_lock(&lock).unwrap_or(false) && !marker.exists() {
        let _ = write(&marker, DUE, &command);
    }
}

fn owns_record(marker: &Path) -> bool {
    marker
        .parent()
        .is_some_and(|parent| parent.symlink_metadata().is_ok_and(|m| m.is_dir()))
}

pub fn is_due(data_dir: &Path, worktree: &Path) -> bool {
    marker(data_dir, worktree).is_some_and(|marker| owns_record(&marker) && marker.exists())
}

/// Run the setup scheduled for this checkout. A failure leaves a usable
/// worktree and a notice. The lock serializes sessions and detects a dead owner.
pub fn run_due(
    data_dir: &Path,
    project: &Path,
    worktree: &Path,
    cancel: &AtomicBool,
) -> io::Result<Option<Notice>> {
    let Some(marker) = marker(data_dir, worktree) else {
        return Ok(None);
    };
    if !owns_record(&marker) {
        return Ok(None);
    }
    let lock = lock_file(&marker)?;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        if try_lock(&lock)? {
            break;
        }
        std::thread::sleep(POLL);
    }
    let mut file = match open_marker(&marker) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut text = String::new();
    (&mut file)
        .take(MAX_RECORD_BYTES + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > MAX_RECORD_BYTES {
        return Err(io::Error::other("Invalid worktree setup record"));
    }
    let (state, command) = text.split_once('\n').unwrap_or_default();
    if state == NOTICE {
        return Ok(Some(Notice {
            marker,
            file,
            warning: command.to_owned(),
        }));
    }
    let warning = if command.is_empty() {
        "Worktree setup status could not be read and setup may be incomplete.".to_owned()
    } else if state == DUE {
        write(&marker, RUNNING, command)?;
        match run(command, project, worktree, cancel, TIMEOUT) {
            Ok(()) => {
                crate::private_file::remove(&marker)?;
                return Ok(None);
            }
            Err(Failure::StoppedBeforeSpawn) => {
                write(&marker, DUE, command)?;
                return Ok(None);
            }
            Err(Failure::Notice(warning)) => warning,
        }
    } else {
        format!("Worktree setup was interrupted and may be incomplete. Its command was: {command}")
    };
    write(&marker, NOTICE, &warning)?;
    let file = open_marker(&marker)?;
    Ok(Some(Notice {
        marker,
        file,
        warning,
    }))
}

/// A pending warning. Holding the file open keeps its identity unique even if
/// another session removes or replaces the marker before this turn completes.
pub struct Notice {
    marker: PathBuf,
    file: File,
    warning: String,
}

impl Notice {
    pub fn warning(&self) -> &str {
        &self.warning
    }

    /// A turn confirmed delivery. Clear only the marker that turn carried.
    /// If the lock is busy, retain the notice for another turn to acknowledge.
    pub fn acknowledge(self) -> io::Result<()> {
        let lock = lock_file(&self.marker)?;
        if !try_lock(&lock)? {
            return Ok(());
        }
        let current = match fs::symlink_metadata(&self.marker) {
            Ok(current) => current,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let delivered = self.file.metadata()?;
        if current.dev() == delivered.dev() && current.ino() == delivered.ino() {
            crate::private_file::remove(&self.marker)?;
        }
        Ok(())
    }
}

/// Only the end of the log: a verbose command can write far more than a warning needs.
fn tail(path: &Path, limit: usize) -> String {
    let mut text = Vec::new();
    if let Ok(mut file) = File::open(path) {
        let length = file.metadata().map_or(0, |metadata| metadata.len());
        let _ = file.seek(SeekFrom::Start(length.saturating_sub(limit as u64)));
        let _ = file.take(limit as u64).read_to_end(&mut text);
    }
    String::from_utf8_lossy(&text).into_owned()
}

#[derive(Debug)]
enum Failure {
    StoppedBeforeSpawn,
    Notice(String),
}

fn run(
    command: &str,
    project: &Path,
    worktree: &Path,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<(), Failure> {
    let failed = |reason: String| Failure::Notice(format!("Worktree setup {reason}"));
    let output = tempfile::NamedTempFile::new().map_err(|e| failed(e.to_string()))?;
    let log = output.reopen().map_err(|e| failed(e.to_string()))?;
    let errors = log.try_clone().map_err(|e| failed(e.to_string()))?;
    let mut process = Command::new("/bin/sh");
    crate::shell_env::apply_gui_env(&mut process);
    if cancel.load(Ordering::Relaxed) {
        return Err(Failure::StoppedBeforeSpawn);
    }
    let mut child = process
        .args(["-c", command])
        .current_dir(worktree)
        .env("VORKSPACE_PROJECT_PATH", project)
        .env("VORKSPACE_WORKTREE_PATH", worktree)
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(errors)
        .process_group(0)
        .spawn()
        .map_err(|e| failed(format!("could not start: {e}")))?;
    // Registered so quitting the app stops it too.
    let _registration = crate::process::register(child.id());
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| failed(e.to_string()))? {
            break status;
        }
        let stopped = cancel.load(Ordering::Relaxed);
        if stopped || started.elapsed() >= timeout {
            crate::process::terminate(&mut child);
            let _ = child.wait();
            return Err(Failure::Notice(format!(
                "Worktree setup {} and may be incomplete. Its command was: {command}",
                if stopped { "was stopped" } else { "timed out" }
            )));
        }
        std::thread::sleep(POLL);
    };
    if status.success() {
        return Ok(());
    }
    if status.signal().is_some() {
        return Err(Failure::Notice(format!(
            "Worktree setup was interrupted and may be incomplete. Its command was: {command}"
        )));
    }
    let text = tail(output.path(), OUTPUT_LIMIT);
    Err(failed(format!("failed: {}", text.trim())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        fs::create_dir(&main).unwrap();
        let git = |args: &[&str]| crate::git::git(&main, args).unwrap();
        git(&["init", "-b", "main"]);
        git(&["config", "user.email", "fixture@example.invalid"]);
        git(&["config", "user.name", "Fixture"]);
        git(&["commit", "--allow-empty", "-m", "Initial"]);
        let tree = dir.path().join("tree");
        git(&[
            "worktree",
            "add",
            "-b",
            "tree",
            "--",
            tree.to_str().unwrap(),
        ]);
        (dir, tree)
    }
    fn record(data: &Path, tree: &Path) -> PathBuf {
        let path = marker(data, tree).unwrap();
        prepare_directory(&path).unwrap();
        path
    }

    fn due(data: &Path, tree: &Path, command: &str) {
        write(&record(data, tree), DUE, command).unwrap();
    }
    fn stopped(value: bool) -> AtomicBool {
        AtomicBool::new(value)
    }

    #[test]
    fn a_due_setup_runs_once_in_the_worktree() {
        let (dir, tree) = repository();
        assert!(!is_due(dir.path(), &tree));
        due(
            dir.path(),
            &tree,
            "printf %s \"$VORKSPACE_PROJECT_PATH\" >> ran",
        );
        assert!(is_due(dir.path(), &tree));
        assert!(run_due(dir.path(), dir.path(), &tree, &stopped(false))
            .unwrap()
            .is_none());
        assert!(run_due(dir.path(), dir.path(), &tree, &stopped(false))
            .unwrap()
            .is_none());
        assert!(!is_due(dir.path(), &tree));
        assert_eq!(
            fs::read_to_string(tree.join("ran")).unwrap(),
            dir.path().to_string_lossy()
        );
    }

    #[test]
    fn a_failure_reaches_the_turn_and_is_not_retried() {
        let (dir, tree) = repository();
        due(
            dir.path(),
            &tree,
            "echo ran >> attempts; echo cannot install >&2; exit 3",
        );
        let notice = run_due(dir.path(), dir.path(), &tree, &stopped(false))
            .unwrap()
            .unwrap();
        assert!(
            notice.warning().contains("cannot install"),
            "{}",
            notice.warning()
        );
        let pending = run_due(dir.path(), dir.path(), &tree, &stopped(false))
            .unwrap()
            .unwrap();
        assert_eq!(pending.warning(), notice.warning());
        assert_eq!(fs::read_to_string(tree.join("attempts")).unwrap(), "ran\n");
        notice.acknowledge().unwrap();
        assert!(!is_due(dir.path(), &tree));
    }

    #[test]
    fn only_the_end_of_a_large_log_is_read() {
        let log = tempfile::NamedTempFile::new().unwrap();
        fs::write(log.path(), format!("{}the reason", "x".repeat(100_000))).unwrap();
        assert_eq!(tail(log.path(), 10), "the reason");
        assert_eq!(tail(Path::new("/no/such/log"), 10), "");
    }

    #[test]
    fn a_second_acknowledgment_cannot_remove_a_replacement_notice() {
        let (dir, tree) = repository();
        let marker = record(dir.path(), &tree);
        let warning = "Worktree setup failed: no network";
        write(&marker, NOTICE, warning).unwrap();
        let first = run_due(dir.path(), dir.path(), &tree, &stopped(false))
            .unwrap()
            .unwrap();
        let second = run_due(dir.path(), dir.path(), &tree, &stopped(false))
            .unwrap()
            .unwrap();
        assert_eq!(first.warning(), warning);
        assert_eq!(second.warning(), warning);
        first.acknowledge().unwrap();
        assert!(!is_due(dir.path(), &tree));

        write(&marker, NOTICE, warning).unwrap();
        second.acknowledge().unwrap();
        assert!(is_due(dir.path(), &tree));
        let replacement = run_due(dir.path(), dir.path(), &tree, &stopped(false))
            .unwrap()
            .unwrap();
        assert_eq!(replacement.warning(), warning);
        replacement.acknowledge().unwrap();
        assert!(!is_due(dir.path(), &tree));
    }

    #[test]
    fn a_setup_stopped_before_spawn_stays_due() {
        let (dir, tree) = repository();
        due(dir.path(), &tree, "touch started");
        assert!(run_due(dir.path(), dir.path(), &tree, &stopped(true))
            .unwrap()
            .is_none());
        assert!(is_due(dir.path(), &tree));
        assert!(!tree.join("started").exists());
        assert!(run_due(dir.path(), dir.path(), &tree, &stopped(false))
            .unwrap()
            .is_none());
        assert!(tree.join("started").exists());
        assert!(!is_due(dir.path(), &tree));
    }

    #[test]
    fn a_setup_whose_owner_died_counts_as_interrupted() {
        let (dir, tree) = repository();
        write(&record(dir.path(), &tree), RUNNING, "npm ci").unwrap();
        let notice = run_due(dir.path(), dir.path(), &tree, &stopped(false))
            .unwrap()
            .unwrap();
        let warning = notice.warning();
        assert!(warning.contains("interrupted"), "{warning}");
        assert!(warning.contains("npm ci"), "{warning}");
    }

    #[test]
    fn a_timeout_kills_the_command_and_is_reported() {
        let (dir, tree) = repository();
        let result = run(
            "sleep 30",
            dir.path(),
            &tree,
            &stopped(false),
            Duration::from_millis(100),
        );
        assert!(matches!(result, Err(Failure::Notice(reason)) if reason.contains("timed out")));
    }

    #[test]
    fn quitting_the_app_stops_a_running_setup() {
        const OWNER: i64 = 719_451;
        let (dir, tree) = repository();
        due(dir.path(), &tree, "sleep 30; touch survived");
        let worker = std::thread::spawn({
            let project = dir.path().to_owned();
            let tree = tree.clone();
            move || {
                crate::process::owned_by(OWNER, || {
                    run_due(&project, &project, &tree, &stopped(false))
                })
            }
        });
        let start = Instant::now();
        // RUNNING is persisted before environment preparation and spawn. Wait
        // for the shutdown registry to own the child, rather than guessing how
        // long shell startup takes under a busy build/test workload.
        while !crate::process::agents()
            .iter()
            .any(|agent| agent.session == OWNER)
        {
            assert!(start.elapsed() < Duration::from_secs(10));
            std::thread::sleep(POLL);
        }
        crate::process::shutdown_all();
        let notice = worker.join().unwrap().unwrap().unwrap();
        let warning = notice.warning();
        assert!(warning.contains("interrupted"), "{warning}");
        assert!(
            is_due(dir.path(), &tree),
            "the next launch has to hear about it"
        );
        assert!(!tree.join("survived").exists());
    }

    #[test]
    fn only_a_configured_project_is_marked() {
        let (dir, tree) = repository();
        mark_due(&[dir.path()], &tree);
        assert!(!is_due(dir.path(), &tree));
        configure(
            Some(dir.path()),
            &HashMap::from([(
                dir.path().to_string_lossy().into_owned(),
                "npm ci".to_owned(),
            )]),
        );
        mark_due(&[Path::new("/elsewhere"), dir.path()], &tree);
        assert!(is_due(dir.path(), &tree.join(".")));
        let isolated = tempfile::tempdir().unwrap();
        assert!(!is_due(isolated.path(), &tree));
        assert!(run_due(isolated.path(), dir.path(), &tree, &stopped(false))
            .unwrap()
            .is_none());
    }
}
