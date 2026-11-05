//! A meeting's recovery checkpoint. The document revision is still the only
//! authority for publishing edits to the saved meeting.
use super::*;
use rustix::fs::AtFlags;
use std::io::ErrorKind;

const DRAFT_CHANGED: &str = "The draft changed in another view. Reopen this meeting to review it.";

pub struct EditLease {
    _lock: Lock,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Receipt {
    pub(crate) version: String,
    pub(crate) revision: u64,
}

#[derive(Debug, Deserialize)]
pub struct DraftWrite {
    pub id: String,
    pub expected: Option<String>,
    pub version: String,
    draft: Draft,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Draft {
    revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transcript: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    view: DraftView,
    title_focused: bool,
    start: u32,
    end: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum DraftView {
    Transcript,
    Summary,
}

#[derive(Serialize, Deserialize)]
struct Slot {
    version: String,
    draft: Option<Draft>,
}

impl DraftWrite {
    pub fn from_params(params: Value) -> Result<Self, String> {
        let edit: Self = decode(params)?;
        let valid_version = |version: &str| {
            version.len() == 32
                && version
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        };
        if !valid_id(&edit.id)
            || !valid_version(&edit.version)
            || edit
                .expected
                .as_deref()
                .is_some_and(|version| !valid_version(version) || version == edit.version)
        {
            return Err("Invalid meeting draft identity".into());
        }
        let oversized =
            |text: &Option<String>, limit| text.as_ref().is_some_and(|text| text.len() > limit);
        if oversized(&edit.draft.title, 4 * MAX_TITLE_CHARS)
            || oversized(&edit.draft.transcript, recording::MAX_TRANSCRIPT)
            || oversized(&edit.draft.summary, MAX_SUMMARY_BYTES)
        {
            return Err("Meeting draft exceeds its size limit.".into());
        }
        Ok(edit)
    }

    /// The last eight digits are a fixed-width sequence within one iframe.
    /// Coalescing may skip an arbitrary number of intermediate checkpoints.
    pub fn follows(&self, version: &str) -> bool {
        follows(&self.version, version)
    }

    pub fn restore_pending(&self, document: &mut Value, error: String) -> Result<(), String> {
        document["draft"] = value(&self.draft)?;
        document["draft_version"] = json!(self.version);
        document["draft_error"] = json!(error);
        Ok(())
    }
}

fn follows(new: &str, old: &str) -> bool {
    new.get(..24) == old.get(..24) && new > old
}

impl Meetings {
    pub fn try_edit(&self, id: &str) -> Result<Option<EditLease>, String> {
        self.metadata_path(id)?;
        match Lock::open(
            &self.directory.join("drafts").join(format!("{id}.lock")),
            false,
        ) {
            Ok(lock) => Ok(Some(EditLease { _lock: lock })),
            Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    fn draft_path(&self, id: &str) -> Result<PathBuf, String> {
        if !valid_id(id) {
            return Err("Invalid meeting id".into());
        }
        Ok(self.directory.join("drafts").join(format!("{id}.json")))
    }

    fn draft_slot(&self, id: &str) -> Result<Option<Slot>, String> {
        let path = self.draft_path(id)?;
        match std::fs::symlink_metadata(&path) {
            Ok(_) => read_json(&path, MAX_EDIT_MESSAGE_BYTES).map(Some),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    pub(crate) fn has_pending_draft(
        &self,
        id: &str,
        applied: Option<&Receipt>,
    ) -> Result<bool, String> {
        Ok(self.draft_slot(id)?.is_some_and(|slot| {
            slot.draft.is_some() && applied.is_none_or(|receipt| receipt.version != slot.version)
        }))
    }

    // Outer None hides blank, absent and consumed checkpoints. Inner None is
    // a meaningful untitled draft, whose row uses the display placeholder.
    pub(crate) fn visible_draft_title(
        &self,
        id: &str,
        applied: Option<&Receipt>,
    ) -> Result<Option<Option<String>>, String> {
        let Some(slot) = self.draft_slot(id)? else {
            return Ok(None);
        };
        if applied.is_some_and(|receipt| receipt.version == slot.version) {
            return Ok(None);
        }
        let Some(draft) = slot.draft else {
            return Ok(None);
        };
        let nonblank = |text: &str| !text.trim().is_empty();
        let title = draft.title.filter(|text| nonblank(text));
        if title.is_some()
            || draft.transcript.as_deref().is_some_and(nonblank)
            || draft.summary.as_deref().is_some_and(nonblank)
        {
            Ok(Some(title))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn remove_draft(&self, id: &str) -> Result<(), String> {
        let path = self.draft_path(id)?;
        let dir = match directory::Directory::open(path.parent().unwrap(), false) {
            Ok(dir) => dir,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.to_string()),
        };
        dir.unlink(path.file_name().unwrap(), AtFlags::empty())
            .map_err(|error| error.to_string())
    }

    pub fn open_document(&self, id: &str) -> Result<Value, String> {
        let _lock = self.lock()?;
        let meeting = self.metadata(id)?;
        let slot = self.draft_slot(id)?;
        let applied = meeting
            .applied_draft
            .as_ref()
            .map(|receipt| receipt.version.clone());
        let mut result = self.read_document(meeting)?;
        if let Some(Slot { version, draft }) = slot {
            if applied.as_ref() != Some(&version) {
                result["draft"] = value(draft)?;
            }
            result["draft_version"] = json!(version);
        }
        Ok(result)
    }

    pub fn checkpoint_draft(&self, edit: &DraftWrite) -> Result<(), String> {
        let _lock = self.lock()?;
        let mut meeting = self.metadata(&edit.id)?;
        let transcript = recording::read(&self.directory, &edit.id)?;
        if transcript.state.phase.active() {
            return Err("Stop recording before editing this meeting.".into());
        }
        meeting.adopt_sequence(transcript.sequence())?;
        if edit.draft.revision > meeting.revision {
            return Err("Invalid meeting draft revision".into());
        }
        let slot = self.draft_slot(&edit.id)?;
        let receipt = meeting.applied_draft.as_ref();
        let applied = receipt.map(|receipt| &receipt.version);
        let active = slot
            .as_ref()
            .filter(|slot| slot.draft.is_some() && applied != Some(&slot.version));
        if active.is_some_and(|slot| slot.version == edit.version) {
            return Ok(());
        }
        let follows_save = slot
            .as_ref()
            .is_some_and(|slot| applied == Some(&slot.version) && edit.follows(&slot.version));
        if slot.as_ref().map(|slot| &slot.version) != edit.expected.as_ref()
            && !active.is_some_and(|slot| edit.follows(&slot.version))
            && !follows_save
        {
            return Err(DRAFT_CHANGED.into());
        }
        #[derive(Serialize)]
        struct Checkpoint<'a> {
            version: &'a str,
            draft: &'a Draft,
        }
        let rebased;
        let draft = if let Some(receipt) = receipt.filter(|receipt| {
            // A restored draft may have been written by a previous iframe.
            (edit.follows(&receipt.version) || edit.expected.as_ref() == Some(&receipt.version))
                && edit.draft.revision.checked_add(1) == Some(receipt.revision)
        }) {
            rebased = Draft {
                revision: receipt.revision,
                ..edit.draft.clone()
            };
            &rebased
        } else {
            &edit.draft
        };
        let checkpoint = Checkpoint {
            version: &edit.version,
            draft,
        };
        atomic_json(&self.draft_path(&edit.id)?, &checkpoint)
    }

    pub fn save_draft(&self, id: &str, version: &str) -> Result<Value, String> {
        let _lock = self.lock()?;
        let meeting = self.metadata(id)?;
        let slot = self.draft_slot(id)?;
        if slot
            .as_ref()
            .is_some_and(|slot| slot.version != version && slot.draft.is_some())
        {
            return Err("More edits arrived while saving. Save again to apply them.".into());
        }
        let slot = slot.filter(|slot| slot.version == version);
        let draft = slot.and_then(|slot| slot.draft).ok_or(DRAFT_CHANGED)?;
        let saved = self.update_locked(meeting, &value(draft)?, Some(version.into()))?;
        // A crash or cleanup error leaves a consumed slot. The receipt was
        // published atomically with the document, so it cannot reopen as input.
        let _ = atomic_json(
            &self.draft_path(id)?,
            &Slot {
                version: version.into(),
                draft: None,
            },
        );
        Ok(saved)
    }

    pub fn discard_draft(&self, id: &str, version: &str) -> Result<(), String> {
        let _lock = self.lock()?;
        self.metadata(id)?;
        if self
            .draft_slot(id)?
            .is_none_or(|slot| slot.version != version)
        {
            return Ok(());
        }
        let tombstone = Slot {
            version: uuid::Uuid::new_v4().simple().to_string(),
            draft: None,
        };
        atomic_json(&self.draft_path(id)?, &tombstone)
    }
}
