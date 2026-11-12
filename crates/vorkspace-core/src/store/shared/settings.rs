//! The settings sidecar is one atomic file shared by independent stores.
use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{BufWriter, ErrorKind, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

#[derive(PartialEq, Eq)]
struct Stamp {
    device: u64,
    inode: u64,
    bytes: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl Stamp {
    fn read(file: &File) -> Result<Self, String> {
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            bytes: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }
}

#[derive(Default)]
struct Snapshot {
    stamp: Option<Stamp>,
    values: BTreeMap<String, String>,
}

pub(super) struct Sidecar {
    path: Option<PathBuf>,
    snapshot: RefCell<Snapshot>,
}

impl Sidecar {
    pub(super) fn open(path: Option<PathBuf>) -> Result<Self, String> {
        let settings = Self {
            path,
            snapshot: RefCell::default(),
        };
        settings.refresh()?;
        Ok(settings)
    }

    #[cfg(test)]
    pub(super) fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    fn refresh(&self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        for _ in 0..3 {
            let mut file = match OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(path)
            {
                Ok(file) => file,
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    let snapshot = self.snapshot.borrow();
                    if snapshot.stamp.is_none() && snapshot.values.is_empty() {
                        return Ok(());
                    }
                    drop(snapshot);
                    self.replace(None, BTreeMap::new());
                    return Ok(());
                }
                Err(error) => return Err(error.to_string()),
            };
            let stamp = Stamp::read(&file)?;
            if self.snapshot.borrow().stamp.as_ref() == Some(&stamp) {
                return Ok(());
            }
            let mut raw = String::new();
            file.read_to_string(&mut raw)
                .map_err(|error| error.to_string())?;
            if stamp != Stamp::read(&file)? {
                continue;
            }
            let mut values: BTreeMap<String, String> =
                serde_json::from_str(&raw).map_err(|error| error.to_string())?;
            // A malformed workspace or roster stops the read, so nothing saves over it.
            for key in ["workspace", "bot-roster"] {
                if let Some(value) = values.get_mut(key) {
                    let json: serde_json::Value = serde_json::from_str(value)
                        .map_err(|e| format!("Cannot read {key}: {e}"))?;
                    *value = json.to_string();
                }
            }
            self.replace(Some(stamp), values);
            return Ok(());
        }
        Err("Settings changed while being read. Try again.".into())
    }

    fn replace(&self, stamp: Option<Stamp>, values: BTreeMap<String, String>) {
        let mut snapshot = self.snapshot.borrow_mut();
        *snapshot = Snapshot { stamp, values };
    }

    pub(super) fn read<T>(
        &self,
        read: impl FnOnce(&BTreeMap<String, String>) -> T,
    ) -> Result<T, String> {
        self.refresh()?;
        Ok(read(&self.snapshot.borrow().values))
    }

    /// Retain this stable sibling lock through the read, edit and publication.
    pub(super) fn lock(&self) -> Result<Option<File>, String> {
        let Some(path) = &self.path else {
            return Ok(None);
        };
        let lock = path.with_file_name(format!(
            "{}.lock",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(lock)
            .map_err(|error| error.to_string())?;
        file.lock().map_err(|error| error.to_string())?;
        Ok(Some(file))
    }

    pub(super) fn save(&self, values: BTreeMap<String, String>) -> Result<(), String> {
        if let Some(path) = &self.path {
            write_atomic(path, &values)?;
        }
        // A second writer can replace the pathname immediately. A subsequent
        // disk read must check its own descriptor before using these values.
        self.replace(None, values);
        Ok(())
    }
}

fn write_atomic(path: &Path, values: &BTreeMap<String, String>) -> Result<(), String> {
    crate::private_file::replace(path, |file| {
        let mut writer = BufWriter::new(file);
        serde_json::to_writer(&mut writer, values).map_err(std::io::Error::other)?;
        writer.flush()
    })
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use std::fs;

    #[test]
    fn independent_stores_observe_replacement_removal_and_restoration() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.db");
        let first = Store::open_vorkspace(&path).unwrap();
        let peer = Store::open_vorkspace(&path).unwrap();
        first.save_setting("fixture", "one").unwrap();
        assert_eq!(first.setting("fixture").unwrap().as_deref(), Some("one"));
        peer.save_setting("fixture", "two").unwrap();
        assert_eq!(first.setting("fixture").unwrap().as_deref(), Some("two"));

        let sidecar = path.with_file_name("state.db.gpui.json");
        fs::remove_file(&sidecar).unwrap();
        assert_eq!(first.setting("fixture").unwrap(), None);
        peer.save_setting("fixture", "new").unwrap();
        assert_eq!(first.setting("fixture").unwrap().as_deref(), Some("new"));
    }

    #[test]
    fn in_place_changes_invalidate_the_snapshot_and_malformed_data_stays_visible_as_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(&path, r#"{"fixture":"one"}"#).unwrap();
        let settings = Sidecar::open(Some(path.clone())).unwrap();
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        file.write_all(br#"{"fixture":"two"}"#).unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))
            .unwrap();
        assert_eq!(settings.read(|s| s["fixture"].clone()).unwrap(), "two");
        fs::write(&path, "malformed").unwrap();
        assert!(settings.read(|s| s.get("fixture").cloned()).is_err());
        fs::write(&path, r#"{"fixture":"new"}"#).unwrap();
        assert_eq!(settings.read(|s| s["fixture"].clone()).unwrap(), "new");
    }
}
