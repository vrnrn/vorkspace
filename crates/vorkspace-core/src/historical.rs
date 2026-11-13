//! Recognises the app's earlier name in historical content: persisted keys,
//! application blocks and recorded shell commands. Fingerprints identify the
//! earlier name without retaining retired branding in the source tree. Paths,
//! IDs and user text are never rewritten.
use serde_json::Value;
use sha2::{Digest, Sha256};

fn fingerprint(name: &str) -> String {
    format!("{:x}", Sha256::digest(name.as_bytes()))
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
