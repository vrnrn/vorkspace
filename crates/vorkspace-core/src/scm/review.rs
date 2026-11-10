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
}

impl DiffSnapshot {
    pub fn read(cwd: &Path) -> Result<Self, String> {
        Self::observe(cwd, None)?.ok_or_else(|| "Missing Git observation".into())
    }

    pub fn observe(cwd: &Path, known: Option<CheckoutStamp>) -> Result<Option<Self>, String> {
        if remote_workspace::is_remote(cwd) {
            return Self::remote(cwd, "git_diff_index").map(Some);
        }
        let root = expand_home(cwd);
        Ok(observe_status(&root, known)?
            .map(|status| Self::local(&root, status, Counts::Full { sync: true })))
    }

    fn local(cwd: &Path, status: LocalStatus, counts: Counts<'_>) -> Self {
        Self {
            index: status.index(cwd, counts),
            versions: Some(status.versions()),
            stamp: status.stamp,
        }
    }

    fn remote(cwd: &Path, command: &str) -> Result<Self, String> {
        #[derive(Deserialize)]
        struct WireSnapshot {
            #[serde(flatten)]
            index: DiffIndex,
            #[serde(default, rename = "fileVersions")]
            versions: Option<Vec<[Option<String>; 2]>>,
        }
        let WireSnapshot { index, versions } =
            remote_workspace::command(command, json!({"cwd":cwd,"includeVersions":true}))?;
        let versions = versions
            .filter(|versions| {
                versions.len() == index.files.len()
                    && versions
                        .iter()
                        .zip(&index.files)
                        .all(|([staged, unstaged], file)| {
                            staged.is_some() == file.staged && unstaged.is_some() == file.unstaged
                        })
            })
            .map(|versions| {
                versions
                    .into_iter()
                    .zip(&index.files)
                    .flat_map(|([staged, unstaged], file)| {
                        [("staged", staged), ("unstaged", unstaged)]
                            .into_iter()
                            .filter_map(move |(side, version)| {
                                Some((
                                    format!("{side}:{}", file.relative),
                                    FileDiffVersion {
                                        index: version?,
                                        head: None,
                                        disk: None,
                                    },
                                ))
                            })
                    })
                    .collect()
            })
            .or_else(|| remote_versions(cwd, &index).ok());
        Ok(Self {
            index,
            versions,
            stamp: None,
        })
    }
}

/// Changed files and counts, without the branch and upstream reads.
pub fn diff_files(cwd: &Path) -> Result<DiffIndex, String> {
    if remote_workspace::is_remote(cwd) {
        return remote_workspace::command("git_diff_files", json!({"cwd":cwd}));
    }
    local_index(cwd, false)
}

pub(super) fn local_index(cwd: &Path, sync: bool) -> Result<DiffIndex, String> {
    let root = expand_home(cwd);
    Ok(
        observe_status(&root, None)?.map_or_else(DiffIndex::default, |status| {
            status.index(&root, Counts::Full { sync })
        }),
    )
}

fn observe_status(
    root: &Path,
    known: Option<CheckoutStamp>,
) -> Result<Option<LocalStatus>, String> {
    match LocalStatus::output(root) {
        Some(raw) => LocalStatus::parse(root, None, &raw, known),
        None if git_is_work_tree(root) => Err("Could not read Git status".into()),
        None => Ok(Some(LocalStatus::default())),
    }
}

/// Read a working-copy review against one resolved Git prefix.
pub struct WorktreeReview {
    cwd: PathBuf,
    // Remote paths are resolved by their own host. The local root prefix is
    // an empty string for a project at the top of the repository.
    prefix: Option<String>,
}

impl WorktreeReview {
    pub fn open(cwd: &Path) -> Result<Self, String> {
        if remote_workspace::is_remote(cwd) {
            return Ok(Self {
                cwd: cwd.to_path_buf(),
                prefix: None,
            });
        }
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
            prefix: Some(prefix.to_owned()),
        })
    }

    pub fn snapshot(&self, previous: Option<&DiffIndex>) -> Result<DiffSnapshot, String> {
        if self.prefix.is_none() {
            return DiffSnapshot::remote(&self.cwd, "git_diff_files");
        }
        Ok(DiffSnapshot::local(
            &self.cwd,
            self.local_status()?,
            Counts::Previous(previous),
        ))
    }

    fn local_status(&self) -> Result<LocalStatus, String> {
        let raw = LocalStatus::output(&self.cwd).ok_or("Could not read Git status")?;
        LocalStatus::parse(&self.cwd, self.prefix.as_deref(), &raw, None)?
            .ok_or_else(|| "Missing Git observation".into())
    }

    /// One file's index or HEAD side against its other side. `staged` compares
    /// HEAD with the index; otherwise the index with the working tree.
    pub fn file(&self, relative: &str, staged: bool) -> Result<FileDiff, String> {
        let root = &self.cwd;
        let Some(prefix) = &self.prefix else {
            return remote_workspace::command(
                "git_file_diff",
                json!({"cwd":root,"relative":relative,"staged":staged}),
            );
        };
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

fn remote_versions(
    cwd: &Path,
    snapshot: &DiffIndex,
) -> Result<HashMap<String, FileDiffVersion>, String> {
    // Older hosts omit versions. Use their bounded file-diff API so that
    // equal line counts still invalidate an externally edited diff.
    use sha2::{Digest, Sha256};
    let checkout = WorktreeReview::open(cwd)?;
    let mut versions = HashMap::new();
    for file in &snapshot.files {
        for staged in [true, false] {
            if if staged { !file.staged } else { !file.unstaged } {
                continue;
            }
            let diff = checkout.file(&file.relative, staged)?;
            let bytes = serde_json::to_vec(&diff).map_err(|error| error.to_string())?;
            versions.insert(
                format!(
                    "{}:{}",
                    if staged { "staged" } else { "unstaged" },
                    file.relative
                ),
                FileDiffVersion {
                    index: format!("{:x}", Sha256::digest(bytes)),
                    head: snapshot.head.clone(),
                    disk: None,
                },
            );
        }
    }
    Ok(versions)
}

pub fn file_diff(cwd: &Path, relative: &str, staged: bool) -> Result<FileDiff, String> {
    WorktreeReview::open(cwd)?.file(relative, staged)
}

/// Read only the Git side an open text editor compares its buffer against.
pub fn editor_baseline(
    cwd: &Path,
    relative: &str,
    remote_staged: Option<bool>,
) -> Result<Option<(String, bool)>, String> {
    let checkout = WorktreeReview::open(cwd)?;
    let root = &checkout.cwd;
    let Some(prefix) = &checkout.prefix else {
        let staged = match remote_staged {
            Some(staged) => staged,
            None => diff_files(cwd)?
                .files
                .iter()
                .find(|file| file.relative == relative)
                .is_some_and(|file| file.staged && !file.unstaged),
        };
        let diff = checkout.file(relative, staged)?;
        return Ok((!diff.binary && !diff.too_large).then_some((diff.original, staged)));
    };
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
}

impl CommitReview {
    pub fn open(cwd: &Path, sha: &str) -> Result<Self, String> {
        if remote_workspace::is_remote(cwd) {
            return Ok(Self {
                cwd: cwd.to_path_buf(),
                sha: sha.to_owned(),
                parent: None,
            });
        }
        let cwd = expand_home(cwd);
        let sha = peel_commit(&cwd, sha)?;
        let parent = git_stdout(&cwd, &["rev-parse", "--verify", &format!("{sha}^")]);
        Ok(Self { cwd, sha, parent })
    }

    /// The files a commit changed, against its first parent (or nothing).
    pub fn files(&self) -> Result<Vec<ChangedFile>, String> {
        let Self { cwd: root, sha, .. } = self;
        if remote_workspace::is_remote(root) {
            return remote_workspace::command("git_commit_files", json!({"cwd":root,"sha":sha}));
        }
        let mut files: HashMap<String, FileAcc> = HashMap::new();
        let mut statuses: HashMap<String, &'static str> = HashMap::new();
        let tree = |kind: &str| {
            git_run(
                root,
                &[
                    "diff-tree",
                    "-z",
                    "--no-commit-id",
                    "-r",
                    "--root",
                    "--no-renames",
                    kind,
                    sha,
                ],
            )
        };
        if let Some(text) = tree("--numstat") {
            add_numstat_map(&text, &mut files);
        }
        if let Some(names) = tree("--name-status") {
            add_name_status(&names, &mut statuses);
        }
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
        } = self;
        if remote_workspace::is_remote(root) {
            return remote_workspace::command(
                "git_commit_file_diff",
                json!({"cwd":root,"sha":sha,"relative":relative}),
            );
        }
        let relative = resolve_repo_path(root, relative)?;
        let original = parent
            .as_ref()
            .and_then(|parent| git_blob(root, &format!("{parent}:{relative}")));
        let current = git_blob(root, &format!("{sha}:{relative}"));
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
    if remote_workspace::is_remote(cwd) {
        let contents = std::str::from_utf8(contents).map_err(|_| "File is not valid UTF-8.")?;
        return remote_workspace::command(
            "git_stage_contents",
            json!({"cwd":cwd,"relative":relative,"contents":contents}),
        );
    }
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
    if spec.len() < 4 || spec.len() > 40 || !spec.bytes().all(|b| b.is_ascii_hexdigit()) {
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
