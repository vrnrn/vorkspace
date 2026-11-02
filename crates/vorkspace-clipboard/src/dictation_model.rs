//! Pinned artifacts share verification, download and inference. A verified temporary file is synced and renamed last.
pub use crate::ModelId;
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};
use transcribe_cpp::{Backend, CancelToken, Model, ModelOptions, RunOptions};
/// Mutation authority for this shared artifact. Readers never need this lock.
pub struct ModelOwner {
    _lock: File,
    path: std::path::PathBuf,
}
impl ModelOwner {
    pub fn acquire(path: &Path) -> Result<Self, String> {
        use std::os::fd::AsRawFd;
        let dir = path.parent().ok_or("Missing model directory")?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path.with_extension("lock"))
            .map_err(|e| e.to_string())?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("Another Vorkspace install is downloading this model. Wait for it to finish, then retry.".into());
        }
        Ok(Self {
            _lock: lock,
            path: path.to_owned(),
        })
    }
    pub fn reap(&self) {
        let path = &self.path;
        let Some(dir) = path.parent() else { return };
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(suffix) = name
                .strip_prefix(&format!("{stem}."))
                .and_then(|s| s.strip_suffix(".download"))
            else {
                continue;
            };
            if !suffix.is_empty()
                && suffix.chars().all(|c| c.is_ascii_digit() || c == '-')
                && entry.file_type().is_ok_and(|t| t.is_file())
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}
pub fn verified(id: ModelId, path: &Path) -> bool {
    verify(id, path, &CancelToken::new()).is_ok()
}
pub fn verify(id: ModelId, path: &Path, cancel: &CancelToken) -> Result<(), String> {
    let mut file = File::open(path).map_err(|_| {
        format!(
            "Download {} in Settings → Clipboard → Dictation.",
            id.descriptor().label
        )
    })?;
    if file.metadata().map_err(|e| e.to_string())?.len() != id.descriptor().bytes {
        return Err("Model size is incorrect. Download it again in Settings.".into());
    }
    let mut hash = Sha256::new();
    let mut buf = vec![0; 1024 * 1024];
    loop {
        if cancel.is_cancelled() {
            return Err("Cancelled".into());
        }
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    if format!("{:x}", hash.finalize()) != id.descriptor().sha256 {
        return Err("Model checksum failed. Download it again in Settings.".into());
    }
    Ok(())
}
pub fn download(
    id: ModelId,
    path: &Path,
    cancel: &CancelToken,
    progress: impl Fn(u64),
) -> Result<(), String> {
    if cancel.is_cancelled() {
        return Err("Download cancelled".into());
    }
    let owner = ModelOwner::acquire(path)?;
    owner.reap();
    // Another install may have completed it while the consent sheet was open.
    if verify(id, path, cancel).is_ok() {
        return Ok(());
    }
    if cancel.is_cancelled() {
        return Err("Download cancelled".into());
    }
    let response = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(20))
        .timeout_read(std::time::Duration::from_secs(30))
        .build()
        .get(id.descriptor().url)
        .call()
        .map_err(|e| format!("Download failed: {e}. Retry in Settings."))?;
    receive_download(id, &owner, response.into_reader(), cancel, progress)
}
fn receive_download(
    id: ModelId,
    owner: &ModelOwner,
    mut reader: impl Read,
    cancel: &CancelToken,
    progress: impl Fn(u64),
) -> Result<(), String> {
    let path = &owner.path;
    let tmp = path.with_extension(format!(
        "{}-{}.download",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| e.to_string())?;
    let result = (|| {
        let mut bytes = 0;
        let mut last = 0;
        let mut buf = vec![0; 256 * 1024];
        loop {
            if cancel.is_cancelled() {
                return Err("Download cancelled".into());
            }
            let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            bytes += n as u64;
            if bytes > id.descriptor().bytes {
                return Err("Model download exceeds expected size".into());
            }
            file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            if bytes - last >= 4 * 1024 * 1024 {
                progress(bytes);
                last = bytes;
            }
        }
        progress(id.descriptor().bytes);
        file.sync_all().map_err(|e| e.to_string())?;
        publish_verified(id, &tmp, path, cancel)
    })();
    let _ = std::fs::remove_file(tmp);
    result
}
fn publish_verified(
    id: ModelId,
    tmp: &Path,
    path: &Path,
    cancel: &CancelToken,
) -> Result<(), String> {
    verify(id, tmp, cancel)?;
    if cancel.is_cancelled() {
        return Err("Download cancelled".into());
    }
    std::fs::rename(tmp, path).map_err(|e| e.to_string())?;
    File::open(path.parent().ok_or("Missing model directory")?)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}
/// One-shot offline verification utility. Interactive turns use dictation_worker.
pub fn transcribe(
    id: ModelId,
    path: &Path,
    pcm: &[f32],
    cancel: &CancelToken,
) -> Result<String, String> {
    if pcm.len() < 1600 || pcm.iter().all(|v| v.abs() < 0.001) {
        return Ok(String::new());
    }
    verify(id, path, cancel)?;
    transcribe_cpp::disable_logging();
    let model = Model::load_with(
        path,
        &ModelOptions {
            backend: Backend::Metal,
            ..Default::default()
        },
    )
    .map_err(|e| format!("Could not load local model: {e}"))?;
    if cancel.is_cancelled() {
        return Err("Cancelled".into());
    }
    let mut session = model.session().map_err(|e| e.to_string())?;
    session.set_cancel_token(cancel);
    // Bound encoder attention memory as well as retained PCM.
    let mut text = Vec::new();
    for chunk in pcm.chunks(16000 * 20) {
        if cancel.is_cancelled() {
            return Err("Cancelled".into());
        }
        let mut padded = chunk.to_vec();
        padded.resize(padded.len().max(16000), 0.);
        let transcript = session
            .run(
                &padded,
                &RunOptions {
                    language: id.descriptor().language.map(str::to_owned),
                    ..Default::default()
                },
            )
            .map_err(|e| format!("Local transcription failed: {e}"))?;
        let part = transcript.text.trim().to_string();
        if !part.is_empty() {
            text.push(part);
        }
    }
    Ok(text.join(" "))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn silence_needs_no_model_and_bad_artifacts_are_rejected() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("model");
        assert_eq!(
            transcribe(
                ModelId::default(),
                &p,
                &vec![0.; 16000],
                &CancelToken::new()
            )
            .unwrap(),
            ""
        );
        assert!(verify(ModelId::default(), &p, &CancelToken::new()).is_err());
        std::fs::write(&p, b"bad").unwrap();
        assert!(
            verify(ModelId::default(), &p, &CancelToken::new())
                .unwrap_err()
                .contains("size")
        );
    }
    #[test]
    fn cancelled_download_does_not_publish_a_model() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("model");
        let token = CancelToken::new();
        token.cancel();
        assert!(download(ModelId::default(), &p, &token, |_| {}).is_err());
        assert!(!p.exists());
        assert!(std::fs::read_dir(d.path()).unwrap().next().is_none());
    }
    #[test]
    fn corrupt_download_never_replaces_an_existing_model() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("model");
        let tmp = d.path().join("download");
        std::fs::write(&path, b"previous model").unwrap();
        let file = File::create(&tmp).unwrap();
        file.set_len(ModelId::UnifiedEnglish.descriptor().bytes)
            .unwrap();
        file.sync_all().unwrap();
        let error =
            publish_verified(ModelId::default(), &tmp, &path, &CancelToken::new()).unwrap_err();
        assert!(error.contains("checksum"));
        assert_eq!(std::fs::read(&path).unwrap(), b"previous model");
    }
    #[test]
    fn crash_cleanup_preserves_published_and_unrelated_files() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("model.gguf");
        for name in [
            "model.gguf",
            "model.12-345.download",
            "other.12-345.download",
            "model.notes.download",
        ] {
            std::fs::write(d.path().join(name), b"keep").unwrap();
        }
        ModelOwner::acquire(&path).unwrap().reap();
        assert!(path.exists());
        assert!(!d.path().join("model.12-345.download").exists());
        assert!(d.path().join("other.12-345.download").exists());
        assert!(d.path().join("model.notes.download").exists());
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    #[test]
    fn contending_owner_cannot_reap_live_download_and_readers_keep_published_model() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("model.gguf");
        std::fs::write(&path, b"published").unwrap();
        let first = ModelOwner::acquire(&path).unwrap();
        let tmp = d.path().join("model.123-456.download");
        std::fs::write(&tmp, b"in flight").unwrap();
        assert!(ModelOwner::acquire(&path).is_err());
        assert_eq!(std::fs::read(&tmp).unwrap(), b"in flight");
        assert_eq!(std::fs::read(&path).unwrap(), b"published");
        drop(first);
        let next = ModelOwner::acquire(&path).unwrap();
        next.reap();
        assert!(!tmp.exists());
        assert_eq!(std::fs::read(&path).unwrap(), b"published");
    }
    struct BlockedReader {
        entered: Arc<Barrier>,
        release: Arc<Barrier>,
        cancel: CancelToken,
    }
    impl Read for BlockedReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.entered.wait();
            self.release.wait();
            assert!(self.cancel.is_cancelled());
            buf[0] = 1;
            Ok(1)
        }
    }
    #[test]
    fn cancelling_acquisition_keeps_the_lock_until_the_worker_returns_and_cleans_its_temp() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("model.gguf");
        std::fs::write(&path, b"published").unwrap();
        let owner = ModelOwner::acquire(&path).unwrap();
        let token = CancelToken::new();
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let reader = BlockedReader {
            entered: entered.clone(),
            release: release.clone(),
            cancel: token.clone(),
        };
        let worker_token = token.clone();
        let worker = std::thread::spawn(move || {
            receive_download(ModelId::default(), &owner, reader, &worker_token, |_| {})
        });
        entered.wait();
        token.cancel();
        assert!(ModelOwner::acquire(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"published");
        release.wait();
        assert!(worker.join().unwrap().is_err());
        assert!(ModelOwner::acquire(&path).is_ok());
        assert!(
            std::fs::read_dir(d.path()).unwrap().all(|e| !e
                .unwrap()
                .path()
                .to_string_lossy()
                .ends_with(".download"))
        );
    }
    #[test]
    fn crashed_owner_releases_the_kernel_lock_and_only_the_next_owner_reaps() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("model.gguf");
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "dictation_model::ownership_tests::crash_child",
                "--nocapture",
            ])
            .env("VORKSPACE_DICTATION_LOCK_FIXTURE", &path)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(d.path().join("model.123-456.download").exists());
        let owner = ModelOwner::acquire(&path).unwrap();
        owner.reap();
        assert!(!d.path().join("model.123-456.download").exists());
    }
    #[test]
    fn crash_child() {
        let Some(path) = std::env::var_os("VORKSPACE_DICTATION_LOCK_FIXTURE") else {
            return;
        };
        let path = std::path::PathBuf::from(path);
        let _owner = ModelOwner::acquire(&path).unwrap();
        std::fs::write(
            path.parent().unwrap().join("model.123-456.download"),
            b"orphan",
        )
        .unwrap();
        std::process::exit(0);
    }
}
