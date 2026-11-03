//! The pinned Core ML snapshot is a directory of independently verified files.
//! Every member uses SHA-256; Git-tracked metadata also checks its blob identity.
use super::{CancelToken, ModelId, ModelOwner};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::{
    ffi::CString,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, OpenOptionsExt},
    },
    path::{Component, Path},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy)]
enum Checksum {
    Sha256(&'static str),
    GitBlob(&'static str, &'static str),
}
#[derive(Clone, Copy)]
struct Member {
    path: &'static str,
    bytes: u64,
    checksum: Checksum,
}
use Checksum::{GitBlob as G, Sha256 as S};

// FluidInference/parakeet-realtime-eou-120m-coreml at 40a23f4c…, 160ms.
const FILES: &[Member] = &[
    Member {
        path: "decoder.mlmodelc/analytics/coremldata.bin",
        bytes: 243,
        checksum: S("3996975a8cbc1949159c55605b3132b39b2484f51acbd55d796d93c70de02b49"),
    },
    Member {
        path: "decoder.mlmodelc/coremldata.bin",
        bytes: 497,
        checksum: S("c3ccbff963d8cf07e2be2bd56ea3384a89ea49628922c6bd95ff62e2ae57dc34"),
    },
    Member {
        path: "decoder.mlmodelc/metadata.json",
        bytes: 3283,
        checksum: G(
            "3581673bcb062f7e0e1e0efa9f59edc5f218d50e",
            "0977480649f2756894b0acfe2fdf4231a991f25e3fe02562bfb71b65ca944575",
        ),
    },
    Member {
        path: "decoder.mlmodelc/model.mil",
        bytes: 7409,
        checksum: G(
            "5ead417fbdc13f8bf0c05a26039b2828503d4eca",
            "b7c084a35bdbc887d69d6226cd533e2c11b2792c37d7352cf878f9f6f3c13555",
        ),
    },
    Member {
        path: "decoder.mlmodelc/weights/weight.bin",
        bytes: 7873600,
        checksum: S("0b4cacecdcd9df79ab1e56de67230baf5a8664d2afe0bb8f3408eefa972cb2f4"),
    },
    Member {
        path: "joint_decision.mlmodelc/analytics/coremldata.bin",
        bytes: 243,
        checksum: S("5bca32ad130dcad6605cc00044c752aa5b45ef57d14c17f2d1a2fa49d6cf55b5"),
    },
    Member {
        path: "joint_decision.mlmodelc/coremldata.bin",
        bytes: 493,
        checksum: S("22d4abc4625b935ee035b5f8ce7cb28d1041b9b01c12173e287bf4b5f5d99625"),
    },
    Member {
        path: "joint_decision.mlmodelc/metadata.json",
        bytes: 3181,
        checksum: G(
            "a0a319857f429f5ec4b106397cd52ec354ef2e96",
            "e970ae87137730020690d24d971813db3633bbdfed602d43b6a9c84deced6dc8",
        ),
    },
    Member {
        path: "joint_decision.mlmodelc/model.mil",
        bytes: 9608,
        checksum: G(
            "172a7579866c84407b32b5f746b7f1ae132599d8",
            "45e8590bc87e34c162b547e43a4f60e64db15b017f48395d7835a6867884804f",
        ),
    },
    Member {
        path: "joint_decision.mlmodelc/weights/weight.bin",
        bytes: 2794182,
        checksum: S("7039b2010a269153f5a96edf28637f921a86ef8822f248f2d6712f7a6bce84b4"),
    },
    Member {
        path: "streaming_encoder.mlmodelc/analytics/coremldata.bin",
        bytes: 243,
        checksum: S("a981b257db79b4f86e6fa06a92562160a0ae71554746c24af24d8634b85f0356"),
    },
    Member {
        path: "streaming_encoder.mlmodelc/coremldata.bin",
        bytes: 670,
        checksum: S("e762abc60d999bcd10aab985b68191a602f2e8e03165cf08671c60f93936037a"),
    },
    Member {
        path: "streaming_encoder.mlmodelc/metadata.json",
        bytes: 5327,
        checksum: G(
            "32148a71ed327b665c0b11778846e2378749683d",
            "75be31534cdd91711b08ba3a46046523eb9be9909618cd569cce1ea79e842a95",
        ),
    },
    Member {
        path: "streaming_encoder.mlmodelc/model.mil",
        bytes: 639646,
        checksum: G(
            "52cb37c36ef8f9be82cd6744ed43a2e13a911b40",
            "709f9280eb0bba1fd698cc252275ba802885c2c53cdb60d399277281dac09b5d",
        ),
    },
    Member {
        path: "streaming_encoder.mlmodelc/weights/weight.bin",
        bytes: 212691776,
        checksum: S("12cd781a4300b52b6687587b7d8e37e0ce5c8ccb1dbea036008275e6abf5070c"),
    },
    Member {
        path: "vocab.json",
        bytes: 17437,
        checksum: G(
            "462081190dbb331a0dde13e99d5cfa99b302f0a7",
            "83fd42ad33dae1bd3ceee6c0bb6c625f314cf0b2dc8430be441ac1e2643d5c36",
        ),
    },
];

pub(super) fn verify(root: &Path, cancel: &CancelToken) -> Result<(), String> {
    verify_members(root, FILES, cancel)
}

fn verify_members(root: &Path, files: &[Member], cancel: &CancelToken) -> Result<(), String> {
    let meta = fs::symlink_metadata(root)
        .map_err(|_| "Download Parakeet Flash in Settings.".to_owned())?;
    if !meta.file_type().is_dir() {
        return Err("Parakeet Flash model directory is invalid.".into());
    }
    verify_tree(root, root, files)?;
    for member in files {
        verify_member(root, member, cancel)?;
    }
    Ok(())
}

fn verify_tree(root: &Path, dir: &Path, files: &[Member]) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let relative = path.strip_prefix(root).map_err(|e| e.to_string())?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_dir()
            && files
                .iter()
                .any(|m| Path::new(m.path).starts_with(relative))
        {
            verify_tree(root, &path, files)?;
        } else if !kind.is_file() || !files.iter().any(|m| Path::new(m.path) == relative) {
            return Err("Parakeet Flash model contains an unexpected file.".into());
        }
    }
    Ok(())
}

fn verify_member(root: &Path, member: &Member, cancel: &CancelToken) -> Result<(), String> {
    let mut path = root.to_path_buf();
    let mut parts = Path::new(member.path).components().peekable();
    while let Some(part) = parts.next() {
        let Component::Normal(name) = part else {
            return Err("Invalid model manifest path".into());
        };
        path.push(name);
        let meta = fs::symlink_metadata(&path)
            .map_err(|_| "Parakeet Flash model is incomplete.".to_owned())?;
        if parts.peek().is_some() {
            if !meta.file_type().is_dir() {
                return Err("Parakeet Flash model directory is invalid.".into());
            }
        } else if !meta.file_type().is_file() || meta.len() != member.bytes {
            return Err("Parakeet Flash model size is incorrect.".into());
        }
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
        .map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() != member.bytes {
        return Err("Parakeet Flash model size is incorrect.".into());
    }
    let mut sha256 = Sha256::new();
    let mut git = Sha1::new();
    if matches!(member.checksum, G(_, _)) {
        git.update(format!("blob {}\0", member.bytes).as_bytes());
    }
    let mut buf = vec![0; 256 * 1024];
    let mut bytes = 0u64;
    loop {
        if cancel.is_cancelled() {
            return Err("Cancelled".into());
        }
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        if bytes > member.bytes {
            return Err("Parakeet Flash model size is incorrect.".into());
        }
        sha256.update(&buf[..n]);
        if matches!(member.checksum, G(_, _)) {
            git.update(&buf[..n]);
        }
    }
    let valid = match member.checksum {
        S(expected) => format!("{:x}", sha256.finalize()) == expected,
        G(expected_git, expected_sha256) => {
            format!("{:x}", git.finalize()) == expected_git
                && format!("{:x}", sha256.finalize()) == expected_sha256
        }
    };
    if bytes != member.bytes || !valid {
        return Err("Parakeet Flash checksum failed. Download it again in Settings.".into());
    }
    Ok(())
}

pub(super) fn download(
    owner: &ModelOwner,
    cancel: &CancelToken,
    progress: impl Fn(u64),
) -> Result<(), String> {
    let path = &owner.path;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let tmp = path.with_extension(format!("{}-{stamp}.download", std::process::id()));
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&tmp)
        .map_err(|e| e.to_string())?;
    let mut stage_owned = true;
    let result = (|| {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(20))
            .timeout_read(std::time::Duration::from_secs(30))
            .build();
        let mut bytes = 0u64;
        let mut reported = 0u64;
        for member in FILES {
            if cancel.is_cancelled() {
                return Err("Download cancelled".into());
            }
            let dest = tmp.join(member.path);
            let parent = dest.parent().ok_or("Invalid model manifest path")?;
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&dest)
                .map_err(|e| e.to_string())?;
            let url = format!(
                "{}{}",
                ModelId::ParakeetRealtimeEou.descriptor().url,
                member.path
            );
            let mut reader = agent
                .get(&url)
                .call()
                .map_err(|e| format!("Download failed: {e}. Retry in Settings."))?
                .into_reader();
            let mut member_bytes = 0u64;
            let mut buf = vec![0; 256 * 1024];
            loop {
                if cancel.is_cancelled() {
                    return Err("Download cancelled".into());
                }
                let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                member_bytes += n as u64;
                if member_bytes > member.bytes {
                    return Err("Model download exceeds expected size".into());
                }
                file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
                bytes += n as u64;
                if bytes - reported >= 4 * 1024 * 1024 {
                    progress(bytes);
                    reported = bytes;
                }
            }
            file.sync_all().map_err(|e| e.to_string())?;
        }
        progress(bytes);
        verify(&tmp, cancel)?;
        if cancel.is_cancelled() {
            return Err("Download cancelled".into());
        }
        sync_directories(&tmp)?;
        publish(&tmp, path, stamp)?;
        stage_owned = false;
        File::open(path.parent().ok_or("Missing model directory")?)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        Ok(())
    })();
    if stage_owned {
        let _ = fs::remove_dir_all(&tmp);
    }
    result
}

fn sync_directories(root: &Path) -> Result<(), String> {
    for name in [
        "decoder.mlmodelc",
        "joint_decision.mlmodelc",
        "streaming_encoder.mlmodelc",
    ] {
        for suffix in ["analytics", "weights", ""] {
            File::open(root.join(name).join(suffix))
                .and_then(|f| f.sync_all())
                .map_err(|e| e.to_string())?;
        }
    }
    File::open(root)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}

// Swap an existing directory atomically, preserving the previous content.
fn publish(tmp: &Path, path: &Path, stamp: u128) -> Result<(), String> {
    let from = CString::new(tmp.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let to = CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let existed = fs::symlink_metadata(path).is_ok();
    let flag = if existed {
        libc::RENAME_SWAP
    } else {
        libc::RENAME_EXCL
    };
    if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), flag) } != 0 {
        return Err(format!(
            "Could not publish the model: {}",
            std::io::Error::last_os_error()
        ));
    }
    if existed {
        let old = path.with_extension(format!("preserved-{}-{stamp}", std::process::id()));
        // If preservation rename fails, leave the old directory at tmp; never delete it.
        let _ = fs::rename(tmp, old);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manifest_size_matches_consent_and_progress() {
        assert_eq!(
            FILES.iter().map(|f| f.bytes).sum::<u64>(),
            ModelId::ParakeetRealtimeEou.descriptor().bytes
        );
    }
    #[test]
    fn member_verification_rejects_missing_changed_and_symlinked_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.bin");
        let spec = Member {
            path: "model.bin",
            bytes: 3,
            checksum: G(
                "f2ba8f84ab5c1bce84a7b441cb1959cfc7093b7f",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
        }; // Git blob "abc"
        assert!(verify_member(dir.path(), &spec, &CancelToken::new()).is_err());
        fs::write(&path, b"abc").unwrap();
        assert!(verify_member(dir.path(), &spec, &CancelToken::new()).is_ok());
        fs::write(&path, b"abd").unwrap();
        assert!(verify_member(dir.path(), &spec, &CancelToken::new()).is_err());
        fs::rename(&path, dir.path().join("other")).unwrap();
        std::os::unix::fs::symlink("other", &path).unwrap();
        assert!(verify_member(dir.path(), &spec, &CancelToken::new()).is_err());
    }
    #[test]
    fn verification_rejects_unlisted_files() {
        let dir = tempfile::tempdir().unwrap();
        let spec = Member {
            path: "model.bin",
            bytes: 3,
            checksum: G(
                "f2ba8f84ab5c1bce84a7b441cb1959cfc7093b7f",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
        };
        fs::write(dir.path().join("model.bin"), b"abc").unwrap();
        assert!(verify_members(dir.path(), &[spec], &CancelToken::new()).is_ok());
        fs::write(dir.path().join("unlisted"), b"x").unwrap();
        assert!(verify_members(dir.path(), &[spec], &CancelToken::new()).is_err());
    }
    #[test]
    fn replacement_preserves_the_old_directory() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.eou");
        let tmp = dir.path().join("stage.download");
        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("old"), b"old").unwrap();
        fs::create_dir(&tmp).unwrap();
        fs::write(tmp.join("new"), b"new").unwrap();
        publish(&tmp, &dest, 42).unwrap();
        assert_eq!(fs::read(dest.join("new")).unwrap(), b"new");
        let old = dest.with_extension(format!("preserved-{}-42", std::process::id()));
        assert_eq!(fs::read(old.join("old")).unwrap(), b"old");
    }
}
