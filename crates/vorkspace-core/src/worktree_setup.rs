//! A project's setup command for new worktrees. Creation only records that setup
//! is due, inside the worktree's private Git folder. The first turn working there
//! runs it, so it has one owner, can be stopped, and can tell the agent how it went.
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{MetadataExt, OpenOptionsExt},
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
const DUE: &str = "due";
const RUNNING: &str = "running";
const NOTICE: &str = "notice";
static COMMANDS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// Per-project commands from the user's own preferences. They never come from the
/// repository, so opening a project can't choose what runs on this Mac.
pub fn set_commands(commands: &HashMap<String, String>) {
    let commands = commands
        .iter()
        .filter(|(_, command)| !command.trim().is_empty())
        .map(|(project, command)| (crate::files::path_key(project), command.clone()))
        .collect();
    *COMMANDS.lock().unwrap_or_else(|e| e.into_inner()) = Some(commands);
}

/// A linked worktree's `.git` file names its private folder.
fn marker(worktree: &Path) -> Option<PathBuf> {
    let link = fs::read_to_string(worktree.join(".git")).ok()?;
    let directory = Path::new(link.strip_prefix("gitdir: ")?.trim());
    Some(worktree.join(directory).join("vorkspace-setup"))
}
fn lock_file(marker: &Path) -> io::Result<File> {
    File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(marker.with_extension("lock"))
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

fn write(marker: &Path, state: &str, content: &str) -> io::Result<File> {
    let parent = marker.parent().unwrap();
    let directory = File::open(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    write!(file, "{state}\n{content}")?;
    file.as_file().sync_all()?;
    let file = file.persist(marker)?;
    // Make the rename durable before a due command is allowed to start.
    directory.sync_all()?;
    Ok(file)
}

fn remove_marker(marker: &Path) -> io::Result<()> {
    let directory = File::open(marker.parent().unwrap())?;
    fs::remove_file(marker)?;
    directory.sync_all()
}

/// Keeps the command chosen when the worktree was made, whatever changes later.
pub fn mark_due(projects: &[&Path], worktree: &Path) {
    let command = {
        let commands = COMMANDS.lock().unwrap_or_else(|e| e.into_inner());
        projects.iter().find_map(|project| {
            let key = crate::files::path_key(&project.to_string_lossy());
            commands.as_ref()?.get(&key).cloned()
        })
    };
    let (Some(command), Some(marker)) = (command, marker(worktree)) else {
        return;
    };
    let Ok(lock) = lock_file(&marker) else {
        return;
    };
    if try_lock(&lock).unwrap_or(false) && !marker.exists() {
        let _ = write(&marker, DUE, &command);
    }
}

pub fn is_due(worktree: &Path) -> bool {
    marker(worktree).is_some_and(|marker| marker.exists())
}

/// Run what is due in `worktree`. The result is for the agent: a failure leaves a
/// usable worktree, so the turn continues and can repair it. Two sessions sharing a
/// worktree take turns on a lock, and an owner that died releases it, which is how
/// a setup killed by a crash or quit is noticed.
pub fn run_due(project: &Path, worktree: &Path, cancel: &AtomicBool) -> io::Result<Option<Notice>> {
    let Some(marker) = marker(worktree) else {
        return Ok(None);
    };
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
    let mut file = match File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&marker)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut text = String::new();
    file.read_to_string(&mut text)?;
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
                remove_marker(&marker)?;
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
    let file = write(&marker, NOTICE, &warning)?;
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
            remove_marker(&self.marker)?;
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
    fn due(tree: &Path, command: &str) {
        write(&marker(tree).unwrap(), DUE, command).unwrap();
    }
    fn stopped(value: bool) -> AtomicBool {
        AtomicBool::new(value)
    }

    #[test]
    fn a_due_setup_runs_once_in_the_worktree() {
        let (dir, tree) = repository();
        assert!(!is_due(&tree));
        due(&tree, "printf %s \"$VORKSPACE_PROJECT_PATH\" >> ran");
        assert!(is_due(&tree));
        assert!(run_due(dir.path(), &tree, &stopped(false))
            .unwrap()
            .is_none());
        assert!(run_due(dir.path(), &tree, &stopped(false))
            .unwrap()
            .is_none());
        assert!(!is_due(&tree));
        assert_eq!(
            fs::read_to_string(tree.join("ran")).unwrap(),
            dir.path().to_string_lossy()
        );
    }

    #[test]
    fn a_failure_reaches_the_turn_and_is_not_retried() {
        let (dir, tree) = repository();
        due(
            &tree,
            "echo ran >> attempts; echo cannot install >&2; exit 3",
        );
        let notice = run_due(dir.path(), &tree, &stopped(false))
            .unwrap()
            .unwrap();
        assert!(
            notice.warning().contains("cannot install"),
            "{}",
            notice.warning()
        );
        let pending = run_due(dir.path(), &tree, &stopped(false))
            .unwrap()
            .unwrap();
        assert_eq!(pending.warning(), notice.warning());
        assert_eq!(fs::read_to_string(tree.join("attempts")).unwrap(), "ran\n");
        notice.acknowledge().unwrap();
        assert!(!is_due(&tree));
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
        let marker = marker(&tree).unwrap();
        let warning = "Worktree setup failed: no network";
        write(&marker, NOTICE, warning).unwrap();
        let first = run_due(dir.path(), &tree, &stopped(false))
            .unwrap()
            .unwrap();
        let second = run_due(dir.path(), &tree, &stopped(false))
            .unwrap()
            .unwrap();
        assert_eq!(first.warning(), warning);
        assert_eq!(second.warning(), warning);
        first.acknowledge().unwrap();
        assert!(!is_due(&tree));

        write(&marker, NOTICE, warning).unwrap();
        second.acknowledge().unwrap();
        assert!(is_due(&tree));
        let replacement = run_due(dir.path(), &tree, &stopped(false))
            .unwrap()
            .unwrap();
        assert_eq!(replacement.warning(), warning);
        replacement.acknowledge().unwrap();
        assert!(!is_due(&tree));
    }

    #[test]
    fn a_setup_stopped_before_spawn_stays_due() {
        let (dir, tree) = repository();
        due(&tree, "touch started");
        assert!(run_due(dir.path(), &tree, &stopped(true))
            .unwrap()
            .is_none());
        assert!(is_due(&tree));
        assert!(!tree.join("started").exists());
        assert!(run_due(dir.path(), &tree, &stopped(false))
            .unwrap()
            .is_none());
        assert!(tree.join("started").exists());
        assert!(!is_due(&tree));
    }

    #[test]
    fn a_setup_whose_owner_died_counts_as_interrupted() {
        let (dir, tree) = repository();
        write(&marker(&tree).unwrap(), RUNNING, "npm ci").unwrap();
        let notice = run_due(dir.path(), &tree, &stopped(false))
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
        let (dir, tree) = repository();
        due(&tree, "sleep 30; touch survived");
        let worker = std::thread::spawn({
            let project = dir.path().to_owned();
            let tree = tree.clone();
            move || run_due(&project, &tree, &stopped(false))
        });
        let start = Instant::now();
        let marker = marker(&tree).unwrap();
        while !fs::read_to_string(&marker).unwrap().starts_with(RUNNING) {
            assert!(start.elapsed() < Duration::from_secs(10));
            std::thread::sleep(POLL);
        }
        std::thread::sleep(Duration::from_millis(200));
        crate::process::shutdown_all();
        let notice = worker.join().unwrap().unwrap().unwrap();
        let warning = notice.warning();
        assert!(warning.contains("interrupted"), "{warning}");
        assert!(is_due(&tree), "the next launch has to hear about it");
    }

    #[test]
    fn only_a_configured_project_is_marked() {
        let (dir, tree) = repository();
        mark_due(&[dir.path()], &tree);
        assert!(!is_due(&tree));
        set_commands(&HashMap::from([(
            dir.path().to_string_lossy().into_owned(),
            "npm ci".to_owned(),
        )]));
        mark_due(&[Path::new("/elsewhere"), dir.path()], &tree);
        assert!(is_due(&tree));
    }
}
