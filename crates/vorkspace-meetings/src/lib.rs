//! Local meeting documents, transcript storage, and summary generation.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    ffi::OsStr,
    fmt::Write as _,
    fs::File,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
};
mod catalog;
mod directory;
mod draft;
mod summary;
pub use catalog::NoteCandidate;
use draft::Receipt;
pub use draft::{DraftWrite, EditLease};
pub use summary::{DEFAULT_PROMPT, Settings, Summary};
use summary::{MAX_SETTINGS_BYTES, MAX_SUMMARY_BYTES};
pub mod recording;
use recording::{State, Transcript, atomic_json, read_json};
const MAX_TITLE_CHARS: usize = 200;
// JSON may encode a single ASCII control byte as a six-byte escape. Bound
// every edit field before allowing it into the draft or document store.
pub const MAX_EDIT_MESSAGE_BYTES: usize =
    6 * (recording::MAX_TRANSCRIPT + MAX_SUMMARY_BYTES + 4 * MAX_TITLE_CHARS) + 4096;
// The canonical document also includes provider and model values, whose
// encoded size is bounded by the settings reader.
const LIMIT: usize = MAX_EDIT_MESSAGE_BYTES + MAX_SETTINGS_BYTES;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Meeting {
    pub id: String,
    pub title: String,
    pub revision: u64,
    pub transcript_revision: u64,
    pub transcript_override: Option<String>,
    pub summary: Option<Summary>,
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    title_set: bool,
    /// The raw transcript sequence already represented by both revisions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transcript_sequence: Option<u64>,
    /// A correction replaces the captured prefix through this sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    override_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    applied_draft: Option<Receipt>,
}
impl Meeting {
    fn adopt_sequence(&mut self, sequence: u64) -> Result<(), String> {
        let delta = sequence
            .checked_sub(self.transcript_sequence.unwrap_or(sequence))
            .ok_or("Saved transcript continuity was lost")?;
        self.revision = self
            .revision
            .checked_add(delta)
            .ok_or("Meeting revision exhausted")?;
        self.transcript_revision = self
            .transcript_revision
            .checked_add(delta)
            .ok_or("Transcript revision exhausted")?;
        self.transcript_sequence = Some(sequence);
        if self.transcript_override.is_some() && self.override_sequence.is_none() {
            // A legacy override represented all text captured before this take.
            self.override_sequence = Some(sequence);
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct Document {
    meeting: Meeting,
    state: State,
    transcript: String,
    segments: Vec<recording::Segment>,
    sequence: u64,
}

fn named(title: &str, title_set: bool) -> bool {
    let title = title.trim();
    !title.is_empty() && (title_set || title != "Untitled meeting")
}

fn has_transcript(
    transcript: &Transcript,
    correction: Option<&str>,
    override_sequence: Option<u64>,
) -> bool {
    let after = if correction.is_some() {
        override_sequence.unwrap_or(transcript.sequence())
    } else {
        0
    };
    correction.is_some_and(|text| !text.trim().is_empty())
        || transcript
            .segments
            .iter()
            .any(|segment| segment.sequence > after && !segment.text.trim().is_empty())
}
#[derive(Clone)]
pub struct Meetings {
    pub directory: PathBuf,
}
struct Lock(File);
impl Lock {
    fn acquire(path: &Path, wait: bool) -> Result<Self, String> {
        Self::open(path, wait).map_err(|error| {
            if error.kind() == std::io::ErrorKind::WouldBlock {
                "This meeting is already being summarized".into()
            } else {
                error.to_string()
            }
        })
    }

    fn open(path: &Path, wait: bool) -> std::io::Result<Self> {
        let invalid_path = || {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid meeting lock path",
            )
        };
        let dir = directory::Directory::open(path.parent().ok_or_else(invalid_path)?, true)?;
        let file = dir.file(
            path.file_name().ok_or_else(invalid_path)?,
            rustix::fs::OFlags::RDWR | rustix::fs::OFlags::CREATE,
        )?;
        if unsafe {
            libc::flock(
                file.as_raw_fd(),
                libc::LOCK_EX | if wait { 0 } else { libc::LOCK_NB },
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self(file))
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|e| e.to_string())
}
fn value(value: impl Serialize) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|e| e.to_string())
}
fn required<'a>(params: &'a Value, key: &str) -> Result<&'a str, String> {
    params[key].as_str().ok_or_else(|| format!("Missing {key}"))
}
impl Meetings {
    fn metadata_path(&self, id: &str) -> Result<PathBuf, String> {
        if !valid_id(id) {
            return Err("Invalid meeting id".into());
        }
        Ok(self.directory.join("meetings").join(format!("{id}.json")))
    }
    fn lock(&self) -> Result<Lock, String> {
        Lock::acquire(&self.directory.join("meetings.lock"), true)
    }
    fn metadata(&self, id: &str) -> Result<Meeting, String> {
        let meeting: Meeting = read_json(&self.metadata_path(id)?, LIMIT)?;
        if meeting.id != id {
            return Err("Meeting metadata has a different recording ID".into());
        }
        Ok(meeting)
    }
    fn save(&self, meeting: &Meeting) -> Result<(), String> {
        atomic_json(&self.metadata_path(&meeting.id)?, meeting)
    }
    pub fn read(&self, id: &str) -> Result<Value, String> {
        value(self.document(id)?)
    }
    fn document(&self, id: &str) -> Result<Document, String> {
        let _lock = self.lock()?;
        self.read_document(self.metadata(id)?)
    }
    fn read_document(&self, mut meeting: Meeting) -> Result<Document, String> {
        let transcript = recording::read(&self.directory, &meeting.id)?;
        let last = transcript.sequence();
        meeting.adopt_sequence(last)?;
        let text = match &meeting.transcript_override {
            Some(prefix) => {
                let after = meeting.override_sequence.unwrap_or(last);
                let suffix =
                    render_segments(transcript.segments.iter().filter(|s| s.sequence > after));
                if suffix.is_empty() {
                    prefix.clone()
                } else if prefix.is_empty() {
                    suffix
                } else {
                    format!("{prefix}\n\n{suffix}")
                }
            }
            None => transcript_markdown(&transcript),
        };
        Ok(Document {
            meeting,
            state: transcript.state,
            transcript: text,
            segments: transcript.segments,
            sequence: last,
        })
    }
    pub fn update(&self, params: &Value) -> Result<Value, String> {
        let id = required(params, "id")?;
        let _lock = self.lock()?;
        self.update_locked(self.metadata(id)?, params, None)
    }
    fn update_locked(
        &self,
        mut meeting: Meeting,
        params: &Value,
        applied_draft: Option<String>,
    ) -> Result<Value, String> {
        let expected = params["revision"]
            .as_u64()
            .ok_or("Missing meeting revision")?;
        let transcript = recording::read(&self.directory, &meeting.id)?;
        meeting.adopt_sequence(transcript.sequence())?;
        let blank_checkpoint = applied_draft.is_some()
            && transcript.state.started_at == 0
            && !named(&meeting.title, meeting.title_set)
            && !has_transcript(
                &transcript,
                meeting.transcript_override.as_deref(),
                meeting.override_sequence,
            )
            && meeting.summary.is_none();
        if meeting.revision != expected {
            return Err("This meeting changed in another view. Reload before saving.".into());
        }
        if let Some(title) = params["title"].as_str() {
            if title.chars().count() > MAX_TITLE_CHARS {
                return Err("Meeting title must be 200 characters or fewer".into());
            }
            meeting.title = title.trim().into();
            meeting.title_set = !meeting.title.is_empty();
        }
        if params.get("transcript").is_some() {
            if transcript.state.phase.active() {
                return Err("Stop recording before editing its transcript".into());
            }
            let text = if params["transcript"].is_null() {
                None
            } else {
                Some(required(params, "transcript")?.to_owned())
            };
            if text
                .as_ref()
                .is_some_and(|text| text.len() > recording::MAX_TRANSCRIPT)
            {
                return Err("Transcript is too large".into());
            }
            meeting.override_sequence = text.as_ref().map(|_| transcript.sequence());
            meeting.transcript_override = text;
            meeting.transcript_revision += 1;
        }
        if !blank_checkpoint
            && !named(&meeting.title, meeting.title_set)
            && !has_transcript(
                &transcript,
                meeting.transcript_override.as_deref(),
                meeting.override_sequence,
            )
            && meeting.summary.is_none()
        {
            return Err("Enter a title or transcript before saving this meeting.".into());
        }
        if let Some(text) = params["summary"].as_str() {
            if text.len() > MAX_SUMMARY_BYTES {
                return Err("Summary is too large".into());
            }
            let Some(summary) = &mut meeting.summary else {
                return Err("Generate a summary first".into());
            };
            summary.text = text.into();
        }
        meeting.revision += 1;
        if let Some(token) = applied_draft {
            meeting.applied_draft = Some(Receipt {
                version: token,
                revision: meeting.revision,
            });
        }
        self.save(&meeting)?;
        value(meeting)
    }
    pub fn delete(&self, id: &str, expected_revision: u64) -> Result<(), String> {
        self.delete_with_commit(id, Some(expected_revision), || Ok(()))
    }

    /// Validate capture ownership and revision before committing the owner's
    /// durable deletion. File removal follows that commit and is retryable.
    /// `None` is reserved for cleanup authorized by an existing tombstone.
    pub fn delete_with_commit(
        &self,
        id: &str,
        expected_revision: Option<u64>,
        commit: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        use rustix::fs::{AtFlags, OFlags};
        use std::{io::ErrorKind, os::unix::ffi::OsStrExt};

        let metadata = self.metadata_path(id)?;
        let _lock = self.lock()?;
        let open = |result: std::io::Result<directory::Directory>| match result {
            Ok(dir) => Ok(Some(dir)),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.to_string()),
        };
        let remove_document = || {
            if let Some(documents) = open(directory::Directory::open(
                &self.directory.join("meetings"),
                false,
            ))? {
                documents
                    .unlink(
                        metadata.file_name().ok_or("Invalid meeting filename")?,
                        AtFlags::empty(),
                    )
                    .map_err(|error| error.to_string())?;
            }
            self.remove_draft(id)
        };
        let Some(recordings) = open(directory::Directory::open(
            &self.directory.join("recordings"),
            false,
        ))?
        else {
            commit()?;
            return remove_document();
        };
        let retired = format!(".deleted-{id}");
        let retired = OsStr::new(&retired);
        let live = open(recordings.child(OsStr::new(id)))?;
        let dir = if let Some(dir) = open(recordings.child(retired))? {
            if live.is_some() {
                return Err("Meeting deletion found both a live and retired recording.".into());
            }
            commit()?;
            dir
        } else {
            let Some(dir) = live else {
                commit()?;
                return remove_document();
            };
            let owner = dir
                .file(OsStr::new("owner.lock"), OFlags::RDWR)
                .map_err(|error| error.to_string())?;
            if unsafe { libc::flock(owner.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                let error = std::io::Error::last_os_error();
                return Err(if error.kind() == ErrorKind::WouldBlock {
                    "Stop recording before deleting this meeting.".into()
                } else {
                    error.to_string()
                });
            }
            if let Some(expected_revision) = expected_revision {
                let mut meeting = self.metadata(id)?;
                meeting.adopt_sequence(recording::read(&self.directory, id)?.sequence())?;
                if meeting.revision != expected_revision {
                    return Err(
                        "This meeting changed. Review the latest version before deleting it."
                            .into(),
                    );
                }
            }
            commit()?;
            // Retire the recording root before removing either data file. Its
            // reserved name makes cleanup retryable without republishing it.
            recordings
                .publish(OsStr::new(id), retired)
                .map_err(|error| error.to_string())?;
            dir
        };
        remove_document()?;
        for entry in dir.entries().map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let name = entry.file_name().to_bytes();
            if name != b"." && name != b".." {
                dir.unlink(OsStr::from_bytes(name), AtFlags::empty())
                    .map_err(|error| error.to_string())?;
            }
        }
        recordings
            .unlink(retired, AtFlags::REMOVEDIR)
            .map_err(|error| error.to_string())
    }
    pub fn prepare(&self, title: &str) -> Result<String, String> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let title = title.trim().chars().take(MAX_TITLE_CHARS).collect();
        let meeting = Self::new_meeting(&id, title);
        {
            let _lock = self.lock()?;
            self.save(&meeting)?;
        }
        Ok(id)
    }

    fn new_meeting(id: &str, title: String) -> Meeting {
        Meeting {
            id: id.into(),
            title_set: !title.is_empty(),
            title,
            revision: 0,
            transcript_revision: 0,
            transcript_override: None,
            summary: None,
            created_at: recording::now_ms(),
            transcript_sequence: None,
            override_sequence: None,
            applied_draft: None,
        }
    }

    /// Materialize a native draft on its first meaningful edit or recording attempt.
    pub fn prepare_document(&self, id: &str) -> Result<(), String> {
        let path = self.metadata_path(id)?;
        let _lock = self.lock()?;
        match std::fs::symlink_metadata(path) {
            Ok(_) => {
                self.metadata(id)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.save(&Self::new_meeting(id, String::new()))?;
            }
            Err(error) => return Err(error.to_string()),
        }
        recording::initialize(&self.directory, id)
    }

    /// Admit one new take against the last durable run, under the same lock as
    /// transcript edits and draft publication. Raw speech appends independently.
    pub fn start_recording(
        &self,
        id: &str,
        run: &str,
        previous_run: Option<&str>,
        source: recording::CaptureSource,
    ) -> Result<recording::Owner, String> {
        if !valid_id(id) || !valid_id(run) || previous_run.is_some_and(|run| !valid_id(run)) {
            return Err("Invalid recording identity".into());
        }
        let _lock = self.lock()?;
        let mut meeting = self.metadata(id)?;
        recording::initialize(&self.directory, id)?;
        if self.has_pending_draft(id, meeting.applied_draft.as_ref())? {
            return Err("Save or discard this meeting's draft before recording.".into());
        }
        // Establish the legacy prefix watermark before the new state admits appends.
        recording::Owner::start(&self.directory, id, run, previous_run, source, |segments| {
            let sequence = segments.last().map_or(0, |segment| segment.sequence);
            meeting.adopt_sequence(sequence)?;
            // Raw JSONL and the editable document have separate byte limits.
            // A large corrected prefix leaves less space for resumed speech.
            let rendered_bytes = meeting.transcript_override.as_ref().map(|prefix| {
                segments
                    .iter()
                    .filter(|segment| {
                        segment.sequence > meeting.override_sequence.unwrap_or(sequence)
                    })
                    .fold(prefix.len(), |used, segment| {
                        used + usize::from(used != 0) * 2 + render_segment(segment).len()
                    })
            });
            self.save(&meeting)?;
            Ok(rendered_bytes)
        })
    }
    pub fn open(root: &Path) -> Result<Self, String> {
        let current = root.join("meetings");
        let legacy_root = root.join("plugins");
        let legacy = legacy_root.join("meetings");
        let legacy_data = legacy.join("data");
        // Keep transcripts and summary settings from the previous Meetings build.
        let directory = if real_directory(&current)? {
            current
        } else if real_directory(&legacy_root)?
            && real_directory(&legacy)?
            && real_directory(&legacy_data)?
        {
            legacy_data
        } else {
            current
        };
        Ok(Self { directory })
    }
}

fn real_directory(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(true),
        Ok(_) => Err("Meeting data paths must be real directories".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"-._".contains(&c))
        && !id.starts_with('.')
        && !id.ends_with('.')
        && !id.contains("..")
}
fn transcript_markdown(transcript: &Transcript) -> String {
    render_segments(transcript.segments.iter())
}
fn render_segments<'a>(segments: impl Iterator<Item = &'a recording::Segment>) -> String {
    let mut segments = segments.collect::<Vec<_>>();
    segments.sort_unstable_by_key(|s| (s.start_ms, s.sequence));
    let capacity = segments.iter().map(|s| s.text.trim().len() + 32).sum();
    let mut text = String::with_capacity(capacity);
    for segment in segments {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        write_segment(&mut text, segment);
    }
    text
}
fn render_segment(segment: &recording::Segment) -> String {
    let mut text = String::with_capacity(segment.text.len() + 32);
    write_segment(&mut text, segment);
    text
}
fn write_segment(text: &mut String, segment: &recording::Segment) {
    let s = segment.start_ms / 1000;
    write!(
        text,
        "[{:02}:{:02}:{:02}] **{}:** {}",
        s / 3600,
        (s / 60) % 60,
        s % 60,
        if segment.source == "microphone" {
            "Microphone"
        } else {
            "System audio"
        },
        segment.text.trim(),
    )
    .expect("writing to a String cannot fail");
}
#[cfg(test)]
mod tests;
