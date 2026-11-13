//! Last successful checkout metadata shared by conversations and Git pickers.
use std::{
    collections::HashMap,
    path::Path,
    sync::{Mutex, OnceLock},
};
use vorkspace_core::{
    files::path_key,
    git::{self, Snapshot},
};

const LIMIT: usize = 24;
const BYTE_LIMIT: usize = 16 * 1024 * 1024;

struct Entry {
    last_used: u64,
    bytes: usize,
    state: Snapshot,
}

#[derive(Default)]
struct Memory {
    clock: u64,
    entries: HashMap<String, Entry>,
}
impl Memory {
    fn get(&mut self, cwd: &str) -> Option<Snapshot> {
        let entry = self.entries.get_mut(&path_key(cwd))?;
        self.clock += 1;
        entry.last_used = self.clock;
        Some(entry.state.clone())
    }
    fn put(&mut self, key: String, state: Snapshot, rows_bytes: usize) {
        // Charge aliases separately. This intentionally overestimates shared rows.
        let bytes = rows_bytes
            + key.capacity()
            + state.current.as_ref().map_or(0, String::capacity)
            + std::mem::size_of::<Entry>();
        if bytes > BYTE_LIMIT {
            self.entries.remove(&key);
            return;
        }
        self.clock += 1;
        self.entries.insert(
            key,
            Entry {
                last_used: self.clock,
                bytes,
                state,
            },
        );
        while self.entries.len() > LIMIT
            || self
                .entries
                .values()
                .map(|entry| entry.bytes)
                .sum::<usize>()
                > BYTE_LIMIT
        {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone())
                .unwrap();
            self.entries.remove(&oldest);
        }
    }
    fn remember(&mut self, cwd: &str, state: &Snapshot) {
        let key = path_key(cwd);
        let rows_bytes = estimated_rows_bytes(state);
        // The sidebar belongs to the project root even when the active chat
        // runs in a linked worktree. Its HEAD must remain the main checkout's.
        if let Some(main) = state
            .worktrees
            .iter()
            .find(|tree| tree.main && !tree.missing)
            && let main_key = path_key(&main.path)
            && main_key != key
        {
            let mut root = state.clone();
            if state.linked {
                root.current = main.branch.clone();
                root.detached = main.branch.is_none();
                root.linked = false;
            }
            self.put(main_key, root, rows_bytes);
        }
        self.put(key, state.clone(), rows_bytes);
    }
}

fn estimated_rows_bytes(state: &Snapshot) -> usize {
    std::mem::size_of_val(state.branches.as_ref())
        + state
            .branches
            .iter()
            .map(|row| row.name.capacity() + row.remote.as_ref().map_or(0, String::capacity))
            .sum::<usize>()
        + std::mem::size_of_val(state.worktrees.as_ref())
        + state
            .worktrees
            .iter()
            .map(|row| {
                row.path.capacity()
                    + row.head.capacity()
                    + row.branch.as_ref().map_or(0, String::capacity)
            })
            .sum::<usize>()
}

fn memory() -> &'static Mutex<Memory> {
    static MEMORY: OnceLock<Mutex<Memory>> = OnceLock::new();
    MEMORY.get_or_init(Default::default)
}
pub fn get(cwd: &str) -> Option<Snapshot> {
    memory().lock().ok()?.get(cwd)
}
pub fn remember(cwd: &str, state: &Snapshot) {
    if let Ok(mut memory) = memory().lock() {
        memory.remember(cwd, state);
    }
}
pub fn load(cwd: &Path) -> Result<Snapshot, String> {
    let state = git::snapshot(cwd)?;
    // Failure leaves the last successful list intact.
    remember(&cwd.to_string_lossy(), &state);
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vorkspace_core::git::Worktree;
    #[test]
    fn linked_checkout_warms_project_lists_without_replacing_its_branch() {
        let mut memory = Memory::default();
        memory.remember(
            "/repo-feature",
            &Snapshot {
                current: Some("feature".into()),
                linked: true,
                worktrees: vec![
                    Worktree {
                        path: "/repo".into(),
                        branch: Some("main".into()),
                        main: true,
                        ..Default::default()
                    },
                    Worktree {
                        path: "/repo-feature".into(),
                        branch: Some("feature".into()),
                        ..Default::default()
                    },
                ]
                .into(),
                ..Default::default()
            },
        );
        assert_eq!(
            memory.get("/repo/").unwrap().current.as_deref(),
            Some("main")
        );
        assert!(!memory.get("/repo").unwrap().linked);
        assert_eq!(
            memory.get("/repo-feature").unwrap().current.as_deref(),
            Some("feature")
        );
    }
    #[test]
    fn recent_projects_survive_with_a_bounded_cache() {
        let mut memory = Memory::default();
        for i in 0..LIMIT {
            memory.remember(&format!("/repo-{i}"), &Snapshot::default());
        }
        memory.get("/repo-0").unwrap();
        memory.remember("/new", &Snapshot::default());
        assert_eq!(memory.entries.len(), LIMIT);
        assert!(memory.get("/repo-0").is_some());
        assert!(memory.get("/repo-1").is_none());
    }

    #[test]
    fn oversized_snapshot_does_not_displace_the_hot_set() {
        let mut memory = Memory::default();
        memory.put("/one".into(), Snapshot::default(), BYTE_LIMIT / 3);
        memory.put("/two".into(), Snapshot::default(), BYTE_LIMIT / 3);
        memory.put("/too-large".into(), Snapshot::default(), BYTE_LIMIT);
        assert!(memory.get("/too-large").is_none());
        assert!(memory.get("/one").is_some());
        memory.put("/three".into(), Snapshot::default(), BYTE_LIMIT / 3);
        assert!(memory.get("/one").is_some());
        assert!(memory.get("/two").is_none());
        assert!(memory.get("/three").is_some());
    }
}
