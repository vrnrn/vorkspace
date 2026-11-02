//! Worktree guards and setup, shared by Settings and the app actions. Removal keeps
//! commits: a branch goes only if Git agrees it is fully merged.
use std::{
    collections::HashMap,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};
pub fn default_root(main: &Path) -> PathBuf {
    main.with_file_name(format!(
        "{}-worktrees",
        main.file_name().unwrap_or_default().to_string_lossy()
    ))
}
/// What removing a worktree would cost. Settings and the app actions share this,
/// so a Bot can never remove something the UI would warn about.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Assessment {
    pub locked: bool,
    pub dirty: Option<bool>,
    pub unpushed: usize,
    pub merged: bool,
}
impl Assessment {
    /// Why removing this would lose work or override the user.
    pub fn blocker(&self) -> Option<String> {
        if self.locked {
            Some("it is locked".into())
        } else if self.dirty != Some(false) {
            Some(match self.dirty {
                Some(_) => "it has uncommitted changes".into(),
                None => "its status could not be read".into(),
            })
        } else if self.merged || self.unpushed == 0 {
            None
        } else {
            Some(format!(
                "it has {} unmerged, unpublished commits",
                self.unpushed
            ))
        }
    }
    pub fn removable(&self) -> bool {
        self.blocker().is_none()
    }
}
pub fn assess(cwd: &Path, tree: &crate::git::Worktree) -> Assessment {
    let list = crate::git::git(cwd, &["worktree", "list", "--porcelain", "-z"]).unwrap_or_default();
    let locked = list.split("\0\0").any(|block| {
        block.starts_with(&format!("worktree {}\0", tree.path))
            && block
                .split('\0')
                .any(|v| v == "locked" || v.starts_with("locked "))
    });
    let dirty = crate::git::git(
        Path::new(&tree.path),
        &["status", "--porcelain", "--untracked-files=normal"],
    )
    .ok()
    .map(|s| !s.is_empty());
    let unpushed = crate::git::git(
        Path::new(&tree.path),
        &["rev-list", "--count", "HEAD", "--not", "--remotes"],
    )
    .ok()
    .and_then(|s| s.trim().parse().ok())
    .unwrap_or(0);
    // The first listed worktree is the main checkout.
    let main = list
        .split('\0')
        .find_map(|field| field.strip_prefix("HEAD "))
        .map(str::to_owned);
    let merged = main
        .into_iter()
        .chain(["refs/remotes/origin/HEAD".to_owned()])
        .any(|target| {
            crate::git::git(cwd, &["merge-base", "--is-ancestor", &tree.head, &target]).is_ok()
        });
    Assessment {
        locked,
        dirty,
        unpushed,
        merged,
    }
}
pub fn check(cwd: &Path, path: &Path) -> Result<crate::git::Worktree, String> {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let key = crate::files::path_key(&canonical.to_string_lossy());
    let tree = crate::git::snapshot(cwd)?
        .worktrees
        .into_iter()
        .find(|t| {
            let canonical = Path::new(&t.path)
                .canonicalize()
                .unwrap_or_else(|_| PathBuf::from(&t.path));
            crate::files::path_key(&canonical.to_string_lossy()) == key
        })
        .ok_or("This path is not a registered worktree of this repository")?;
    if tree.main {
        return Err("The main working copy cannot be deleted".into());
    }
    let assessment = assess(cwd, &tree);
    if tree.branch.is_none() && !assessment.locked && !assessment.removable() {
        return Err("Create a branch for this detached worktree before deleting it.".into());
    }
    if assessment.locked {
        return Err("This worktree is locked. Unlock it in Git before deleting it.".into());
    }
    Ok(tree)
}
pub fn remove(cwd: &Path, path: &Path) -> Result<(), String> {
    let tree = check(cwd, path)?;
    let assessment = assess(cwd, &tree);
    crate::provider::discard_worktree(Path::new(&tree.path));
    crate::git::git(cwd, &["worktree", "remove", "--force", "--", &tree.path])?;
    // `-d` refuses anything Git doesn't consider merged, so this can only tidy.
    if let Some(branch) = tree.branch.as_deref().filter(|_| assessment.merged) {
        let _ = crate::git::git(cwd, &["branch", "-d", "--", branch]);
    }
    Ok(())
}
/// The app actions' removal: it refuses rather than discards.
pub fn remove_if_safe(cwd: &Path, path: &Path) -> Result<(), String> {
    let tree = check(cwd, path)?;
    if let Some(blocker) = assess(cwd, &tree).blocker() {
        return Err(format!(
            "This worktree was kept because {blocker}. Tell the user rather than removing it."
        ));
    }
    remove(cwd, path)
}
/// The rows that describe a project's worktrees; `sessions` answers who uses one.
pub fn describe(
    project: &Path,
    sessions_using: impl Fn(&str) -> Result<Vec<i64>, String>,
) -> Result<serde_json::Value, String> {
    let remote = crate::files::remote::is_remote(project);
    let mut rows = Vec::new();
    for tree in crate::git::snapshot(project)?.worktrees {
        let mut row = serde_json::json!({
            "path": tree.path, "branch": tree.branch, "head": tree.head, "main": tree.main,
        });
        if !remote && !tree.main {
            let assessment = assess(project, &tree);
            let sessions = sessions_using(&tree.path)?;
            row["locked"] = assessment.locked.into();
            row["dirty"] = serde_json::json!(assessment.dirty);
            row["unpublishedCommits"] = assessment.unpushed.into();
            row["merged"] = assessment.merged.into();
            row["removable"] = (assessment.removable() && sessions.is_empty()).into();
            row["sessions"] = serde_json::json!(sessions);
        }
        rows.push(row);
    }
    Ok(rows.into())
}
/// `du` in kilobytes: this is only a display figure, so a failure just hides it.
pub fn disk_size(path: &str) -> Option<u64> {
    let output = Command::new("/usr/bin/du")
        .args(["-sk", "--", path])
        .output()
        .ok()?;
    let kilobytes = String::from_utf8_lossy(&output.stdout);
    Some(kilobytes.split_whitespace().next()?.parse::<u64>().ok()? * 1024)
}
const SETUP_TIMEOUT: Duration = Duration::from_secs(600);
const SETUP_POLL: Duration = Duration::from_millis(50);
const SETUP_OUTPUT: usize = 2000;
static SETUP_COMMANDS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// Per-project commands from the user's own preferences. They never come from the
/// repository, so opening a project can't choose what runs on this Mac.
pub fn set_setup_commands(commands: &HashMap<String, String>) {
    let commands = commands
        .iter()
        .filter(|(_, command)| !command.trim().is_empty())
        .map(|(project, command)| (crate::files::path_key(project), command.clone()))
        .collect();
    *SETUP_COMMANDS.lock().unwrap_or_else(|e| e.into_inner()) = Some(commands);
}
fn setup_command(project: &Path) -> Option<String> {
    let commands = SETUP_COMMANDS.lock().unwrap_or_else(|e| e.into_inner());
    commands
        .as_ref()?
        .get(&crate::files::path_key(&project.to_string_lossy()))
        .cloned()
}
pub fn has_setup(project: &Path) -> bool {
    !crate::files::remote::is_remote(project) && setup_command(project).is_some()
}
/// Run the project's setup command in a new worktree, off the UI thread. A failure
/// is a warning: the worktree is still usable, and the caller reports it.
pub fn prepare(project: &Path, worktree: &Path, cancel: &AtomicBool) -> Option<String> {
    if crate::files::remote::is_remote(project) {
        return None;
    }
    let command = setup_command(project)?;
    run_setup(&command, project, worktree, cancel, SETUP_TIMEOUT).err()
}
fn run_setup(
    command: &str,
    project: &Path,
    worktree: &Path,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<(), String> {
    let failed = |reason: String| format!("Worktree setup {reason}");
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
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| failed(e.to_string()))? {
            break status;
        }
        let stopped = cancel.load(Ordering::Relaxed);
        if stopped || started.elapsed() >= timeout {
            crate::process::terminate(&mut child);
            let _ = child.wait();
            return Err(failed(
                if stopped { "was stopped" } else { "timed out" }.into(),
            ));
        }
        std::thread::sleep(SETUP_POLL);
    };
    if status.success() {
        return Ok(());
    }
    let text = std::fs::read(output.path()).unwrap_or_default();
    let text = String::from_utf8_lossy(&text[text.len().saturating_sub(SETUP_OUTPUT)..]);
    Err(failed(format!("failed: {}", text.trim())))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| crate::git::git(dir.path(), args).unwrap();
        git(&["init", "-b", "main"]);
        git(&["config", "user.email", "fixture@example.invalid"]);
        git(&["config", "user.name", "Fixture"]);
        std::fs::write(dir.path().join("file"), "before").unwrap();
        git(&["add", "file"]);
        git(&["commit", "-m", "Initial"]);
        dir
    }
    fn commit(path: &Path, name: &str) {
        std::fs::write(path.join(name), "work").unwrap();
        crate::git::git(path, &["add", name]).unwrap();
        crate::git::git(path, &["commit", "-m", name]).unwrap();
    }
    fn branch_exists(dir: &Path, name: &str) -> bool {
        crate::git::git(
            dir,
            &["show-ref", "--verify", &format!("refs/heads/{name}")],
        )
        .is_ok()
    }
    fn tidy(dir: &Path) {
        let _ = std::fs::remove_dir_all(default_root(dir));
    }
    #[test]
    fn removal_guards_and_branch_retention() {
        let dir = repo();
        assert!(check(dir.path(), dir.path()).unwrap_err().contains("main"));
        let tree = crate::git::create_named_worktree(dir.path(), "settings-parity").unwrap();
        let path = Path::new(&tree.path);
        std::fs::write(path.join("untracked"), "discard in disposable test").unwrap();
        assert_eq!(assess(dir.path(), &tree).dirty, Some(true));
        crate::git::git(dir.path(), &["worktree", "lock", "--", &tree.path]).unwrap();
        assert!(check(dir.path(), path).unwrap_err().contains("locked"));
        crate::git::git(dir.path(), &["worktree", "unlock", "--", &tree.path]).unwrap();
        commit(path, "unmerged");
        remove(dir.path(), path).unwrap();
        assert!(!path.exists());
        assert!(branch_exists(dir.path(), "settings-parity"));
        tidy(dir.path());
    }
    #[test]
    fn a_merged_branch_leaves_with_its_worktree() {
        let dir = repo();
        let tree = crate::git::create_named_worktree(dir.path(), "finished").unwrap();
        let path = Path::new(&tree.path);
        commit(path, "feature");
        crate::git::git(dir.path(), &["merge", "--ff-only", "finished"]).unwrap();
        remove_if_safe(dir.path(), path).unwrap();
        assert!(!path.exists());
        assert!(!branch_exists(dir.path(), "finished"));
        tidy(dir.path());
    }
    #[test]
    fn an_agent_can_never_discard_work() {
        let dir = repo();
        let tree = crate::git::create_named_worktree(dir.path(), "in-progress").unwrap();
        let path = Path::new(&tree.path);
        std::fs::write(path.join("draft"), "unsaved").unwrap();
        let refused = remove_if_safe(dir.path(), path).unwrap_err();
        assert!(refused.contains("uncommitted"), "{refused}");
        std::fs::remove_file(path.join("draft")).unwrap();
        commit(path, "unique");
        let refused = remove_if_safe(dir.path(), path).unwrap_err();
        assert!(refused.contains("unpublished"), "{refused}");
        assert!(path.join("unique").exists());
        remove(dir.path(), path).unwrap();
        tidy(dir.path());
    }
    #[test]
    fn a_detached_worktree_goes_only_when_nothing_in_it_is_unique() {
        let dir = repo();
        let path = dir.path().join("detached");
        let add = |path: &Path| {
            crate::git::git(
                dir.path(),
                &[
                    "worktree",
                    "add",
                    "--detach",
                    path.to_str().unwrap(),
                    "HEAD",
                ],
            )
            .unwrap();
        };
        add(&path);
        remove(dir.path(), &path).unwrap();
        assert!(!path.exists());
        add(&path);
        commit(&path, "orphan");
        assert!(remove(dir.path(), &path).unwrap_err().contains("detached"));
        crate::git::git(
            dir.path(),
            &["worktree", "remove", "--force", path.to_str().unwrap()],
        )
        .unwrap();
    }
    #[test]
    fn setup_runs_in_the_worktree_and_reports_failure_without_losing_it() {
        let dir = repo();
        let tree = crate::git::create_named_worktree(dir.path(), "setup").unwrap();
        let path = Path::new(&tree.path);
        let run = |command: &str, timeout| {
            run_setup(command, dir.path(), path, &AtomicBool::new(false), timeout)
        };
        run(
            "printf %s \"$VORKSPACE_PROJECT_PATH\" > marker",
            SETUP_TIMEOUT,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(path.join("marker")).unwrap(),
            dir.path().to_string_lossy()
        );
        let failed = run("echo cannot install >&2; exit 3", SETUP_TIMEOUT).unwrap_err();
        assert!(failed.contains("cannot install"), "{failed}");
        let timed_out = run("sleep 30", Duration::from_millis(100)).unwrap_err();
        assert!(timed_out.contains("timed out"), "{timed_out}");
        let stopped = run_setup(
            "sleep 30",
            dir.path(),
            path,
            &AtomicBool::new(true),
            SETUP_TIMEOUT,
        )
        .unwrap_err();
        assert!(stopped.contains("stopped"), "{stopped}");
        remove(dir.path(), path).unwrap();
        tidy(dir.path());
    }
    #[test]
    fn only_a_configured_project_runs_setup() {
        let dir = repo();
        let tree = crate::git::create_named_worktree(dir.path(), "unset").unwrap();
        let path = Path::new(&tree.path);
        let cancel = AtomicBool::new(false);
        assert_eq!(prepare(dir.path(), path, &cancel), None);
        set_setup_commands(&HashMap::from([(
            dir.path().to_string_lossy().into_owned(),
            "touch prepared".to_owned(),
        )]));
        assert_eq!(prepare(dir.path(), path, &cancel), None);
        assert!(path.join("prepared").exists());
        tidy(dir.path());
    }
}
