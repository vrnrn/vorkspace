//! Read compatibility for persisted identities from earlier desktop versions.
//! Fingerprints identify historical names without retaining retired branding in
//! the source tree. Data stays at its existing path; opening never copies a DB,
//! rewrites a transcript, or changes a provider session ID.
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

fn fingerprint(name: &str) -> String {
    format!("{:x}", Sha256::digest(name.as_bytes()))
}

fn existing_child(root: &Path, identity: &str) -> Option<PathBuf> {
    std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .find_map(|entry| {
            (fingerprint(&entry.file_name().to_string_lossy()) == identity).then(|| entry.path())
        })
}

pub fn vorkspace_database() -> Option<PathBuf> {
    let base = crate::shell_env::home()?.join("Library/Application Support");
    Some(base.join("com.vorkspace.desktop/vorkspace.db"))
}

/// Normalize app-owned persisted identities only. Paths, IDs and user text stay exact.
pub(crate) fn key(key: &str) -> String {
    let end = key.find(['.', ':', '-']).unwrap_or(key.len());
    let prefix = &key[..end];
    let replacement = if fingerprint(prefix)
        == "01b8c92e5e3a72e0400c920bd7376918189956e379ee280afa4bd4e653df1d9a"
    {
        "vorkspace"
    } else {
        prefix
    };
    format!("{replacement}{}", &key[end..])
}

/// Rename a known field at its owning schema level. Opaque metadata is retained.
fn field(value: &mut Value, identity: &str, current: &str) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    let old = object
        .keys()
        .find(|name| fingerprint(name) == identity)
        .cloned();
    if let Some(old) = old {
        if let Some(value) = object.remove(&old) {
            object.entry(current).or_insert(value);
        }
    }
}
/// Only known app-owned metadata is interpreted; nested tool payloads and text
/// remain opaque, including when ordinary edits save the block again.
pub(crate) fn block(value: &mut Value) {
    if let Some(object) = value.as_object_mut() {
        for old in object
            .keys()
            .filter(|name| {
                !name.starts_with("bot")
                    && [
                        "Habit",
                        "Card",
                        "Reaction",
                        "SessionCompletion",
                        "PeerMessage",
                        "SpawnedSessions",
                    ]
                    .iter()
                    .any(|suffix| name.ends_with(suffix))
            })
            .cloned()
            .collect::<Vec<_>>()
        {
            let current = match fingerprint(&old).as_str() {
                "2c658edc2b1c279e0e09d2a05509361ca3d256bd1f43c64a2715ca52bb8e3206" => {
                    Some("botHabit")
                }
                "4dd1106359c54269d46ad2f71efe553e9a97ca285d4a6daea78a1c230e959c9b" => {
                    Some("botCard")
                }
                "2d7aad824eab31db08aad2976d73ed6768a7833fa79b93aa50076fdfd71f28dd" => {
                    Some("botReaction")
                }
                "5f238f25e2f3d93b29cceaae8d8db76cdeef5ae8147d7481e7d587767b36fc78" => {
                    Some("botSessionCompletion")
                }
                "d131ac3c80a76da275d7e6b0345ee7bf5314b03872b23a2ceeb01460c028fb44" => {
                    Some("botPeerMessage")
                }
                "409c710995f90531f0d2dc1b598d72beecef67ef4825ed53d0c0cc5c74ab934d" => {
                    Some("botSpawnedSessions")
                }
                _ => None,
            };
            if let Some(current) = current {
                if let Some(value) = object.remove(&old) {
                    object.entry(current).or_insert(value);
                }
            }
        }
    }
    if let Some(peer) = value.get_mut("botPeerMessage") {
        field(
            peer,
            "a367212661cd53967e23173f783720ddc743bf4a7f011cbc52dbe54c67e9a875",
            "fromBotId",
        );
        field(
            peer,
            "63befccfd967bc398ba39b3b5b0b06fd368688fcb933f61a1e97c9f9dad67e13",
            "toBotId",
        );
    }
    for thread in value
        .get_mut("btwThreads")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        for message in thread
            .get_mut("messages")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            for child in message
                .get_mut("blocks")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                block(child);
            }
        }
    }
}

/// Select an existing complete owner in place. An empty counterpart directory
/// does not replace a saved owner; two populated owners are never combined.
pub(crate) fn has_owner_records(path: &Path, depth: u8) -> bool {
    if !std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink()) {
        return false;
    }
    std::fs::read_dir(path).ok().is_some_and(|entries| {
        entries.flatten().any(|entry| {
            entry.file_type().map_or(true, |kind| {
                !kind.is_dir() || depth == 0 || has_owner_records(&entry.path(), depth - 1)
            })
        })
    })
}

pub(crate) fn bot_families(root: &Path, provider_home: bool) -> Vec<PathBuf> {
    let mut paths = vec![root.to_owned()];
    let identity = if provider_home {
        "3c5593b09cae5aab4027ccb4da4eaa3d2ab63276e3566466341b6769acc7be1b"
    } else {
        "9b9e74c140f4ffe20089b4929a5609a790039039534159550ce45964efe895f2"
    };
    if let Some(earlier) = root
        .parent()
        .and_then(|base| existing_child(base, identity))
        .filter(|path| {
            std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
        })
    {
        paths.push(earlier);
    }
    paths
}
pub(crate) fn bot_owner(root: &Path, owner: &str, provider_home: bool) -> PathBuf {
    let current = root.join(owner);
    if has_owner_records(&current, 4) {
        return current;
    }
    for earlier in bot_families(root, provider_home)
        .into_iter()
        .skip(1)
        .map(|p| p.join(owner))
    {
        if has_owner_records(&earlier, 4) {
            return earlier;
        }
    }
    current
}

pub(crate) fn application_block(block: &Value) -> bool {
    block.as_object().is_some_and(|object| {
        object
            .iter()
            .any(|(name, value)| value == true && key(name) == "vorkspace")
    })
}

/// Historical shell receipts can name the earlier executable, but cannot make
/// arbitrary shell commands into accepted app effects.
pub(crate) fn app_executable(name: &str) -> bool {
    matches!(name, "vorkspace" | "vorkspace-gpui") || key(name) == "vorkspace"
}

#[cfg(test)]
mod tests;
