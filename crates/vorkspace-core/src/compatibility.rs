//! Read compatibility for persisted identities from earlier desktop versions.
//! Fingerprints identify historical names without retaining retired branding in
//! the source tree. Data stays at its existing path; opening never copies a DB,
//! rewrites a transcript, or changes a provider session ID.
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

fn fingerprint(name: &str) -> String {
    format!("{:x}", Sha256::digest(name.as_bytes()))
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
