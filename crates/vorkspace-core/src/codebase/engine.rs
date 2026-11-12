//! A pinned, optional component. Never run upstream's installer or modify a
//! provider's configuration; its executable and notices stay in app storage.
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

pub(crate) const VERSION: &str = "0.12.1";
const BINARY: &str = "codebase-memory-mcp";

fn asset() -> Result<(&'static str, u64, &'static str), String> {
    match std::env::consts::ARCH {
        "aarch64" => Ok((
            "arm64",
            49_875_423,
            "9bfcbe38414b47969a3e1d1c1999f5357409387c86fa3d24221b5458267660d2",
        )),
        "x86_64" => Ok((
            "amd64",
            50_555_159,
            "ca6e64397a8ae80cec898a8205a6aa16750ac798acc05a0c3ed983837944a581",
        )),
        _ => Err("Codebase Memory is unavailable on this architecture".into()),
    }
}

pub(super) fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err("Codebase operation cancelled".into())
    } else {
        Ok(())
    }
}

pub(super) fn private_dir(path: &Path) -> Result<(), String> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(|e| e.to_string())?;
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
        return Err("Codebase storage must be a directory owned by this user".into());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())
}

fn installed(directory: &Path, checksum: &str) -> bool {
    fs::symlink_metadata(directory)
        .is_ok_and(|m| m.is_dir() && m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o077 == 0)
        && fs::symlink_metadata(directory.join("complete")).is_ok_and(|m| {
            m.is_file() && m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o022 == 0
        })
        && fs::read_to_string(directory.join("complete")).is_ok_and(|value| value == checksum)
        && [BINARY, "LICENSE", "THIRD_PARTY_NOTICES.md"]
            .iter()
            .all(|name| {
                fs::symlink_metadata(directory.join(name)).is_ok_and(|metadata| {
                    metadata.is_file()
                        && metadata.uid() == unsafe { libc::geteuid() }
                        && metadata.len() > 0
                        && metadata.mode() & 0o022 == 0
                        && (*name != BINARY || metadata.mode() & 0o100 != 0)
                })
            })
}

pub(super) fn ensure(root: &Path, cancel: &AtomicBool) -> Result<PathBuf, String> {
    check_cancel(cancel)?;
    let (arch, size, checksum) = asset()?;
    let directory = root.join(format!("engine-{VERSION}-{arch}"));
    private_dir(root)?;
    if installed(&directory, checksum) {
        return Ok(directory.join(BINARY));
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(root.join("install.lock"))
        .map_err(|e| e.to_string())?;
    loop {
        check_cancel(cancel)?;
        match lock.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => return Err(error.to_string()),
        }
    }
    if installed(&directory, checksum) {
        return Ok(directory.join(BINARY));
    }
    let stage = tempfile::Builder::new()
        .prefix("component-")
        .tempdir_in(root)
        .map_err(|e| e.to_string())?;
    let archive_path = stage.path().join("download.tar.gz");
    let mut archive = OpenOptions::new()
        .write(true)
        .read(true)
        .create_new(true)
        .mode(0o600)
        .open(&archive_path)
        .map_err(|e| e.to_string())?;
    let response = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(5)).timeout(Duration::from_secs(180)).build()
        .get(&format!("https://github.com/DeusData/codebase-memory-mcp/releases/download/v{VERSION}/{BINARY}-darwin-{arch}.tar.gz"))
        .call().map_err(|e| format!("Could not download Codebase Memory: {e}"))?;
    let mut reader = response.into_reader();
    let mut hash = Sha256::new();
    let mut received = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        check_cancel(cancel)?;
        let count = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        received += count as u64;
        if received > size {
            return Err("Codebase download exceeded its expected size".into());
        }
        hash.update(&buffer[..count]);
        archive
            .write_all(&buffer[..count])
            .map_err(|e| e.to_string())?;
    }
    if received != size || format!("{:x}", hash.finalize()) != checksum {
        return Err("Codebase download did not match its pinned checksum".into());
    }
    archive.sync_all().map_err(|e| e.to_string())?;
    let unpacked = stage.path().join("component");
    private_dir(&unpacked)?;
    extract(
        File::open(&archive_path).map_err(|e| e.to_string())?,
        &unpacked,
        cancel,
    )?;
    check_cancel(cancel)?;
    let mut marker = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(unpacked.join("complete"))
        .map_err(|e| e.to_string())?;
    marker
        .write_all(checksum.as_bytes())
        .map_err(|e| e.to_string())?;
    marker.sync_all().map_err(|e| e.to_string())?;
    File::open(&unpacked)
        .and_then(|file| file.sync_all())
        .map_err(|e| e.to_string())?;
    // Only component-owned files live here. Preserve an incomplete prior
    // directory until its fully verified successor is ready to publish.
    if directory.exists() {
        fs::rename(&directory, stage.path().join("previous")).map_err(|e| e.to_string())?;
    }
    fs::rename(&unpacked, &directory).map_err(|e| e.to_string())?;
    File::open(root)
        .and_then(|file| file.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(directory.join(BINARY))
}

fn extract(input: impl Read, destination: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(input));
    let mut seen = std::collections::HashSet::new();
    for entry in archive.entries().map_err(|e| e.to_string())? {
        check_cancel(cancel)?;
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?;
        let Some(name) = path.to_str() else {
            return Err("Invalid component path".into());
        };
        let limit = match name {
            BINARY => 400 * 1024 * 1024,
            "LICENSE" | "THIRD_PARTY_NOTICES.md" => 2 * 1024 * 1024,
            "install.sh" => continue,
            _ => return Err("Unexpected file in Codebase component".into()),
        };
        if !entry.header().entry_type().is_file()
            || entry.size() > limit
            || !seen.insert(name.to_owned())
        {
            return Err("Invalid Codebase component entry".into());
        }
        let name = name.to_owned();
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(destination.join(&name))
            .map_err(|e| e.to_string())?;
        let mut buffer = [0u8; 65536];
        loop {
            check_cancel(cancel)?;
            let count = entry.read(&mut buffer).map_err(|e| e.to_string())?;
            if count == 0 {
                break;
            }
            file.write_all(&buffer[..count])
                .map_err(|e| e.to_string())?;
        }
        file.set_permissions(fs::Permissions::from_mode(if name == BINARY {
            0o500
        } else {
            0o400
        }))
        .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
    }
    if seen.len() != 3 {
        return Err("Codebase component is incomplete".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "Downloads the pinned component from GitHub into a disposable directory"]
    fn pinned_component_installs_atomically_and_reuses_verified_files() {
        let root = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let binary = ensure(root.path(), &cancel).unwrap();
        let (_, _, checksum) = asset().unwrap();
        assert!(installed(binary.parent().unwrap(), checksum));
        let modified = fs::metadata(&binary).unwrap().modified().unwrap();
        assert_eq!(ensure(root.path(), &cancel).unwrap(), binary);
        assert_eq!(fs::metadata(&binary).unwrap().modified().unwrap(), modified);
        assert!(!binary.parent().unwrap().join("install.sh").exists());
        assert!(fs::read_dir(root.path()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("component-")));
    }
    fn archive(link: bool) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for name in [BINARY, "LICENSE", "THIRD_PARTY_NOTICES.md", "install.sh"] {
            let mut header = tar::Header::new_gnu();
            header.set_size(4);
            header.set_mode(0o700);
            if link && name == BINARY {
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_link_name("/tmp/outside").unwrap();
            }
            header.set_cksum();
            builder
                .append_data(&mut header, name, &b"test"[..])
                .unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }
    #[test]
    fn extracts_only_the_pinned_component_files_and_never_installer() {
        let root = tempfile::tempdir().unwrap();
        extract(&archive(false)[..], root.path(), &AtomicBool::new(false)).unwrap();
        assert!(root.path().join(BINARY).is_file());
        assert!(!root.path().join("install.sh").exists());
        assert_eq!(
            fs::metadata(root.path().join(BINARY)).unwrap().mode() & 0o777,
            0o500
        );
    }
    #[test]
    fn links_incomplete_components_and_cancelled_extraction_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        assert!(extract(&archive(true)[..], root.path(), &AtomicBool::new(false)).is_err());
        assert!(!installed(root.path(), "checksum"));
        assert!(extract(&archive(false)[..], root.path(), &AtomicBool::new(true)).is_err());
        assert!(fs::read_dir(root.path()).unwrap().next().is_none());
    }
}
