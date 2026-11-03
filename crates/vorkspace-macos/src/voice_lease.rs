//! Shared microphone and notch admission for Vox and local dictation.
use std::{
    fs::{File, OpenOptions},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
};
pub struct Lease(File);
impl Lease {
    pub fn acquire() -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!("vorkspace-voice-{}.lock", unsafe {
            libc::getuid()
        }));
        Self::at(&path)
    }
    fn at(path: &std::path::Path) -> Result<Self, String> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map_err(|e| e.to_string())?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("Another voice feature is using the microphone. Stop it first.".into());
        }
        Ok(Self(file))
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_the_owner_can_hold_the_lease_and_drop_releases_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("voice.lock");
        let first = Lease::at(&path).unwrap();
        assert!(Lease::at(&path).is_err());
        drop(first);
        assert!(Lease::at(&path).is_ok());
    }
}
