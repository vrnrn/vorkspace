//! Admission for model workers in cooperating helpers of the same macOS user.
//! The persistent lock file must never be removed: replacing a locked inode
//! would let a second helper acquire a different lock for the same name.
use std::{
    fs::{File, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::Path,
    thread::{self, JoinHandle},
};

#[derive(Debug)]
pub enum AcquireError {
    Busy,
    Unavailable(String),
}

pub struct EngineLease {
    _file: File,
}

impl EngineLease {
    /// Resolve the account home, not HOME or TMPDIR, so cooperating helpers
    /// launched by different bundles and environments use the same lock.
    pub fn acquire() -> Result<Self, AcquireError> {
        let home = crate::dictation_path::system_home()
            .filter(|path| path.is_absolute())
            .ok_or_else(|| {
                AcquireError::Unavailable("Could not locate your account home folder".into())
            })?;
        Self::acquire_at(&home.join(".vorkspace-dictation-engine.lock"))
    }

    /// An explicit path is used by isolated, no-microphone process fixtures.
    pub fn acquire_at(path: &Path) -> Result<Self, AcquireError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|error| AcquireError::Unavailable(error.to_string()))?;
        let metadata = file
            .metadata()
            .map_err(|error| AcquireError::Unavailable(error.to_string()))?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::getuid() }
            || metadata.mode() & 0o077 != 0
            || metadata.nlink() != 1
        {
            return Err(AcquireError::Unavailable(
                "The model lock must be an owner-only regular file".into(),
            ));
        }
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                return Err(AcquireError::Busy);
            }
            return Err(AcquireError::Unavailable(error.to_string()));
        }
        Ok(Self { _file: file })
    }

    /// Even a cancelled or failed worker retains admission through the join.
    pub fn join<T>(self, worker: JoinHandle<T>) -> thread::Result<T> {
        let result = worker.join();
        drop(self);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lock_is_exclusive_and_released_after_join_even_on_panic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("engine.lock");
        let lease = EngineLease::acquire_at(&path).unwrap();
        assert!(matches!(
            EngineLease::acquire_at(&path),
            Err(AcquireError::Busy)
        ));
        let worker = thread::spawn(|| panic!("worker failed"));
        assert!(lease.join(worker).is_err());
        assert!(EngineLease::acquire_at(&path).is_ok());
    }
    #[test]
    fn symlinks_and_nonprivate_existing_files_fail_closed() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::write(&target, "").unwrap();
        let link = dir.path().join("link");
        symlink(&target, &link).unwrap();
        assert!(matches!(
            EngineLease::acquire_at(&link),
            Err(AcquireError::Unavailable(_))
        ));
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            EngineLease::acquire_at(&target),
            Err(AcquireError::Unavailable(_))
        ));
    }
}
