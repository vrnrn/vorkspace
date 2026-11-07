//! Read-only projection of native CLI conversations. Provider files own context;
//! the Store owns presentation, import receipts and the incremental read cursor.
use crate::{accounts::Profile, kind::Kind, transcript::Transcript};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

mod parse;
#[cfg(test)]
pub(crate) mod tests;

const LINE_LIMIT: u64 = 16 << 20;
const BATCH_LIMIT: u64 = 32 << 20;
const TEXT_LIMIT: usize = 8 << 20;

/// The same queue identity is used by Claude's JSONL and our turn receipt.
pub fn input_uuid(id: &str) -> String {
    uuid::Uuid::parse_str(id)
        .unwrap_or_else(|_| {
            let hash = Sha256::digest(id.as_bytes());
            uuid::Uuid::from_bytes(hash[..16].try_into().unwrap())
        })
        .to_string()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub provider: String,
    pub account: String,
    pub thread: String,
    pub home: PathBuf,
}
impl Binding {
    pub fn for_bot(
        provider: &str,
        thread: &str,
        account: &Profile,
        root: &Path,
    ) -> Result<Option<Self>, String> {
        let kind = Kind::from_id(provider);
        if !matches!(kind, Kind::Codex | Kind::Claude) {
            return Ok(None);
        }
        let home = if kind == Kind::Codex {
            crate::bot::codex_home(root, &account.id)?
        } else {
            None
        }
        .map(Ok)
        .unwrap_or_else(|| account.home(kind))?;
        Ok(Some(Self {
            provider: provider.into(),
            account: account.id.clone(),
            thread: thread.into(),
            home,
        }))
    }
    pub fn key(&self) -> String {
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(self).expect("serializable binding"))
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Stamp {
    device: u64,
    inode: u64,
    len: u64,
    modified: (i64, i64),
}
fn stamp(path: &Path) -> Result<Stamp, String> {
    let m = path.symlink_metadata().map_err(|e| e.to_string())?;
    if !m.is_file() {
        return Err("Native transcript is not an ordinary file".into());
    }
    Ok(Stamp {
        device: m.dev(),
        inode: m.ino(),
        len: m.len(),
        modified: (m.mtime(), m.mtime_nsec()),
    })
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Cursor {
    path: PathBuf,
    stamps: Vec<Stamp>,
    offset: u64,
    /// Last bytes before the cursor detect replacement/truncation at the read boundary.
    anchor: Vec<u8>,
    verified: bool,
    caught_up: bool,
    pending: Option<Turn>,
    /// Older cursors lack record IDs for their pending Claude messages.
    #[serde(default)]
    claude_message_receipts: bool,
    pub compaction: Option<String>,
    #[serde(default)]
    pub context: Option<crate::context::Usage>,
}
impl Cursor {
    pub(crate) fn legacy_claude_published(&self, turn_id: &str, message: &Message) -> bool {
        // A completed legacy cursor kept the UUIDs of its published response
        // even after clearing its visible messages. Use these if a rewrite
        // discarded the earlier receipt's leaf record.
        !self.claude_message_receipts
            && self.caught_up
            && self.pending.as_ref().is_some_and(|turn| {
                turn.id == turn_id
                    && turn.complete
                    && turn.messages.is_empty()
                    && message
                        .claude_uuid
                        .as_ref()
                        .is_some_and(|id| id == turn_id || turn.item_ids.contains(id))
            })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub text: String,
    pub at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claude_uuid: Option<String>,
}
impl Message {
    pub(crate) fn claude_receipt(&self) -> Option<String> {
        self.claude_uuid
            .as_ref()
            .map(|uuid| format!("claude-message:{uuid}"))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ClaudeRecord {
    pub uuid: String,
    /// The previous importer could publish this prefix under the leaf UUID.
    pub messages: usize,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Turn {
    pub id: String,
    pub started_at: Option<u64>,
    pub completed_at: Option<u64>,
    pub model: Option<String>,
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) claude_records: Vec<ClaudeRecord>,
    #[serde(default)]
    item_ids: std::collections::HashSet<String>,
    #[serde(default)]
    event_messages: Vec<Message>,
    #[serde(default)]
    complete: bool,
}
impl Turn {
    fn push(
        &mut self,
        role: &str,
        text: String,
        at: Option<u64>,
        claude_uuid: Option<&str>,
    ) -> Result<(), String> {
        let bytes: usize = self
            .messages
            .iter()
            .chain(&self.event_messages)
            .map(|m| m.text.len())
            .sum();
        if bytes.saturating_add(text.len()) > TEXT_LIMIT {
            return Err(
                "Native turn exceeds the transcript text limit; its source is preserved.".into(),
            );
        }
        if !text.is_empty() {
            self.messages.push(Message {
                role: role.into(),
                text,
                at,
                claude_uuid: claude_uuid.map(str::to_owned),
            });
        }
        Ok(())
    }
    pub fn transcript(&self) -> Transcript {
        let mut transcript = Transcript::default();
        for message in &self.messages {
            let at = transcript.begin(&message.role);
            transcript.rows[at].sent_at = message.at;
            transcript.append(&message.text);
        }
        transcript
    }
}

pub struct Batch {
    pub cursor: Cursor,
    pub turns: Vec<Turn>,
    pub changed: bool,
    pub more: bool,
    pub active: bool,
}

/// Incremental, bounded reads. No provider process, credentials, or API calls.
/// A malformed complete record fails without advancing the durable cursor;
/// an incomplete last line waits for the next append.
pub fn read(binding: &Binding, previous: Option<&Cursor>) -> Result<Option<Batch>, String> {
    let kind = Kind::from_id(&binding.provider);
    let mut path = match previous.map(|c| &c.path).filter(|p| p.is_file()) {
        Some(path) => path.clone(),
        None => match crate::accounts::find_native_thread(&binding.home, kind, &binding.thread)? {
            Some(path) => path,
            None => return Ok(None),
        },
    };
    if kind == Kind::Codex
        && path.extension().is_some_and(|ext| ext == "jsonl")
        && path.with_extension("jsonl.zst").is_file()
    {
        path = path.with_extension("jsonl.zst");
    }
    // A cached path is never accepted from outside this provider account.
    let root = binding.home.canonicalize().map_err(|e| e.to_string())?;
    if !path
        .canonicalize()
        .map_err(|e| e.to_string())?
        .starts_with(&root)
    {
        return Err("Native transcript escaped its provider account".into());
    }
    let compressed = path.extension().is_some_and(|ext| ext == "zst");
    let tail = path.with_extension("");
    let mut stamps = vec![stamp(&path)?];
    if compressed && tail.exists() {
        stamps.push(stamp(&tail)?);
    }
    let same_file = previous.is_some_and(|c| {
        c.path == path
            && (kind != Kind::Claude || c.claude_message_receipts)
            && c.stamps
                .first()
                .zip(stamps.first())
                .is_some_and(|(a, b)| a.device == b.device && a.inode == b.inode && b.len >= a.len)
            && (!compressed || c.stamps.first() == stamps.first())
            && c.stamps.len() <= stamps.len()
            && c.stamps
                .iter()
                .zip(&stamps)
                .all(|(a, b)| a.device == b.device && a.inode == b.inode && b.len >= a.len)
    });
    let mut cursor = if same_file {
        previous.cloned().unwrap()
    } else {
        Cursor::default()
    };
    if cursor.caught_up && cursor.path == path && cursor.stamps == stamps {
        return Ok(Some(Batch {
            active: cursor.pending.as_ref().is_some_and(|t| !t.complete),
            cursor,
            turns: vec![],
            changed: false,
            more: false,
        }));
    }
    let mut source: Box<dyn Read> = if compressed {
        let decoder =
            zstd::stream::read::Decoder::new(File::open(&path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        if stamps.len() > 1 {
            Box::new(
                decoder.chain(
                    File::open(&tail)
                        .map_err(|e| e.to_string())?
                        .take(stamps[1].len),
                ),
            )
        } else {
            Box::new(decoder)
        }
    } else {
        let mut file = File::open(&path).map_err(|e| e.to_string())?;
        // Append-only owners can be seeked directly; rewrites start over and
        // stable native identities make that replay idempotent at the Store.
        if cursor.offset > 0 {
            file.seek(SeekFrom::Start(
                cursor.offset.saturating_sub(cursor.anchor.len() as u64),
            ))
            .map_err(|e| e.to_string())?;
            let mut anchor = vec![0; cursor.anchor.len()];
            if file.read_exact(&mut anchor).is_err()
                || anchor != cursor.anchor
                || (cursor
                    .stamps
                    .first()
                    .is_some_and(|s| s.len == stamps[0].len)
                    && cursor.stamps != stamps)
            {
                cursor = Cursor::default();
            }
        }
        file.seek(SeekFrom::Start(cursor.offset))
            .map_err(|e| e.to_string())?;
        Box::new(file.take(stamps[0].len.saturating_sub(cursor.offset)))
    };
    if compressed && cursor.offset > 0 {
        let skipped = std::io::copy(
            &mut source.by_ref().take(cursor.offset),
            &mut std::io::sink(),
        )
        .map_err(|e| e.to_string())?;
        if skipped != cursor.offset {
            return Err("Compressed native transcript changed while reading".into());
        }
    }
    let mut reader = BufReader::new(source);
    let start = cursor.offset;
    let mut turns = Vec::new();
    let mut more = false;
    loop {
        if cursor.offset - start >= BATCH_LIMIT {
            more = !reader.fill_buf().map_err(|e| e.to_string())?.is_empty();
            break;
        }
        let mut line = Vec::new();
        let n = reader
            .by_ref()
            .take(LINE_LIMIT + 1)
            .read_until(b'\n', &mut line)
            .map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        if n as u64 > LINE_LIMIT {
            return Err(
                "Native transcript record exceeds the read limit; its source is preserved.".into(),
            );
        }
        if line.last() != Some(&b'\n') {
            break;
        }
        let value: Value = serde_json::from_slice(&line)
            .map_err(|e| format!("Could not read native transcript record: {e}"))?;
        match kind {
            Kind::Codex => parse::codex(&value, binding, &mut cursor, &mut turns)?,
            Kind::Claude => parse::claude(&value, binding, &mut cursor, &mut turns)?,
            _ => unreachable!(),
        }
        cursor.offset += n as u64;
        cursor.anchor = line[line.len().saturating_sub(256)..].to_vec();
    }
    // Claude has no task-complete record in its transcript. end_turn is the
    // native completion marker; keep its text blocks together until this read.
    if kind == Kind::Claude
        && cursor
            .pending
            .as_ref()
            .is_some_and(|t| t.complete && !t.messages.is_empty())
        && !more
    {
        let pending = cursor.pending.as_mut().unwrap();
        turns.push(Turn {
            id: pending.id.clone(),
            started_at: pending.started_at,
            completed_at: pending.completed_at,
            model: pending.model.clone(),
            messages: std::mem::take(&mut pending.messages),
            claude_records: std::mem::take(&mut pending.claude_records),
            complete: true,
            ..Default::default()
        });
        // Keep the root and UUIDs in the pending turn for late blocks.
    }
    if !cursor.verified && cursor.offset > 0 {
        return Err("Native transcript does not identify the saved provider session".into());
    }
    cursor.path = path;
    // A bounded catch-up must resume even if the file has not changed again.
    cursor.stamps = stamps;
    cursor.caught_up = !more;
    cursor.claude_message_receipts = kind == Kind::Claude;
    let active = cursor.pending.as_ref().is_some_and(|t| !t.complete);
    Ok(Some(Batch {
        cursor,
        turns,
        changed: true,
        more,
        active,
    }))
}
