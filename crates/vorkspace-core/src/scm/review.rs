//! Working-tree and commit reads for unified review, and partial staging.
use super::status::{Counts, LocalStatus};
use super::*;
use crate::shell_env::{expand_home, path_to_js};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
};

/// fs.rs `MAX_TEXT_FILE_BYTES`: larger sides show as too large to diff.
pub const MAX_TEXT_FILE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    pub path: String,
    pub relative: String,
    pub status: String,
    pub original: String,
    pub current: String,
    pub binary: bool,
    pub too_large: bool,
}

pub struct DiffSnapshot {
    pub index: DiffIndex,
    pub versions: Option<HashMap<String, FileDiffVersion>>,
    pub stamp: Option<CheckoutStamp>,
    pub local_head: Option<LocalHead>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalHead {
    Missing,
    Attached(String),
    Detached(Option<String>),
}

impl DiffSnapshot {
    pub fn read(cwd: &Path) -> Result<Self, String> {
        Self::observe(cwd, None)?.ok_or_else(|| "Missing Git observation".into())
    }

    pub fn observe(cwd: &Path, known: Option<CheckoutStamp>) -> Result<Option<Self>, String> {
        let root = expand_home(cwd);
        Ok(match observe_status(&root, known)? {
            StatusObservation::Unchanged => None,
            StatusObservation::Missing => Some(Self {
                index: DiffIndex::default(),
                versions: Some(HashMap::new()),
                stamp: None,
                local_head: Some(LocalHead::Missing),
            }),
            StatusObservation::Present(status) => {
                Some(Self::local(&root, status, Counts::Full { sync: true }))
            }
        })
    }

    fn local(cwd: &Path, status: LocalStatus, counts: Counts<'_>) -> Self {
        let index = status.index(cwd, counts);
        let local_head = match &status.branch {
            Some(branch) => LocalHead::Attached(branch.clone()),
            None if index.head.is_some() => LocalHead::Detached(index.branch.clone()),
            None => LocalHead::Missing,
        };
        Self {
            index,
            versions: Some(status.versions()),
            stamp: status.stamp,
            local_head: Some(local_head),
        }
    }
}

/// Changed files and counts, without the branch and upstream reads.
pub fn diff_files(cwd: &Path) -> Result<DiffIndex, String> {
    local_index(cwd, false)
}

pub(super) fn local_index(cwd: &Path, sync: bool) -> Result<DiffIndex, String> {
    let root = expand_home(cwd);
    Ok(match observe_status(&root, None)? {
        StatusObservation::Present(status) => status.index(&root, Counts::Full { sync }),
        StatusObservation::Unchanged | StatusObservation::Missing => DiffIndex::default(),
    })
}

enum StatusObservation {
    Unchanged,
    Missing,
    Present(LocalStatus),
}

fn observe_status(root: &Path, known: Option<CheckoutStamp>) -> Result<StatusObservation, String> {
    match LocalStatus::output(root) {
        Some(raw) => LocalStatus::parse(root, None, &raw, known)
            .map(|status| status.map_or(StatusObservation::Unchanged, StatusObservation::Present)),
        None if git_is_work_tree(root) => Err("Could not read Git status".into()),
        None => Ok(StatusObservation::Missing),
    }
}

/// Read a working-copy review against one resolved Git prefix.
pub struct WorktreeReview {
    cwd: PathBuf,
    // The root prefix is empty for a project at the top of the repository.
    prefix: String,
}

impl WorktreeReview {
    pub fn open(cwd: &Path) -> Result<Self, String> {
        let cwd = expand_home(cwd);
        let location = git_output(
            &cwd,
            &["rev-parse", "--is-inside-work-tree", "--show-prefix"],
        )
        .ok_or("Not a git repository")?;
        let location = std::str::from_utf8(&location).map_err(|error| error.to_string())?;
        let prefix = location
            .strip_prefix("true\n")
            .and_then(|prefix| prefix.strip_suffix('\n'))
            .ok_or("Not a Git working copy")?;
        Ok(Self {
            cwd,
            prefix: prefix.to_owned(),
        })
    }

    pub fn snapshot(&self, previous: Option<&DiffIndex>) -> Result<DiffSnapshot, String> {
        Ok(DiffSnapshot::local(
            &self.cwd,
            self.local_status()?,
            Counts::Previous(previous),
        ))
    }

    fn local_status(&self) -> Result<LocalStatus, String> {
        let raw = LocalStatus::output(&self.cwd).ok_or("Could not read Git status")?;
        LocalStatus::parse(&self.cwd, Some(&self.prefix), &raw, None)?
            .ok_or_else(|| "Missing Git observation".into())
    }

    /// One file's index or HEAD side against its other side. `staged` compares
    /// HEAD with the index; otherwise the index with the working tree.
    pub fn file(&self, relative: &str, staged: bool) -> Result<FileDiff, String> {
        let root = &self.cwd;
        let prefix = &self.prefix;
        let relative = resolve_repo_path(root, relative)?;
        let abs = root.join(&relative);
        let index_spec = format!(":{prefix}{relative}");
        let (original, current) = if staged {
            let head_spec = format!("HEAD:{prefix}{relative}");
            (git_blob(root, &head_spec), git_blob(root, &index_spec))
        } else {
            let current = read_worktree_file(root, &relative, MAX_TEXT_FILE_BYTES)
                .map_err(|error| error.to_string())?;
            (git_blob(root, &index_spec), current)
        };
        Ok(text_diff(
            path_to_js(&abs),
            relative,
            if staged { "added" } else { "untracked" },
            original,
            current,
        ))
    }
}

pub fn file_diff(cwd: &Path, relative: &str, staged: bool) -> Result<FileDiff, String> {
    WorktreeReview::open(cwd)?.file(relative, staged)
}

/// Read only the Git side an open text editor compares its buffer against.
pub fn editor_baseline(cwd: &Path, relative: &str) -> Result<Option<(String, bool)>, String> {
    let checkout = WorktreeReview::open(cwd)?;
    let root = &checkout.cwd;
    let prefix = &checkout.prefix;
    let relative = resolve_repo_path(root, relative)?;
    let status = git_output(
        root,
        &[
            "--literal-pathspecs",
            "status",
            "--porcelain=v1",
            "-z",
            "--no-renames",
            "--untracked-files=all",
            "--",
            &relative,
        ],
    )
    .ok_or("Could not read the file's Git status")?;
    let mut staged = false;
    let mut unstaged = false;
    for record in status.split(|&byte| byte == 0) {
        let [index, disk, b' ', path @ ..] = record else {
            continue;
        };
        if path.strip_prefix(prefix.as_bytes()) != Some(relative.as_bytes()) {
            continue;
        }
        staged |= *index != b' ' && *index != b'?';
        unstaged |= *disk != b' ';
    }
    let staged = staged && !unstaged;
    let rev = if staged { "HEAD:" } else { ":" };
    let base = match git_blob(root, &format!("{rev}{prefix}{relative}")) {
        None => Some(String::new()),
        Some(FileContents::Bytes(bytes)) if !bytes.contains(&0) => Some(
            String::from_utf8(bytes)
                .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned()),
        ),
        _ => None,
    };
    Ok(base.map(|base| (base, staged)))
}

/// Resolve a commit once before listing its files or reading any of their sides.
pub struct CommitReview {
    cwd: PathBuf,
    sha: String,
    parent: Option<String>,
    prefix: String,
}

impl CommitReview {
    pub fn open(cwd: &Path, sha: &str) -> Result<Self, String> {
        let WorktreeReview { cwd, prefix } = WorktreeReview::open(cwd)?;

        let sha = peel_commit(&cwd, sha)?;
        let parent = git_stdout(&cwd, &["rev-parse", "--verify", &format!("{sha}^")]);
        Ok(Self {
            cwd,
            sha,
            parent,
            prefix,
        })
    }

    /// The files a commit changed, against its first parent (or nothing).
    pub fn files(&self) -> Result<Vec<ChangedFile>, String> {
        let Self {
            cwd: root,
            sha,
            parent,
            ..
        } = self;
        let mut files: HashMap<String, FileAcc> = HashMap::new();
        let mut statuses: HashMap<String, &'static str> = HashMap::new();
        let tree = |kind: &str| {
            let mut args = vec![
                "diff-tree",
                "-z",
                "--no-commit-id",
                "-r",
                "--root",
                "--relative",
                "--no-renames",
                kind,
            ];
            args.extend(parent.as_deref());
            args.extend([sha.as_str(), "--", "."]);
            git_run(root, &args).ok_or("Could not list the commit's changes")
        };
        add_numstat_map(&tree("--numstat")?, &mut files);
        add_name_status(&tree("--name-status")?, &mut statuses);
        let mut out = Vec::with_capacity(files.len().max(statuses.len()));
        let mut seen = HashSet::new();
        let file = |relative: String, status: &str, additions, deletions| ChangedFile {
            path: path_to_js(&root.join(&relative)),
            relative,
            status: status.to_string(),
            additions,
            deletions,
            staged: false,
            unstaged: false,
        };
        for (relative, acc) in files {
            seen.insert(relative.clone());
            let status = statuses.get(&relative).copied().unwrap_or("modified");
            out.push(file(relative, status, acc.additions, acc.deletions));
        }
        for (relative, status) in statuses {
            if !seen.contains(&relative) {
                out.push(file(relative, status, 0, 0));
            }
        }
        out.sort_by(|a, b| a.relative.cmp(&b.relative));
        Ok(out)
    }

    /// One file in a commit against its first parent.
    pub fn file(&self, relative: &str) -> Result<FileDiff, String> {
        let Self {
            cwd: root,
            sha,
            parent,
            prefix,
        } = self;

        let relative = resolve_repo_path(root, relative)?;
        let original = parent
            .as_ref()
            .and_then(|parent| git_blob(root, &format!("{parent}:{prefix}{relative}")));
        let current = git_blob(root, &format!("{sha}:{prefix}{relative}"));
        Ok(text_diff(
            path_to_js(&root.join(&relative)),
            relative,
            "added",
            original,
            current,
        ))
    }
}

/// Writes `contents` as the file's index entry, keeping its mode: stages a
/// single hunk without touching the working tree.
pub fn stage_contents(cwd: &Path, relative: &str, contents: &[u8]) -> Result<(), String> {
    let root = expand_home(cwd);
    let relative = resolve_repo_path(&root, relative)?;
    if contents.len() as u64 > MAX_TEXT_FILE_BYTES {
        return Err("File too large".into());
    }
    let hash = hash_object(&root, &relative, contents)?;
    let mode = index_mode(&root, &relative).unwrap_or_else(|| "100644".into());
    git_checked(
        &root,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &mode,
            &hash,
            &relative,
        ],
    )
}

fn text_diff(
    path: String,
    relative: String,
    added: &str,
    original: Option<FileContents>,
    current: Option<FileContents>,
) -> FileDiff {
    let status = match (original.is_some(), current.is_some()) {
        (false, true) => added,
        (true, false) => "deleted",
        _ => "modified",
    };
    let too_large = [&original, &current]
        .iter()
        .any(|side| matches!(side, Some(FileContents::TooLarge)));
    let bytes = |side| match side {
        Some(FileContents::Bytes(bytes)) => bytes,
        _ => Vec::new(),
    };
    let (original, current) = (bytes(original), bytes(current));
    let binary = original.contains(&0) || current.contains(&0);
    let (original, current) = if binary || too_large {
        (String::new(), String::new())
    } else {
        let decode = |bytes| {
            String::from_utf8(bytes)
                .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
        };
        (decode(original), decode(current))
    };
    FileDiff {
        path,
        relative,
        status: status.into(),
        original,
        current,
        binary,
        too_large,
    }
}
fn git_blob(root: &Path, spec: &str) -> Option<FileContents> {
    let (bytes, truncated) = git_output_capped(
        root,
        &["cat-file", "-p", spec],
        MAX_TEXT_FILE_BYTES as usize,
        None,
    )?;
    Some(if truncated {
        FileContents::TooLarge
    } else {
        FileContents::Bytes(bytes)
    })
}
fn peel_commit(root: &Path, spec: &str) -> Result<String, String> {
    let spec = spec.trim();
    if !(4..=64).contains(&spec.len()) || !spec.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Invalid commit".into());
    }
    git_stdout(
        root,
        &["rev-parse", "--verify", &format!("{spec}^{{commit}}")],
    )
    .ok_or_else(|| "Unknown commit".into())
}
fn index_mode(root: &Path, relative: &str) -> Option<String> {
    let out = git_run(
        root,
        &["--literal-pathspecs", "ls-files", "--stage", "--", relative],
    )?;
    let mode = out.lines().next()?.split_whitespace().next()?;
    (mode.len() == 6 && mode.bytes().all(|b| b.is_ascii_digit())).then(|| mode.to_string())
}
fn hash_object(root: &Path, relative: &str, contents: &[u8]) -> Result<String, String> {
    let mut command = git_cmd();
    command
        .arg("--no-pager")
        .arg("-C")
        .arg(root)
        .args(["hash-object", "-w", "--path", relative, "--stdin"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "hash-object stdin".to_string())?;
    stdin.write_all(contents).map_err(|e| e.to_string())?;
    drop(stdin);
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err([stderr.trim(), stdout.trim()]
            .into_iter()
            .find(|m| !m.is_empty())
            .unwrap_or("git hash-object failed")
            .to_string());
    }
    let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if hash.len() != 40 && hash.len() != 64 {
        return Err("git hash-object returned an invalid hash".into());
    }
    Ok(hash)
}
