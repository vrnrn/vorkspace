use super::{expand_home, git_output, remote_workspace};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fs::{self, OpenOptions},
    io::{self, ErrorKind, Read},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Component, Path, PathBuf},
};

/// A checkout can use separate worktree and common Git directories. Include
/// all three inputs so a branch change or index replacement can wake readers.
pub fn checkout_watch_paths(cwd: &Path) -> Option<Vec<PathBuf>> {
    if remote_workspace::is_remote(cwd) {
        return None;
    }
    let cwd = expand_home(cwd);
    let resolve = |args: &[&str]| {
        let output = git_output(&cwd, args)?;
        let path = Path::new(OsStr::from_bytes(output.strip_suffix(b"\n")?));
        path.canonicalize().ok()
    };
    let mut roots = [
        resolve(&["rev-parse", "--show-toplevel"])?,
        resolve(&["rev-parse", "--absolute-git-dir"])?,
        resolve(&["rev-parse", "--path-format=absolute", "--git-common-dir"])?,
    ];
    roots.sort_unstable();
    let mut paths = Vec::new();
    for root in roots {
        if !paths.iter().any(|path| root.starts_with(path)) {
            paths.push(root);
        }
    }
    Some(paths)
}

/// Inputs that can change a local checkout's diff between full reconciliations.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CheckoutStamp([u8; 32]);

impl CheckoutStamp {
    pub fn read(cwd: &Path) -> Option<Self> {
        if remote_workspace::is_remote(cwd) {
            return None;
        }
        let cwd = expand_home(cwd);
        let status = git_output(
            &cwd,
            &[
                "status",
                "--porcelain=v2",
                "-z",
                "--branch",
                "--no-renames",
                "--untracked-files=all",
                "--",
                ".",
            ],
        )?;
        let mut hash = Sha256::new();
        hash.update(&status);
        let mut records = status.split(|&byte| byte == 0).filter(|r| !r.is_empty());
        let paths = records.by_ref().find(|record| !record.starts_with(b"# "));
        if let Some(first) = paths {
            // Porcelain paths are repository-relative even for a nested project.
            let root = git_output(&cwd, &["rev-parse", "--show-toplevel"])?;
            let root = Path::new(OsStr::from_bytes(root.strip_suffix(b"\n")?));
            hash.update(root.as_os_str().as_bytes());
            for record in std::iter::once(first).chain(records) {
                let path = dirty_path(record)?;
                if !path
                    .components()
                    .all(|part| matches!(part, Component::Normal(_)))
                {
                    return None;
                }
                match fs::symlink_metadata(root.join(path)) {
                    Ok(meta) if meta.is_file() || meta.is_symlink() => {
                        hash.update([1]);
                        for value in [meta.dev(), meta.ino(), meta.len()] {
                            hash.update(value.to_le_bytes());
                        }
                        for value in [
                            meta.mtime(),
                            meta.mtime_nsec(),
                            meta.ctime(),
                            meta.ctime_nsec(),
                        ] {
                            hash.update(value.to_le_bytes());
                        }
                    }
                    Err(error) if error.kind() == ErrorKind::NotFound => hash.update([0]),
                    // Nested repositories and failed reads need the full path.
                    _ => return None,
                }
            }
        }
        Some(Self(hash.finalize().into()))
    }
}

fn dirty_path(record: &[u8]) -> Option<&Path> {
    let fields = match record.first()? {
        b'?' => 2,
        b'1' => 9,
        b'u' => 11,
        _ => return None,
    };
    let mut parts = record.splitn(fields, |&byte| byte == b' ');
    if fields > 2 && parts.nth(2)? != b"N..." {
        return None;
    }
    let bytes = parts.last().filter(|path| !path.is_empty())?;
    Some(Path::new(OsStr::from_bytes(bytes)))
}

pub(super) enum FileContents {
    Bytes(Vec<u8>),
    TooLarge,
}

pub(super) fn read_worktree_file(
    root: &Path,
    relative: &str,
    limit: u64,
) -> io::Result<Option<FileContents>> {
    let root = root.canonicalize()?;
    let path = root.join(relative);
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Ok(None);
    };
    let parent = match parent.canonicalize() {
        Ok(parent) if parent.starts_with(&root) => parent,
        Ok(_) => return Err(io::Error::other("File is outside the checkout")),
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let path = parent.join(name);
    let meta = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if meta.is_symlink() {
        let target = fs::read_link(&path)?;
        return Ok(Some(FileContents::Bytes(
            target.as_os_str().as_bytes().to_vec(),
        )));
    }
    if !meta.is_file() {
        return Ok(None);
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Ok(None);
    }
    if meta.len() > limit {
        return Ok(Some(FileContents::TooLarge));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    Ok(Some(if bytes.len() as u64 > limit {
        FileContents::TooLarge
    } else {
        FileContents::Bytes(bytes)
    }))
}
