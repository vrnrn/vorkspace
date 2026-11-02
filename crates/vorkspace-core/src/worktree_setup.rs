//! A project's setup command for new worktrees. Creation only records that setup
//! is due, inside the worktree's private Git folder. The first turn working there
//! runs it, so it has one owner, can be stopped, and can tell the agent how it went.
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    os::{
        fd::AsRawFd,
        unix::process::{CommandExt, ExitStatusExt},
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
const INTERRUPTED: &str = "interrupted";
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
fn read(marker: &Path) -> Option<(String, String)> {
    let text = fs::read_to_string(marker).ok()?;
    let (state, command) = text.split_once('\n')?;
    Some((state.to_owned(), command.to_owned()))
}
fn write(marker: &Path, state: &str, command: &str) {
    let _ = fs::write(marker, format!("{state}\n{command}"));
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
    if let (Some(command), Some(marker)) = (command, marker(worktree)) {
        write(&marker, DUE, &command);
    }
}

pub fn is_due(worktree: &Path) -> bool {
    marker(worktree).is_some_and(|marker| marker.exists())
}

/// Run what is due in `worktree`. The result is for the agent: a failure leaves a
/// usable worktree, so the turn continues and can repair it. Two sessions sharing a
/// worktree take turns on a lock, and an owner that died releases it, which is how
/// a setup killed by a crash or quit is noticed.
pub fn run_due(project: &Path, worktree: &Path, cancel: &AtomicBool) -> Option<String> {
    let marker = marker(worktree)?;
    let lock = File::create(marker.with_extension("lock")).ok()?;
    while unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        std::thread::sleep(POLL);
    }
    let (state, command) = read(&marker)?;
    if state == NOTICE {
        let _ = fs::remove_file(&marker);
        return Some(command);
    }
    if state != DUE {
        let _ = fs::remove_file(&marker);
        return Some(format!(
            "Worktree setup was interrupted and may be incomplete. Its command was: {command}"
        ));
    }
    write(&marker, RUNNING, &command);
    let result = run(&command, project, worktree, cancel, TIMEOUT);
    match &result {
        Err(Failure::Interrupted(_)) => write(&marker, INTERRUPTED, &command),
        _ => {
            let _ = fs::remove_file(&marker);
        }
    }
    result.err().map(|failure| match failure {
        Failure::Interrupted(reason) | Failure::Failed(reason) => reason,
    })
}

/// Keep a warning until a prompt carries it. The turn can still end first, such as
/// when the provider CLI is missing, and a native slash command has to reach its
/// harness unchanged.
pub fn hold_notice(worktree: &Path, warning: &str) {
    if let Some(marker) = marker(worktree) {
        write(&marker, NOTICE, warning);
    }
}
/// The warning is in a prompt now, so no later turn repeats it.
pub fn clear_notice(worktree: &Path) {
    if let Some(marker) = marker(worktree) {
        let _ = fs::remove_file(marker);
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
    /// It may have done part of its work, so it must not be silently forgotten.
    Interrupted(String),
    Failed(String),
}

fn run(
    command: &str,
    project: &Path,
    worktree: &Path,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<(), Failure> {
    let failed = |reason: String| Failure::Failed(format!("Worktree setup {reason}"));
    let output = tempfile::NamedTempFile::new().map_err(|e| failed(e.to_string()))?;
    let log = output.reopen().map_err(|e| failed(e.to_string()))?;
    let errors = log.try_clone().map_err(|e| failed(e.to_string()))?;
    let mut process = Command::new("/bin/sh");
    crate::shell_env::apply_gui_env(&mut process);
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
            return Err(Failure::Interrupted(format!(
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
        return Err(Failure::Interrupted(format!(
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
        write(&marker(tree).unwrap(), DUE, command);
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
        assert_eq!(run_due(dir.path(), &tree, &stopped(false)), None);
        assert_eq!(run_due(dir.path(), &tree, &stopped(false)), None);
        assert!(!is_due(&tree));
        assert_eq!(
            fs::read_to_string(tree.join("ran")).unwrap(),
            dir.path().to_string_lossy()
        );
    }

    #[test]
    fn a_failure_reaches_the_turn_and_is_not_retried() {
        let (dir, tree) = repository();
        due(&tree, "echo cannot install >&2; exit 3");
        let warning = run_due(dir.path(), &tree, &stopped(false)).unwrap();
        assert!(warning.contains("cannot install"), "{warning}");
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
    fn a_held_warning_lasts_until_a_prompt_carries_it() {
        let (dir, tree) = repository();
        hold_notice(&tree, "Worktree setup failed: no network");
        assert!(is_due(&tree));
        let warning = run_due(dir.path(), &tree, &stopped(false)).unwrap();
        assert_eq!(warning, "Worktree setup failed: no network");
        hold_notice(&tree, &warning);
        assert!(is_due(&tree), "this turn ended before reaching a prompt");
        clear_notice(&tree);
        assert_eq!(run_due(dir.path(), &tree, &stopped(false)), None);
    }

    #[test]
    fn a_stopped_setup_is_reported_next_turn_rather_than_replayed() {
        let (dir, tree) = repository();
        due(&tree, "touch started; sleep 30");
        let warning = run_due(dir.path(), &tree, &stopped(true)).unwrap();
        assert!(warning.contains("was stopped"), "{warning}");
        assert!(is_due(&tree), "the next turn has to hear about it");
        let _ = fs::remove_file(tree.join("started"));
        let warning = run_due(dir.path(), &tree, &stopped(false)).unwrap();
        assert!(warning.contains("may be incomplete"), "{warning}");
        assert!(warning.contains("sleep 30"), "{warning}");
        assert!(!tree.join("started").exists(), "not replayed");
        assert!(!is_due(&tree));
    }

    #[test]
    fn a_setup_whose_owner_died_counts_as_interrupted() {
        let (dir, tree) = repository();
        write(&marker(&tree).unwrap(), RUNNING, "npm ci");
        let warning = run_due(dir.path(), &tree, &stopped(false)).unwrap();
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
        assert!(
            matches!(result, Err(Failure::Interrupted(reason)) if reason.contains("timed out"))
        );
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
        while read(&marker(&tree).unwrap()).is_some_and(|(state, _)| state != RUNNING) {
            assert!(start.elapsed() < Duration::from_secs(10));
            std::thread::sleep(POLL);
        }
        std::thread::sleep(Duration::from_millis(200));
        crate::process::shutdown_all();
        let warning = worker.join().unwrap().unwrap();
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
