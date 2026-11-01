//! Each Bot keeps its files in the app's data folder, never in a repo: it
//! works in repos with full file access, so a soul kept there could be
//! rewritten outside the app's controlled CLI or by a pull request.
//!
//! ```text
//! <app data>/bots/
//! ├── index.json            from when each project had its own Bot:
//! │                         folder id → the project keys it answered for
//! └── <bot id>/
//!     ├── SOUL.md           who it is; edited by the user or at their request
//!     ├── MEMORY.md         what it has learned; loads into every turn
//!     ├── habits.json       what it does on its own, on a schedule
//!     └── memory/           topic notes read on demand, and archive.md
//! ```
//!
//! A Bot is not tied to a project, so its folder is named by its own id. One
//! that was a project's Bot can still use its indexed project owner in place.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Original project-to-owner identities remain authoritative in place.
const INDEX_FILE: &str = "index.json";
const SOUL_FILE: &str = "SOUL.md";
const MEMORY_FILE: &str = "MEMORY.md";
const MEMORY_DIR: &str = "memory";
const HABITS_FILE: &str = "habits.json";
/// Larger than anything that would load: the soul and memory both have far
/// smaller prompt budgets, this only stops a runaway write.
const FILE_MAX: usize = 256 * 1024;
pub const CONFLICT: &str = "conflict";

static FILES_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Default, Serialize, Deserialize)]
struct AgentIndex {
    #[serde(default)]
    agents: Vec<AgentEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AgentEntry {
    id: String,
    /// Project keys (normalized paths) its project's Bot answered for.
    projects: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFile {
    /// Absent when the file does not exist yet.
    pub text: Option<String>,
    /// Hash of the text, or of "" for a missing file.
    pub hash: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFiles {
    pub id: String,
    pub dir: String,
    /// Absent until the app seeds it; the seed needs the user's old settings.
    pub soul: Option<String>,
    pub soul_hash: String,
    pub memory: String,
    pub memory_hash: String,
    pub memory_path: String,
    /// Topic notes under memory/, by name without `.md`; not the archive.
    pub topics: Vec<String>,
}

/// The files the app reads and writes for an agent, relative to its folder:
/// SOUL.md, MEMORY.md, habits.json, or one `memory/<name>.md`. Nothing nested, nothing
/// hidden, so a path from the agent's CLI can never leave the folder.
fn resolve_file(dir: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative == SOUL_FILE || relative == MEMORY_FILE || relative == HABITS_FILE {
        return Ok(dir.join(relative));
    }
    let name = relative
        .strip_prefix("memory/")
        .and_then(|rest| rest.strip_suffix(".md"))
        .ok_or_else(|| format!("Not an agent file: {relative}"))?;
    let valid = !name.is_empty()
        && name.len() <= 80
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '-' | '_'));
    if !valid {
        return Err(format!("Not an agent file: {relative}"));
    }
    Ok(dir.join(MEMORY_DIR).join(format!("{name}.md")))
}

fn topics(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir.join(MEMORY_DIR))
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().is_file())
                .filter_map(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let topic = name.strip_suffix(".md")?.to_string();
                    (topic != "archive" && !topic.starts_with('.')).then_some(topic)
                })
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

pub fn content_hash(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Write beside the target and rename over it, so a crash mid-write leaves
/// the old file whole. Owner-only, since memory can hold personal details.
fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;

    let temp = path.with_extension(format!("{}.tmp", crate::bot::new_id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
    let mut file = options
        .open(&temp)
        .map_err(|e| format!("{}: {e}", temp.display()))?;
    let result = (|| {
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)?;
        std::fs::File::open(path.parent().unwrap_or(Path::new(".")))?.sync_all()?;
        Ok::<_, std::io::Error>(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.map_err(|e| format!("{}: {e}", path.display()))
}

fn read_optional(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn read_index(root: &Path) -> Result<AgentIndex, String> {
    match std::fs::read_to_string(root.join(INDEX_FILE)) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| format!("Cannot read existing Bot owner index: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AgentIndex::default()),
        Err(e) => Err(format!("Cannot read existing Bot owner index: {e}")),
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// The Bot's folder, created with an empty memory on first use. A Bot that
/// was a project's own keeps that project's indexed folder as its owner.
fn agent_dir(root: &Path, bot: &str, legacy_project: Option<&str>) -> Result<PathBuf, String> {
    if !valid_id(bot) {
        return Err("Not a Bot id".into());
    }
    let mut dir = crate::compatibility::bot_owner(root, bot, false);
    if !crate::compatibility::has_owner_records(&dir, 4) {
        if let Some(project) = legacy_project.filter(|project| !project.trim().is_empty()) {
            for family in crate::compatibility::bot_families(root, false) {
                if let Some(owner) = read_index(&family)?.agents.iter().find(|entry| {
                    valid_id(&entry.id) && entry.projects.iter().any(|key| key == project)
                }) {
                    let indexed = family.join(&owner.id);
                    if std::fs::symlink_metadata(&indexed)
                        .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
                    {
                        dir = indexed;
                        break;
                    }
                }
            }
        }
    }
    std::fs::create_dir_all(dir.join(MEMORY_DIR)).map_err(|e| format!("{}: {e}", dir.display()))?;
    let memory = dir.join(MEMORY_FILE);
    if !memory.exists() {
        write_atomic(&memory, "")?;
    }
    Ok(dir)
}

pub fn load(root: &Path, bot: &str, legacy_project: Option<&str>) -> Result<AgentFiles, String> {
    let _guard = FILES_LOCK.lock().map_err(|e| e.to_string())?;
    let dir = agent_dir(root, bot, legacy_project)?;
    let soul = read_optional(&dir.join(SOUL_FILE))?;
    let memory_path = dir.join(MEMORY_FILE);
    let memory = read_optional(&memory_path)?.unwrap_or_default();
    Ok(AgentFiles {
        id: bot.to_string(),
        soul_hash: content_hash(soul.as_deref().unwrap_or("")),
        soul,
        memory_hash: content_hash(&memory),
        memory,
        memory_path: memory_path.to_string_lossy().into_owned(),
        topics: topics(&dir),
        dir: dir.to_string_lossy().into_owned(),
    })
}

pub fn read(
    root: &Path,
    bot: &str,
    legacy_project: Option<&str>,
    relative: &str,
) -> Result<AgentFile, String> {
    let _guard = FILES_LOCK.lock().map_err(|e| e.to_string())?;
    let dir = agent_dir(root, bot, legacy_project)?;
    let text = read_optional(&resolve_file(&dir, relative)?)?;
    Ok(AgentFile {
        hash: content_hash(text.as_deref().unwrap_or("")),
        text,
    })
}

/// Saves only over the text the editor opened, so the user never overwrites
/// what the agent wrote while they were typing. No hash means overwrite.
pub fn save(
    root: &Path,
    bot: &str,
    legacy_project: Option<&str>,
    relative: &str,
    text: &str,
    expected_hash: Option<&str>,
) -> Result<String, String> {
    if text.len() > FILE_MAX {
        return Err(format!("{relative} is too large"));
    }
    let _guard = FILES_LOCK.lock().map_err(|e| e.to_string())?;
    let dir = agent_dir(root, bot, legacy_project)?;
    let path = resolve_file(&dir, relative)?;
    if let Some(expected) = expected_hash {
        let current = read_optional(&path)?.unwrap_or_default();
        if content_hash(&current) != expected {
            return Err(CONFLICT.into());
        }
    }
    write_atomic(&path, text)?;
    Ok(content_hash(text))
}

#[cfg(test)]
mod tests;
