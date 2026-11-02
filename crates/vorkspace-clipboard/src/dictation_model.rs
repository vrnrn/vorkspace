//! One pinned artifact. A verified temporary file is synced and renamed last.
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};
use transcribe_cpp::{Backend, CancelToken, Model, ModelOptions, RunOptions};
pub const NAME: &str = "parakeet-unified-en-0.6b-Q8_0.gguf";
pub const BYTES: u64 = 731_357_568;
pub const SHA256: &str = "4b50b6dd862bf6e346929aaf4f5eaacec003bfa3f56462d6c874b41ef2f38795";
pub const URL: &str = "https://huggingface.co/handy-computer/parakeet-unified-en-0.6b-gguf/resolve/7e948f21b7bdbac698d3318db9d350f1096f3b6c/parakeet-unified-en-0.6b-Q8_0.gguf";
/// Called only after the helper's per-install lock is acquired. A crashed owner's
/// unpublished downloads can be discarded without touching the verified model.
pub fn reap_downloads(path: &Path) {
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
pub fn verify(path: &Path, cancel: &CancelToken) -> Result<(), String> {
    let mut file = File::open(path).map_err(|_| {
        "Download the English model in Settings → Clipboard → Dictation.".to_string()
    })?;
    if file.metadata().map_err(|e| e.to_string())?.len() != BYTES {
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
    if format!("{:x}", hash.finalize()) != SHA256 {
        return Err("Model checksum failed. Download it again in Settings.".into());
    }
    Ok(())
}
pub fn download(path: &Path, cancel: &CancelToken, progress: impl Fn(u64)) -> Result<(), String> {
    if cancel.is_cancelled() {
        return Err("Download cancelled".into());
    }
    let dir = path.parent().ok_or("Missing model directory")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| e.to_string())?;
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
        let response = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(20))
            .timeout_read(std::time::Duration::from_secs(30))
            .build()
            .get(URL)
            .call()
            .map_err(|e| format!("Download failed: {e}. Retry in Settings."))?;
        let mut reader = response.into_reader();
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
            if bytes > BYTES {
                return Err("Model download exceeds expected size".into());
            }
            file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            if bytes - last >= 4 * 1024 * 1024 {
                progress(bytes);
                last = bytes;
            }
        }
        progress(BYTES);
        file.sync_all().map_err(|e| e.to_string())?;
        publish_verified(&tmp, path, cancel)
    })();
    let _ = std::fs::remove_file(tmp);
    result
}
fn publish_verified(tmp: &Path, path: &Path, cancel: &CancelToken) -> Result<(), String> {
    verify(tmp, cancel)?;
    if cancel.is_cancelled() {
        return Err("Download cancelled".into());
    }
    std::fs::rename(tmp, path).map_err(|e| e.to_string())?;
    File::open(path.parent().ok_or("Missing model directory")?)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}
pub fn transcribe(path: &Path, pcm: &[f32], cancel: &CancelToken) -> Result<String, String> {
    if pcm.len() < 1600 || pcm.iter().all(|v| v.abs() < 0.001) {
        return Ok(String::new());
    }
    verify(path, cancel)?;
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
                    language: Some("en".into()),
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
            transcribe(&p, &vec![0.; 16000], &CancelToken::new()).unwrap(),
            ""
        );
        assert!(verify(&p, &CancelToken::new()).is_err());
        std::fs::write(&p, b"bad").unwrap();
        assert!(
            verify(&p, &CancelToken::new())
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
        assert!(download(&p, &token, |_| {}).is_err());
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
        file.set_len(BYTES).unwrap();
        file.sync_all().unwrap();
        let error = publish_verified(&tmp, &path, &CancelToken::new()).unwrap_err();
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
        reap_downloads(&path);
        assert!(path.exists());
        assert!(!d.path().join("model.12-345.download").exists());
        assert!(d.path().join("other.12-345.download").exists());
        assert!(d.path().join("model.notes.download").exists());
    }
}
