//! Where the app keeps its database by default.
use std::path::PathBuf;

pub fn vorkspace_database() -> Option<PathBuf> {
    let base = crate::shell_env::home()?.join("Library/Application Support");
    Some(base.join("com.vorkspace.desktop/vorkspace.db"))
}
