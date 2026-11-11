//! Working-copy operations used by the native composer. Git protects dirty files;
//! these operations never force a checkout, stash, or discard changes.
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

#[derive(Clone, Debug, Deserialize)]
pub struct Branch {
    pub name: String,
    pub remote: Option<String>,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    pub path: String,
    pub branch: Option<String>,
    pub head: String,
    #[serde(alias = "isMain")]
    pub main: bool,
    pub missing: bool,
}
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub current: Option<String>,
    pub detached: bool,
    pub linked: bool,
    pub branches: Arc<[Branch]>,
    pub worktrees: Arc<[Worktree]>,
}

pub struct CheckoutHead {
    pub current: Option<String>,
    pub detached: bool,
    pub linked: bool,
}

impl From<Snapshot> for CheckoutHead {
    fn from(snapshot: Snapshot) -> Self {
        Self {
            current: snapshot.current,
            detached: snapshot.detached,
            linked: snapshot.linked,
        }
    }
}
pub(crate) fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    command
        .current_dir(cwd)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0");
    crate::process::configure_child(&mut command);
    let out = command
        .output()
        .map_err(|e| format!("Could not run Git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    String::from_utf8(out.stdout).map_err(|e| e.to_string())
}
pub fn snapshot(cwd: &Path) -> Result<Snapshot, String> {
    let trees = worktrees(cwd)?;
    let CheckoutHead {
        current,
        detached,
        linked,
    } = local_head(cwd, &trees)?;
    let branches = git(
        cwd,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname)",
            "refs/heads/",
            "refs/remotes/",
        ],
    )?
    .lines()
    .filter_map(|line| {
        if let Some(name) = line.strip_prefix("refs/heads/") {
            Some(Branch {
                name: name.into(),
                remote: None,
            })
        } else {
            let (remote, name) = line.strip_prefix("refs/remotes/")?.split_once('/')?;
            (name != "HEAD").then(|| Branch {
                name: name.into(),
                remote: Some(remote.into()),
            })
        }
    })
    .collect();
    Ok(Snapshot {
        current,
        detached,
        linked,
        branches,
        worktrees: trees.into(),
    })
}

/// Resolve the current checkout without enumerating its branch choices.
pub fn head(cwd: &Path) -> Result<CheckoutHead, String> {
    local_head(cwd, &worktrees(cwd)?)
}

fn local_head(cwd: &Path, trees: &[Worktree]) -> Result<CheckoutHead, String> {
    let canonical = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_owned());
    let working_copy = trees
        .iter()
        .filter_map(|tree| {
            let root = std::fs::canonicalize(&tree.path).ok()?;
            (canonical.starts_with(&root)
                && !canonical.starts_with(root.join(".git"))
                && !tree.head.is_empty())
            .then_some((root, tree))
        })
        .max_by_key(|(root, _)| root.components().count());
    let (current, linked) = if let Some((_, tree)) = working_copy {
        (tree.branch.clone(), !tree.main)
    } else {
        // Custom Git layouts can put the checkout outside its registered path.
        // Git must also reject bare repositories and its own metadata directory.
        let root = git(cwd, &["rev-parse", "--show-toplevel"])?;
        let root = std::fs::canonicalize(root.trim()).unwrap_or(canonical);
        let current = git(cwd, &["symbolic-ref", "--short", "HEAD"])
            .ok()
            .map(|s| s.trim().to_owned());
        let linked = trees
            .iter()
            .any(|t| !t.main && std::fs::canonicalize(&t.path).ok().as_ref() == Some(&root));
        (current, linked)
    };
    let detached = current.is_none();
    let current = current.or_else(|| {
        git(cwd, &["rev-parse", "--short", "HEAD"])
            .ok()
            .map(|s| s.trim().to_owned())
    });
    Ok(CheckoutHead {
        current,
        detached,
        linked,
    })
}

/// Listing working copies does not require reading every local/remote branch.
pub fn worktrees(cwd: &Path) -> Result<Vec<Worktree>, String> {
    let mut trees = Vec::new();
    let mut tree = Worktree::default();
    for field in git(cwd, &["worktree", "list", "--porcelain", "-z"])?.split('\0') {
        if field.is_empty() {
            if !tree.path.is_empty() {
                tree.main = trees.is_empty();
                tree.missing = !Path::new(&tree.path).is_dir();
                trees.push(std::mem::take(&mut tree));
            }
        } else if let Some(path) = field.strip_prefix("worktree ") {
            tree.path = path.into();
        } else if let Some(branch) = field.strip_prefix("branch refs/heads/") {
            tree.branch = Some(branch.into());
        } else if let Some(head) = field.strip_prefix("HEAD ") {
            tree.head = head.into();
        }
    }
    Ok(trees)
}
pub fn default_worktree_root(main: &Path) -> PathBuf {
    main.with_file_name(format!(
        "{}-worktrees",
        main.file_name().unwrap_or_default().to_string_lossy()
    ))
}
fn valid_branch(cwd: &Path, name: &str) -> Result<(), String> {
    if name.is_empty() || name.starts_with(['-', '@']) {
        return Err("Enter a valid branch name".into());
    }
    git(cwd, &["check-ref-format", "--branch", name]).map(|_| ())
}
pub fn checkout(cwd: &Path, branch: &Branch) -> Result<(), String> {
    valid_branch(cwd, &branch.name)?;
    if let Some(remote) = &branch.remote {
        let reference = format!("refs/remotes/{remote}/{}", branch.name);
        // Verify the chosen remote even when a same-named local branch exists.
        git(cwd, &["show-ref", "--verify", "--quiet", &reference])
            .map_err(|_| "Choose an available remote branch".to_owned())?;
        let local = format!("refs/heads/{}", branch.name);
        if git(cwd, &["show-ref", "--verify", "--quiet", &local]).is_ok() {
            let upstream = git(cwd, &["for-each-ref", "--format=%(upstream)", &local])?;
            if upstream.trim() != reference {
                return Err(format!(
                    "Local branch ‘{}’ already exists and does not track {remote}/{}. Select the local branch instead.",
                    branch.name, branch.name
                ));
            }
            git(cwd, &["switch", "--no-guess", "--", &branch.name])?;
        } else {
            git(
                cwd,
                &["switch", "--track", "-c", &branch.name, "--", &reference],
            )?;
        }
    } else {
        git(cwd, &["switch", "--no-guess", "--", &branch.name])?;
    }
    Ok(())
}
pub fn create_branch(cwd: &Path, name: &str) -> Result<(), String> {
    valid_branch(cwd, name)?;
    git(cwd, &["switch", "-c", name])?;
    Ok(())
}
pub fn create_worktree(cwd: &Path, base: &str, session: i64) -> Result<PathBuf, String> {
    let trees = worktrees(cwd)?;
    let main = trees.first().ok_or("No working copies found")?;
    let main = Path::new(&main.path);
    let parent = default_worktree_root(main);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let name = format!("mc/native-{session}-{stamp}");
    let path = parent.join(name.replace('/', "-"));
    let commit = git(
        cwd,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{base}^{{commit}}"),
        ],
    )?;
    std::fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
    git(
        cwd,
        &[
            "worktree",
            "add",
            "--no-track",
            "-b",
            &name,
            "--",
            &path.to_string_lossy(),
            commit.trim(),
        ],
    )?;
    crate::worktree_setup::mark_due(&[main, cwd], &path);
    Ok(path)
}

/// SidebarWorktreeSwitcher: create a named copy from the selected checkout's
/// HEAD, alongside the main project's other worktrees. Existing files and
/// branches are never replaced.
pub fn create_named_worktree(cwd: &Path, branch: &str) -> Result<Worktree, String> {
    create_project_worktree(cwd, branch, None, false)
}
pub fn create_project_worktree(
    cwd: &Path,
    branch: &str,
    base: Option<&str>,
    existing: bool,
) -> Result<Worktree, String> {
    let branch = branch.trim();
    valid_branch(cwd, branch)?;
    let trees = worktrees(cwd)?;
    if trees
        .iter()
        .any(|tree| tree.branch.as_deref() == Some(branch))
    {
        return Err("This branch already has a working copy. Select it from the picker.".into());
    }
    let main = Path::new(&trees.first().ok_or("No working copies found")?.path);
    let parent = default_worktree_root(main);
    let slug: String = branch
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let path = parent.join(slug);
    if path.exists() {
        return Err(format!(
            "{} already exists. Choose another branch name.",
            path.display()
        ));
    }
    let reference = if existing {
        format!("refs/heads/{branch}")
    } else {
        base.unwrap_or("HEAD").into()
    };
    let commit = git(
        cwd,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{reference}^{{commit}}"),
        ],
    )?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    if existing {
        git(
            cwd,
            &["worktree", "add", "--", &path.to_string_lossy(), branch],
        )?;
    } else {
        git(
            cwd,
            &[
                "worktree",
                "add",
                "--no-track",
                "-b",
                branch,
                "--",
                &path.to_string_lossy(),
                commit.trim(),
            ],
        )?;
    }
    crate::worktree_setup::mark_due(&[main, cwd], &path);
    worktrees(cwd)?
        .into_iter()
        .find(|tree| Path::new(&tree.path) == path)
        .ok_or_else(|| {
            "Worktree created, but could not be found. Refresh the working copies.".into()
        })
}

#[cfg(test)]
mod tests;
