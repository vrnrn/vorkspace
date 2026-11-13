//! Durable transcript ownership shared by the audio helper and Meetings.
//! Audio is never stored. The owner lock covers preparation through final commit.
use crate::{directory::Directory, valid_id};
use rustix::fs::OFlags;
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    ffi::OsStr,
    fs::File,
    io::{BufRead, BufReader, Read, Write},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
pub const MAX_TRANSCRIPT: usize = 2 * 1024 * 1024;
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSource {
    Microphone,
    #[default]
    MicrophoneAndSystem,
}
impl CaptureSource {
    pub fn allows(self, source: &str) -> bool {
        source == "microphone" || (self == Self::MicrophoneAndSystem && source == "system")
    }
}
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
    #[serde(default)]
    pub run: Option<String>,
    #[serde(default)]
    pub source: CaptureSource,
    pub started_at: u64,
    #[serde(default)]
    pub run_started_at: Option<u64>,
    #[serde(default)]
    pub offset_ms: u64,
    pub ended_at: Option<u64>,
    pub phase: Phase,
    pub error: Option<String>,
}
impl State {
    pub fn dormant(id: String) -> Self {
        Self {
            id,
            run: None,
            source: CaptureSource::default(),
            started_at: 0,
            run_started_at: None,
            offset_ms: 0,
            ended_at: None,
            phase: Phase::Stopped,
            error: None,
        }
    }
    pub fn starting(id: String, run: String, source: CaptureSource) -> Self {
        let start = now_ms();
        Self {
            run: Some(run),
            source,
            started_at: start,
            run_started_at: Some(start),
            phase: Phase::Starting,
            ..Self::dormant(id)
        }
    }
    pub fn duration_ms(&self) -> u64 {
        let start = self.run_started_at.unwrap_or(self.started_at);
        let end = self
            .ended_at
            .unwrap_or_else(|| if self.phase.active() { now_ms() } else { start });
        self.offset_ms.saturating_add(end.saturating_sub(start))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Segment {
    pub sequence: u64,
    pub source: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Deserialize)]
struct SegmentLine<'a> {
    sequence: u64,
    #[serde(borrow)]
    source: Cow<'a, str>,
    start_ms: u64,
    end_ms: u64,
    #[serde(borrow)]
    text: Cow<'a, str>,
}

impl From<SegmentLine<'_>> for Segment {
    fn from(line: SegmentLine<'_>) -> Self {
        Self {
            sequence: line.sequence,
            source: line.source.into_owned(),
            start_ms: line.start_ms,
            end_ms: line.end_ms,
            text: line.text.into_owned(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transcript {
    pub state: State,
    pub segments: Vec<Segment>,
}
impl Transcript {
    pub(crate) fn sequence(&self) -> u64 {
        self.segments.last().map_or(0, |segment| segment.sequence)
    }
}
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
pub(crate) fn directory(base: &Path, id: &str) -> Result<PathBuf, String> {
    if !valid_id(id) {
        return Err("Invalid recording id".into());
    }
    Ok(base.join("recordings").join(id))
}
pub(crate) fn atomic_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let dir = parent(path, true)?;
    atomic_json_in(&dir, name(path)?, value)
}

fn name(path: &Path) -> Result<&OsStr, String> {
    path.file_name()
        .ok_or_else(|| "Missing meeting filename".into())
}

fn parent(path: &Path, create: bool) -> Result<Directory, String> {
    Directory::open(path.parent().ok_or("Missing data directory")?, create)
        .map_err(|error| error.to_string())
}

fn atomic_json_in(dir: &Directory, name: &OsStr, value: &impl Serialize) -> Result<(), String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let temporary = format!(
        ".write-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let temporary = OsStr::new(&temporary);
    let mut file = dir
        .file(temporary, OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL)
        .map_err(|error| error.to_string())?;
    let result = (|| {
        let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .and_then(|_| dir.publish(temporary, name))
            .map_err(|error| error.to_string())
    })();
    if result.is_err() {
        dir.remove(temporary);
    }
    result
}

pub(crate) fn read_json<T: serde::de::DeserializeOwned>(
    path: &Path,
    limit: usize,
) -> Result<T, String> {
    read_json_in(&parent(path, false)?, name(path)?, limit)
}

fn read_json_in<T: serde::de::DeserializeOwned>(
    dir: &Directory,
    name: &OsStr,
    limit: usize,
) -> Result<T, String> {
    let file = dir
        .file(name, OFlags::RDONLY)
        .map_err(|error| error.to_string())?;
    read_json_file(file, limit)
}

fn read_json_file<T: serde::de::DeserializeOwned>(file: File, limit: usize) -> Result<T, String> {
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > limit {
        return Err("Saved meeting data exceeds its limit".into());
    }
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

fn read_state(dir: &Directory, id: &str) -> Result<Option<State>, String> {
    let file = match dir.file(OsStr::new("state.json"), OFlags::RDONLY) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let state: State = read_json_file(file, 16 * 1024)?;
    if state.id != id || state.run.as_ref().is_some_and(|run| !valid_id(run)) {
        return Err("Recording state has an invalid identity".into());
    }
    Ok(Some(state))
}

fn lock_owner(dir: &Directory, flags: OFlags) -> Result<File, String> {
    let lock = dir
        .file(OsStr::new("owner.lock"), OFlags::RDWR | flags)
        .map_err(|error| error.to_string())?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let error = std::io::Error::last_os_error();
        return Err(if error.kind() == std::io::ErrorKind::WouldBlock {
            "Recording already has an owner".into()
        } else {
            error.to_string()
        });
    }
    Ok(lock)
}

/// Publish a dormant recording root after its metadata and empty log exist.
pub(crate) fn initialize(base: &Path, id: &str) -> Result<(), String> {
    let path = directory(base, id)?;
    let dir = Directory::open(&path, true).map_err(|error| error.to_string())?;
    if read_state(&dir, id)?.is_some() {
        return Ok(());
    }
    let _owner = lock_owner(&dir, OFlags::CREATE)?;
    if read_state(&dir, id)?.is_some() {
        return Ok(());
    }
    let log = dir
        .file(
            OsStr::new("transcript.jsonl"),
            OFlags::WRONLY | OFlags::CREATE,
        )
        .map_err(|error| error.to_string())?;
    if log.metadata().map_err(|error| error.to_string())?.len() != 0 {
        return Err("Recording state is missing for an existing transcript".into());
    }
    log.sync_all().map_err(|error| error.to_string())?;
    atomic_json_in(&dir, OsStr::new("state.json"), &State::dormant(id.into()))
}

pub struct Owner {
    directory: Directory,
    _lock: File,
    log: File,
    pub state: State,
    bytes: usize,
    next: u64,
    rendered_bytes: Option<usize>,
}
impl Owner {
    pub fn create(base: &Path, id: &str) -> Result<Self, String> {
        let directory = directory(base, id)?;
        // Reusing an ID is rejected. A Start with an uncertain outcome is never replayed.
        let directory = parent(&directory, true)?
            .create_child(OsStr::new(id))
            .map_err(|error| error.to_string())?;
        let lock = lock_owner(&directory, OFlags::CREATE | OFlags::EXCL)?;
        let log = directory
            .file(
                OsStr::new("transcript.jsonl"),
                OFlags::WRONLY | OFlags::APPEND | OFlags::CREATE | OFlags::EXCL,
            )
            .map_err(|error| error.to_string())?;
        let state = State {
            started_at: now_ms(),
            phase: Phase::Starting,
            ..State::dormant(id.into())
        };
        log.sync_all().map_err(|error| error.to_string())?;
        atomic_json_in(&directory, OsStr::new("state.json"), &state)?;
        Ok(Self {
            directory,
            _lock: lock,
            log,
            state,
            bytes: 0,
            next: 0,
            rendered_bytes: None,
        })
    }

    pub(crate) fn start(
        base: &Path,
        id: &str,
        run: &str,
        previous_run: Option<&str>,
        source: CaptureSource,
        before_publish: impl FnOnce(&[Segment]) -> Result<Option<usize>, String>,
    ) -> Result<Self, String> {
        if !valid_id(run) || previous_run == Some(run) {
            return Err("Invalid recording run identity".into());
        }
        let directory =
            Directory::open(&directory(base, id)?, false).map_err(|error| error.to_string())?;
        let lock = lock_owner(&directory, OFlags::empty())?;
        let previous = read_state(&directory, id)?.ok_or("Missing recording state")?;
        if previous.run.as_deref() != previous_run {
            return Err("The recording changed before this take could start".into());
        }
        let log = directory
            .file(
                OsStr::new("transcript.jsonl"),
                OFlags::RDWR | OFlags::APPEND,
            )
            .map_err(|error| error.to_string())?;
        let (segments, bytes, complete) = read_segments(&log)?;
        let next = segments.len() as u64;
        let rendered_bytes = before_publish(&segments)?;
        if !complete {
            log.set_len(bytes as u64)
                .and_then(|_| log.sync_all())
                .map_err(|error| error.to_string())?;
        }
        let mut state = State::starting(id.into(), run.into(), source);
        if previous.started_at != 0 {
            state.started_at = previous.started_at;
        }
        // An interrupted run has no trustworthy wall-clock endpoint.
        let duration = if previous.phase.active() {
            previous.offset_ms
        } else {
            previous.duration_ms()
        };
        state.offset_ms = segments
            .iter()
            .fold(duration, |end, segment| end.max(segment.end_ms));
        atomic_json_in(&directory, OsStr::new("state.json"), &state)?;
        Ok(Self {
            directory,
            _lock: lock,
            log,
            state,
            bytes,
            next,
            rendered_bytes,
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
        atomic_json_in(&self.directory, OsStr::new("state.json"), &next)?;
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
            || !self.state.source.allows(source)
            || end_ms < start_ms
            || text.len() > 64 * 1024
        {
            return Err("Invalid transcript segment".into());
        }
        let shifted = |time: u64| {
            time.checked_add(self.state.offset_ms)
                .ok_or("Transcript timestamp exceeds its limit")
        };
        let segment = Segment {
            sequence: self.next + 1,
            source: source.into(),
            start_ms: shifted(start_ms)?,
            end_ms: shifted(end_ms)?,
            text,
        };
        let mut bytes = serde_json::to_vec(&segment).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        let rendered_bytes = self
            .rendered_bytes
            .map(|used| used + usize::from(used != 0) * 2 + crate::render_segment(&segment).len());
        if self.bytes + bytes.len() > MAX_TRANSCRIPT
            || rendered_bytes.is_some_and(|used| used > MAX_TRANSCRIPT)
        {
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
        self.rendered_bytes = rendered_bytes;
        Ok(segment)
    }
}
/// Recovery acquires the same owner lock before interpreting an active root.
/// A live recorder always wins; missing/unknown files never mean a finished turn.
pub fn state(base: &Path, id: &str) -> Result<State, String> {
    let dir = Directory::open(&directory(base, id)?, false).map_err(|error| error.to_string())?;
    state_in(&dir, id)
}

fn state_in(dir: &Directory, id: &str) -> Result<State, String> {
    let lock = dir
        .file(OsStr::new("owner.lock"), OFlags::RDWR)
        .map_err(|error| error.to_string())?;
    let result = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::WouldBlock {
            return Err(error.to_string());
        }
    }
    let mut state = read_state(dir, id)?.ok_or("Missing recording state")?;
    if result == 0 && state.phase.active() {
        state.phase = Phase::Interrupted;
        state.ended_at = None;
        state.error =
            Some("Recording was interrupted. All saved transcript segments are retained.".into());
        atomic_json_in(dir, OsStr::new("state.json"), &state)?;
    }
    Ok(state)
}

pub fn read(base: &Path, id: &str) -> Result<Transcript, String> {
    let dir = Directory::open(&directory(base, id)?, false).map_err(|error| error.to_string())?;
    let state = state_in(&dir, id)?;
    let file = dir
        .file(OsStr::new("transcript.jsonl"), OFlags::RDONLY)
        .map_err(|error| error.to_string())?;
    let (segments, _, _) = read_segments(&file)?;
    Ok(Transcript { state, segments })
}

pub(crate) fn has_saved_text(
    base: &Path,
    id: &str,
    correction: Option<&str>,
    override_sequence: Option<u64>,
    saved_sequence: Option<u64>,
) -> Result<bool, String> {
    let dir = Directory::open(&directory(base, id)?, false).map_err(|error| error.to_string())?;
    let file = dir
        .file(OsStr::new("transcript.jsonl"), OFlags::RDONLY)
        .map_err(|error| error.to_string())?;
    let after = match correction {
        // A legacy correction covers the full captured prefix.
        Some(_) => override_sequence.unwrap_or(u64::MAX),
        None => 0,
    };
    let mut has_text = correction.is_some_and(|text| !text.trim().is_empty());
    let mut last = 0;
    scan_segments(&file, |segment| {
        last = segment.sequence;
        has_text |= segment.sequence > after && !segment.text.trim().is_empty();
    })?;
    if saved_sequence.is_some_and(|expected| last < expected) {
        return Err("Saved transcript continuity was lost".into());
    }
    Ok(has_text)
}

fn read_segments(file: &File) -> Result<(Vec<Segment>, usize, bool), String> {
    let mut segments = Vec::new();
    let (end, complete) = scan_segments(file, |segment| segments.push(segment.into()))?;
    Ok((segments, end, complete))
}

fn scan_segments(
    file: &File,
    mut visit: impl FnMut(SegmentLine<'_>),
) -> Result<(usize, bool), String> {
    let mut reader = BufReader::new(file.take(MAX_TRANSCRIPT as u64 + 1));
    let mut line = Vec::new();
    let mut end = 0;
    let mut expected = 1;
    loop {
        line.clear();
        let count = reader
            .read_until(b'\n', &mut line)
            .map_err(|e| e.to_string())?;
        if count == 0 {
            return Ok((end, true));
        }
        if end + count > MAX_TRANSCRIPT {
            return Err("Transcript exceeds its storage limit".into());
        }
        if line.pop() != Some(b'\n') {
            // Only an incomplete trailing write may be ignored.
            return Ok((end, false));
        }
        end += count;
        if line.is_empty() {
            continue;
        }
        let segment: SegmentLine<'_> = serde_json::from_slice(&line).map_err(|e| e.to_string())?;
        if segment.sequence != expected {
            return Err("Transcript segment continuity was lost".into());
        }
        expected += 1;
        visit(segment);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
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
    fn resume_truncates_only_a_partial_tail_and_advances_the_sequence() {
        let dir = tempfile::tempdir().unwrap();
        initialize(dir.path(), "sample").unwrap();
        let mut owner = Owner::start(
            dir.path(),
            "sample",
            "first",
            None,
            CaptureSource::Microphone,
            |_| Ok(None),
        )
        .unwrap();
        let first = owner
            .append("microphone", 0, 500, "Before a crash.".into())
            .unwrap();
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
        let recovered = state(dir.path(), "sample").unwrap();
        assert_eq!(recovered.phase, Phase::Interrupted);
        assert_eq!(recovered.run.as_deref(), Some("first"));
        let mut resumed = Owner::start(
            dir.path(),
            "sample",
            "second",
            Some("first"),
            CaptureSource::Microphone,
            |_| Ok(None),
        )
        .unwrap();
        let second = resumed
            .append("microphone", 0, 200, "After a crash.".into())
            .unwrap();
        assert_eq!(second.sequence, 2);
        assert_eq!(second.start_ms, first.end_ms);
        resumed.phase(Phase::Stopped, None).unwrap();
        drop(resumed);
        let transcript = read(dir.path(), "sample").unwrap();
        assert_eq!(transcript.state.run.as_deref(), Some("second"));
        assert_eq!(transcript.segments, vec![first, second]);
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
