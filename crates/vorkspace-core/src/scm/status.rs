//! A single porcelain observation owns paths, index sides and dirty versions.
use super::{
    add_name_status, add_numstat_map, git_output, git_run, git_stdout, sync_for, text_line_count,
    ChangedFile, DiffIndex, FileAcc, Sync,
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, Metadata},
    io::ErrorKind,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Component, Path},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckoutStamp([u8; 32]);

/// A diff side can change without changing its Git status or line counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiffVersion {
    pub(super) index: String,
    pub(super) head: Option<String>,
    pub(super) disk: Option<DiskVersion>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DiskVersion {
    len: u64,
    identity: (u64, u64),
    modified: (i64, i64),
    changed: (i64, i64),
}

impl DiskVersion {
    fn new(meta: &Metadata) -> Self {
        Self {
            len: meta.len(),
            identity: (meta.dev(), meta.ino()),
            modified: (meta.mtime(), meta.mtime_nsec()),
            changed: (meta.ctime(), meta.ctime_nsec()),
        }
    }

    fn hash(&self, hash: &mut Sha256) {
        hash.update([1]);
        for value in [self.len, self.identity.0, self.identity.1] {
            hash.update(value.to_le_bytes());
        }
        for value in [
            self.modified.0,
            self.modified.1,
            self.changed.0,
            self.changed.1,
        ] {
            hash.update(value.to_le_bytes());
        }
    }
}

#[derive(Default)]
struct FileStatus {
    index: String,
    disk: Option<DiskVersion>,
    staged: bool,
    unstaged: bool,
    untracked: bool,
    deleted: bool,
    added: bool,
}

#[derive(Default)]
pub(super) struct LocalStatus {
    pub(super) stamp: Option<CheckoutStamp>,
    head: Option<String>,
    branch: Option<String>,
    upstream: Option<String>,
    ahead_behind: Option<(i64, i64)>,
    files: HashMap<String, FileStatus>,
}

impl LocalStatus {
    pub(super) fn output(root: &Path) -> Option<Vec<u8>> {
        git_output(
            root,
            &[
                "status",
                "--porcelain=v2",
                "-z",
                "--branch",
                "--ahead-behind",
                "--no-renames",
                "--untracked-files=all",
                "--",
                ".",
            ],
        )
    }

    pub(super) fn parse(
        root: &Path,
        prefix: Option<&str>,
        raw: &[u8],
        known: Option<CheckoutStamp>,
    ) -> Result<Option<Self>, String> {
        let has_files = raw
            .split(|&byte| byte == 0)
            .any(|record| !record.is_empty() && !record.starts_with(b"# "));
        let prefix_buffer;
        let prefix = match prefix {
            Some(prefix) => prefix,
            None if has_files => {
                prefix_buffer = git_output(root, &["rev-parse", "--show-prefix"])
                    .ok_or("Could not resolve the Git checkout prefix")?;
                std::str::from_utf8(
                    prefix_buffer
                        .strip_suffix(b"\n")
                        .ok_or("Invalid Git checkout prefix")?,
                )
                .map_err(|error| error.to_string())?
            }
            None => "",
        };
        let mut status = Self::default();
        let mut hash = Sha256::new();
        hash.update(root.as_os_str().as_bytes());
        hash.update(raw);
        let mut cacheable = true;
        let mut files = Vec::new();
        for record in raw
            .split(|&byte| byte == 0)
            .filter(|record| !record.is_empty())
        {
            let Ok(record) = std::str::from_utf8(record) else {
                continue;
            };
            if let Some(header) = record.strip_prefix("# ") {
                status.read_header(header);
                continue;
            }
            let record = parse_record(record).ok_or("Unexpected Git status record")?;
            let Some(relative) = record
                .path
                .strip_prefix(prefix)
                .filter(|path| !path.is_empty())
            else {
                continue;
            };
            if !Path::new(relative)
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
            {
                continue;
            }
            cacheable &= !record.submodule;
            let disk = if record.y != b'.' {
                match fs::symlink_metadata(root.join(relative)) {
                    Ok(meta) if meta.is_file() || meta.is_symlink() => {
                        let disk = DiskVersion::new(&meta);
                        disk.hash(&mut hash);
                        Some(disk)
                    }
                    Err(error) if error.kind() == ErrorKind::NotFound => {
                        hash.update([0]);
                        None
                    }
                    _ => {
                        cacheable = false;
                        None
                    }
                }
            } else {
                None
            };
            files.push((relative, record, disk));
        }
        status.stamp = cacheable.then(|| CheckoutStamp(hash.finalize().into()));
        if known.is_some() && status.stamp == known {
            return Ok(None);
        }
        status.files.reserve(files.len());
        for (relative, record, disk) in files {
            let file = status.files.entry(relative.to_owned()).or_default();
            // A staged deletion and an untracked replacement can share a path.
            if matches!(record.index, IndexEntry::Untracked) {
                file.untracked = true;
            } else {
                file.index = record.index.version();
                file.staged = record.x != b'.';
                file.deleted = record.x == b'D' || record.y == b'D';
                file.added = record.x == b'A' || record.y == b'A';
            }
            if record.y != b'.' {
                file.unstaged = true;
                file.disk = disk;
            }
        }
        Ok(Some(status))
    }

    fn read_header(&mut self, header: &str) {
        let Some((key, value)) = header.split_once(' ') else {
            return;
        };
        match key {
            "branch.oid" if value != "(initial)" => self.head = Some(value.to_owned()),
            "branch.head" if value != "(detached)" && value != "HEAD" => {
                self.branch = Some(value.to_owned())
            }
            "branch.upstream" => self.upstream = Some(value.to_owned()),
            "branch.ab" => {
                self.ahead_behind = value.split_once(' ').and_then(|(ahead, behind)| {
                    Some((
                        ahead.strip_prefix('+')?.parse().ok()?,
                        behind.strip_prefix('-')?.parse().ok()?,
                    ))
                });
            }
            _ => {}
        }
    }

    pub(super) fn versions(&self) -> HashMap<String, FileDiffVersion> {
        let mut versions = HashMap::new();
        for (relative, file) in &self.files {
            if file.staged {
                versions.insert(
                    format!("staged:{relative}"),
                    FileDiffVersion {
                        index: file.index.clone(),
                        head: self.head.clone(),
                        disk: None,
                    },
                );
            }
            if file.unstaged {
                versions.insert(
                    format!("unstaged:{relative}"),
                    FileDiffVersion {
                        index: file.index.clone(),
                        head: None,
                        disk: file.disk.clone(),
                    },
                );
            }
        }
        versions
    }

    pub(super) fn index(&self, root: &Path, counts: Counts<'_>) -> DiffIndex {
        let full = matches!(counts, Counts::Full { .. });
        let sync = if matches!(counts, Counts::Full { sync: true }) {
            sync_for(
                root,
                self.upstream.clone(),
                self.ahead_behind,
                self.head.is_some(),
            )
        } else {
            Sync::default()
        };
        let (net, statuses) = if full
            && self
                .files
                .values()
                .any(|file| !file.untracked || file.staged)
        {
            read_counts(root, self.head.is_some())
        } else {
            (HashMap::new(), HashMap::new())
        };
        let previous: HashMap<_, _> = match counts {
            Counts::Previous(previous) => previous
                .into_iter()
                .flat_map(|index| &index.files)
                .map(|file| (file.relative.as_str(), (file.additions, file.deletions)))
                .collect(),
            Counts::Full { .. } => HashMap::new(),
        };
        let untracked_root = (full && self.files.values().any(|file| file.untracked))
            .then(|| root.canonicalize().ok())
            .flatten();
        let mut files = Vec::with_capacity(self.files.len());
        for (relative, file) in &self.files {
            let (mut additions, deletions) = if full {
                net.get(relative)
                    .map_or((0, 0), |counts| (counts.additions, counts.deletions))
            } else {
                previous.get(relative.as_str()).copied().unwrap_or_default()
            };
            if file.untracked && additions == 0 {
                if let Some(root) = &untracked_root {
                    additions = text_line_count(root, relative);
                }
            }
            let absolute = root.join(relative);
            let status = if file.untracked {
                "untracked"
            } else if let Some(status) = statuses.get(relative) {
                status
            } else if file.deleted && (!full || !absolute.exists()) {
                "deleted"
            } else if !full && file.added {
                "added"
            } else {
                "modified"
            };
            files.push(ChangedFile {
                path: crate::shell_env::path_to_js(&absolute),
                relative: relative.clone(),
                status: status.to_owned(),
                additions,
                deletions,
                staged: file.staged,
                unstaged: file.unstaged,
            });
        }
        files.sort_unstable_by(|a, b| a.relative.cmp(&b.relative));
        let (additions, deletions) = files.iter().fold((0, 0), |(a, d), file| {
            (a + file.additions, d + file.deletions)
        });
        DiffIndex {
            branch: self.branch.clone().or_else(|| {
                self.head
                    .as_ref()
                    .and_then(|_| git_stdout(root, &["rev-parse", "--short", "HEAD"]))
            }),
            head: self.head.clone(),
            files,
            additions,
            deletions,
            remote: sync.remote,
            upstream: sync.upstream,
            default_branch: sync.default_branch,
            ahead: sync.ahead,
            behind: sync.behind,
            ahead_of_default: sync.ahead_of_default,
            head_pushed: sync.head_pushed,
        }
    }
}

pub(super) enum Counts<'a> {
    Full { sync: bool },
    Previous(Option<&'a DiffIndex>),
}

enum IndexEntry<'a> {
    Untracked,
    Ordinary {
        mode: &'a str,
        oid: &'a str,
    },
    Conflict {
        modes: [&'a str; 3],
        oids: [&'a str; 3],
    },
}

impl IndexEntry<'_> {
    fn version(&self) -> String {
        match self {
            Self::Untracked => String::new(),
            Self::Ordinary { mode, oid } => format!("{mode}:{oid}"),
            Self::Conflict {
                modes: [a, b, c],
                oids: [x, y, z],
            } => format!("{a}:{x} {b}:{y} {c}:{z}"),
        }
    }
}

struct Record<'a> {
    path: &'a str,
    index: IndexEntry<'a>,
    x: u8,
    y: u8,
    submodule: bool,
}

fn parse_record(record: &str) -> Option<Record<'_>> {
    let fields = match record.as_bytes().first()? {
        b'?' => {
            return Some(Record {
                path: record.strip_prefix("? ")?,
                index: IndexEntry::Untracked,
                x: b'.',
                y: b'?',
                submodule: false,
            })
        }
        b'1' => 9,
        b'u' => 11,
        _ => return None,
    };
    let mut parts = record.splitn(fields, ' ');
    let kind = parts.next()?;
    let [x, y] = *parts.next()?.as_bytes() else {
        return None;
    };
    let submodule = parts.next()? != "N...";
    let (index, path) = if kind == "1" {
        let [_head_mode, mode, _worktree_mode, _head_oid, oid, path] =
            std::array::from_fn(|_| parts.next().unwrap_or(""));
        (IndexEntry::Ordinary { mode, oid }, path)
    } else {
        let [mode1, mode2, mode3, _worktree_mode, oid1, oid2, oid3, path] =
            std::array::from_fn(|_| parts.next().unwrap_or(""));
        (
            IndexEntry::Conflict {
                modes: [mode1, mode2, mode3],
                oids: [oid1, oid2, oid3],
            },
            path,
        )
    };
    if path.is_empty() {
        return None;
    }
    Some(Record {
        path,
        index,
        x,
        y,
        submodule,
    })
}

fn read_counts(
    root: &Path,
    has_head: bool,
) -> (HashMap<String, FileAcc>, HashMap<String, &'static str>) {
    let mut counts = HashMap::new();
    let mut statuses = HashMap::new();
    let sides: &[&[&str]] = if has_head {
        &[&["HEAD"]]
    } else {
        &[&[], &["--cached"]]
    };
    for side in sides {
        let base = ["diff", "-z", "--relative", "--no-ext-diff"];
        let mut args = base.to_vec();
        args.push("--numstat");
        args.extend_from_slice(side);
        args.extend(["--", "."]);
        if let Some(text) = git_run(root, &args) {
            add_numstat_map(&text, &mut counts);
        }
        args.truncate(base.len());
        args.extend(["--name-status", "--no-renames"]);
        args.extend_from_slice(side);
        args.extend(["--", "."]);
        if let Some(text) = git_run(root, &args) {
            add_name_status(&text, &mut statuses);
        }
    }
    (counts, statuses)
}
