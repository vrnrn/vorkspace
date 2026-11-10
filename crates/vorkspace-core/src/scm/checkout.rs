use super::{expand_home, git_output, remote_workspace};
use std::{
    ffi::OsStr,
    fs::{self, OpenOptions},
    io::{self, ErrorKind, Read},
    os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    path::{Path, PathBuf},
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

pub(super) enum FileContents {
    Bytes(Vec<u8>),
    TooLarge,
}

pub(super) fn read_worktree_file(
    root: &Path,
    relative: &str,
    limit: u64,
) -> io::Result<Option<FileContents>> {
    read_worktree_file_at(&root.canonicalize()?, relative, limit)
}

pub(super) fn read_worktree_file_at(
    root: &Path,
    relative: &str,
    limit: u64,
) -> io::Result<Option<FileContents>> {
    let path = root.join(relative);
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Ok(None);
    };
    let parent = match parent.canonicalize() {
        Ok(parent) if parent.starts_with(root) => parent,
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
