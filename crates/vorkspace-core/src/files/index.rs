//! Bounded, cancellable project listing shared by mentions and Quick Open.
//! Ported from the retired Tauri filesystem service.
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
    if crate::remote_workspace::is_remote(Path::new(cwd)) {
        let files =
            crate::remote_workspace::command("list_project_files", serde_json::json!({"cwd":cwd}))?;
        return Ok(
            if cancel.is_some_and(|token| token.load(Ordering::Acquire)) {
                Vec::new()
            } else {
                files
            },
        );
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
    Ok(walk_project_files(&root, cancel.into()))
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
        let relative = path_to_js(Path::new(String::from_utf8_lossy(rel).as_ref()));
        if relative.ends_with('/') || path_has_skipped_dir(&relative) {
            continue;
        }
        let path = root.join(&relative);
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name == ".DS_Store" {
            continue;
        }
        files.push(ProjectFile {
            name: name.to_string(),
            path: path_to_js(&path),
            relative,
        });
        if files.len() >= MAX_PROJECT_FILES {
            break;
        }
    }
    Some(files)
}

/// When a file walk should stop early.
///
/// The flag variant is what a search uses. `AfterEntries` exists so the
/// per-entry check can be tested without a thread and a sleep: setting the flag
/// from outside only proves the walk stops *somewhere*, which the up-front
/// check would satisfy on its own, and timing a real cancellation is flaky on
/// loaded CI.
#[derive(Clone, Copy)]
enum WalkStop<'a> {
    Never,
    Flag(&'a AtomicBool),
}

impl<'a> WalkStop<'a> {
    fn stopped(&self, _entries: usize) -> bool {
        match self {
            WalkStop::Never => false,
            WalkStop::Flag(flag) => flag.load(Ordering::Acquire),
        }
    }
}

impl<'a> From<Option<&'a AtomicBool>> for WalkStop<'a> {
    fn from(cancel: Option<&'a AtomicBool>) -> Self {
        match cancel {
            Some(flag) => WalkStop::Flag(flag),
            None => WalkStop::Never,
        }
    }
}

fn walk_project_files(root: &Path, stop: WalkStop<'_>) -> Vec<ProjectFile> {
    let ignore = Ignore::load(root);
    let mut files = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    let mut visited = 0usize;
    let mut seen = 0usize;

    while let Some(dir) = dirs.pop() {
        if stop.stopped(seen) {
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
            seen += 1;
            if stop.stopped(seen) {
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
                if skip_walk_dir_name(name) || ignore.matches(name) || is_private_dir(&path) {
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
        .split(std::path::is_separator)
        .any(skip_walk_dir_name)
}

/// Directories the OS guards behind a consent prompt. macOS pops "would like to
/// access data from other apps" the first time a process reads another app's
/// container, and the grant is per-folder — so a walk that brushes past a few of
/// them prompts again on every launch. Nothing in here is a user project, so the
/// indexer treats them as if they did not exist.
fn is_private_dir(path: &Path) -> bool {
    if path.extension().is_some_and(|ext| ext == "app") {
        return true;
    }
    let guarded = [
        dirs_home().map(|home| PathBuf::from(home).join("Library")),
        dirs_home().map(|home| PathBuf::from(home).join(".Trash")),
        Some(PathBuf::from("/Library")),
        Some(PathBuf::from("/System")),
    ];
    guarded
        .iter()
        .flatten()
        .any(|guarded| path == guarded.as_path())
}

/// Roots too broad to index. Walking a home or volume root is never useful for
/// Quick Open — it buries project files under tens of thousands of dotfiles and
/// caches — and it is the one thing guaranteed to reach a private dir.
pub(crate) fn is_indexable_root(root: &Path) -> bool {
    if is_private_dir(root) {
        return false;
    }
    if root.parent().is_none() {
        return false;
    }
    let too_broad = [
        dirs_home().map(PathBuf::from),
        Some(PathBuf::from("/Users")),
        Some(PathBuf::from("/Applications")),
        Some(PathBuf::from("/Volumes")),
        Some(PathBuf::from("/home")),
    ];
    !too_broad
        .iter()
        .flatten()
        .any(|broad| root == broad.as_path())
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let files = list_project_files(root.to_str().unwrap()).unwrap();
        assert!(files.iter().any(|file| file.relative == "visible.txt"));
        assert!(!files.iter().any(|file| file.relative == "ignored.txt"));
        let cancel = AtomicBool::new(true);
        assert!(
            list_project_files_sync_cancellable(root.to_str().unwrap(), Some(&cancel))
                .unwrap()
                .is_empty()
        );
    }
}
