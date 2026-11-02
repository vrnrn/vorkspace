//! Worktree guards and setup, shared by Settings and the app actions. Removal keeps
//! commits: a branch goes only if Git agrees it is fully merged.
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
};

/// Ignored folders that tools regenerate. Any other ignored file, such as `.env`,
/// may be the only copy, so it has to stop an agent from removing a worktree.
const REGENERATED: [&str; 12] = [
    ".cache",
    ".next",
    ".turbo",
    ".venv",
    "__pycache__",
    "build",
    "coverage",
    "dist",
    "node_modules",
    "target",
    "venv",
    ".pytest_cache",
];
static REMOVING: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Held while a worktree is being removed. Turns and terminals check it, so nothing
/// starts inside a folder that is about to disappear.
pub struct RemovalGuard(String);
impl RemovalGuard {
    pub fn claim(path: &str) -> Result<Self, String> {
        let key = crate::files::path_key(path);
        let mut removing = REMOVING.lock().unwrap_or_else(|e| e.into_inner());
        if !removing.get_or_insert_default().insert(key.clone()) {
            return Err("This worktree is already being removed.".into());
        }
        Ok(Self(key))
    }
}
impl Drop for RemovalGuard {
    fn drop(&mut self) {
        let mut removing = REMOVING.lock().unwrap_or_else(|e| e.into_inner());
        removing.get_or_insert_default().remove(&self.0);
    }
}
pub fn is_being_removed(path: &str) -> bool {
    let removing = REMOVING.lock().unwrap_or_else(|e| e.into_inner());
    removing
        .iter()
        .flatten()
        .any(|root| crate::files::is_equal_or_inside(path, root))
}
pub fn default_root(main: &Path) -> PathBuf {
    main.with_file_name(format!(
        "{}-worktrees",
        main.file_name().unwrap_or_default().to_string_lossy()
    ))
}
/// What removing a worktree would cost. Settings and the app actions share this,
/// so a Bot can never remove something the UI would warn about.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Assessment {
    pub locked: bool,
    pub dirty: Option<bool>,
    pub unpushed: usize,
    pub merged: bool,
    /// Ignored files that nothing would regenerate.
    pub ignored: Vec<String>,
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
        } else if let Some(file) = self.ignored.first() {
            Some(format!(
                "it has ignored files that may be irreplaceable, such as {file}"
            ))
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
    let ignored = crate::git::git(
        Path::new(&tree.path),
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
        ],
    )
    .unwrap_or_default()
    .lines()
    .map(|entry| entry.trim_end_matches('/'))
    .filter(|entry| !entry.split('/').any(|part| REGENERATED.contains(&part)))
    .take(5)
    .map(str::to_owned)
    .collect();
    Assessment {
        locked,
        dirty,
        unpushed,
        merged,
        ignored,
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
    remove_merged_branch(cwd, &tree, &assessment);
    Ok(())
}
/// `-d` refuses anything Git doesn't consider merged, so this can only tidy.
fn remove_merged_branch(cwd: &Path, tree: &crate::git::Worktree, assessment: &Assessment) {
    if let Some(branch) = tree.branch.as_deref().filter(|_| assessment.merged) {
        let _ = crate::git::git(cwd, &["branch", "-d", "--", branch]);
    }
}
/// The app actions' removal. It refuses rather than discards, and Git's own check
/// without `--force` catches anything written after this assessment.
pub fn remove_if_safe(cwd: &Path, path: &Path) -> Result<(), String> {
    let tree = check(cwd, path)?;
    let assessment = assess(cwd, &tree);
    if let Some(blocker) = assessment.blocker() {
        return Err(format!(
            "This worktree was kept because {blocker}. Tell the user rather than removing it."
        ));
    }
    crate::git::git(cwd, &["worktree", "remove", "--", &tree.path]).map_err(|error| {
        format!("This worktree was kept because Git refused to remove it: {error}")
    })?;
    crate::provider::discard_worktree(Path::new(&tree.path));
    remove_merged_branch(cwd, &tree, &assessment);
    Ok(())
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
    fn ignored_files_stop_an_agent_unless_tools_regenerate_them() {
        let dir = repo();
        std::fs::write(dir.path().join(".gitignore"), ".env\ntarget/\n").unwrap();
        crate::git::git(dir.path(), &["add", ".gitignore"]).unwrap();
        crate::git::git(dir.path(), &["commit", "-m", "Ignore"]).unwrap();
        let tree = crate::git::create_named_worktree(dir.path(), "local-files").unwrap();
        let path = Path::new(&tree.path);
        std::fs::create_dir(path.join("target")).unwrap();
        std::fs::write(path.join("target/output"), "built").unwrap();
        assert_eq!(assess(dir.path(), &tree).blocker(), None);
        std::fs::write(path.join(".env"), "SECRET=1").unwrap();
        let refused = remove_if_safe(dir.path(), path).unwrap_err();
        assert!(refused.contains(".env"), "{refused}");
        assert!(path.join(".env").exists());
        std::fs::remove_file(path.join(".env")).unwrap();
        remove_if_safe(dir.path(), path).unwrap();
        assert!(!path.exists());
        tidy(dir.path());
    }
    #[test]
    fn an_alias_of_a_worktree_resolves_to_its_registered_path() {
        let dir = repo();
        let tree = crate::git::create_named_worktree(dir.path(), "aliased").unwrap();
        let alias = format!("{}/.", tree.path);
        assert_eq!(
            check(dir.path(), Path::new(&alias)).unwrap().path,
            tree.path
        );
        remove(dir.path(), Path::new(&tree.path)).unwrap();
        tidy(dir.path());
    }
    #[test]
    fn a_claimed_worktree_blocks_new_work_until_released() {
        let guard = RemovalGuard::claim("/project-worktrees/feature").unwrap();
        assert!(is_being_removed("/project-worktrees/feature/src"));
        assert!(!is_being_removed("/project-worktrees/other"));
        assert!(RemovalGuard::claim("/project-worktrees/feature").is_err());
        drop(guard);
        assert!(!is_being_removed("/project-worktrees/feature"));
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
}
