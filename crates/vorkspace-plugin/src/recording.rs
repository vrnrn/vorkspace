//! Durable transcript ownership shared by the native audio host and plugins.
//! Audio is never stored. The owner lock covers preparation through final commit.
use crate::valid_id;
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
pub const MAX_TRANSCRIPT: usize = 2 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Starting,
    Recording,
    Finishing,
    Stopped,
    Failed,
    Interrupted,
}
impl Phase {
    pub fn active(&self) -> bool {
        matches!(self, Self::Starting | Self::Recording | Self::Finishing)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct State {
    pub id: String,
    pub started_at: u64,
    pub ended_at: Option<u64>,
    pub phase: Phase,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Segment {
    pub sequence: u64,
    pub source: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transcript {
    pub state: State,
    pub segments: Vec<Segment>,
}
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
pub fn directory(base: &Path, id: &str) -> Result<PathBuf, String> {
    if !valid_id(id) {
        return Err("Invalid recording id".into());
    }
    Ok(base.join("recordings").join(id))
}
pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().ok_or("Missing data directory")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| e.to_string())?;
    let temporary = parent.join(format!(
        ".write-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        std::fs::rename(&temporary, path).map_err(|e| e.to_string())?;
        File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}
pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path, limit: usize) -> Result<T, String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err("Saved plugin data exceeds its limit".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}
fn lock_file(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| e.to_string())
}
pub struct Owner {
    directory: PathBuf,
    _lock: File,
    log: File,
    pub state: State,
    bytes: usize,
    next: u64,
}
impl Owner {
    pub fn create(base: &Path, id: &str) -> Result<Self, String> {
        let directory = directory(base, id)?;
        std::fs::create_dir_all(directory.parent().unwrap()).map_err(|e| e.to_string())?;
        // Reusing an ID is rejected: uncertain start effects are never replayed.
        std::fs::create_dir(&directory).map_err(|e| e.to_string())?;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(directory.join("owner.lock"))
            .map_err(|e| e.to_string())?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("Recording already has an owner".into());
        }
        let log = OpenOptions::new()
            .append(true)
            .create_new(true)
            .mode(0o600)
            .open(directory.join("transcript.jsonl"))
            .map_err(|e| e.to_string())?;
        let state = State {
            id: id.into(),
            started_at: now_ms(),
            ended_at: None,
            phase: Phase::Starting,
            error: None,
        };
        atomic_json(&directory.join("state.json"), &state)?;
        Ok(Self {
            directory,
            _lock: lock,
            log,
            state,
            bytes: 0,
            next: 0,
        })
    }
    pub fn phase(&mut self, phase: Phase, error: Option<String>) -> Result<(), String> {
        if !self.state.phase.active() {
            return Err("Recording has already ended".into());
        }
        let mut next = self.state.clone();
        next.phase = phase;
        next.error = error;
        if !next.phase.active() {
            self.log.sync_all().map_err(|e| e.to_string())?;
            next.ended_at = Some(now_ms());
        }
        atomic_json(&self.directory.join("state.json"), &next)?;
        self.state = next;
        Ok(())
    }
    pub fn append(
        &mut self,
        source: &str,
        start_ms: u64,
        end_ms: u64,
        text: String,
    ) -> Result<Segment, String> {
        if !self.state.phase.active()
            || !["microphone", "system"].contains(&source)
            || end_ms < start_ms
            || text.len() > 64 * 1024
        {
            return Err("Invalid transcript segment".into());
        }
        let segment = Segment {
            sequence: self.next + 1,
            source: source.into(),
            start_ms,
            end_ms,
            text,
        };
        let mut bytes = serde_json::to_vec(&segment).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        if self.bytes + bytes.len() > MAX_TRANSCRIPT {
            return Err(
                "This recording reached its transcript size limit. The saved text is preserved."
                    .into(),
            );
        }
        self.log
            .write_all(&bytes)
            .and_then(|_| self.log.sync_data())
            .map_err(|e| e.to_string())?;
        self.bytes += bytes.len();
        self.next += 1;
        Ok(segment)
    }
}
/// Recovery acquires the same owner lock before interpreting an active root.
/// A live recorder always wins; missing/unknown files never mean a finished turn.
pub fn state(base: &Path, id: &str) -> Result<State, String> {
    let directory = directory(base, id)?;
    let lock = lock_file(&directory.join("owner.lock"))?;
    let owned = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    let mut state: State = read_json(&directory.join("state.json"), 16 * 1024)?;
    if owned && state.phase.active() {
        state.phase = Phase::Interrupted;
        state.ended_at = Some(now_ms());
        state.error =
            Some("Recording was interrupted. All saved transcript segments are retained.".into());
        atomic_json(&directory.join("state.json"), &state)?;
    }
    Ok(state)
}
pub fn read(base: &Path, id: &str) -> Result<Transcript, String> {
    let state = state(base, id)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(directory(base, id)?.join("transcript.jsonl"))
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_TRANSCRIPT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_TRANSCRIPT {
        return Err("Transcript exceeds its storage limit".into());
    }
    let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    let mut segments = Vec::<Segment>::new();
    for line in bytes[..end]
        .split(|b| *b == b'\n')
        .filter(|b| !b.is_empty())
    {
        let segment: Segment = serde_json::from_slice(line).map_err(|e| e.to_string())?;
        if segment.sequence != segments.len() as u64 + 1 {
            return Err("Transcript segment continuity was lost".into());
        }
        segments.push(segment);
    }
    // Ignore only an incomplete trailing write; completed corrupt data fails closed.
    Ok(Transcript { state, segments })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_owner_preserves_segments_and_recovery_never_interrupts_live_work() {
        let dir = tempfile::tempdir().unwrap();
        let mut owner = Owner::create(dir.path(), "sample").unwrap();
        owner.phase(Phase::Recording, None).unwrap();
        owner
            .append("microphone", 0, 1000, "Hello 🦊".into())
            .unwrap();
        assert_eq!(state(dir.path(), "sample").unwrap().phase, Phase::Recording);
        assert!(Owner::create(dir.path(), "sample").is_err());
        drop(owner);
        let transcript = read(dir.path(), "sample").unwrap();
        assert_eq!(transcript.state.phase, Phase::Interrupted);
        assert_eq!(transcript.segments[0].text, "Hello 🦊");
    }
    #[test]
    fn stop_commits_and_partial_tail_cannot_erase_prior_truth() {
        let dir = tempfile::tempdir().unwrap();
        let mut owner = Owner::create(dir.path(), "sample").unwrap();
        owner.append("system", 100, 300, "Decided.".into()).unwrap();
        owner.phase(Phase::Stopped, None).unwrap();
        assert!(owner.append("system", 400, 500, "late".into()).is_err());
        drop(owner);
        OpenOptions::new()
            .append(true)
            .open(
                directory(dir.path(), "sample")
                    .unwrap()
                    .join("transcript.jsonl"),
            )
            .unwrap()
            .write_all(b"{incomplete")
            .unwrap();
        let saved = read(dir.path(), "sample").unwrap();
        assert_eq!(saved.state.phase, Phase::Stopped);
        assert_eq!(saved.segments.len(), 1);
        assert!(directory(dir.path(), "../elsewhere").is_err());
    }
    #[test]
    fn failed_phase_commit_does_not_publish_success_or_replace_saved_text() {
        let dir = tempfile::tempdir().unwrap();
        let mut owner = Owner::create(dir.path(), "sample").unwrap();
        owner
            .append("microphone", 0, 100, "Preserved.".into())
            .unwrap();
        let path = directory(dir.path(), "sample").unwrap().join("state.json");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(owner.phase(Phase::Stopped, None).is_err());
        assert_eq!(owner.state.phase, Phase::Starting);
        assert_eq!(owner.state.ended_at, None);
        assert!(
            std::fs::read_to_string(path.parent().unwrap().join("transcript.jsonl"))
                .unwrap()
                .contains("Preserved.")
        );
    }
}
