//! Bounded, cancellable project listing shared by mentions and Quick Open.
use super::Ignore;
use crate::scm::git_output_capped;
use crate::shell_env::dirs_home;
use crate::shell_env::expand_home;
use crate::shell_env::path_to_js;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
const MAX_PROJECT_FILES: usize = 20_000;
const MAX_WALK_DIRS: usize = 4_000;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFile {
    pub name: String,
    pub path: String,
    pub relative: String,
}

/// Workspace files for Quick Open. Prefer `git ls-files` (gitignore-aware,
/// index-backed); otherwise a bounded walk that never descends into vendor dirs.
pub fn list_project_files(cwd: &str) -> Result<Vec<ProjectFile>, String> {
    list_project_files_sync_cancellable(cwd, None)
}

/// Same listing, but aborts as soon as `cancel` is set.
///
/// Both halves of the enumeration can outlast a cancelled search on their own,
/// so both take the flag: `git ls-files` and the walk it falls back to. The
/// walk is the one that can run for minutes, but the listing is the one that
/// can hold a hundred megabytes, so neither is left uninterruptible.
pub fn list_project_files_sync_cancellable(
    cwd: &str,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<ProjectFile>, String> {
    if cancel.is_some_and(|token| token.load(Ordering::Acquire)) {
        return Ok(Vec::new());
    }

    let root = expand_home(Path::new(cwd));
    if !root.is_dir() {
        return Err(format!("{}: Not a directory", root.display()));
    }
    if !is_indexable_root(&root) {
        return Ok(Vec::new());
    }
    if cancel.is_some_and(|token| token.load(Ordering::Acquire)) {
        return Ok(Vec::new());
    }
    if let Some(files) = git_ls_files(&root, cancel) {
        return Ok(files);
    }
    // `git_ls_files` returns `None` for "not a git repo", truncation, and
    // cancel. The first two should walk; cancel must not, or a cancelled
    // `ls-files` would start enumerating the tree it just avoided.
    if cancel.is_some_and(|token| token.load(Ordering::Acquire)) {
        return Ok(Vec::new());
    }
    Ok(walk_project_files(&root, cancel))
}

const MAX_LS_FILES_BYTES: usize = 8 * 1024 * 1024;

fn git_ls_files(root: &Path, cancel: Option<&AtomicBool>) -> Option<Vec<ProjectFile>> {
    // Routed through the bounded reader rather than `Command::output()`: this
    // is the path a cancelled search actually takes, because a repository with
    // a git index never reaches the walk. `output()` buffers the whole listing
    // before anything can look at it, so on a large monorepo it held ~100 MB
    // and ran to completion with no way to interrupt it — the exact cost the
    // walk's cancel check was added to avoid, one function earlier.
    let (raw, truncated) = git_output_capped(
        root,
        &["ls-files", "-co", "--exclude-standard", "-z"],
        MAX_LS_FILES_BYTES,
        cancel,
    )?;
    if truncated {
        // A partial listing would silently hide files from search. Fall back to
        // the walk, which is bounded by its own budget.
        return None;
    }

    let mut files = Vec::new();
    for rel in raw.split(|b| *b == 0) {
        if cancel.is_some_and(|token| token.load(Ordering::Acquire)) {
            return Some(Vec::new());
        }
        if rel.is_empty() {
            continue;
        }
        let Ok(relative) = std::str::from_utf8(rel) else {
            continue;
        };
        if relative.ends_with('/') || path_has_skipped_dir(relative) {
            continue;
        }
        let path = root.join(relative);
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name == ".DS_Store" {
            continue;
        }
        files.push(ProjectFile {
            name: name.to_string(),
            path: path_to_js(&path),
            relative: relative.to_owned(),
        });
        if files.len() >= MAX_PROJECT_FILES {
            break;
        }
    }
    Some(files)
}

fn walk_project_files(root: &Path, cancel: Option<&AtomicBool>) -> Vec<ProjectFile> {
    let cancelled = || cancel.is_some_and(|flag| flag.load(Ordering::Acquire));
    let ignore = Ignore::load(root);
    let home = dirs_home().map(PathBuf::from);
    let mut files = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    let mut visited = 0usize;

    while let Some(dir) = dirs.pop() {
        if cancelled() {
            break;
        }
        visited += 1;
        if visited > MAX_WALK_DIRS || files.len() >= MAX_PROJECT_FILES {
            break;
        }
        let Ok(reader) = std::fs::read_dir(&dir) else {
            continue;
        };
        for ent in reader {
            if cancelled() {
                return files;
            }
            let Ok(ent) = ent else { continue };
            let name = ent.file_name();
            let Some(name) = name.to_str() else { continue };
            if name == ".DS_Store" {
                continue;
            }
            let path = ent.path();
            let is_dir = match ent.file_type() {
                Ok(t) if t.is_symlink() => continue,
                Ok(t) => t.is_dir(),
                Err(_) => path.is_dir(),
            };
            if is_dir {
                if skip_walk_dir_name(name)
                    || ignore.matches(name)
                    || is_private_dir(&path, home.as_deref())
                {
                    continue;
                }
                dirs.push(path);
                continue;
            }
            if ignore.matches(name) {
                continue;
            }
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            let relative = path_to_js(relative);
            files.push(ProjectFile {
                name: name.to_string(),
                path: path_to_js(&path),
                relative,
            });
            if files.len() >= MAX_PROJECT_FILES {
                break;
            }
        }
    }
    files
}

fn skip_walk_dir_name(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | "out"
            | ".next"
            | ".nuxt"
            | ".output"
            | ".cache"
            | ".turbo"
            | ".parcel-cache"
            | ".vercel"
            | ".svelte-kit"
            | "coverage"
            | "__pycache__"
            | ".venv"
            | "venv"
            | ".tox"
            | ".mypy_cache"
            | ".pytest_cache"
            | ".gradle"
            | ".idea"
            | "Pods"
            | "vendor"
            | "bower_components"
            | ".yarn"
            | ".pnpm-store"
    )
}

fn path_has_skipped_dir(relative: &str) -> bool {
    relative
        .rsplit_once(std::path::is_separator)
        .is_some_and(|(parents, _)| {
            parents
                .split(std::path::is_separator)
                .any(skip_walk_dir_name)
        })
}

/// Directories the OS guards behind a consent prompt. macOS pops "would like to
/// access data from other apps" the first time a process reads another app's
/// container, and the grant is per-folder — so a walk that brushes past a few of
/// them prompts again on every launch. Nothing in here is a user project, so the
/// indexer treats them as if they did not exist.
fn is_private_dir(path: &Path, home: Option<&Path>) -> bool {
    path.extension().is_some_and(|ext| ext == "app")
        || path == Path::new("/Library")
        || path == Path::new("/System")
        || (home.is_some()
            && path.parent() == home
            && matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some("Library" | ".Trash")
            ))
}

/// Roots too broad to index. Walking a home or volume root is never useful for
/// Quick Open — it buries project files under tens of thousands of dotfiles and
/// caches — and it is the one thing guaranteed to reach a private dir.
pub(crate) fn is_indexable_root(root: &Path) -> bool {
    let home = dirs_home().map(PathBuf::from);
    if root.parent().is_none()
        || home.as_deref() == Some(root)
        || is_private_dir(root, home.as_deref())
    {
        return false;
    }
    !["/Users", "/Applications", "/Volumes", "/home"]
        .into_iter()
        .any(|path| root == Path::new(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scm::git_cmd;
    #[test]
    fn indexes_untracked_files_respects_ignores_and_cancels() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        assert!(git_cmd()
            .arg("init")
            .arg("-q")
            .arg(root)
            .status()
            .unwrap()
            .success());
        std::fs::write(root.join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(root.join("visible.txt"), "context").unwrap();
        std::fs::write(root.join("ignored.txt"), "ignore").unwrap();
        std::fs::write(root.join("build"), "ordinary file").unwrap();
        std::fs::create_dir(root.join("vendor")).unwrap();
        std::fs::write(root.join("vendor/dep"), "skipped directory").unwrap();
        let files = list_project_files(root.to_str().unwrap()).unwrap();
        assert!(files.iter().any(|file| file.relative == "visible.txt"));
        assert!(files.iter().any(|file| file.relative == "build"));
        assert!(!files.iter().any(|file| file.relative == "ignored.txt"));
        assert!(!files.iter().any(|file| file.relative == "vendor/dep"));
        let cancel = AtomicBool::new(true);
        assert!(
            list_project_files_sync_cancellable(root.to_str().unwrap(), Some(&cancel))
                .unwrap()
                .is_empty()
        );
    }
}
