use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

use crate::dirs_home;

pub(crate) const MAX_TEXT_FILE_BYTES: u64 = 8 * 1024 * 1024;
pub(crate) const MAX_ATTACHMENT_EMBED_BYTES: u64 = 20 * 1024 * 1024;
pub(crate) const MAX_PREVIEW_BYTES: u64 = 25 * 1024 * 1024;
const MAX_DOCUMENT_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_GENERATED_IMAGE_BYTES: u64 = 25 * 1024 * 1024;
const MAX_GENERATED_IMAGE_DATA_BYTES: u64 = MAX_GENERATED_IMAGE_BYTES * 4 / 3 + 4;
const GENERATED_IMAGE_DIR: &str = "generated-images";

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedImageAsset {
    path: String,
    mime_type: String,
    size: u64,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectLocation {
    path: String,
    identity: String,
}

/// Resolve a project by filesystem identity when its directory was renamed.
///
/// A rename preserves the directory identity. We only inspect direct siblings
/// of the missing path, which keeps this bounded and avoids a filesystem watch
/// or a broad disk search.
#[tauri::command(async)]
pub fn resolve_project_location(
    path: String,
    identity: Option<String>,
) -> Result<Option<ProjectLocation>, String> {
    resolve_project_location_sync(&path, identity.as_deref())
}

fn resolve_project_location_sync(
    path: &str,
    identity: Option<&str>,
) -> Result<Option<ProjectLocation>, String> {
    let path = expand_home(path);
    if path.is_dir() {
        let identity = directory_identity(&path)?;
        return Ok(Some(ProjectLocation {
            path: path_to_js(&path),
            identity,
        }));
    }

    let Some(identity) = identity.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let Some(parent) = path.parent() else {
        return Ok(None);
    };
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    for entry in entries.flatten() {
        let candidate = entry.path();
        if !candidate.is_dir() {
            continue;
        }
        let Ok(candidate_identity) = directory_identity(&candidate) else {
            continue;
        };
        if candidate_identity == identity {
            return Ok(Some(ProjectLocation {
                path: path_to_js(&candidate),
                identity: candidate_identity,
            }));
        }
    }
    Ok(None)
}

#[cfg(unix)]
fn directory_identity(path: &Path) -> Result<String, String> {
    use std::os::unix::fs::MetadataExt;

    let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
    Ok(format!("unix:{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn directory_identity(path: &Path) -> Result<String, String> {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS,
    };

    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .map_err(|error| error.to_string())?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    let succeeded = unsafe {
        GetFileInformationByHandle(file.as_raw_handle() as _, std::ptr::addr_of_mut!(info))
    };
    if succeeded == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let file_index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    Ok(format!(
        "windows:{}:{file_index}",
        info.dwVolumeSerialNumber
    ))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntry {
    name: String,
    path: String,
    is_dir: bool,
    ignored: bool,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OmpInterjectionAnchor {
    id: String,
    after_assistant_text: String,
    after_occurrence: usize,
    after_assistant_text_concat: String,
    after_concat_occurrence: usize,
    /// A directly following text-only answer can have been coalesced into the
    /// parent block by old builds. Require both full texts before splitting it.
    following_assistant_text: Option<String>,
    following_assistant_text_concat: Option<String>,
    text: String,
    custom_type: String,
    severity: Option<String>,
}

/// One active-path assistant message in file order. Both join forms of its
/// text parts are alternatives for the same message, never two messages.
#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct OmpAssistantText {
    text: String,
    concat: String,
}

/// Recover displayed OMP custom messages that older vorkspace builds omitted
/// from their persisted transcript. The provider id is already stored with the
/// session; matching the original JSONL keeps the repair deterministic instead
/// of guessing from neighbouring reasoning text.
#[tauri::command(async)]
pub fn omp_session_interjections(
    provider_session_id: String,
) -> Result<Vec<OmpInterjectionAnchor>, String> {
    let Some(path) = omp_session_path(&provider_session_id)? else {
        return Ok(Vec::new());
    };
    parse_omp_interjections(&path)
}

#[tauri::command(async)]
pub fn omp_active_assistant_texts(
    provider_session_id: String,
) -> Result<Vec<OmpAssistantText>, String> {
    let Some(path) = omp_session_path(&provider_session_id)? else {
        return Ok(Vec::new());
    };
    active_omp_assistant_texts(&path)
}

/// Recover Bash commands that older UI builds saved as a bare "Shell" row.
/// Claude's own transcript retains the complete tool input by tool-use id.
#[tauri::command(async)]
pub fn claude_shell_commands(
    app: AppHandle,
    provider_session_id: String,
    provider_account_id: Option<String>,
    tool_ids: Vec<String>,
) -> Result<HashMap<String, String>, String> {
    if provider_session_id.is_empty()
        || !provider_session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err("Invalid Claude provider session id".into());
    }
    if tool_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let config_dir = match provider_account_id.as_deref() {
        Some(id) if id != "default" => crate::harness::provider_account_path(&app, "claude", id)?,
        _ => match std::env::var_os("CLAUDE_CONFIG_DIR") {
            Some(path) => PathBuf::from(path),
            None => {
                PathBuf::from(dirs_home().ok_or("Home directory is unavailable")?).join(".claude")
            }
        },
    };
    let transcript_name = format!("{provider_session_id}.jsonl");
    let root = config_dir.join("projects");
    let Some(path) = std::fs::read_dir(root).ok().and_then(|projects| {
        projects.flatten().find_map(|project| {
            let candidate = project.path().join(&transcript_name);
            candidate.is_file().then_some(candidate)
        })
    }) else {
        return Ok(HashMap::new());
    };
    claude_shell_commands_from_file(&path, &tool_ids)
}

fn claude_shell_commands_from_file(
    path: &Path,
    tool_ids: &[String],
) -> Result<HashMap<String, String>, String> {
    let wanted: HashSet<&str> = tool_ids.iter().map(String::as_str).collect();
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut commands = HashMap::new();
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|error| error.to_string())?;
        let Ok(record) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if record.get("type").and_then(serde_json::Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(content) = record
            .pointer("/message/content")
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        for block in content {
            let Some(id) = block.get("id").and_then(serde_json::Value::as_str) else {
                continue;
            };
            if wanted.contains(id)
                && block.get("name").and_then(serde_json::Value::as_str) == Some("Bash")
            {
                if let Some(command) = block
                    .pointer("/input/command")
                    .and_then(serde_json::Value::as_str)
                    .filter(|command| !command.trim().is_empty())
                {
                    commands.insert(id.to_owned(), command.to_owned());
                }
            }
        }
        if commands.len() == wanted.len() {
            break;
        }
    }
    Ok(commands)
}

fn omp_session_path(provider_session_id: &str) -> Result<Option<PathBuf>, String> {
    if provider_session_id.is_empty()
        || !provider_session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err("Invalid OMP provider session id".into());
    }
    let root = dirs_home()
        .map(PathBuf::from)
        .ok_or("Home directory is unavailable")?
        .join(".omp/agent/sessions");
    Ok(find_omp_session_file(&root, provider_session_id))
}

fn find_omp_session_file(root: &Path, provider_session_id: &str) -> Option<PathBuf> {
    let suffix = format!("_{provider_session_id}.jsonl");
    let projects = std::fs::read_dir(root).ok()?;
    for project in projects.flatten() {
        let path = project.path();
        if path.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(&suffix))
        {
            return Some(path);
        }
        if !path.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(path) else {
            continue;
        };
        for entry in entries.flatten() {
            let candidate = entry.path();
            if candidate.is_file()
                && candidate
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(&suffix))
            {
                return Some(candidate);
            }
        }
    }
    None
}

fn omp_message_text(value: &serde_json::Value, separator: &str) -> String {
    if let Some(text) = value.as_str() {
        return text.to_owned();
    }
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| {
            (part.get("type").and_then(serde_json::Value::as_str) == Some("text"))
                .then(|| part.get("text").and_then(serde_json::Value::as_str))
                .flatten()
        })
        .collect::<Vec<_>>()
        .join(separator)
}

fn read_omp_entries(path: &Path) -> Result<Vec<serde_json::Value>, String> {
    let file = std::fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut entries = Vec::new();
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { continue };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        entries.push(value);
    }
    Ok(entries)
}

fn omp_active_ids(entries: &[serde_json::Value]) -> HashSet<&str> {
    let nodes: HashMap<_, _> = entries
        .iter()
        .filter_map(|value| value["id"].as_str().map(|id| (id, value)))
        .collect();
    let mut active = HashSet::new();
    let mut cursor = entries.last().and_then(|value| value["id"].as_str());
    while let Some(id) = cursor {
        if !active.insert(id) {
            break;
        }
        cursor = nodes.get(id).and_then(|value| value["parentId"].as_str());
    }
    active
}

// Return the ordered sequence, not per-text counts: a status split may only
// be merged with the message at its own source position.
fn active_omp_assistant_texts(path: &Path) -> Result<Vec<OmpAssistantText>, String> {
    let entries = read_omp_entries(path)?;
    let active = omp_active_ids(&entries);
    Ok(entries
        .iter()
        .filter(|value| {
            value["type"] == "message"
                && value["message"]["role"] == "assistant"
                && value["id"].as_str().is_some_and(|id| active.contains(id))
        })
        .map(|value| {
            let content = &value["message"]["content"];
            OmpAssistantText {
                text: omp_message_text(content, "\n"),
                concat: omp_message_text(content, ""),
            }
        })
        .filter(|message| !message.text.trim().is_empty())
        .collect())
}

fn parse_omp_interjections(path: &Path) -> Result<Vec<OmpInterjectionAnchor>, String> {
    let entries = read_omp_entries(path)?;
    let nodes: HashMap<_, _> = entries
        .iter()
        .filter_map(|value| value["id"].as_str().map(|id| (id, value)))
        .collect();
    let active = omp_active_ids(&entries);
    let mut assistants: HashMap<&str, (String, usize, String, usize)> = HashMap::new();
    let mut occurrences: HashMap<String, usize> = HashMap::new();
    let mut concat_occurrences: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<OmpInterjectionAnchor> = Vec::new();
    // Metadata and compaction participate in ancestry, not anchor text.
    // Keep file order for occurrences and notes, but exclude abandoned branches.
    for value in &entries {
        if !value["id"].as_str().is_some_and(|id| active.contains(id)) {
            continue;
        }
        match value.get("type").and_then(serde_json::Value::as_str) {
            Some("message")
                if value
                    .pointer("/message/role")
                    .and_then(serde_json::Value::as_str)
                    == Some("assistant") =>
            {
                let Some(id) = value.get("id").and_then(serde_json::Value::as_str) else {
                    continue;
                };
                let text = omp_message_text(&value["message"]["content"], "\n");
                let concat_text = omp_message_text(&value["message"]["content"], "");
                if text.trim().is_empty() {
                    continue;
                }
                if let Some(anchor) = out.last_mut() {
                    let text_only = value["message"]["content"]
                        .as_array()
                        .is_some_and(|parts| parts.iter().all(|part| part["type"] == "text"));
                    if text_only
                        && value.get("parentId").and_then(serde_json::Value::as_str)
                            == Some(anchor.id.as_str())
                    {
                        anchor.following_assistant_text = Some(text.clone());
                        anchor.following_assistant_text_concat = Some(concat_text.clone());
                    }
                }
                let occurrence = occurrences.entry(text.clone()).or_default();
                *occurrence += 1;
                let concat_occurrence = concat_occurrences.entry(concat_text.clone()).or_default();
                *concat_occurrence += 1;
                assistants.insert(id, (text, *occurrence, concat_text, *concat_occurrence));
            }
            Some("custom_message")
                if value.get("display").and_then(serde_json::Value::as_bool) == Some(true) =>
            {
                let Some(id) = value.get("id").and_then(serde_json::Value::as_str) else {
                    continue;
                };
                let Some(parent_id) = value.get("parentId").and_then(serde_json::Value::as_str)
                else {
                    continue;
                };
                let mut ancestor = Some(parent_id);
                let mut seen = HashSet::new();
                let mut assistant = None;
                while let Some(id) = ancestor {
                    if !active.contains(id) || !seen.insert(id) {
                        break;
                    }
                    if let Some(found) = assistants.get(id) {
                        assistant = Some(found);
                        break;
                    }
                    let Some(parent) = nodes.get(id) else { break };
                    if matches!(
                        parent
                            .pointer("/message/role")
                            .and_then(serde_json::Value::as_str),
                        Some("user" | "assistant")
                    ) {
                        break;
                    }
                    ancestor = parent["parentId"].as_str();
                }
                let Some((after_assistant_text, after_occurrence, concat_text, concat_occurrence)) =
                    assistant
                else {
                    continue;
                };
                let custom_type = value
                    .get("customType")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("custom")
                    .to_owned();
                let mut severity = None;
                let mut note_bodies = Vec::new();
                if custom_type == "advisor" {
                    for note in value
                        .pointer("/details/notes")
                        .and_then(serde_json::Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        if let Some(body) = note.get("note").and_then(serde_json::Value::as_str) {
                            note_bodies.push(body);
                        }
                        let next = note.get("severity").and_then(serde_json::Value::as_str);
                        if next == Some("blocker")
                            || (next == Some("concern") && severity.as_deref() != Some("blocker"))
                            || (next == Some("nit") && severity.is_none())
                        {
                            severity = next.map(str::to_owned);
                        }
                    }
                }
                let text = if note_bodies.is_empty() {
                    omp_message_text(&value["content"], "\n")
                } else {
                    note_bodies.join("\n\n")
                };
                // Tool results, metadata and note chains seal the same preceding
                // assistant prose. Notes resolving there stack in source order.
                out.push(OmpInterjectionAnchor {
                    id: id.to_owned(),
                    after_assistant_text: after_assistant_text.clone(),
                    after_occurrence: *after_occurrence,
                    after_assistant_text_concat: concat_text.clone(),
                    after_concat_occurrence: *concat_occurrence,
                    following_assistant_text: None,
                    following_assistant_text_concat: None,
                    text,
                    custom_type,
                    severity,
                });
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Immediate children of `path` (project tree). Folders first, then files.
#[tauri::command(async)]
pub fn list_dir(path: String) -> Result<Vec<DirEntry>, String> {
    list_dir_sync(&expand_home(&path))
}

pub(crate) fn list_dir_sync(dir: &Path) -> Result<Vec<DirEntry>, String> {
    let reader = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;

    let mut out = Vec::new();
    for ent in reader {
        let Ok(ent) = ent else { continue };
        let name = ent.file_name();
        let Some(name) = name.to_str() else { continue };
        if name == ".DS_Store" {
            continue;
        }
        let path = ent.path();
        let is_dir = ent
            .file_type()
            .map(|t| t.is_dir() || (t.is_symlink() && path.is_dir()))
            .unwrap_or_else(|_| path.is_dir());
        out.push(DirEntry {
            ignored: false,
            name: name.to_string(),
            path: path_to_js(&path),
            is_dir,
        });
    }

    let names: Vec<&str> = out.iter().map(|e| e.name.as_str()).collect();
    let ignored = git_ignored_names(dir, &names).unwrap_or_else(|| {
        let ignore = Ignore::load(dir);
        names
            .iter()
            .filter(|n| ignore.matches(n))
            .map(|n| n.to_string())
            .collect()
    });
    for entry in &mut out {
        entry.ignored = entry.name == ".git" || ignored.contains(&entry.name);
    }

    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| compare_natural_names(&a.name, &b.name))
    });
    Ok(out)
}

fn compare_natural_names(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let a_start = i;
            let b_start = j;
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let a_digits = &a[a_start..i];
            let b_digits = &b[b_start..j];
            let a_value = a_digits
                .iter()
                .position(|digit| *digit != b'0')
                .map_or(&a_digits[a_digits.len()..], |start| &a_digits[start..]);
            let b_value = b_digits
                .iter()
                .position(|digit| *digit != b'0')
                .map_or(&b_digits[b_digits.len()..], |start| &b_digits[start..]);
            let order = a_value
                .len()
                .cmp(&b_value.len())
                .then_with(|| a_value.cmp(b_value));
            if order != Ordering::Equal {
                return order;
            }
        } else {
            let order = a[i].to_ascii_lowercase().cmp(&b[j].to_ascii_lowercase());
            if order != Ordering::Equal {
                return order;
            }
            i += 1;
            j += 1;
        }
    }
    (a.len() - i).cmp(&(b.len() - j)).then_with(|| a.cmp(b))
}

const MAX_PROJECT_FILES: usize = 20_000;
const MAX_WALK_DIRS: usize = 4_000;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFile {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) relative: String,
}

/// Workspace files for Quick Open. Prefer `git ls-files` (gitignore-aware,
/// index-backed); otherwise a bounded walk that never descends into vendor dirs.
#[tauri::command]
pub async fn list_project_files(cwd: String) -> Result<Vec<ProjectFile>, String> {
    crate::background_work::read(move || list_project_files_sync(&cwd))
        .await
        .map_err(|e| e.to_string())?
}

pub(crate) fn list_project_files_sync(cwd: &str) -> Result<Vec<ProjectFile>, String> {
    list_project_files_sync_cancellable(cwd, None)
}

/// Same listing, but aborts as soon as `cancel` is set.
///
/// Both halves of the enumeration can outlast a cancelled search on their own,
/// so both take the flag: `git ls-files` and the walk it falls back to. The
/// walk is the one that can run for minutes, but the listing is the one that
/// can hold a hundred megabytes, so neither is left uninterruptible.
pub(crate) fn list_project_files_sync_cancellable(
    cwd: &str,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<ProjectFile>, String> {
    let root = expand_home(cwd);
    if !root.is_dir() {
        return Err(format!("{}: Not a directory", root.display()));
    }
    if !is_indexable_root(&root) {
        return Ok(Vec::new());
    }
    if cancel.is_some_and(|token| token.load(Ordering::Acquire)) {
        return Ok(Vec::new());
    }
    if let Some(files) = git_ls_files(&root, cancel) {
        return Ok(files);
    }
    // `git_ls_files` returns `None` for "not a git repo", truncation, and
    // cancel. The first two should walk; cancel must not, or a cancelled
    // `ls-files` would start enumerating the tree it just avoided.
    if cancel.is_some_and(|token| token.load(Ordering::Acquire)) {
        return Ok(Vec::new());
    }
    Ok(walk_project_files(&root, cancel.into()))
}

const CHECK_IGNORE_SOME_MATCHED: i32 = 0;
const CHECK_IGNORE_NONE_MATCHED: i32 = 1;

fn git_ignored_names(dir: &Path, names: &[&str]) -> Option<HashSet<String>> {
    if names.is_empty() {
        return Some(HashSet::new());
    }
    let mut child = git_cmd()
        .arg("-C")
        .arg(dir)
        .args(["check-ignore", "--stdin", "-z"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let mut input = Vec::with_capacity(names.iter().map(|n| n.len() + 1).sum());
    for name in names {
        input.extend_from_slice(name.as_bytes());
        input.push(0);
    }
    let mut stdin = child.stdin.take()?;
    // Write on a separate thread: a large listing can fill the stdout pipe
    // while git still waits for stdin, which would deadlock a serial writer.
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let output = child.wait_with_output().ok()?;
    writer.join().ok()?.ok()?;

    if !matches!(
        output.status.code(),
        Some(CHECK_IGNORE_SOME_MATCHED | CHECK_IGNORE_NONE_MATCHED)
    ) {
        return None;
    }
    Some(
        output
            .stdout
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect(),
    )
}

/// Ceiling for one `git ls-files` listing. A 2M-file monorepo emits roughly
/// 120 MB of NUL-separated paths; past this the walk takes over, which is
/// bounded by `MAX_PROJECT_FILES` and cancellable.
const MAX_LS_FILES_BYTES: usize = 8 * 1024 * 1024;

fn git_ls_files(root: &Path, cancel: Option<&AtomicBool>) -> Option<Vec<ProjectFile>> {
    // Routed through the bounded reader rather than `Command::output()`: this
    // is the path a cancelled search actually takes, because a repository with
    // a git index never reaches the walk. `output()` buffers the whole listing
    // before anything can look at it, so on a large monorepo it held ~100 MB
    // and ran to completion with no way to interrupt it — the exact cost the
    // walk's cancel check was added to avoid, one function earlier.
    let (raw, truncated) = git_output_capped(
        root,
        &["ls-files", "-co", "--exclude-standard", "-z"],
        MAX_LS_FILES_BYTES,
        cancel,
    )?;
    if truncated {
        // A partial listing would silently hide files from search. Fall back to
        // the walk, which is bounded by its own budget.
        return None;
    }

    let mut files = Vec::new();
    for rel in raw.split(|b| *b == 0) {
        if cancel.is_some_and(|token| token.load(Ordering::Acquire)) {
            return Some(Vec::new());
        }
        if rel.is_empty() {
            continue;
        }
        let relative = path_to_js(Path::new(String::from_utf8_lossy(rel).as_ref()));
        if relative.ends_with('/') || path_has_skipped_dir(&relative) {
            continue;
        }
        let path = root.join(&relative);
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name == ".DS_Store" {
            continue;
        }
        files.push(ProjectFile {
            name: name.to_string(),
            path: path_to_js(&path),
            relative,
        });
        if files.len() >= MAX_PROJECT_FILES {
            break;
        }
    }
    Some(files)
}

#[derive(Debug, Clone, Default)]
pub(crate) struct GitInfo {
    pub branch: Option<String>,
    pub repo: Option<String>,
}

/// `git_info_for` costs up to three `git` subprocesses, and it sits inside
/// both `session_upsert` (which runs every time a transcript is persisted) and
/// `list_by_project` (every project switch). Caching it for a beat keeps a
/// busy session from respawning git on every keystroke-driven save; the branch
/// can lag by at most `GIT_INFO_TTL`, which only affects a label.
const GIT_INFO_TTL: Duration = Duration::from_secs(3);

static GIT_INFO_CACHE: Mutex<Option<HashMap<PathBuf, (Instant, GitInfo)>>> = Mutex::new(None);

pub(crate) fn git_info_for(root: &Path) -> GitInfo {
    if let Ok(mut guard) = GIT_INFO_CACHE.lock() {
        let cache = guard.get_or_insert_with(HashMap::new);
        cache.retain(|_, (at, _)| at.elapsed() < GIT_INFO_TTL);
        if let Some((_, info)) = cache.get(root) {
            return info.clone();
        }
    }
    let info = git_info_uncached(root);
    if let Ok(mut guard) = GIT_INFO_CACHE.lock() {
        guard
            .get_or_insert_with(HashMap::new)
            .insert(root.to_path_buf(), (Instant::now(), info.clone()));
    }
    info
}

fn git_info_uncached(root: &Path) -> GitInfo {
    let Some(top) = git_stdout(root, &["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
    else {
        return GitInfo {
            branch: None,
            repo: file_name(root),
        };
    };
    GitInfo {
        branch: git_branch(&top),
        repo: git_origin_repo(&top).or_else(|| file_name(&top)),
    }
}

#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffStats {
    pub files: i64,
    pub additions: i64,
    pub deletions: i64,
}

/// Uncommitted line counts for the opened folder: staged + unstaged vs HEAD,
/// plus untracked (gitignore-aware) files counted as additions.
#[tauri::command]
pub async fn git_diff_stats(cwd: String) -> Result<GitDiffStats, String> {
    crate::background_work::read(move || git_diff_stats_for(&expand_home(&cwd)))
        .await
        .map_err(|e| e.to_string())
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitChangedFile {
    pub path: String,
    pub relative: String,
    pub status: String,
    pub additions: i64,
    pub deletions: i64,
    pub staged: bool,
    pub unstaged: bool,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffIndex {
    pub branch: Option<String>,
    pub head: Option<String>,
    pub files: Vec<GitChangedFile>,
    pub additions: i64,
    pub deletions: i64,
    pub remote: Option<String>,
    pub upstream: Option<String>,
    pub default_branch: Option<String>,
    pub ahead: i64,
    pub behind: i64,
    pub ahead_of_default: i64,
    pub head_pushed: bool,
}

/// Changed files in the opened folder, with per-file line counts and status.
#[tauri::command]
pub async fn git_diff_index(cwd: String) -> Result<GitDiffIndex, String> {
    crate::background_work::read(move || git_diff_index_for(&expand_home(&cwd)))
        .await
        .map_err(|e| e.to_string())
}

/// Changed files and counts without branch/upstream synchronization metadata.
#[tauri::command]
pub async fn git_diff_files(cwd: String) -> Result<GitDiffIndex, String> {
    crate::background_work::read(move || git_diff_files_for(&expand_home(&cwd)))
        .await
        .map_err(|e| e.to_string())
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitFileDiff {
    pub path: String,
    pub relative: String,
    pub status: String,
    pub original: String,
    pub current: String,
    pub binary: bool,
    pub too_large: bool,
}

/// Contents for one changed file. Staged diffs compare HEAD to the index;
/// unstaged diffs compare the index to the working tree.
#[tauri::command]
pub async fn git_file_diff(
    cwd: String,
    relative: String,
    staged: bool,
) -> Result<GitFileDiff, String> {
    crate::background_work::read(move || git_file_diff_for(&expand_home(&cwd), &relative, staged))
        .await
        .map_err(|e| e.to_string())?
}

const GIT_HISTORY_DEFAULT: u32 = 200;
const GIT_HISTORY_MAX: u32 = 500;

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHistoryRef {
    pub name: String,
    pub kind: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHistoryCommit {
    pub sha: String,
    pub short_sha: String,
    pub parents: Vec<String>,
    pub author: String,
    pub timestamp: i64,
    pub subject: String,
    pub refs: Vec<GitHistoryRef>,
    pub head: bool,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHistory {
    pub head: Option<String>,
    pub commits: Vec<GitHistoryCommit>,
}

/// Recent commits for the Graph view: HEAD, upstream, and the default
/// branch. Newest first, with parent SHAs for the graph.
#[tauri::command]
pub async fn git_history(cwd: String, limit: Option<u32>) -> Result<GitHistory, String> {
    crate::background_work::read(move || git_history_for(&expand_home(&cwd), limit))
        .await
        .map_err(|e| e.to_string())?
}

/// Files changed in one commit (first parent / root).
#[tauri::command]
pub async fn git_commit_files(cwd: String, sha: String) -> Result<Vec<GitChangedFile>, String> {
    crate::background_work::read(move || git_commit_files_for(&expand_home(&cwd), &sha))
        .await
        .map_err(|e| e.to_string())?
}

/// Parent vs commit contents for one path in a historical commit.
#[tauri::command]
pub async fn git_commit_file_diff(
    cwd: String,
    sha: String,
    relative: String,
) -> Result<GitFileDiff, String> {
    crate::background_work::read(move || {
        git_commit_file_diff_for(&expand_home(&cwd), &sha, &relative)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Stage a changed file (`git add`).
#[tauri::command]
pub async fn git_stage_file(cwd: String, relative: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || git_stage_file_for(&expand_home(&cwd), &relative))
        .await
        .map_err(|e| e.to_string())?
}

/// Write `contents` into the index for one path, leaving the working tree alone.
#[tauri::command]
pub async fn git_stage_contents(
    cwd: String,
    relative: String,
    contents: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        git_stage_contents_for(&expand_home(&cwd), &relative, contents.as_bytes())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Unstage a file (`git restore --staged`).
#[tauri::command]
pub async fn git_unstage_file(cwd: String, relative: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        git_unstage_file_for(&expand_home(&cwd), &relative)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Discard uncommitted changes so the file matches HEAD (or delete if untracked).
#[tauri::command]
pub async fn git_discard_file(cwd: String, relative: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        git_discard_file_for(&expand_home(&cwd), &relative)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Discard every unstaged change (restore tracked files; delete untracked).
#[tauri::command]
pub async fn git_discard_all(cwd: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || git_discard_all_for(&expand_home(&cwd)))
        .await
        .map_err(|e| e.to_string())?
}

/// Stage every changed file in the repo.
#[tauri::command]
pub async fn git_stage_all(cwd: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        git_checked(&expand_home(&cwd), &["add", "-A", "--", "."])
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Unstage every staged file.
#[tauri::command]
pub async fn git_unstage_all(cwd: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        git_checked(&expand_home(&cwd), &["restore", "--staged", "--", "."])
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitStagedContext {
    pub branch: Option<String>,
    pub summary: String,
    pub patch: String,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitFileLocation {
    pub root: String,
    pub relative: String,
}

/// Locate each file's own checkout, including files whose parents were deleted.
#[tauri::command]
pub async fn git_locate_files(paths: Vec<String>) -> Result<Vec<Option<GitFileLocation>>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        paths
            .iter()
            .map(|path| git_locate_file(&expand_home(path)))
            .collect()
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Staged diff (or unstaged vs HEAD if nothing is staged) for commit text generation.
#[tauri::command]
pub async fn git_staged_context(
    cwd: String,
    paths: Option<Vec<String>>,
) -> Result<GitStagedContext, String> {
    tauri::async_runtime::spawn_blocking(move || match paths {
        Some(paths) => git_selected_context_for(&expand_home(&cwd), &paths),
        None => git_staged_context_for(&expand_home(&cwd)),
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Create a commit from the current index, or rewrite HEAD with it when `amend` is set.
#[tauri::command]
pub async fn git_commit(
    cwd: String,
    message: String,
    amend: bool,
    paths: Option<Vec<String>>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = expand_home(&cwd);
        if let Some(paths) = paths {
            if amend {
                return Err("Selected-file commits cannot amend HEAD".into());
            }
            git_commit_selected_for(&root, &message, &paths)
        } else if amend {
            git_commit_amend_for(&root, &message)
        } else {
            git_commit_for(&root, &message)
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Full message (subject and body) of the commit at HEAD.
#[tauri::command]
pub async fn git_head_message(cwd: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || git_head_message_for(&expand_home(&cwd)))
        .await
        .map_err(|e| e.to_string())?
}

/// Push the current branch to its upstream, or set upstream on first push.
#[tauri::command]
pub async fn git_push(cwd: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || git_push_for(&expand_home(&cwd)))
        .await
        .map_err(|e| e.to_string())?
}

/// Fast-forward the current branch from its upstream.
#[tauri::command]
pub async fn git_pull(cwd: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        git_checked(&expand_home(&cwd), &["pull", "--ff-only"])
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Pull incoming commits, then push local commits.
#[tauri::command]
pub async fn git_sync(cwd: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || git_sync_changes_for(&expand_home(&cwd)))
        .await
        .map_err(|e| e.to_string())?
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitRangeContext {
    pub base: String,
    pub head: String,
    pub commit_summary: String,
    pub diff_summary: String,
    pub diff_patch: String,
}

/// Commits and diff between the default branch and HEAD, for PR text generation.
#[tauri::command]
pub async fn git_range_context(cwd: String) -> Result<GitRangeContext, String> {
    tauri::async_runtime::spawn_blocking(move || git_range_context_for(&expand_home(&cwd)))
        .await
        .map_err(|e| e.to_string())?
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitPr {
    pub number: i64,
    pub title: String,
    pub url: String,
    pub state: String,
}

/// Latest pull request for the current branch, if `gh` can see one.
#[tauri::command]
pub async fn git_pr_status(cwd: String) -> Result<Option<GitPr>, String> {
    tauri::async_runtime::spawn_blocking(move || Ok(git_pr_status_for(&expand_home(&cwd))))
        .await
        .map_err(|e| e.to_string())?
}

#[derive(Deserialize)]
struct GitPrCreateInput {
    title: String,
    body: String,
    base: String,
    head: String,
}

/// Create a GitHub pull request with `gh` and return its URL.
#[tauri::command]
pub async fn git_pr_create(
    cwd: String,
    title: String,
    body: String,
    base: String,
    head: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        git_pr_create_for(
            &expand_home(&cwd),
            &GitPrCreateInput {
                title,
                body,
                base,
                head,
            },
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubLabel {
    pub name: String,
    pub color: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubAssignee {
    pub login: String,
    pub avatar_url: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubWorkItem {
    pub kind: String,
    pub number: i64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub state_reason: String,
    pub created_at: String,
    pub updated_at: String,
    pub labels: Vec<GitHubLabel>,
    pub assignees: Vec<GitHubAssignee>,
    pub draft: bool,
    pub repo: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubStatus {
    pub connected: bool,
    pub installed: bool,
    pub authenticated: bool,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum GitHubStarStatus {
    Starred,
    NotStarred,
    Unavailable,
}

const MONOCODE_STAR_ENDPOINT: &str = "/user/starred/vrnrn/vorkspace";

/// Whether the GitHub CLI is installed and has an active authenticated account.
#[tauri::command]
pub async fn git_github_status() -> Result<GitHubStatus, String> {
    tauri::async_runtime::spawn_blocking(git_github_status_for)
        .await
        .map_err(|error| error.to_string())
}

fn git_github_status_for() -> GitHubStatus {
    let Some(program) = crate::harness::resolve_gui_binary("gh") else {
        return GitHubStatus {
            connected: false,
            installed: false,
            authenticated: false,
        };
    };
    let mut cmd = Command::new(program);
    cmd.args(["auth", "status", "--active", "--hostname", "github.com"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_PAGER", "cat")
        .env("GIT_PAGER", "cat");
    crate::harness::apply_gui_env(&mut cmd);
    crate::hide_window_console(&mut cmd);
    let authenticated = cmd
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    GitHubStatus {
        connected: authenticated,
        installed: true,
        authenticated,
    }
}

/// Whether the active GitHub CLI account has starred the vorkspace repository.
#[tauri::command]
pub async fn github_monocode_star_status() -> Result<GitHubStarStatus, String> {
    tauri::async_runtime::spawn_blocking(github_monocode_star_status_for)
        .await
        .map_err(|error| error.to_string())
}

fn github_monocode_star_status_for() -> GitHubStarStatus {
    let result = gh_run(
        Path::new("."),
        &["api", "--silent", MONOCODE_STAR_ENDPOINT],
        true,
    );
    github_star_status_from_result(result)
}

fn github_star_status_from_result(result: Result<String, String>) -> GitHubStarStatus {
    match result {
        Ok(_) => GitHubStarStatus::Starred,
        Err(error) if error.contains("HTTP 404") => GitHubStarStatus::NotStarred,
        Err(_) => GitHubStarStatus::Unavailable,
    }
}

/// Star the vorkspace repository for the active GitHub CLI account.
#[tauri::command]
pub async fn github_star_monocode() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(|| {
        gh_run(
            Path::new("."),
            &["api", "--silent", "--method", "PUT", MONOCODE_STAR_ENDPOINT],
            true,
        )
        .map(|_| ())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// `owner/repo` for the GitHub remote of this working copy, via `gh`.
#[tauri::command]
pub async fn git_github_repo(cwd: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || git_github_repo_for(&expand_home(&cwd)))
        .await
        .map_err(|e| e.to_string())?
}

/// The GitHub remote of this working copy and, when it is a fork, its parent.
#[tauri::command]
pub async fn git_github_repositories(cwd: String) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || git_github_repositories_for(&expand_home(&cwd)))
        .await
        .map_err(|e| e.to_string())?
}

/// Open issues or pull requests for one GitHub repository, via `gh`.
#[tauri::command]
pub async fn git_github_work_items(
    cwd: String,
    repo: String,
    kind: String,
    assigned_to_me: bool,
    state: String,
    search: String,
    limit: Option<u32>,
) -> Result<Vec<GitHubWorkItem>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        git_github_work_items_for(
            &expand_home(&cwd),
            &repo,
            &kind,
            assigned_to_me,
            &state,
            &search,
            limit.unwrap_or(40),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// One issue or pull request by number, used when session navigation misses
/// the existing Inbox cache.
#[tauri::command]
pub async fn git_github_work_item(
    cwd: String,
    repo: String,
    kind: String,
    number: i64,
) -> Result<GitHubWorkItem, String> {
    tauri::async_runtime::spawn_blocking(move || {
        git_github_work_item_for(&expand_home(&cwd), &repo, &kind, number)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubWorkItemDetails {
    pub body: String,
    pub author: String,
    pub author_avatar_url: String,
    pub base_ref_name: String,
    pub head_ref_name: String,
    pub review_decision: String,
}

/// Issue or pull request body for the inbox detail pane.
#[tauri::command]
pub async fn git_github_work_item_details(
    cwd: String,
    repo: String,
    kind: String,
    number: i64,
) -> Result<GitHubWorkItemDetails, String> {
    tauri::async_runtime::spawn_blocking(move || {
        git_github_work_item_details_for(&expand_home(&cwd), &repo, &kind, number)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubWorkItemComment {
    pub id: String,
    pub kind: String,
    pub author: String,
    pub author_avatar_url: String,
    pub body: String,
    pub created_at: String,
    pub url: String,
    pub state: String,
    pub path: String,
    pub line: Option<i64>,
    pub resolved: bool,
    pub thread_id: String,
    pub replies: Vec<GitHubWorkItemComment>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubWorkItemCommit {
    pub oid: String,
    pub message_headline: String,
    pub author: String,
    pub committed_date: String,
    pub url: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubWorkItemThread {
    pub comments: Vec<GitHubWorkItemComment>,
    pub commits: Vec<GitHubWorkItemCommit>,
    pub truncated: bool,
    pub review_decision: String,
    pub base_ref_name: String,
    pub head_ref_name: String,
}

/// Conversation for the inbox detail pane: comments, reviews, and review threads.
#[tauri::command]
pub async fn git_github_work_item_thread(
    cwd: String,
    repo: String,
    kind: String,
    number: i64,
) -> Result<GitHubWorkItemThread, String> {
    tauri::async_runtime::spawn_blocking(move || {
        git_github_work_item_thread_for(&expand_home(&cwd), &repo, &kind, number)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Post a conversation comment, or a reply on a review thread.
#[tauri::command]
pub async fn git_github_work_item_comment(
    cwd: String,
    repo: String,
    kind: String,
    number: i64,
    body: String,
    in_reply_to: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        git_github_work_item_comment_for(
            &expand_home(&cwd),
            &repo,
            &kind,
            number,
            &body,
            &in_reply_to,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Merge or change the lifecycle state of a GitHub pull request via `gh`.
#[tauri::command]
pub async fn git_github_pr_action(
    cwd: String,
    repo: String,
    number: i64,
    action: String,
) -> Result<GitHubWorkItem, String> {
    tauri::async_runtime::spawn_blocking(move || {
        git_github_pr_action_for(&expand_home(&cwd), &repo, number, &action)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubPrFile {
    pub path: String,
    pub additions: i64,
    pub deletions: i64,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubPrDiff {
    pub additions: i64,
    pub deletions: i64,
    pub files: Vec<GitHubPrFile>,
    pub patch: String,
    pub truncated: bool,
}

const MAX_PR_DIFF_BYTES: usize = 2 * 1024 * 1024;

/// Unified diff and file stats for a pull request, via `gh`.
/// When `full_context` is true, prefer a large-context `git diff` between the PR OIDs.
#[tauri::command]
pub async fn git_github_pr_diff(
    cwd: String,
    repo: String,
    number: i64,
    full_context: Option<bool>,
) -> Result<GitHubPrDiff, String> {
    let full_context = full_context.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        git_github_pr_diff_for(&expand_home(&cwd), &repo, number, full_context)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubPrCheck {
    pub name: String,
    pub workflow: String,
    pub state: String,
    pub url: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubPrChecks {
    pub head_oid: String,
    pub checks: Vec<GitHubPrCheck>,
}

/// CI checks for one pull request, targeted explicitly by `repo` and `number` via `gh`.
#[tauri::command]
pub async fn git_github_pr_checks(
    cwd: String,
    repo: String,
    number: i64,
) -> Result<GitHubPrChecks, String> {
    tauri::async_runtime::spawn_blocking(move || {
        git_github_pr_checks_for(&expand_home(&cwd), &repo, number)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GitHubCheckDetails {
    steps: Vec<GitHubCheckStep>,
    annotations: Vec<GitHubCheckAnnotation>,
    notice: Option<String>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct GitHubCheckStep {
    name: String,
    state: String,
    started_at: Option<String>,
    completed_at: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct GitHubCheckAnnotation {
    #[serde(default)]
    path: String,
    #[serde(default, rename(deserialize = "start_line"))]
    line: u64,
    #[serde(default)]
    message: String,
    #[serde(default, rename(deserialize = "annotation_level"))]
    level: String,
}

#[tauri::command]
pub async fn git_github_check_details(
    cwd: String,
    repo: String,
    job_id: String,
) -> Result<GitHubCheckDetails, String> {
    tauri::async_runtime::spawn_blocking(move || {
        github_check_details_with(&repo, &job_id, |endpoint| {
            gh_checked(
                &expand_home(&cwd),
                &["api", "--hostname", "github.com", endpoint],
            )
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

fn github_check_details_with(
    repo: &str,
    job_id: &str,
    mut fetch: impl FnMut(&str) -> Result<String, String>,
) -> Result<GitHubCheckDetails, String> {
    let (owner, name) = split_github_repo(repo)?;
    // Only repository slugs and numeric IDs can enter API paths.
    if [&owner, &name].iter().any(|part| {
        matches!(part.as_str(), "." | "..")
            || !part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    }) || job_id.parse::<u64>().ok().filter(|id| *id > 0).is_none()
        || !job_id.bytes().all(|c| c.is_ascii_digit())
    {
        return Err("Invalid GitHub repository or job ID".into());
    }
    #[derive(Deserialize)]
    struct Step {
        name: String,
        status: String,
        conclusion: Option<String>,
        started_at: Option<String>,
        completed_at: Option<String>,
    }
    #[derive(Deserialize)]
    struct Job {
        id: u64,
        #[serde(default)]
        steps: Vec<Step>,
        check_run_url: Option<String>,
    }
    let prefix = format!("repos/{owner}/{name}");
    let json = fetch(&format!("{prefix}/actions/jobs/{job_id}"))?;
    let job: Job = serde_json::from_str(&json).map_err(|error| error.to_string())?;
    if job.id.to_string() != job_id {
        return Err("GitHub returned a different job".into());
    }
    let mut details = GitHubCheckDetails {
        steps: job
            .steps
            .into_iter()
            .map(|step| GitHubCheckStep {
                name: step.name,
                state: github_check_state(
                    &step.status,
                    step.conclusion.as_deref().unwrap_or_default(),
                ),
                started_at: step.started_at,
                completed_at: step.completed_at,
            })
            .collect(),
        annotations: vec![],
        notice: None,
    };
    let check_prefix = format!("https://api.github.com/{prefix}/check-runs/");
    let check_id = job
        .check_run_url
        .as_deref()
        .and_then(|value| value.strip_prefix(&check_prefix))
        .filter(|value| !value.is_empty() && value.bytes().all(|c| c.is_ascii_digit()));
    if let Some(check_id) = check_id {
        let annotations = fetch(&format!(
            "{prefix}/check-runs/{check_id}/annotations?per_page=100"
        ))
        .and_then(|json| {
            serde_json::from_str::<Vec<GitHubCheckAnnotation>>(&json)
                .map_err(|error| error.to_string())
        });
        match annotations {
            Ok(annotations) => {
                if annotations.len() == 100 {
                    details.notice = Some(
                        "Showing the first 100 annotations. View the full log on GitHub for more."
                            .into(),
                    );
                }
                details.annotations = annotations;
            }
            Err(_) => {
                details.notice =
                    Some("Could not load error annotations. View the full log on GitHub.".into())
            }
        }
    } else {
        details.notice =
            Some("Error annotations are unavailable. View the full log on GitHub.".into());
    }
    Ok(details)
}

#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitBranches {
    pub current: Option<String>,
    pub detached: bool,
    pub branches: Vec<GitBranchEntry>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitBranchEntry {
    pub name: String,
    pub current: bool,
    pub remote: Option<String>,
}

/// Local branches, plus remote-only branches that can be checked out.
#[tauri::command]
pub async fn git_branches(cwd: String) -> Result<GitBranches, String> {
    tauri::async_runtime::spawn_blocking(move || Ok(git_branches_for(&expand_home(&cwd))))
        .await
        .map_err(|e| e.to_string())?
}

/// Switch to an existing local branch, or create a local tracking branch from a remote.
#[tauri::command]
pub async fn git_checkout(
    cwd: String,
    name: String,
    remote: Option<String>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        git_checkout_for(&expand_home(&cwd), &name, remote.as_deref())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Create a branch from HEAD and switch to it.
#[tauri::command]
pub async fn git_create_branch(cwd: String, name: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || git_create_branch_for(&expand_home(&cwd), &name))
        .await
        .map_err(|e| e.to_string())?
}

/// Stash tracked and untracked local changes so a checkout can proceed.
#[tauri::command]
pub async fn git_stash(cwd: String, message: Option<String>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        git_stash_for(&expand_home(&cwd), message.as_deref())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn git_diff_stats_for(root: &Path) -> GitDiffStats {
    if !git_is_work_tree(root) {
        return GitDiffStats::default();
    }
    let mut files: HashMap<String, FileAcc> = HashMap::new();
    if let Some(text) = git_run(
        root,
        &[
            "diff",
            "-z",
            "--no-ext-diff",
            "--numstat",
            "HEAD",
            "--",
            ".",
        ],
    ) {
        add_numstat_map(&text, &mut files);
    } else {
        if let Some(text) = git_run(
            root,
            &["diff", "-z", "--no-ext-diff", "--numstat", "--", "."],
        ) {
            add_numstat_map(&text, &mut files);
        }
        if let Some(text) = git_run(
            root,
            &[
                "diff",
                "-z",
                "--no-ext-diff",
                "--cached",
                "--numstat",
                "--",
                ".",
            ],
        ) {
            add_numstat_map(&text, &mut files);
        }
    }
    add_untracked_map(root, &mut files);
    let mut additions = 0i64;
    let mut deletions = 0i64;
    for acc in files.values() {
        additions += acc.additions;
        deletions += acc.deletions;
    }
    GitDiffStats {
        files: files.len() as i64,
        additions,
        deletions,
    }
}

#[derive(Clone, Default)]
struct FileAcc {
    additions: i64,
    deletions: i64,
    untracked: bool,
    staged: bool,
    unstaged: bool,
}

pub(crate) fn git_diff_index_for(root: &Path) -> GitDiffIndex {
    git_diff_index_with(root, true)
}

/// File list + counts only. Skips ahead/behind/remote lookups used by Git chrome.
pub(crate) fn git_diff_files_for(root: &Path) -> GitDiffIndex {
    git_diff_index_with(root, false)
}

fn git_diff_index_with(root: &Path, include_sync: bool) -> GitDiffIndex {
    let mut files: HashMap<String, FileAcc> = HashMap::new();
    let mut statuses: HashMap<String, &'static str> = HashMap::new();

    if let Some(text) = git_run(
        root,
        &[
            "diff",
            "-z",
            "--relative",
            "--no-ext-diff",
            "--numstat",
            "HEAD",
            "--",
            ".",
        ],
    ) {
        add_numstat_map(&text, &mut files);
        if let Some(names) = git_run(
            root,
            &[
                "diff",
                "-z",
                "--relative",
                "--no-ext-diff",
                "--name-status",
                "--no-renames",
                "HEAD",
                "--",
                ".",
            ],
        ) {
            add_name_status(&names, &mut statuses);
        }
    } else {
        if let Some(text) = git_run(
            root,
            &[
                "diff",
                "-z",
                "--relative",
                "--no-ext-diff",
                "--numstat",
                "--",
                ".",
            ],
        ) {
            add_numstat_map(&text, &mut files);
        }
        if let Some(text) = git_run(
            root,
            &[
                "diff",
                "-z",
                "--relative",
                "--no-ext-diff",
                "--cached",
                "--numstat",
                "--",
                ".",
            ],
        ) {
            add_numstat_map(&text, &mut files);
        }
        if let Some(names) = git_run(
            root,
            &[
                "diff",
                "-z",
                "--relative",
                "--no-ext-diff",
                "--name-status",
                "--no-renames",
                "--",
                ".",
            ],
        ) {
            add_name_status(&names, &mut statuses);
        }
        if let Some(names) = git_run(
            root,
            &[
                "diff",
                "-z",
                "--relative",
                "--no-ext-diff",
                "--cached",
                "--name-status",
                "--no-renames",
                "--",
                ".",
            ],
        ) {
            add_name_status(&names, &mut statuses);
        }
    }
    add_untracked_map(root, &mut files);
    mark_cached_and_unstaged(root, &mut files);

    let mut out = Vec::with_capacity(files.len());
    let mut additions = 0i64;
    let mut deletions = 0i64;
    for (relative, acc) in files {
        additions += acc.additions;
        deletions += acc.deletions;
        let abs = root.join(&relative);
        let status = if acc.untracked {
            "untracked"
        } else if let Some(status) = statuses.get(&relative) {
            *status
        } else if !abs.exists() {
            "deleted"
        } else {
            "modified"
        };
        out.push(GitChangedFile {
            path: path_to_js(&abs),
            relative,
            status: status.to_string(),
            additions: acc.additions,
            deletions: acc.deletions,
            staged: acc.staged,
            unstaged: acc.untracked || acc.unstaged,
        });
    }
    out.sort_by(|a, b| a.relative.cmp(&b.relative));
    let sync = if include_sync {
        git_sync_for(root)
    } else {
        GitSync::default()
    };
    GitDiffIndex {
        branch: git_branch(root),
        head: git_stdout(root, &["rev-parse", "HEAD"]),
        files: out,
        additions,
        deletions,
        remote: sync.remote,
        upstream: sync.upstream,
        default_branch: sync.default_branch,
        ahead: sync.ahead,
        behind: sync.behind,
        ahead_of_default: sync.ahead_of_default,
        head_pushed: sync.head_pushed,
    }
}

fn add_numstat_map(text: &str, files: &mut HashMap<String, FileAcc>) {
    // `git diff -z --numstat` ends each record with NUL. A rename is
    // `add <TAB> del <TAB> <NUL> old <NUL> new <NUL>`; every other record is
    // `add <TAB> del <TAB> path <NUL>`. The path itself may contain spaces,
    // quotes, tabs, or non-ASCII bytes, which a newline listing would C-quote.
    let mut fields = text.split('\0');
    while let Some(record) = fields.next() {
        if record.is_empty() {
            continue;
        }
        let mut parts = record.splitn(3, '\t');
        let Some(add) = parts.next() else { continue };
        let Some(del) = parts.next() else { continue };
        let Some(path) = parts.next() else { continue };
        let path = if path.is_empty() {
            let _previous = fields.next();
            match fields.next() {
                Some(destination) if !destination.is_empty() => destination,
                _ => continue,
            }
        } else {
            path
        };
        let relative = literal_diff_path(path);
        if relative.is_empty() {
            continue;
        }
        let entry = files.entry(relative).or_default();
        if add != "-" && del != "-" {
            entry.additions += add.parse::<i64>().unwrap_or(0);
            entry.deletions += del.parse::<i64>().unwrap_or(0);
        }
    }
}

fn add_name_status(text: &str, statuses: &mut HashMap<String, &'static str>) {
    // `--no-renames -z` is `status <NUL> path <NUL>`, repeated.
    let mut fields = text.split('\0');
    while let Some(code) = fields.next() {
        if code.is_empty() {
            continue;
        }
        let Some(path) = fields.next() else {
            break;
        };
        let status = match code.as_bytes().first() {
            Some(b'A') => "added",
            Some(b'D') => "deleted",
            Some(b'M' | b'T') => "modified",
            _ => continue,
        };
        let relative = literal_diff_path(path);
        if !relative.is_empty() {
            statuses.insert(relative, status);
        }
    }
}

fn literal_diff_path(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() {
        String::new()
    } else {
        path_to_js(Path::new(path))
    }
}

fn normalize_diff_path(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() {
        return String::new();
    }
    let path = if let Some((_, new)) = path.split_once(" => ") {
        new.trim_end_matches('}')
    } else {
        path
    };
    path_to_js(Path::new(path))
}

const MAX_UNTRACKED_BYTES: u64 = 1024 * 1024;

fn add_untracked_map(root: &Path, files: &mut HashMap<String, FileAcc>) {
    let Some(stdout) = git_run(
        root,
        &["ls-files", "-o", "--exclude-standard", "-z", "--", "."],
    ) else {
        return;
    };
    for rel in stdout.split('\0') {
        if rel.is_empty() {
            continue;
        }
        let relative = path_to_js(Path::new(rel));
        let entry = files.entry(relative.clone()).or_default();
        entry.untracked = true;
        if entry.additions == 0 {
            entry.additions = text_line_count(&root.join(rel));
        }
    }
}

fn text_line_count(path: &Path) -> i64 {
    let Ok(meta) = std::fs::metadata(path) else {
        return 0;
    };
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_UNTRACKED_BYTES {
        return 0;
    }
    let Ok(bytes) = std::fs::read(path) else {
        return 0;
    };
    if bytes.contains(&0) {
        return 0;
    }
    let mut lines = 1i64;
    for byte in &bytes {
        if *byte == b'\n' {
            lines += 1;
        }
    }
    if bytes.last() == Some(&b'\n') {
        lines -= 1;
    }
    lines
}

fn mark_cached_and_unstaged(root: &Path, files: &mut HashMap<String, FileAcc>) {
    if let Some(names) = git_run(
        root,
        &[
            "diff",
            "-z",
            "--relative",
            "--cached",
            "--name-only",
            "--no-renames",
            "--",
            ".",
        ],
    ) {
        for line in names.split('\0') {
            let relative = literal_diff_path(line);
            if !relative.is_empty() {
                files.entry(relative).or_default().staged = true;
            }
        }
    }
    if let Some(names) = git_run(
        root,
        &[
            "diff",
            "-z",
            "--relative",
            "--name-only",
            "--no-renames",
            "--",
            ".",
        ],
    ) {
        for line in names.split('\0') {
            let relative = literal_diff_path(line);
            if !relative.is_empty() {
                files.entry(relative).or_default().unstaged = true;
            }
        }
    }
}

fn git_file_diff_for(root: &Path, relative: &str, staged: bool) -> Result<GitFileDiff, String> {
    let relative = normalize_diff_path(relative);
    if relative.is_empty()
        || relative.starts_with('/')
        || relative
            .split('/')
            .any(|part| part.is_empty() || part == "..")
    {
        return Err("Invalid path".into());
    }
    let abs = root.join(&relative);
    if !abs.starts_with(root) {
        return Err("Invalid path".into());
    }
    if !git_is_work_tree(root) {
        return Err("Not a git repository".into());
    }

    let prefix = git_stdout(root, &["rev-parse", "--show-prefix"]).unwrap_or_default();
    let index_spec = format!(":{prefix}{relative}");
    let (original, current) = if staged {
        let head_spec = format!("HEAD:{prefix}{relative}");
        (git_blob(root, &head_spec), git_blob(root, &index_spec))
    } else {
        let current = if abs.is_file() {
            Some(std::fs::read(&abs).unwrap_or_default())
        } else {
            None
        };
        (git_blob(root, &index_spec), current)
    };
    let had_original = original.is_some();
    let had_current = current.is_some();
    let orig = original.unwrap_or_default();
    let current = current.unwrap_or_default();
    let binary = orig.contains(&0) || current.contains(&0);
    let too_large =
        orig.len() as u64 > MAX_TEXT_FILE_BYTES || current.len() as u64 > MAX_TEXT_FILE_BYTES;
    let status = if !had_original && had_current {
        if staged {
            "added"
        } else {
            "untracked"
        }
    } else if had_original && !had_current {
        "deleted"
    } else {
        "modified"
    };
    let (original_text, current_text) = if binary || too_large {
        (String::new(), String::new())
    } else {
        (
            String::from_utf8_lossy(&orig).into_owned(),
            String::from_utf8_lossy(&current).into_owned(),
        )
    };
    Ok(GitFileDiff {
        path: path_to_js(&abs),
        relative,
        status: status.to_string(),
        original: original_text,
        current: current_text,
        binary,
        too_large,
    })
}

/// Tips for the Graph filter: current HEAD, its upstream, and
/// the repo default branch. Omits `refs/stash` and unmerged local branches.
fn git_history_tips(root: &Path) -> Vec<String> {
    let mut tips = vec!["HEAD".to_string()];
    if git_stdout(root, &["rev-parse", "--abbrev-ref", "@{upstream}"]).is_some() {
        tips.push("@{upstream}".to_string());
    }
    if let Some(remote) = git_remote_name(root) {
        if let Some(branch) = git_default_branch(root, Some(remote.as_str())) {
            let spec = format!("{remote}/{branch}");
            if git_ref_exists(root, &format!("refs/remotes/{spec}")) {
                tips.push(spec);
            }
        }
    }
    tips
}

fn git_history_for(root: &Path, limit: Option<u32>) -> Result<GitHistory, String> {
    if !git_is_work_tree(root) {
        return Ok(GitHistory::default());
    }
    let n = limit
        .unwrap_or(GIT_HISTORY_DEFAULT)
        .clamp(1, GIT_HISTORY_MAX);
    let count = n.to_string();
    let head = git_stdout(root, &["rev-parse", "HEAD"]);
    let remotes = git_remote_names(root);
    let tips = git_history_tips(root);
    let mut args = vec![
        "log".to_string(),
        "--topo-order".to_string(),
        "--decorate=short".to_string(),
        "--max-count".to_string(),
        count,
        "--format=%H%x00%h%x00%P%x00%an%x00%at%x00%D%x00%s%x1e".to_string(),
    ];
    args.extend(tips);
    let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
    let Some(text) = git_run(root, &args_ref) else {
        return Ok(GitHistory {
            head,
            commits: Vec::new(),
        });
    };
    Ok(GitHistory {
        commits: parse_git_history_log(&text, head.as_deref(), &remotes),
        head,
    })
}

fn parse_git_history_log(
    text: &str,
    head: Option<&str>,
    remotes: &[String],
) -> Vec<GitHistoryCommit> {
    let mut commits = Vec::new();
    for record in text.split('\u{1e}') {
        let record = record.trim();
        if record.is_empty() {
            continue;
        }
        let mut fields = record.split('\0');
        let Some(sha) = fields.next() else { continue };
        let Some(short_sha) = fields.next() else {
            continue;
        };
        let Some(parents) = fields.next() else {
            continue;
        };
        let Some(author) = fields.next() else {
            continue;
        };
        let Some(timestamp) = fields.next() else {
            continue;
        };
        let Some(decorations) = fields.next() else {
            continue;
        };
        let subject = fields.next().unwrap_or("");
        if sha.is_empty() {
            continue;
        }
        let (is_head, refs) = parse_git_decorations(decorations, head, sha, remotes);
        commits.push(GitHistoryCommit {
            sha: sha.to_string(),
            short_sha: if short_sha.is_empty() {
                sha.chars().take(7).collect()
            } else {
                short_sha.to_string()
            },
            parents: parents.split_whitespace().map(str::to_string).collect(),
            author: author.to_string(),
            timestamp: timestamp.parse().unwrap_or(0),
            subject: subject.to_string(),
            refs,
            head: is_head,
        });
    }
    commits
}

fn parse_git_decorations(
    raw: &str,
    head: Option<&str>,
    sha: &str,
    remotes: &[String],
) -> (bool, Vec<GitHistoryRef>) {
    let mut is_head = head == Some(sha);
    let mut refs = Vec::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some(name) = part.strip_prefix("HEAD -> ") {
            is_head = true;
            if !name.is_empty() {
                refs.push(GitHistoryRef {
                    name: name.to_string(),
                    kind: "local".into(),
                });
            }
        } else if part == "HEAD" {
            is_head = true;
        } else if let Some(tag) = part.strip_prefix("tag: ") {
            if !tag.is_empty() {
                refs.push(GitHistoryRef {
                    name: tag.to_string(),
                    kind: "tag".into(),
                });
            }
        } else if part.ends_with("/HEAD") {
            continue;
        } else if remotes
            .iter()
            .any(|remote| part == remote || part.starts_with(&format!("{remote}/")))
        {
            refs.push(GitHistoryRef {
                name: part.to_string(),
                kind: "remote".into(),
            });
        } else {
            refs.push(GitHistoryRef {
                name: part.to_string(),
                kind: "local".into(),
            });
        }
    }
    (is_head, refs)
}

fn git_remote_names(root: &Path) -> Vec<String> {
    git_run(root, &["remote"])
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

fn git_peel_commit(root: &Path, spec: &str) -> Result<String, String> {
    let spec = spec.trim();
    if spec.len() < 4 || spec.len() > 40 || !spec.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Invalid commit".into());
    }
    let peeled = format!("{spec}^{{commit}}");
    git_stdout(root, &["rev-parse", "--verify", &peeled]).ok_or_else(|| "Unknown commit".into())
}

fn git_commit_files_for(root: &Path, sha: &str) -> Result<Vec<GitChangedFile>, String> {
    if !git_is_work_tree(root) {
        return Err("Not a git repository".into());
    }
    let sha = git_peel_commit(root, sha)?;
    let mut files: HashMap<String, FileAcc> = HashMap::new();
    let mut statuses: HashMap<String, &'static str> = HashMap::new();
    if let Some(text) = git_run(
        root,
        &[
            "diff-tree",
            "-z",
            "--no-commit-id",
            "-r",
            "--root",
            "--no-renames",
            "--numstat",
            &sha,
        ],
    ) {
        add_numstat_map(&text, &mut files);
    }
    if let Some(names) = git_run(
        root,
        &[
            "diff-tree",
            "-z",
            "--no-commit-id",
            "-r",
            "--root",
            "--no-renames",
            "--name-status",
            &sha,
        ],
    ) {
        add_name_status(&names, &mut statuses);
    }
    let mut out = Vec::with_capacity(files.len().max(statuses.len()));
    let mut seen = HashSet::new();
    for (relative, acc) in files {
        seen.insert(relative.clone());
        let status = statuses.get(&relative).copied().unwrap_or("modified");
        out.push(GitChangedFile {
            path: path_to_js(&root.join(&relative)),
            relative,
            status: status.to_string(),
            additions: acc.additions,
            deletions: acc.deletions,
            staged: false,
            unstaged: false,
        });
    }
    for (relative, status) in statuses {
        if seen.contains(&relative) {
            continue;
        }
        out.push(GitChangedFile {
            path: path_to_js(&root.join(&relative)),
            relative,
            status: status.to_string(),
            additions: 0,
            deletions: 0,
            staged: false,
            unstaged: false,
        });
    }
    out.sort_by(|a, b| a.relative.cmp(&b.relative));
    Ok(out)
}

fn git_commit_file_diff_for(root: &Path, sha: &str, relative: &str) -> Result<GitFileDiff, String> {
    let relative = resolve_repo_path(root, relative)?;
    if !git_is_work_tree(root) {
        return Err("Not a git repository".into());
    }
    let sha = git_peel_commit(root, sha)?;
    let parent = git_stdout(root, &["rev-parse", "--verify", &format!("{sha}^")]);
    let original_bytes = match &parent {
        Some(parent) => git_blob(root, &format!("{parent}:{relative}")).unwrap_or_default(),
        None => Vec::new(),
    };
    let current_bytes = git_blob(root, &format!("{sha}:{relative}")).unwrap_or_default();
    let binary = original_bytes.contains(&0) || current_bytes.contains(&0);
    let too_large = original_bytes.len() as u64 > MAX_TEXT_FILE_BYTES
        || current_bytes.len() as u64 > MAX_TEXT_FILE_BYTES;
    let status = if original_bytes.is_empty() && !current_bytes.is_empty() {
        "added"
    } else if !original_bytes.is_empty() && current_bytes.is_empty() {
        "deleted"
    } else {
        "modified"
    };
    let (original, current) = if binary || too_large {
        (String::new(), String::new())
    } else {
        (
            String::from_utf8_lossy(&original_bytes).into_owned(),
            String::from_utf8_lossy(&current_bytes).into_owned(),
        )
    };
    Ok(GitFileDiff {
        path: path_to_js(&root.join(&relative)),
        relative,
        status: status.to_string(),
        original,
        current,
        binary,
        too_large,
    })
}

fn git_stage_file_for(root: &Path, relative: &str) -> Result<(), String> {
    let relative = resolve_repo_path(root, relative)?;
    git_checked(root, &["--literal-pathspecs", "add", "--", &relative])
}

fn git_stage_contents_for(root: &Path, relative: &str, contents: &[u8]) -> Result<(), String> {
    let relative = resolve_repo_path(root, relative)?;
    if contents.len() as u64 > MAX_TEXT_FILE_BYTES {
        return Err("File too large".into());
    }
    let hash = git_hash_object(root, &relative, contents)?;
    let mode = git_index_mode(root, &relative).unwrap_or_else(|| "100644".into());
    git_checked(
        root,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &mode,
            &hash,
            &relative,
        ],
    )
}

fn git_index_mode(root: &Path, relative: &str) -> Option<String> {
    let out = git_run(root, &["ls-files", "--stage", "--", relative])?;
    let mode = out.lines().next()?.split_whitespace().next()?;
    if mode.len() == 6 && mode.bytes().all(|b| b.is_ascii_digit()) {
        Some(mode.to_string())
    } else {
        None
    }
}

fn git_hash_object(root: &Path, relative: &str, contents: &[u8]) -> Result<String, String> {
    let mut child = git_cmd()
        .arg("--no-pager")
        .arg("-C")
        .arg(root)
        .args(["hash-object", "-w", "--path", relative, "--stdin"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "hash-object stdin".to_string())?;
    stdin.write_all(contents).map_err(|e| e.to_string())?;
    drop(stdin);
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let msg = stderr.trim();
        if !msg.is_empty() {
            return Err(msg.to_string());
        }
        let msg = stdout.trim();
        if !msg.is_empty() {
            return Err(msg.to_string());
        }
        return Err("git hash-object failed".into());
    }
    let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if hash.len() != 40 && hash.len() != 64 {
        return Err("git hash-object returned an invalid hash".into());
    }
    Ok(hash)
}

fn git_unstage_file_for(root: &Path, relative: &str) -> Result<(), String> {
    let relative = resolve_repo_path(root, relative)?;
    git_checked(
        root,
        &[
            "--literal-pathspecs",
            "restore",
            "--staged",
            "--",
            &relative,
        ],
    )
}

fn git_discard_file_for(root: &Path, relative: &str) -> Result<(), String> {
    let relative = resolve_repo_path(root, relative)?;
    let abs = root.join(&relative);
    if git_checked(root, &["ls-files", "--error-unmatch", "--", &relative]).is_err() {
        if abs.is_file() {
            std::fs::remove_file(&abs).map_err(|e| e.to_string())?;
        } else if abs.exists() {
            git_checked(root, &["clean", "-fd", "--", &relative])?;
        }
        return Ok(());
    }
    git_checked(root, &["restore", "--worktree", "--", &relative])
}

fn git_discard_all_for(root: &Path) -> Result<(), String> {
    let files: Vec<String> = git_diff_index_for(root)
        .files
        .into_iter()
        .filter(|file| file.unstaged)
        .map(|file| file.relative)
        .collect();
    for relative in files {
        git_discard_file_for(root, &relative)?;
    }
    Ok(())
}

fn git_staged_context_for(root: &Path) -> Result<GitStagedContext, String> {
    let mut summary = git_run(root, &["diff", "--cached", "--stat", "--", "."]).unwrap_or_default();
    let mut patch =
        git_run(root, &["diff", "--cached", "--no-ext-diff", "--", "."]).unwrap_or_default();

    if summary.trim().is_empty() && patch.trim().is_empty() {
        summary = git_run(root, &["diff", "HEAD", "--stat", "--", "."]).unwrap_or_default();
        patch = git_run(root, &["diff", "HEAD", "--no-ext-diff", "--", "."]).unwrap_or_default();
        if let Some(untracked) = git_run(root, &["ls-files", "--others", "--exclude-standard"]) {
            let names = untracked.trim();
            if !names.is_empty() {
                if !summary.trim().is_empty() {
                    summary.push('\n');
                }
                summary.push_str("Untracked files:\n");
                summary.push_str(names);
            }
        }
    }

    if summary.trim().is_empty() && patch.trim().is_empty() {
        return Err("No changes to summarize".into());
    }

    Ok(GitStagedContext {
        branch: git_branch(root),
        summary,
        patch,
    })
}

fn git_locate_file(path: &Path) -> Result<Option<GitFileLocation>, String> {
    if !path.is_absolute() || path.as_os_str().to_string_lossy().contains('\0') {
        return Err("Invalid file path".into());
    }
    let mut parent = path.parent().ok_or("Invalid file path")?;
    while !parent.is_dir() {
        parent = parent.parent().ok_or("File has no existing parent")?;
    }
    // Canonicalize parents rather than the file: deleted files still have a
    // location, and a tracked symlink belongs to the checkout holding the link.
    let directory = std::fs::canonicalize(parent).map_err(|e| e.to_string())?;
    let actual = directory.join(path.strip_prefix(parent).map_err(|e| e.to_string())?);
    let output = git_cmd()
        .args(["-C"])
        .arg(&directory)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        return if error.contains("not a git repository") {
            Ok(None)
        } else {
            Err(error.trim().into())
        };
    }
    let top = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
    let top = Path::new(top.trim_end_matches(['\r', '\n']));
    let repo = std::fs::canonicalize(top).map_err(|e| e.to_string())?;
    let relative = actual.strip_prefix(&repo).map_err(|e| e.to_string())?;
    Ok(Some(GitFileLocation {
        // Git supplies a usable Windows path without canonicalize's verbatim prefix.
        root: path_to_js(top),
        relative: path_to_js(relative),
    }))
}

/// A selection names literal files in exactly one checkout, never a directory
/// or a pathspec that can widen the commit to other files or repositories.
fn git_selected_paths(root: &Path, paths: &[String]) -> Result<Vec<String>, String> {
    if paths.is_empty() {
        return Err("Select at least one file".into());
    }
    let canonical_root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    let mut selected = Vec::new();
    for path in paths {
        let relative = path_to_js(Path::new(path));
        if relative.contains('\0')
            || Path::new(&relative).is_absolute()
            || Path::new(&relative)
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
            || relative.split('/').any(|part| {
                part.is_empty() || matches!(part, "." | "..") || part.eq_ignore_ascii_case(".git")
            })
        {
            return Err("Invalid selected file path".into());
        }
        let target = canonical_root.join(&relative);
        let exists = match std::fs::symlink_metadata(&target) {
            Ok(entry) if entry.is_dir() => {
                return Err("Select files rather than directories".into())
            }
            Ok(_) => true,
            Err(error) if error.kind() == ErrorKind::NotFound => false,
            Err(error) => return Err(error.to_string()),
        };
        let location = git_locate_file(&canonical_root.join(&relative))?
            .ok_or("Selected file is outside Git")?;
        if std::fs::canonicalize(&location.root).map_err(|e| e.to_string())? != canonical_root {
            return Err("Selected files must belong to this repository".into());
        }
        let tracked = git_output(
            root,
            &["--literal-pathspecs", "ls-files", "-z", "--", &relative],
        )
        .ok_or("Could not validate selected files")?;
        if tracked
            .split(|byte| *byte == 0)
            .any(|entry| !entry.is_empty() && entry != relative.as_bytes())
        {
            return Err("Select files rather than directories".into());
        }
        let head_type = git_stdout(root, &["cat-file", "-t", &format!("HEAD:{relative}")]);
        if head_type.as_deref() == Some("tree") {
            return Err("Select files rather than directories".into());
        }
        if !exists && tracked.is_empty() && head_type.as_deref() != Some("blob") {
            return Err("Selected file does not exist in this repository".into());
        }
        if !selected.contains(&relative) {
            selected.push(relative);
        }
    }
    Ok(selected)
}

fn git_commit_selected_for(root: &Path, message: &str, paths: &[String]) -> Result<(), String> {
    if message.trim().is_empty() {
        return Err("Commit message cannot be empty".into());
    }
    let paths = git_selected_paths(root, paths)?;
    let existing: Vec<&str> = paths
        .iter()
        .filter(|path| std::fs::symlink_metadata(root.join(path)).is_ok())
        .map(String::as_str)
        .collect();
    let mut add = vec!["--literal-pathspecs", "add", "-A", "--"];
    add.extend(existing);
    if add.len() > 4 {
        git_checked(root, &add)?;
    }
    // --only takes the chosen working-tree files and preserves every other
    // staged entry. Add existing files to make new files eligible for --only;
    // already-staged deletions no longer match `git add` but do match `commit`.
    let mut args = vec![
        "--literal-pathspecs",
        "commit",
        "--only",
        "--cleanup=strip",
        "-m",
        message.trim(),
        "--",
    ];
    args.extend(paths.iter().map(String::as_str));
    git_checked(root, &args).map_err(with_signing_hint)
}

struct GitSelectionIndex(PathBuf);

impl Drop for GitSelectionIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn git_selected_context_for(root: &Path, paths: &[String]) -> Result<GitStagedContext, String> {
    let paths = git_selected_paths(root, paths)?;
    let index_dir = std::env::temp_dir().join(format!("monocode-git-selection-{}", Uuid::new_v4()));
    let directory = std::fs::DirBuilder::new();
    #[cfg(unix)]
    let directory = {
        use std::os::unix::fs::DirBuilderExt;
        let mut directory = directory;
        directory.mode(0o700);
        directory
    };
    directory.create(&index_dir).map_err(|e| e.to_string())?;
    let index = GitSelectionIndex(index_dir);
    let run = |args: &[&str]| -> Result<String, String> {
        let output = git_cmd()
            .arg("--literal-pathspecs")
            .arg("-C")
            .arg(root)
            .args(args)
            .env("GIT_INDEX_FILE", index.0.join("index"))
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("PATH", crate::harness::gui_search_path())
            .output()
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().into());
        }
        String::from_utf8(output.stdout).map_err(|e| e.to_string())
    };
    if git_stdout(root, &["rev-parse", "--verify", "HEAD"]).is_some() {
        run(&["read-tree", "HEAD"])?;
    } else {
        run(&["read-tree", "--empty"])?;
    }
    let mut add = vec!["add", "-A", "--"];
    add.extend(paths.iter().map(String::as_str));
    run(&add)?;
    Ok(GitStagedContext {
        branch: git_branch(root),
        summary: run(&["diff", "--cached", "--stat", "--no-renames"])?,
        patch: run(&[
            "diff",
            "--cached",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
        ])?,
    })
}

fn git_commit_for(root: &Path, message: &str) -> Result<(), String> {
    git_commit_args(root, message, &[])
}

fn git_commit_amend_for(root: &Path, message: &str) -> Result<(), String> {
    git_commit_args(root, message, &["--amend"])
}

fn git_commit_args(root: &Path, message: &str, extra: &[&str]) -> Result<(), String> {
    let message = message.trim();
    if message.is_empty() {
        return Err("Commit message cannot be empty".into());
    }
    let mut args = vec!["commit"];
    args.extend_from_slice(extra);
    args.extend(["--cleanup=strip", "-m", message]);
    git_checked(root, &args).map_err(with_signing_hint)
}

/// Explain why signing fails here when the same commit works in a terminal.
fn with_signing_hint(error: String) -> String {
    if !error.contains("failed to sign") && !error.contains("ssh-keygen") {
        return error;
    }
    format!(
        "{error}\n\nGit couldn't sign this commit. vorkspace runs git without a terminal, \
         so your signer needs a GUI passphrase prompt (e.g. pinentry-mac) or an unlocked agent."
    )
}

fn git_head_message_for(root: &Path) -> Result<String, String> {
    git_stdout(root, &["log", "-1", "--pretty=%B"]).ok_or_else(|| "No commits yet".to_string())
}

fn git_push_for(root: &Path) -> Result<(), String> {
    if git_stdout(root, &["rev-parse", "--abbrev-ref", "@{upstream}"]).is_some() {
        return git_checked(root, &["push"]);
    }
    let remote = git_remote_name(root).ok_or_else(|| "No git remote to push to".to_string())?;
    git_checked(root, &["push", "-u", &remote, "HEAD"])
}

fn git_sync_changes_for(root: &Path) -> Result<(), String> {
    if git_stdout(root, &["rev-parse", "--abbrev-ref", "@{upstream}"]).is_some() {
        git_checked(root, &["pull", "--no-edit", "--ff"]).map_err(with_signing_hint)?;
        return git_checked(root, &["push"]);
    }
    git_push_for(root)
}

fn git_range_context_for(root: &Path) -> Result<GitRangeContext, String> {
    let head = git_branch(root).ok_or_else(|| "Not on a branch".to_string())?;
    let remote = git_remote_name(root);
    let default_branch = git_default_branch(root, remote.as_deref())
        .ok_or_else(|| "Could not resolve the default branch".to_string())?;
    let base_ref = match &remote {
        Some(remote)
            if git_ref_exists(root, &format!("refs/remotes/{remote}/{default_branch}")) =>
        {
            format!("{remote}/{default_branch}")
        }
        _ => default_branch.clone(),
    };
    let spec = format!("{base_ref}...HEAD");
    let commit_summary =
        git_run(root, &["log", "--format=%s", &format!("{base_ref}..HEAD")]).unwrap_or_default();
    let diff_summary = git_run(root, &["diff", "--stat", &spec]).unwrap_or_default();
    let diff_patch = git_run(root, &["diff", "--no-ext-diff", &spec]).unwrap_or_default();
    if commit_summary.trim().is_empty() && diff_patch.trim().is_empty() {
        return Err("No commits to include in a pull request".into());
    }
    Ok(GitRangeContext {
        base: default_branch,
        head,
        commit_summary,
        diff_summary,
        diff_patch,
    })
}

fn git_pr_status_for(root: &Path) -> Option<GitPr> {
    let branch = git_branch(root)?;
    let repo = git_github_repo_for(root).ok()?;
    let head = github_pr_head_filter(&repo, &branch)?;
    let json = gh_stdout(
        root,
        &[
            "pr",
            "list",
            "--head",
            &head,
            "--json",
            "number,title,url,state",
            "--limit",
            "20",
            "--state",
            "all",
        ],
    )?;
    parse_gh_pr_list(&json)
}

fn github_pr_head_filter(repo: &str, branch: &str) -> Option<String> {
    let (owner, _) = split_github_repo(repo).ok()?;
    Some(format!("{owner}:{branch}"))
}

fn git_github_repo_for(root: &Path) -> Result<String, String> {
    let json = gh_checked(root, &["repo", "view", "--json", "nameWithOwner"])?;
    #[derive(Deserialize)]
    struct View {
        #[serde(rename = "nameWithOwner")]
        name_with_owner: String,
    }
    let view: View = serde_json::from_str(&json).map_err(|error| error.to_string())?;
    let slug = view.name_with_owner.trim();
    if slug.is_empty() || !slug.contains('/') {
        return Err("GitHub did not return a repository".into());
    }
    Ok(slug.to_string())
}

fn git_github_repositories_for(root: &Path) -> Result<Vec<String>, String> {
    let json = gh_checked(root, &["repo", "view", "--json", "nameWithOwner,parent"])?;
    parse_github_repositories(&json)
}

fn parse_github_repositories(json: &str) -> Result<Vec<String>, String> {
    #[derive(Deserialize)]
    struct Owner {
        login: String,
    }
    #[derive(Deserialize)]
    struct Parent {
        name: String,
        owner: Owner,
    }
    #[derive(Deserialize)]
    struct View {
        #[serde(rename = "nameWithOwner")]
        name_with_owner: String,
        #[serde(default)]
        parent: Option<Parent>,
    }

    let view: View = serde_json::from_str(json).map_err(|error| error.to_string())?;
    let (owner, name) = split_github_repo(&view.name_with_owner)?;
    let mut repos = vec![format!("{owner}/{name}")];
    if let Some(parent) = view.parent {
        let parent = format!("{}/{}", parent.owner.login, parent.name);
        let (owner, name) = split_github_repo(&parent)?;
        let parent = format!("{owner}/{name}");
        if !repos[0].eq_ignore_ascii_case(&parent) {
            repos.push(parent);
        }
    }
    Ok(repos)
}

fn git_github_work_items_for(
    root: &Path,
    repo: &str,
    kind: &str,
    assigned_to_me: bool,
    state: &str,
    search: &str,
    limit: u32,
) -> Result<Vec<GitHubWorkItem>, String> {
    let kind = kind.trim();
    if kind != "issue" && kind != "pr" {
        return Err("Unknown GitHub task kind".into());
    }
    let (owner, name) = split_github_repo(repo)?;
    let repo = format!("{owner}/{name}");
    let state = if state.trim().eq_ignore_ascii_case("all") {
        "all"
    } else {
        "open"
    };
    let limit = limit.clamp(1, 100).to_string();
    let fields = if kind == "pr" {
        "number,title,url,state,createdAt,updatedAt,labels,assignees,isDraft"
    } else {
        "number,title,url,state,stateReason,createdAt,updatedAt,labels,assignees"
    };
    let mut args = vec![
        kind.to_string(),
        "list".into(),
        "--state".into(),
        state.into(),
        "--limit".into(),
        limit,
        "--repo".into(),
        repo.clone(),
        "--json".into(),
        fields.into(),
    ];
    if assigned_to_me {
        args.push("--assignee".into());
        args.push("@me".into());
    }
    let search = search.trim();
    if !search.is_empty() {
        args.push("--search".into());
        args.push(search.to_string());
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let json = gh_checked(root, &refs)?;
    parse_github_work_items(&json, kind, &repo)
}

fn git_github_work_item_for(
    root: &Path,
    repo: &str,
    kind: &str,
    number: i64,
) -> Result<GitHubWorkItem, String> {
    let kind = kind.trim();
    if kind != "issue" && kind != "pr" {
        return Err("Unknown GitHub task kind".into());
    }
    if number <= 0 {
        return Err("GitHub task number must be positive".into());
    }
    let (owner, name) = split_github_repo(repo)?;
    let repo = format!("{owner}/{name}");
    let number = number.to_string();
    let fields = if kind == "pr" {
        "number,title,url,state,createdAt,updatedAt,labels,assignees,isDraft"
    } else {
        "number,title,url,state,stateReason,createdAt,updatedAt,labels,assignees"
    };
    let json = gh_checked(
        root,
        &[kind, "view", &number, "--repo", &repo, "--json", fields],
    )?;
    parse_github_work_item(&json, kind, &repo)
}

fn github_pr_action_args(repo: &str, number: i64, action: &str) -> Result<Vec<String>, String> {
    if number <= 0 {
        return Err("GitHub pull request number must be positive".into());
    }
    let (owner, name) = split_github_repo(repo)?;
    let repo = format!("{owner}/{name}");
    let number = number.to_string();
    let args = match action.trim() {
        "merge" => vec!["pr", "merge", &number, "--repo", &repo, "--merge"],
        "squash" => vec!["pr", "merge", &number, "--repo", &repo, "--squash"],
        "rebase" => vec!["pr", "merge", &number, "--repo", &repo, "--rebase"],
        "draft" => vec!["pr", "ready", &number, "--repo", &repo, "--undo"],
        "ready" => vec!["pr", "ready", &number, "--repo", &repo],
        "close" => vec!["pr", "close", &number, "--repo", &repo],
        "reopen" => vec!["pr", "reopen", &number, "--repo", &repo],
        _ => return Err("Unknown GitHub pull request action".into()),
    };
    Ok(args.into_iter().map(str::to_string).collect())
}

fn git_github_pr_action_for(
    root: &Path,
    repo: &str,
    number: i64,
    action: &str,
) -> Result<GitHubWorkItem, String> {
    let args = github_pr_action_args(repo, number, action)?;
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    // Successful mutation commands do not always write to stdout. Their exit
    // status confirms the action ran; the follow-up view fetches the new state.
    gh_run(root, &refs, true)?;
    git_github_work_item_for(root, repo, "pr", number)
}

fn git_github_work_item_details_for(
    root: &Path,
    repo: &str,
    kind: &str,
    number: i64,
) -> Result<GitHubWorkItemDetails, String> {
    let kind = kind.trim();
    if kind != "issue" && kind != "pr" {
        return Err("Unknown GitHub task kind".into());
    }
    if number <= 0 {
        return Err("Invalid GitHub item number".into());
    }
    let (owner, name) = split_github_repo(repo)?;
    let repo = format!("{owner}/{name}");
    let number = number.to_string();
    let fields = if kind == "pr" {
        "body,author,baseRefName,headRefName,reviewDecision"
    } else {
        "body,author"
    };
    let json = gh_checked(
        root,
        &[kind, "view", &number, "--repo", &repo, "--json", fields],
    )?;
    parse_github_work_item_details(&json)
}

fn parse_github_work_item_details(json: &str) -> Result<GitHubWorkItemDetails, String> {
    #[derive(Deserialize)]
    struct Author {
        #[serde(default)]
        login: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Row {
        #[serde(default)]
        body: String,
        #[serde(default)]
        author: Option<Author>,
        #[serde(default)]
        base_ref_name: String,
        #[serde(default)]
        head_ref_name: String,
        #[serde(default)]
        review_decision: Option<String>,
    }
    let row: Row = serde_json::from_str(json).map_err(|error| error.to_string())?;
    let author = row.author.map(|author| author.login).unwrap_or_default();
    let author_avatar_url = github_avatar_url(&author);
    Ok(GitHubWorkItemDetails {
        body: row.body,
        author,
        author_avatar_url,
        base_ref_name: row.base_ref_name,
        head_ref_name: row.head_ref_name,
        review_decision: row.review_decision.unwrap_or_default(),
    })
}

const GITHUB_ISSUE_THREAD_QUERY: &str = r#"
query InboxIssueThread($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    issue(number: $number) {
      comments(last: 40) {
        totalCount
        nodes {
          id
          author { login }
          body
          createdAt
          url
          isMinimized
        }
      }
    }
  }
}
"#;

const GITHUB_PR_THREAD_QUERY: &str = r#"
query InboxPullRequestThread($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      reviewDecision
      baseRefName
      headRefName
      commits(last: 40) {
        totalCount
        nodes {
          commit {
            oid
            messageHeadline
            committedDate
            url
            author {
              name
              user { login }
            }
          }
        }
      }
      comments(last: 40) {
        totalCount
        nodes {
          id
          author { login }
          body
          createdAt
          url
          isMinimized
        }
      }
      reviews(last: 40) {
        totalCount
        nodes {
          id
          author { login }
          body
          state
          submittedAt
          url
        }
      }
      reviewThreads(last: 20) {
        totalCount
        nodes {
          id
          isResolved
          path
          comments(first: 8) {
            totalCount
            nodes {
              id
              author { login }
              body
              createdAt
              url
              path
              line
              originalLine
              isMinimized
            }
          }
        }
      }
    }
  }
}
"#;

const GITHUB_REVIEW_REPLY_MUTATION: &str = r#"
mutation InboxReviewReply($threadId: ID!, $body: String!) {
  addPullRequestReviewThreadReply(input: {
    pullRequestReviewThreadId: $threadId
    body: $body
  }) {
    comment { url }
  }
}
"#;

fn git_github_work_item_thread_for(
    root: &Path,
    repo: &str,
    kind: &str,
    number: i64,
) -> Result<GitHubWorkItemThread, String> {
    let kind = kind.trim();
    if kind != "issue" && kind != "pr" {
        return Err("Unknown GitHub task kind".into());
    }
    if number <= 0 {
        return Err("Invalid GitHub item number".into());
    }
    let (owner, name) = split_github_repo(repo)?;
    let query = if kind == "pr" {
        GITHUB_PR_THREAD_QUERY
    } else {
        GITHUB_ISSUE_THREAD_QUERY
    };
    let owner_field = format!("owner={owner}");
    let name_field = format!("name={name}");
    let number_field = format!("number={number}");
    let json = gh_checked(
        root,
        &[
            "api",
            "graphql",
            "-f",
            &format!("query={query}"),
            "-F",
            &owner_field,
            "-F",
            &name_field,
            "-F",
            &number_field,
        ],
    )?;
    parse_github_work_item_thread(&json, kind)
}

fn github_comment_input<'a>(
    kind: &'a str,
    number: i64,
    body: &'a str,
) -> Result<(&'a str, &'a str), String> {
    let kind = kind.trim();
    if kind != "issue" && kind != "pr" {
        return Err("Unknown GitHub task kind".into());
    }
    if number <= 0 {
        return Err("Invalid GitHub item number".into());
    }
    let body = body.trim();
    if body.is_empty() {
        return Err("Comment cannot be empty".into());
    }
    Ok((kind, body))
}

fn git_github_work_item_comment_for(
    root: &Path,
    repo: &str,
    kind: &str,
    number: i64,
    body: &str,
    in_reply_to: &str,
) -> Result<String, String> {
    let (kind, body) = github_comment_input(kind, number, body)?;
    let (owner, name) = split_github_repo(repo)?;
    let repo = format!("{owner}/{name}");
    let reply = in_reply_to.trim();
    if !reply.is_empty() {
        return git_github_review_reply_for(root, reply, body);
    }
    let number = number.to_string();
    with_temp_markdown(body, |path| {
        let output = gh_checked(
            root,
            &[
                kind,
                "comment",
                &number,
                "--repo",
                &repo,
                "--body-file",
                path,
            ],
        )?;
        github_url_from_output(&output, "GitHub did not return a comment URL")
    })
}

fn git_github_review_reply_for(root: &Path, thread_id: &str, body: &str) -> Result<String, String> {
    if !valid_github_node_id(thread_id) {
        return Err("Invalid review thread".into());
    }
    let thread_field = format!("threadId={thread_id}");
    with_temp_markdown(body, |path| {
        let body_field = format!("body=@{path}");
        let json = gh_checked(
            root,
            &[
                "api",
                "graphql",
                "-f",
                &format!("query={GITHUB_REVIEW_REPLY_MUTATION}"),
                "-F",
                &thread_field,
                "-F",
                &body_field,
            ],
        )?;
        parse_github_review_reply_url(&json)
    })
}

fn with_temp_markdown(
    body: &str,
    run: impl FnOnce(&str) -> Result<String, String>,
) -> Result<String, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let temp_dir = std::env::temp_dir();
    let mut created = None;
    for attempt in 0..100 {
        let candidate = temp_dir.join(format!("monocode-comment-{pid}-{stamp}-{attempt}.md"));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&candidate) {
            Ok(file) => {
                created = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    let (path, mut file) =
        created.ok_or_else(|| "Could not create temporary markdown file".to_string())?;
    if let Err(error) = file.write_all(body.as_bytes()) {
        let _ = std::fs::remove_file(&path);
        return Err(error.to_string());
    }
    drop(file);
    let path_str = path.to_string_lossy().into_owned();
    let result = run(&path_str);
    let _ = std::fs::remove_file(&path);
    result
}

fn valid_github_node_id(id: &str) -> bool {
    let id = id.trim();
    !id.is_empty()
        && id.len() < 256
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '='))
}

fn github_url_from_output(output: &str, missing: &str) -> Result<String, String> {
    output
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with("http://") || line.starts_with("https://"))
        .map(str::to_string)
        .ok_or_else(|| {
            if output.trim().is_empty() {
                missing.to_string()
            } else {
                output.trim().to_string()
            }
        })
}

fn parse_github_review_reply_url(json: &str) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|error| error.to_string())?;
    if let Some(message) = value
        .get("errors")
        .and_then(|errors| errors.as_array())
        .and_then(|errors| errors.first())
        .and_then(|error| error.get("message"))
        .and_then(|message| message.as_str())
        .map(str::trim)
        .filter(|message| !message.is_empty())
    {
        return Err(message.to_string());
    }
    let url = value
        .pointer("/data/addPullRequestReviewThreadReply/comment/url")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim();
    if url.starts_with("http://") || url.starts_with("https://") {
        return Ok(url.to_string());
    }
    Err("GitHub did not return a comment URL".into())
}

fn split_github_repo(slug: &str) -> Result<(String, String), String> {
    let slug = slug.trim();
    let Some((owner, name)) = slug.split_once('/') else {
        return Err("GitHub did not return a repository".into());
    };
    let owner = owner.trim();
    let name = name.trim();
    if owner.is_empty()
        || name.is_empty()
        || name.contains('/')
        || owner.chars().any(char::is_whitespace)
        || name.chars().any(char::is_whitespace)
    {
        return Err("GitHub did not return a repository".into());
    }
    Ok((owner.to_string(), name.to_string()))
}

#[derive(Deserialize)]
struct GithubGraphqlEnvelope {
    #[serde(default)]
    data: Option<GithubGraphqlData>,
    #[serde(default)]
    errors: Vec<GithubGraphqlError>,
}

#[derive(Deserialize)]
struct GithubGraphqlError {
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
struct GithubGraphqlData {
    repository: Option<GithubGraphqlRepository>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubGraphqlRepository {
    issue: Option<GithubGraphqlIssue>,
    pull_request: Option<GithubGraphqlPullRequest>,
}

#[derive(Deserialize)]
struct GithubGraphqlIssue {
    #[serde(default)]
    comments: GithubGraphqlNodes<GithubGraphqlComment>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubGraphqlPullRequest {
    #[serde(default)]
    review_decision: Option<String>,
    #[serde(default)]
    base_ref_name: String,
    #[serde(default)]
    head_ref_name: String,
    #[serde(default)]
    commits: GithubGraphqlNodes<GithubGraphqlCommitNode>,
    #[serde(default)]
    comments: GithubGraphqlNodes<GithubGraphqlComment>,
    #[serde(default)]
    reviews: GithubGraphqlNodes<GithubGraphqlReview>,
    #[serde(default)]
    review_threads: GithubGraphqlNodes<GithubGraphqlReviewThread>,
}

#[derive(Deserialize)]
struct GithubGraphqlCommitNode {
    commit: GithubGraphqlCommit,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubGraphqlCommit {
    #[serde(default)]
    oid: String,
    #[serde(default)]
    message_headline: String,
    #[serde(default)]
    committed_date: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    author: Option<GithubGraphqlCommitAuthor>,
}

#[derive(Deserialize)]
struct GithubGraphqlCommitAuthor {
    #[serde(default)]
    name: String,
    #[serde(default)]
    user: Option<GithubGraphqlActor>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubGraphqlNodes<T> {
    total_count: i64,
    nodes: Vec<T>,
}

impl<T> Default for GithubGraphqlNodes<T> {
    fn default() -> Self {
        Self {
            total_count: 0,
            nodes: Vec::new(),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubGraphqlComment {
    #[serde(default)]
    id: String,
    #[serde(default)]
    author: Option<GithubGraphqlActor>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    is_minimized: bool,
    #[serde(default)]
    path: String,
    #[serde(default)]
    line: Option<i64>,
    #[serde(default)]
    original_line: Option<i64>,
}

#[derive(Deserialize)]
struct GithubGraphqlReview {
    #[serde(default)]
    id: String,
    #[serde(default)]
    author: Option<GithubGraphqlActor>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    state: String,
    #[serde(default, rename = "submittedAt")]
    submitted_at: Option<String>,
    #[serde(default)]
    url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubGraphqlReviewThread {
    #[serde(default)]
    id: String,
    #[serde(default)]
    is_resolved: bool,
    #[serde(default)]
    path: String,
    #[serde(default)]
    comments: GithubGraphqlNodes<GithubGraphqlComment>,
}

#[derive(Deserialize)]
struct GithubGraphqlActor {
    #[serde(default)]
    login: String,
}

fn parse_github_work_item_thread(json: &str, kind: &str) -> Result<GitHubWorkItemThread, String> {
    let envelope: GithubGraphqlEnvelope =
        serde_json::from_str(json).map_err(|error| error.to_string())?;
    let graphql_error = envelope
        .errors
        .iter()
        .map(|error| error.message.trim())
        .find(|message| !message.is_empty())
        .map(str::to_string);
    let Some(repository) = envelope.data.and_then(|data| data.repository) else {
        return Err(graphql_error.unwrap_or_else(|| "GitHub item not found".into()));
    };

    let mut comments = Vec::new();
    let mut commits = Vec::new();
    let mut truncated = false;
    let mut review_decision = String::new();
    let mut base_ref_name = String::new();
    let mut head_ref_name = String::new();

    if kind == "pr" {
        let Some(pull) = repository.pull_request else {
            return Err(graphql_error.unwrap_or_else(|| "GitHub pull request not found".into()));
        };
        review_decision = pull.review_decision.unwrap_or_default();
        base_ref_name = pull.base_ref_name;
        head_ref_name = pull.head_ref_name;
        truncated |= github_nodes_truncated(&pull.commits);
        commits.extend(pull.commits.nodes.into_iter().filter_map(|node| {
            let commit = node.commit;
            if commit.oid.trim().is_empty() || commit.committed_date.trim().is_empty() {
                return None;
            }
            let author = commit
                .author
                .map(|author| {
                    author
                        .user
                        .map(|user| user.login)
                        .filter(|login| !login.trim().is_empty())
                        .unwrap_or(author.name)
                })
                .unwrap_or_default();
            Some(GitHubWorkItemCommit {
                oid: commit.oid,
                message_headline: commit.message_headline,
                author,
                committed_date: commit.committed_date,
                url: commit.url,
            })
        }));
        truncated |= github_nodes_truncated(&pull.comments);
        comments.extend(
            pull.comments
                .nodes
                .into_iter()
                .filter_map(github_issue_comment),
        );
        truncated |= github_nodes_truncated(&pull.reviews);
        comments.extend(
            pull.reviews
                .nodes
                .into_iter()
                .filter_map(github_review_comment),
        );
        truncated |= github_nodes_truncated(&pull.review_threads);
        comments.extend(
            pull.review_threads
                .nodes
                .into_iter()
                .filter_map(github_review_thread),
        );
    } else {
        let Some(issue) = repository.issue else {
            return Err(graphql_error.unwrap_or_else(|| "GitHub issue not found".into()));
        };
        truncated |= github_nodes_truncated(&issue.comments);
        comments.extend(
            issue
                .comments
                .nodes
                .into_iter()
                .filter_map(github_issue_comment),
        );
    }

    comments.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(GitHubWorkItemThread {
        comments,
        commits,
        truncated,
        review_decision,
        base_ref_name,
        head_ref_name,
    })
}

fn github_nodes_truncated<T>(nodes: &GithubGraphqlNodes<T>) -> bool {
    (nodes.nodes.len() as i64) < nodes.total_count
}

fn github_actor_login(author: Option<GithubGraphqlActor>) -> String {
    author
        .map(|author| author.login)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn github_comment_id(kind: &str, id: &str, author: &str, created_at: &str) -> String {
    let id = id.trim();
    if !id.is_empty() {
        return id.to_string();
    }
    format!("{kind}:{author}:{created_at}")
}

fn github_issue_comment(comment: GithubGraphqlComment) -> Option<GitHubWorkItemComment> {
    github_mapped_comment(comment, "comment", "", false)
}

fn github_mapped_comment(
    comment: GithubGraphqlComment,
    kind: &str,
    fallback_path: &str,
    resolved: bool,
) -> Option<GitHubWorkItemComment> {
    if comment.is_minimized {
        return None;
    }
    let author = github_actor_login(comment.author);
    let created_at = comment.created_at.trim().to_string();
    let path = if comment.path.trim().is_empty() {
        fallback_path.trim().to_string()
    } else {
        comment.path.trim().to_string()
    };
    Some(GitHubWorkItemComment {
        id: github_comment_id(kind, &comment.id, &author, &created_at),
        kind: kind.to_string(),
        author_avatar_url: github_avatar_url(&author),
        author,
        body: comment.body,
        created_at,
        url: comment.url,
        state: String::new(),
        path,
        line: comment.line.or(comment.original_line),
        resolved,
        thread_id: String::new(),
        replies: Vec::new(),
    })
}

fn github_review_comment(review: GithubGraphqlReview) -> Option<GitHubWorkItemComment> {
    let state = review.state.trim().to_uppercase();
    if state.is_empty() || state == "PENDING" {
        return None;
    }
    let body = review.body.trim();
    if state == "COMMENTED" && body.is_empty() {
        return None;
    }
    let created_at = review
        .submitted_at
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_string();
    if created_at.is_empty() {
        return None;
    }
    let author = github_actor_login(review.author);
    Some(GitHubWorkItemComment {
        id: github_comment_id("review", &review.id, &author, &created_at),
        kind: "review".into(),
        author_avatar_url: github_avatar_url(&author),
        author,
        body: review.body,
        created_at,
        url: review.url,
        state,
        path: String::new(),
        line: None,
        resolved: false,
        thread_id: String::new(),
        replies: Vec::new(),
    })
}

fn github_review_thread(thread: GithubGraphqlReviewThread) -> Option<GitHubWorkItemComment> {
    let thread_id = thread.id.trim().to_string();
    let mut mapped = thread.comments.nodes.into_iter().filter_map(|comment| {
        github_mapped_comment(comment, "review_comment", &thread.path, thread.is_resolved)
    });
    let mut first = mapped.next()?;
    first.thread_id = thread_id.clone();
    first.replies = mapped
        .map(|mut reply| {
            reply.thread_id = thread_id.clone();
            reply
        })
        .collect();
    Some(first)
}

fn github_avatar_url(login: &str) -> String {
    let login = login.trim();
    if login.is_empty() {
        return String::new();
    }
    let mut encoded = String::with_capacity(login.len());
    for byte in login.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    format!("https://avatars.githubusercontent.com/{encoded}?s=64")
}

const PR_FULL_CONTEXT_LINES: &str = "999999";

fn git_github_pr_diff_for(
    root: &Path,
    repo: &str,
    number: i64,
    full_context: bool,
) -> Result<GitHubPrDiff, String> {
    if number <= 0 {
        return Err("Invalid pull request number".into());
    }
    let (owner, name) = split_github_repo(repo)?;
    let repo = format!("{owner}/{name}");
    let number = number.to_string();
    let fields = if full_context {
        "files,additions,deletions,baseRefOid,headRefOid"
    } else {
        "files,additions,deletions"
    };
    let json = gh_run(
        root,
        &["pr", "view", &number, "--repo", &repo, "--json", fields],
        false,
    )?;
    let mut diff = parse_github_pr_diff_meta(&json)?;
    let (patch, truncated) = if full_context {
        let (base, head) = parse_github_pr_oids(&json)?;
        git_diff_full_context(root, &base, &head)?
    } else {
        (
            gh_run(root, &["pr", "diff", &number, "--repo", &repo], true)?,
            false,
        )
    };
    if truncated || patch.len() > MAX_PR_DIFF_BYTES {
        diff.truncated = true;
    } else {
        diff.patch = patch;
    }
    if diff.additions == 0 && diff.deletions == 0 {
        diff.additions = diff.files.iter().map(|file| file.additions).sum();
        diff.deletions = diff.files.iter().map(|file| file.deletions).sum();
    }
    Ok(diff)
}

fn parse_github_pr_oids(json: &str) -> Result<(String, String), String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Row {
        base_ref_oid: String,
        head_ref_oid: String,
    }
    let row: Row = serde_json::from_str(json).map_err(|error| error.to_string())?;
    let base = row.base_ref_oid.trim();
    let head = row.head_ref_oid.trim();
    if base.is_empty() || head.is_empty() {
        return Err("Pull request is missing base or head commit".into());
    }
    Ok((base.to_string(), head.to_string()))
}

fn git_github_pr_checks_for(
    root: &Path,
    repo: &str,
    number: i64,
) -> Result<GitHubPrChecks, String> {
    let args = github_pr_checks_args(repo, number)?;
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let json = gh_checked(root, &refs)?;
    parse_github_pr_checks(&json)
}

fn github_pr_checks_args(repo: &str, number: i64) -> Result<Vec<String>, String> {
    if number <= 0 {
        return Err("GitHub pull request number must be positive".into());
    }
    let (owner, name) = split_github_repo(repo)?;
    let repo = format!("{owner}/{name}");
    let number = number.to_string();
    Ok(vec![
        "pr".into(),
        "view".into(),
        number,
        "--repo".into(),
        repo,
        "--json".into(),
        "headRefOid,statusCheckRollup".into(),
    ])
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GitHubStatusCheckRow {
    #[serde(default, rename = "__typename")]
    typename: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    context: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    state: String,
    #[serde(default)]
    workflow_name: String,
    #[serde(default)]
    details_url: Option<String>,
    #[serde(default)]
    target_url: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    started_at: Option<String>,
    #[serde(default)]
    completed_at: Option<String>,
}

fn parse_github_pr_checks(json: &str) -> Result<GitHubPrChecks, String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Row {
        #[serde(default)]
        head_ref_oid: String,
        #[serde(default)]
        status_check_rollup: Option<Vec<GitHubStatusCheckRow>>,
    }
    let row: Row = serde_json::from_str(json).map_err(|error| error.to_string())?;
    let head_oid = row.head_ref_oid.trim().to_string();
    if head_oid.is_empty() {
        return Err("Pull request is missing head commit".into());
    }
    Ok(GitHubPrChecks {
        head_oid,
        checks: row
            .status_check_rollup
            .unwrap_or_default()
            .into_iter()
            .map(github_pr_check_from_row)
            .collect(),
    })
}

fn github_pr_check_from_row(row: GitHubStatusCheckRow) -> GitHubPrCheck {
    let is_status_context = if row.typename.is_empty() {
        !row.context.is_empty()
    } else {
        row.typename.eq_ignore_ascii_case("StatusContext")
    };
    if is_status_context {
        return GitHubPrCheck {
            name: row.context,
            workflow: String::new(),
            state: github_check_conclusion_state(&row.state),
            url: row.target_url.filter(|url| !url.trim().is_empty()),
            started_at: row.created_at,
            completed_at: None,
        };
    }
    GitHubPrCheck {
        name: row.name,
        workflow: row.workflow_name,
        state: github_check_state(&row.status, row.conclusion.as_deref().unwrap_or_default()),
        url: row.details_url.filter(|url| !url.trim().is_empty()),
        started_at: row.started_at,
        completed_at: row.completed_at,
    }
}

/// An unfinished CheckRun reports its execution state instead of a stale
/// conclusion. Only a completed or entirely missing status trusts the
/// conclusion; any other nonempty status stays unknown.
fn github_check_state(status: &str, conclusion: &str) -> String {
    match status.trim().to_ascii_uppercase().as_str() {
        "QUEUED" | "IN_PROGRESS" | "PENDING" | "WAITING" | "REQUESTED" => {
            return "pending".into();
        }
        "COMPLETED" | "" => return github_check_conclusion_state(conclusion),
        _ => {}
    }
    "unknown".into()
}

fn github_check_conclusion_state(value: &str) -> String {
    match value.trim().to_ascii_uppercase().as_str() {
        "SUCCESS" => "pass".into(),
        "FAILURE" | "ERROR" | "TIMED_OUT" | "ACTION_REQUIRED" | "STARTUP_FAILURE" => "fail".into(),
        "QUEUED" | "IN_PROGRESS" | "PENDING" | "WAITING" | "REQUESTED" => "pending".into(),
        "NEUTRAL" | "SKIPPED" => "skipping".into(),
        "CANCELLED" => "cancel".into(),
        _ => "unknown".into(),
    }
}

fn git_diff_full_context(root: &Path, base: &str, head: &str) -> Result<(String, bool), String> {
    ensure_git_commit(root, base)?;
    ensure_git_commit(root, head)?;
    ensure_merge_base(root, base, head)?;
    let context = format!("-U{PR_FULL_CONTEXT_LINES}");
    let three_dot = format!("{base}...{head}");
    let (bytes, truncated) = git_output_capped(
        root,
        &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            &context,
            &three_dot,
        ],
        MAX_PR_DIFF_BYTES,
        None,
    )
    .ok_or_else(|| format!("git diff failed for {base}...{head}"))?;
    if truncated {
        return Ok((String::new(), true));
    }
    Ok((String::from_utf8_lossy(&bytes).into_owned(), false))
}

fn ensure_merge_base(root: &Path, base: &str, head: &str) -> Result<(), String> {
    if merge_base_exists(root, base, head) {
        return Ok(());
    }
    if let Some(remote) = github_fetch_remote(root) {
        for deepen in ["50", "200", "800"] {
            let _ = git_output(root, &["fetch", "--no-tags", "--deepen", deepen, &remote]);
            if merge_base_exists(root, base, head) {
                return Ok(());
            }
        }
    }
    Err(format!(
        "Cannot find a merge base for {base} and {head}. Fetch more history and try again."
    ))
}

fn merge_base_exists(root: &Path, base: &str, head: &str) -> bool {
    git_output(root, &["merge-base", base, head]).is_some()
}

fn ensure_git_commit(root: &Path, oid: &str) -> Result<(), String> {
    let spec = format!("{oid}^{{commit}}");
    if git_output(root, &["cat-file", "-e", &spec]).is_some() {
        return Ok(());
    }
    if let Some(remote) = github_fetch_remote(root) {
        let _ = git_output(root, &["fetch", "--no-tags", "--depth", "1", &remote, oid]);
    }
    if git_output(root, &["cat-file", "-e", &spec]).is_some() {
        return Ok(());
    }
    Err(format!(
        "Missing git commit {oid}. Fetch the pull request refs and try again."
    ))
}

fn github_fetch_remote(root: &Path) -> Option<String> {
    if let Some(name) = gh_resolved_remote(root) {
        return Some(name);
    }
    if let Some(url) = gh_repo_view_url(root) {
        if let Some(name) = remote_matching_github_url(root, &url) {
            return Some(name);
        }
    }
    git_remote_name(root)
}

fn gh_resolved_remote(root: &Path) -> Option<String> {
    let listed = git_stdout(
        root,
        &["config", "--get-regexp", r"remote\..*\.gh-resolved"],
    )?;
    for line in listed.lines() {
        let key = line.split_whitespace().next()?;
        let name = key.strip_prefix("remote.")?.strip_suffix(".gh-resolved")?;
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    None
}

fn gh_repo_view_url(root: &Path) -> Option<String> {
    let text = gh_stdout(root, &["repo", "view", "--json", "url"])?;
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()?
        .get("url")?
        .as_str()
        .map(str::to_owned)
        .filter(|url| !url.trim().is_empty())
}

fn remote_matching_github_url(root: &Path, url: &str) -> Option<String> {
    let remotes = git_stdout(root, &["remote", "-v"])?;
    let wanted = normalize_github_remote_url(url);
    for line in remotes.lines() {
        let mut parts = line.split_whitespace();
        let name = parts.next()?;
        let remote_url = parts.next()?;
        if normalize_github_remote_url(remote_url) == wanted {
            return Some(name.to_string());
        }
    }
    None
}

fn normalize_github_remote_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/').trim_end_matches(".git");
    if let Some((_, rest)) = trimmed.split_once("github.com:") {
        return format!(
            "github.com/{}",
            rest.trim_start_matches('/').to_ascii_lowercase()
        );
    }
    if let Some((_, rest)) = trimmed.split_once("github.com/") {
        return format!(
            "github.com/{}",
            rest.trim_start_matches('/').to_ascii_lowercase()
        );
    }
    trimmed.to_ascii_lowercase()
}

fn parse_github_pr_diff_meta(json: &str) -> Result<GitHubPrDiff, String> {
    #[derive(Deserialize)]
    struct FileRow {
        path: String,
        #[serde(default)]
        additions: i64,
        #[serde(default)]
        deletions: i64,
    }
    #[derive(Deserialize)]
    struct Row {
        #[serde(default)]
        additions: i64,
        #[serde(default)]
        deletions: i64,
        #[serde(default)]
        files: Vec<FileRow>,
    }
    let row: Row = serde_json::from_str(json).map_err(|error| error.to_string())?;
    Ok(GitHubPrDiff {
        additions: row.additions,
        deletions: row.deletions,
        files: row
            .files
            .into_iter()
            .map(|file| GitHubPrFile {
                path: file.path,
                additions: file.additions,
                deletions: file.deletions,
            })
            .collect(),
        patch: String::new(),
        truncated: false,
    })
}

fn parse_github_work_items(
    json: &str,
    kind: &str,
    repo: &str,
) -> Result<Vec<GitHubWorkItem>, String> {
    #[derive(Deserialize)]
    struct RowLabel {
        name: String,
        #[serde(default)]
        color: String,
    }
    #[derive(Deserialize)]
    struct RowAssignee {
        login: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Row {
        number: i64,
        title: String,
        url: String,
        state: String,
        #[serde(default)]
        state_reason: String,
        #[serde(default)]
        created_at: String,
        #[serde(default)]
        updated_at: String,
        #[serde(default)]
        labels: Vec<RowLabel>,
        #[serde(default)]
        assignees: Vec<RowAssignee>,
        #[serde(default)]
        is_draft: bool,
    }
    let rows: Vec<Row> = serde_json::from_str(json).map_err(|error| error.to_string())?;
    Ok(rows
        .into_iter()
        .map(|row| GitHubWorkItem {
            kind: kind.to_string(),
            number: row.number,
            title: row.title,
            url: row.url,
            state: row.state.to_lowercase(),
            state_reason: row.state_reason.to_lowercase(),
            created_at: row.created_at,
            updated_at: row.updated_at,
            labels: row
                .labels
                .into_iter()
                .map(|label| GitHubLabel {
                    name: label.name,
                    color: label.color,
                })
                .collect(),
            assignees: row
                .assignees
                .into_iter()
                .map(|assignee| GitHubAssignee {
                    avatar_url: github_avatar_url(&assignee.login),
                    login: assignee.login,
                })
                .collect(),
            draft: row.is_draft,
            repo: repo.to_string(),
        })
        .collect())
}

fn parse_github_work_item(json: &str, kind: &str, repo: &str) -> Result<GitHubWorkItem, String> {
    let wrapped = format!("[{json}]");
    parse_github_work_items(&wrapped, kind, repo)?
        .into_iter()
        .next()
        .ok_or_else(|| "GitHub did not return a work item".into())
}

fn parse_gh_pr_list(json: &str) -> Option<GitPr> {
    #[derive(Deserialize)]
    struct Row {
        number: i64,
        title: String,
        url: String,
        state: String,
    }
    let rows: Vec<Row> = serde_json::from_str(json).ok()?;
    let mut best: Option<GitPr> = None;
    for row in rows {
        let pr = GitPr {
            number: row.number,
            title: row.title,
            url: row.url,
            state: row.state.to_lowercase(),
        };
        if pr.state == "open" {
            return Some(pr);
        }
        if best.is_none() {
            best = Some(pr);
        }
    }
    best
}

fn git_pr_create_for(root: &Path, input: &GitPrCreateInput) -> Result<String, String> {
    let title = input.title.trim();
    if title.is_empty() {
        return Err("Pull request title cannot be empty".into());
    }
    let result = with_temp_markdown(input.body.trim(), |body_path| {
        gh_checked(
            root,
            &[
                "pr",
                "create",
                "--title",
                title,
                "--body-file",
                body_path,
                "--base",
                input.base.trim(),
                "--head",
                input.head.trim(),
            ],
        )
    });
    result.and_then(|output| {
        output
            .lines()
            .rev()
            .find(|line| line.starts_with("http://") || line.starts_with("https://"))
            .map(|line| line.trim().to_string())
            .ok_or_else(|| {
                if output.trim().is_empty() {
                    "gh returned no pull request URL".into()
                } else {
                    output
                }
            })
    })
}

fn gh_stdout(root: &Path, args: &[&str]) -> Option<String> {
    gh_run(root, args, false).ok()
}

fn gh_checked(root: &Path, args: &[&str]) -> Result<String, String> {
    gh_run(root, args, false)
}

struct GitHubRateLimitBackoff {
    until: SystemTime,
    error: String,
}

// Shared by all webviews, including background Inbox and PR checks requests.
static GITHUB_RATE_LIMIT_BACKOFF: Mutex<Option<GitHubRateLimitBackoff>> = Mutex::new(None);

fn gh_run(root: &Path, args: &[&str], allow_empty: bool) -> Result<String, String> {
    gh_with_backoff(
        &GITHUB_RATE_LIMIT_BACKOFF,
        args,
        allow_empty,
        |args, allow_empty| gh_run_raw(root, args, allow_empty),
    )
}

fn github_rate_limit_error(
    backoff: &mut Option<GitHubRateLimitBackoff>,
    now: SystemTime,
) -> Option<String> {
    if let Some(active) = backoff.as_ref().filter(|active| now < active.until) {
        return Some(active.error.clone());
    }
    *backoff = None;
    None
}

fn gh_with_backoff(
    backoff: &Mutex<Option<GitHubRateLimitBackoff>>,
    args: &[&str],
    allow_empty: bool,
    mut run: impl FnMut(&[&str], bool) -> Result<String, String>,
) -> Result<String, String> {
    if let Ok(mut slot) = backoff.lock() {
        if let Some(error) = github_rate_limit_error(&mut slot, SystemTime::now()) {
            return Err(error);
        }
    }
    let result = run(args, allow_empty);
    let Err(error) = &result else {
        return result;
    };
    let message = error.to_lowercase();
    let primary = message.contains("api rate limit") && message.contains("exceeded");
    let secondary = message.contains("secondary rate limit") || message.contains("abuse detection");
    if !primary && !secondary {
        return result;
    }
    // Stop other requests immediately and resolve the reset once. If GitHub
    // cannot return it, retry after a minute rather than hammering the API.
    if let Ok(mut slot) = backoff.lock() {
        if github_rate_limit_error(&mut slot, SystemTime::now()).is_some() {
            return result;
        }
        *slot = Some(GitHubRateLimitBackoff {
            until: SystemTime::now() + Duration::from_secs(60),
            error: error.clone(),
        });
    } else {
        return result;
    }
    if primary && message.contains("graphql") {
        // The REST /rate_limit endpoint can disagree with the live GraphQL
        // quota. Query the same resource that reported the exhausted budget.
        let reset = run(
            &[
                "api",
                "graphql",
                "-f",
                "query=query { rateLimit { remaining resetAt } }",
            ],
            false,
        )
        .and_then(|json| parse_github_rate_limit_backoff(&json));
        if let Ok(until) = reset {
            if let Ok(mut slot) = backoff.lock() {
                *slot = until
                    .filter(|until| *until > SystemTime::now())
                    .map(|until| GitHubRateLimitBackoff {
                        until,
                        error: error.clone(),
                    });
            }
        }
    }
    result
}

fn parse_github_rate_limit_backoff(json: &str) -> Result<Option<SystemTime>, String> {
    let response: serde_json::Value =
        serde_json::from_str(json).map_err(|error| error.to_string())?;
    let rate = &response["data"]["rateLimit"];
    let remaining = rate["remaining"]
        .as_u64()
        .ok_or("GitHub did not return its remaining quota")?;
    if remaining > 0 {
        return Ok(None);
    }
    let reset = rate["resetAt"]
        .as_str()
        .ok_or("GitHub did not return its rate-limit reset")?;
    let reset = time::OffsetDateTime::parse(reset, &time::format_description::well_known::Rfc3339)
        .map_err(|error| error.to_string())?
        .unix_timestamp();
    let seconds = u64::try_from(reset).map_err(|error| error.to_string())?;
    UNIX_EPOCH
        .checked_add(Duration::from_secs(seconds.saturating_add(1)))
        .map(Some)
        .ok_or_else(|| "Invalid GitHub rate-limit reset".into())
}

fn gh_run_raw(root: &Path, args: &[&str], allow_empty: bool) -> Result<String, String> {
    let program = crate::harness::resolve_gui_binary("gh")
        .ok_or_else(|| "GitHub CLI (`gh`) is not installed.".to_string())?;
    let mut cmd = Command::new(&program);
    cmd.current_dir(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_PAGER", "cat")
        .env("GIT_PAGER", "cat");
    crate::harness::apply_gui_env(&mut cmd);
    crate::hide_window_console(&mut cmd);
    let output = cmd.output().map_err(|error| {
        if error.kind() == ErrorKind::NotFound {
            "GitHub CLI (`gh`) is not installed.".to_string()
        } else {
            error.to_string()
        }
    })?;
    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if text.is_empty() {
            if allow_empty {
                return Ok(String::new());
            }
            return Err("gh returned no output".into());
        }
        return Ok(text);
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("gh {} failed", args.join(" "))
    };
    Err(detail)
}

pub(crate) fn resolve_repo_path(root: &Path, relative: &str) -> Result<String, String> {
    let relative = normalize_diff_path(relative);
    if relative.is_empty()
        || relative.starts_with('/')
        || relative
            .split('/')
            .any(|part| part.is_empty() || part == "..")
    {
        return Err("Invalid path".into());
    }
    let abs = root.join(&relative);
    if !abs.starts_with(root) {
        return Err("Invalid path".into());
    }
    Ok(relative)
}

/// Any git can start a filter: in an LFS repo `status`, `diff`, `add`, and
/// checkouts all run `git-lfs`, which a Finder launch's bare PATH lacks. The
/// cached search path finds it without waiting on the login shell.
fn git_cmd() -> Command {
    let mut cmd = Command::new("git");
    crate::background_work::configure_child(&mut cmd);
    crate::hide_window_console(&mut cmd);
    cmd.env("PATH", crate::harness::cached_gui_search_path());
    cmd
}

fn git_cmd_for_args_with_path(args: &[&str], gui_path: impl FnOnce() -> String) -> Command {
    let mut cmd = git_cmd();
    let args = args.strip_prefix(&["--literal-pathspecs"]).unwrap_or(args);
    if matches!(
        args.first().copied(),
        Some("commit" | "push" | "pull" | "fetch" | "clone")
    ) {
        // Signers, hooks, and credential helpers may only be on the login-shell PATH.
        cmd.env("PATH", gui_path());
    }
    cmd
}

fn git_cmd_for_args(args: &[&str]) -> Command {
    git_cmd_for_args_with_path(args, crate::harness::gui_search_path)
}

pub(crate) fn git_checked(root: &Path, args: &[&str]) -> Result<(), String> {
    let mut cmd = git_cmd_for_args(args);
    let output = cmd
        .arg("--no-pager")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let msg = stderr.trim();
    if !msg.is_empty() {
        return Err(msg.to_string());
    }
    let msg = stdout.trim();
    if !msg.is_empty() {
        return Err(msg.to_string());
    }
    Err(format!("git {} failed", args.join(" ")))
}

fn git_blob(root: &Path, spec: &str) -> Option<Vec<u8>> {
    git_output(root, &["cat-file", "-p", spec])
}

fn git_run(root: &Path, args: &[&str]) -> Option<String> {
    git_output(root, args).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

fn git_output(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = git_cmd_for_args(args)
        .arg("--no-pager")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .ok()?;
    if git_status_ok(&output.status, args) {
        return Some(output.stdout);
    }
    None
}

/// Queued 8 KiB chunks allowed between the git reader and the consumer.
///
/// `max_bytes` caps the buffer being built, not what the reader can run ahead:
/// with an unbounded channel a slow or descheduled consumer lets the reader
/// queue output with no limit at all. A small bound instead lets git's own
/// stdout pipe (16 KiB on macOS, 64 KiB on Linux) fill and stall the child, so
/// a read now holds `max_bytes` plus this queue, one scratch chunk, and the
/// kernel's pipe — not `max_bytes` alone.
///
/// Cancelling is unaffected: the receiver is dropped on return, which unblocks
/// a reader parked in `send`. A reader parked in `read` instead is a different
/// case, and `stop` explains why that one is left detached.
const GIT_OUTPUT_QUEUE_CHUNKS: usize = 16;

pub(crate) fn git_output_capped(
    root: &Path,
    args: &[&str],
    max_bytes: usize,
    cancel: Option<&AtomicBool>,
) -> Option<(Vec<u8>, bool)> {
    let mut child = git_cmd()
        .arg("--no-pager")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    let (sender, receiver) = mpsc::sync_channel(GIT_OUTPUT_QUEUE_CHUNKS);
    let reader = thread::Builder::new()
        .name("git-output-reader".to_string())
        .spawn(move || {
            let mut chunk = vec![0u8; 8192];
            loop {
                match stdout.read(&mut chunk) {
                    Ok(0) => {
                        let _ = sender.send(Ok(Vec::new()));
                        break;
                    }
                    Ok(read) => {
                        if sender.send(Ok(chunk[..read].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(Err(error));
                        break;
                    }
                }
            }
        });
    let mut reader = match reader {
        Ok(reader) => Some(reader),
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
    };
    let stop = |child: &mut std::process::Child, reader: &mut Option<thread::JoinHandle<()>>| {
        let _ = child.kill();
        let _ = child.wait();
        // Dropping the handle detaches the reader. A killed git process can leave
        // a shell descendant holding the pipe, so joining here would re-block the
        // cancelled search until that unrelated descendant exits.
        reader.take();
    };

    let mut buf = Vec::new();
    loop {
        if cancel.is_some_and(|token| token.load(Ordering::Acquire)) {
            stop(&mut child, &mut reader);
            return None;
        }
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(Ok(chunk)) if chunk.is_empty() => break,
            Ok(Ok(chunk)) => {
                let remaining = max_bytes.saturating_sub(buf.len());
                if chunk.len() > remaining {
                    buf.extend_from_slice(&chunk[..remaining]);
                    stop(&mut child, &mut reader);
                    return Some((buf, true));
                }
                buf.extend_from_slice(&chunk);
            }
            Ok(Err(_)) => {
                stop(&mut child, &mut reader);
                return None;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if cancel.is_some_and(|token| token.load(Ordering::Acquire)) {
                    stop(&mut child, &mut reader);
                    return None;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                stop(&mut child, &mut reader);
                return None;
            }
        }
    }
    let _ = reader.take().map(thread::JoinHandle::join);
    let status = child.wait().ok()?;
    if git_status_ok(&status, args) {
        Some((buf, false))
    } else {
        None
    }
}

fn git_status_ok(status: &std::process::ExitStatus, args: &[&str]) -> bool {
    status.success()
        || (status.code() == Some(1) && matches!(args.first().copied(), Some("diff" | "grep")))
}

fn git_branch(root: &Path) -> Option<String> {
    git_head_branch(root).or_else(|| git_stdout(root, &["rev-parse", "--short", "HEAD"]))
}

fn git_head_branch(root: &Path) -> Option<String> {
    git_stdout(root, &["symbolic-ref", "--short", "HEAD"]).filter(|branch| branch != "HEAD")
}

fn git_is_work_tree(root: &Path) -> bool {
    git_stdout(root, &["rev-parse", "--is-inside-work-tree"]).as_deref() == Some("true")
}

fn git_branches_for(root: &Path) -> GitBranches {
    if !git_is_work_tree(root) {
        return GitBranches::default();
    }

    let current_branch = git_head_branch(root);
    let head_sha = git_stdout(root, &["rev-parse", "--short", "HEAD"]);
    let detached = current_branch.is_none() && head_sha.is_some();
    let current = current_branch.clone().or(head_sha);

    let mut branches = Vec::new();
    let mut local_names = HashSet::new();
    if let Some(text) = git_run(
        root,
        &[
            "for-each-ref",
            "--format=%(refname:short)\t%(HEAD)",
            "refs/heads",
        ],
    ) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let (name, head) = line.split_once('\t').unwrap_or((line, ""));
            if name.is_empty() {
                continue;
            }
            local_names.insert(name.to_string());
            branches.push(GitBranchEntry {
                name: name.to_string(),
                current: head.trim() == "*",
                remote: None,
            });
        }
    }

    if let Some(name) = &current_branch {
        if !local_names.contains(name) {
            local_names.insert(name.clone());
            branches.push(GitBranchEntry {
                name: name.clone(),
                current: true,
                remote: None,
            });
        }
    }

    if let Some(text) = git_run(
        root,
        &["for-each-ref", "--format=%(refname:short)", "refs/remotes"],
    ) {
        for line in text.lines() {
            let full = line.trim();
            if full.is_empty() {
                continue;
            }
            let Some((remote, name)) = full.split_once('/') else {
                continue;
            };
            if remote.is_empty() || name.is_empty() || name == "HEAD" || name.ends_with("/HEAD") {
                continue;
            }
            if local_names.contains(name) {
                continue;
            }
            branches.push(GitBranchEntry {
                name: name.to_string(),
                current: false,
                remote: Some(remote.to_string()),
            });
        }
    }

    branches.sort_by(|a, b| {
        b.current
            .cmp(&a.current)
            .then(a.remote.is_some().cmp(&b.remote.is_some()))
            .then_with(|| {
                a.name
                    .to_ascii_lowercase()
                    .cmp(&b.name.to_ascii_lowercase())
            })
            .then_with(|| a.remote.cmp(&b.remote))
    });

    GitBranches {
        current,
        detached,
        branches,
    }
}

fn git_checkout_for(root: &Path, name: &str, remote: Option<&str>) -> Result<String, String> {
    if !git_is_work_tree(root) {
        return Err("Not a git repository".into());
    }
    let name = git_branch_name(root, name)?;
    if let Some(remote) = remote.map(str::trim).filter(|value| !value.is_empty()) {
        if git_head_branch(root).as_deref() == Some(name.as_str()) {
            return Ok(name);
        }
        git_switch(root, &["checkout", "--track", &format!("{remote}/{name}")])?;
        return Ok(name);
    }
    if git_head_branch(root).as_deref() == Some(name.as_str()) {
        return Ok(name);
    }
    if git_ref_exists(root, &format!("refs/heads/{name}")) {
        git_switch(root, &["checkout", &name])?;
        return Ok(name);
    }
    if let Some(remote) = git_remote_name(root) {
        let spec = format!("refs/remotes/{remote}/{name}");
        if git_ref_exists(root, &spec) {
            git_switch(root, &["checkout", "--track", &format!("{remote}/{name}")])?;
            return Ok(name);
        }
    }
    Err(format!("Branch {name} not found"))
}

fn git_create_branch_for(root: &Path, name: &str) -> Result<String, String> {
    if !git_is_work_tree(root) {
        return Err("Not a git repository".into());
    }
    let name = git_branch_name(root, name)?;
    if git_ref_exists(root, &format!("refs/heads/{name}"))
        || git_head_branch(root).as_deref() == Some(name.as_str())
    {
        return Err(format!("Branch {name} already exists"));
    }
    git_switch(root, &["checkout", "-b", &name])?;
    Ok(name)
}

fn git_stash_for(root: &Path, message: Option<&str>) -> Result<(), String> {
    if !git_is_work_tree(root) {
        return Err("Not a git repository".into());
    }
    match message.map(str::trim).filter(|value| !value.is_empty()) {
        Some(message) => git_checked(
            root,
            &["stash", "push", "--include-untracked", "-m", message],
        ),
        None => git_checked(root, &["stash", "push", "--include-untracked"]),
    }
}

fn git_switch(root: &Path, args: &[&str]) -> Result<(), String> {
    git_checked(root, args).map_err(map_local_changes_err)
}

fn map_local_changes_err(err: String) -> String {
    if checkout_blocked_by_changes(&err) {
        "Your local changes would be overwritten. Commit or stash them first.".into()
    } else {
        err
    }
}

fn checkout_blocked_by_changes(err: &str) -> bool {
    let text = err.to_ascii_lowercase();
    text.contains("would be overwritten")
        || text.contains("commit your changes or stash")
        || text.contains("please move or remove them before")
}

fn git_branch_name(root: &Path, name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Branch name cannot be empty".into());
    }
    let output = git_cmd()
        .arg("-C")
        .arg(root)
        .args(["check-ref-format", "--branch", name])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!("'{name}' is not a valid branch name"));
    }
    let normalized = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if normalized.is_empty() {
        return Err(format!("'{name}' is not a valid branch name"));
    }
    Ok(normalized)
}

fn git_origin_repo(root: &Path) -> Option<String> {
    git_url_repo_name(&git_stdout(root, &["remote", "get-url", "origin"])?)
}

#[derive(Default)]
struct GitSync {
    remote: Option<String>,
    upstream: Option<String>,
    default_branch: Option<String>,
    ahead: i64,
    behind: i64,
    ahead_of_default: i64,
    head_pushed: bool,
}

fn git_sync_for(root: &Path) -> GitSync {
    let remote = git_remote_name(root);
    let upstream = git_stdout(root, &["rev-parse", "--abbrev-ref", "@{upstream}"]);
    let default_branch = git_default_branch(root, remote.as_deref());
    let default_ref = match (&remote, &default_branch) {
        (Some(remote), Some(branch)) => Some(format!("{remote}/{branch}")),
        _ => None,
    };
    let (ahead, behind) = if upstream.is_some() {
        git_ahead_behind(root, "@{upstream}")
    } else if let Some(base) = default_ref.as_deref() {
        git_ahead_behind(root, base)
    } else {
        (0, 0)
    };
    let ahead_of_default = if let Some(base) = default_ref.as_deref() {
        git_ahead_behind(root, base).0
    } else {
        ahead
    };
    let head_pushed = git_stdout(
        root,
        &[
            "for-each-ref",
            "--count=1",
            "--contains",
            "HEAD",
            "refs/remotes",
        ],
    )
    .is_some();
    GitSync {
        remote,
        upstream,
        default_branch,
        ahead,
        behind,
        ahead_of_default,
        head_pushed,
    }
}

fn git_remote_name(root: &Path) -> Option<String> {
    let remotes = git_stdout(root, &["remote"])?;
    let mut names = remotes
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty());
    let first = names.next()?.to_string();
    if first == "origin" || names.any(|name| name == "origin") {
        return Some("origin".into());
    }
    Some(first)
}

fn git_default_branch(root: &Path, remote: Option<&str>) -> Option<String> {
    if let Some(remote) = remote {
        if let Some(head) = git_stdout(
            root,
            &[
                "symbolic-ref",
                "--short",
                &format!("refs/remotes/{remote}/HEAD"),
            ],
        ) {
            if let Some((_, name)) = head.split_once('/') {
                return Some(name.to_string());
            }
            return Some(head);
        }
        for name in ["main", "master"] {
            if git_ref_exists(root, &format!("refs/remotes/{remote}/{name}")) {
                return Some(name.to_string());
            }
        }
    }
    for name in ["main", "master"] {
        if git_ref_exists(root, &format!("refs/heads/{name}")) {
            return Some(name.to_string());
        }
    }
    None
}

fn git_ref_exists(root: &Path, spec: &str) -> bool {
    git_output(root, &["show-ref", "--verify", "--quiet", spec]).is_some()
}

fn git_ahead_behind(root: &Path, base: &str) -> (i64, i64) {
    let spec = format!("{base}...HEAD");
    let Some(text) = git_stdout(root, &["rev-list", "--left-right", "--count", &spec]) else {
        return (0, 0);
    };
    let mut parts = text.split_whitespace();
    let behind = parts
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let ahead = parts
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    (ahead, behind)
}

fn git_stdout(root: &Path, args: &[&str]) -> Option<String> {
    let output = git_cmd().arg("-C").arg(root).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn file_name(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

/// When a file walk should stop early.
///
/// The flag variant is what a search uses. `AfterEntries` exists so the
/// per-entry check can be tested without a thread and a sleep: setting the flag
/// from outside only proves the walk stops *somewhere*, which the up-front
/// check would satisfy on its own, and timing a real cancellation is flaky on
/// loaded CI.
#[derive(Clone, Copy)]
enum WalkStop<'a> {
    Never,
    Flag(&'a AtomicBool),
    #[cfg(test)]
    AfterEntries(usize),
}

impl<'a> WalkStop<'a> {
    fn stopped(&self, _entries: usize) -> bool {
        match self {
            WalkStop::Never => false,
            WalkStop::Flag(flag) => flag.load(Ordering::Acquire),
            #[cfg(test)]
            WalkStop::AfterEntries(limit) => _entries >= *limit,
        }
    }
}

impl<'a> From<Option<&'a AtomicBool>> for WalkStop<'a> {
    fn from(cancel: Option<&'a AtomicBool>) -> Self {
        match cancel {
            Some(flag) => WalkStop::Flag(flag),
            None => WalkStop::Never,
        }
    }
}

fn walk_project_files(root: &Path, stop: WalkStop<'_>) -> Vec<ProjectFile> {
    let ignore = Ignore::load(root);
    let mut files = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    let mut visited = 0usize;
    let mut seen = 0usize;

    while let Some(dir) = dirs.pop() {
        if stop.stopped(seen) {
            break;
        }
        visited += 1;
        if visited > MAX_WALK_DIRS || files.len() >= MAX_PROJECT_FILES {
            break;
        }
        let Ok(reader) = std::fs::read_dir(&dir) else {
            continue;
        };
        for ent in reader {
            seen += 1;
            if stop.stopped(seen) {
                return files;
            }
            let Ok(ent) = ent else { continue };
            let name = ent.file_name();
            let Some(name) = name.to_str() else { continue };
            if name == ".DS_Store" {
                continue;
            }
            let path = ent.path();
            let is_dir = match ent.file_type() {
                Ok(t) if t.is_symlink() => continue,
                Ok(t) => t.is_dir(),
                Err(_) => path.is_dir(),
            };
            if is_dir {
                if skip_walk_dir_name(name) || ignore.matches(name) || is_private_dir(&path) {
                    continue;
                }
                dirs.push(path);
                continue;
            }
            if ignore.matches(name) {
                continue;
            }
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            let relative = path_to_js(relative);
            files.push(ProjectFile {
                name: name.to_string(),
                path: path_to_js(&path),
                relative,
            });
            if files.len() >= MAX_PROJECT_FILES {
                break;
            }
        }
    }
    files
}

fn skip_walk_dir_name(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | "out"
            | ".next"
            | ".nuxt"
            | ".output"
            | ".cache"
            | ".turbo"
            | ".parcel-cache"
            | ".vercel"
            | ".svelte-kit"
            | "coverage"
            | "__pycache__"
            | ".venv"
            | "venv"
            | ".tox"
            | ".mypy_cache"
            | ".pytest_cache"
            | ".gradle"
            | ".idea"
            | "Pods"
            | "vendor"
            | "bower_components"
            | ".yarn"
            | ".pnpm-store"
    )
}

fn path_has_skipped_dir(relative: &str) -> bool {
    relative
        .split(std::path::is_separator)
        .any(skip_walk_dir_name)
}

/// Directories the OS guards behind a consent prompt. macOS pops "would like to
/// access data from other apps" the first time a process reads another app's
/// container, and the grant is per-folder — so a walk that brushes past a few of
/// them prompts again on every launch. Nothing in here is a user project, so the
/// indexer treats them as if they did not exist.
fn is_private_dir(path: &Path) -> bool {
    if path.extension().is_some_and(|ext| ext == "app") {
        return true;
    }
    if !cfg!(target_os = "macos") {
        return false;
    }
    let guarded = [
        dirs_home().map(|home| PathBuf::from(home).join("Library")),
        dirs_home().map(|home| PathBuf::from(home).join(".Trash")),
        Some(PathBuf::from("/Library")),
        Some(PathBuf::from("/System")),
    ];
    guarded
        .iter()
        .flatten()
        .any(|guarded| path == guarded.as_path())
}

/// Roots too broad to index. Walking a home or volume root is never useful for
/// Quick Open — it buries project files under tens of thousands of dotfiles and
/// caches — and it is the one thing guaranteed to reach a private dir.
fn is_indexable_root(root: &Path) -> bool {
    if is_private_dir(root) {
        return false;
    }
    if root.parent().is_none() {
        return false;
    }
    let too_broad = [
        dirs_home().map(PathBuf::from),
        Some(PathBuf::from("/Users")),
        Some(PathBuf::from("/Applications")),
        Some(PathBuf::from("/Volumes")),
        Some(PathBuf::from("/home")),
    ];
    !too_broad
        .iter()
        .flatten()
        .any(|broad| root == broad.as_path())
}

fn resolve_under(parent: &Path, name: &str) -> Result<PathBuf, String> {
    if name.starts_with('/') || name.starts_with('\\') {
        return Err("A file or folder name cannot start with a slash.".into());
    }

    let trimmed = name.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() || trimmed.chars().all(char::is_whitespace) {
        return Err("A file or folder name must be provided.".into());
    }

    let mut dest = parent.to_path_buf();
    for segment in trimmed.split(['/', '\\']) {
        if segment.is_empty() {
            continue;
        }
        if segment == "." || segment == ".." || segment.len() > 255 {
            return Err(format!(
                "The name {trimmed} is not valid as a file or folder name. Please choose a different name."
            ));
        }
        dest.push(segment);
    }

    if !dest.starts_with(parent) {
        return Err("Invalid path".into());
    }
    Ok(dest)
}

fn file_label(path: &Path, fallback: &str) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(fallback)
        .to_string()
}

fn already_exists(label: &str) -> String {
    format!(
        "A file or folder {label} already exists at this location. Please choose a different name."
    )
}

/// Create a file or folder under `parent`. `name` may contain `/` or `\` to
/// nest. Returns the created path.
#[tauri::command(async)]
pub fn create_path(parent: String, name: String, is_dir: bool) -> Result<String, String> {
    let parent_dir = expand_home(&parent);
    let dest = resolve_under(&parent_dir, &name)?;
    let label = file_label(&dest, &name);

    if dest.exists() {
        return Err(already_exists(&label));
    }

    if is_dir {
        std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    } else {
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::File::create_new(&dest).map_err(|e| {
            if e.kind() == ErrorKind::AlreadyExists {
                already_exists(&label)
            } else {
                e.to_string()
            }
        })?;
    }

    Ok(dest.to_string_lossy().into_owned())
}

pub(crate) fn expand_home(path: &str) -> PathBuf {
    if path == "~" {
        return dirs_home()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(path));
    }
    let rest = path.strip_prefix("~/").or_else(|| {
        if cfg!(windows) {
            path.strip_prefix("~\\")
        } else {
            None
        }
    });
    if let Some(rest) = rest {
        if let Some(home) = dirs_home() {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

pub(crate) fn path_to_js(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.into_owned()
    }
}

#[cfg(all(test, unix))]
#[test]
fn preserves_unix_backslash_filenames() {
    assert_eq!(path_to_js(Path::new(r"/tmp/a\b.txt")), r"/tmp/a\b.txt");
    assert_eq!(expand_home(r"~\literal"), PathBuf::from(r"~\literal"));
}

/// Name-only `.gitignore` subset for directories outside a git repository.
/// Inside a repository `git check-ignore` is the source of truth.
struct Ignore {
    exact: HashSet<String>,
    suffixes: Vec<String>,
}

impl Ignore {
    fn load(from: &Path) -> Self {
        let mut exact = HashSet::from([".git".into()]);
        let mut suffixes = Vec::new();
        let root = project_root(from);
        if let Ok(text) = std::fs::read_to_string(root.join(".gitignore")) {
            for raw in text.lines() {
                let line = raw.trim();
                if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
                    continue;
                }
                let line = line.trim_end_matches('/');
                if line.contains('/') {
                    continue;
                }
                if let Some(ext) = line.strip_prefix("*.") {
                    if !ext.is_empty() && !ext.contains('*') {
                        suffixes.push(format!(".{ext}"));
                    }
                    continue;
                }
                exact.insert(line.to_string());
            }
        }
        Self { exact, suffixes }
    }

    fn matches(&self, name: &str) -> bool {
        self.exact.contains(name) || self.suffixes.iter().any(|s| name.ends_with(s))
    }
}

fn project_root(start: &Path) -> PathBuf {
    let mut dir = start;
    loop {
        if dir.join(".git").exists() || dir.join(".gitignore").exists() {
            return dir.to_path_buf();
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return start.to_path_buf(),
        }
    }
}

/// Clone `url` into `parent`/`<repo-name>` and return the new directory.
#[tauri::command]
pub async fn clone_repo(url: String, parent: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || clone_repo_sync(&url, &parent))
        .await
        .map_err(|e| e.to_string())?
}

fn clone_repo_sync(url: &str, parent: &str) -> Result<String, String> {
    let url = url.trim();
    if !is_git_url(url) {
        return Err("Enter an https, ssh, or git URL".into());
    }
    let name = repo_name(url)?;
    let dest = expand_home(parent).join(&name);
    if dest.exists() {
        return Err(format!("{} already exists", dest.display()));
    }
    let dest_str = dest.to_str().ok_or("Invalid destination path")?;
    let output = git_cmd_for_args(&["clone"])
        .args(["clone", "--", url, dest_str])
        .output()
        .map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                "git is not installed".into()
            } else {
                format!("git clone failed: {e}")
            }
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let msg = stderr
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("git clone failed");
        return Err(msg.trim().to_string());
    }
    Ok(dest.to_string_lossy().into_owned())
}

fn is_git_url(url: &str) -> bool {
    url.starts_with("https://")
        || url.starts_with("http://")
        || url.starts_with("git@")
        || url.starts_with("ssh://")
        || url.starts_with("git://")
}

fn repo_name(url: &str) -> Result<String, String> {
    git_url_repo_name(url).ok_or_else(|| "Could not infer repository name from URL".into())
}

fn git_url_repo_name(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let name = trimmed.rsplit(['/', ':']).next().unwrap_or("").trim();
    if name.is_empty() || name == "." || name == ".." || name.contains(['\\', '/']) {
        return None;
    }
    Some(name.to_string())
}

/// First few lines of a text file for tool previews.
#[tauri::command(async)]
pub fn read_file_preview(
    path: String,
    max_lines: usize,
    start_line: Option<usize>,
) -> Result<Vec<String>, String> {
    use std::io::{BufRead, BufReader};

    let path = expand_home(&path);
    let meta = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err("Not a file".into());
    }

    let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
    let reader = BufReader::new(file);
    let limit = max_lines.clamp(1, 12);
    let start = start_line.unwrap_or(1).max(1);
    let mut lines = Vec::new();
    for (i, line) in reader.lines().enumerate() {
        let line_no = i + 1;
        if line_no < start {
            continue;
        }
        if lines.len() >= limit {
            break;
        }
        let mut line = line.map_err(|e| e.to_string())?;
        if line.contains('\0') {
            return Err("Binary file".into());
        }
        if line.len() > 200 {
            line.truncate(199);
            line.push('…');
        }
        lines.push(line);
    }
    Ok(lines)
}

const MAX_STAT_FILES: usize = 64;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMtime {
    path: String,
    mtime_ms: Option<u64>,
}

fn file_mtime_ms(meta: &std::fs::Metadata) -> Option<u64> {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
}

/// Metadata only — used to notice disk changes on currently open editors.
#[tauri::command(async)]
pub fn stat_files(paths: Vec<String>) -> Result<Vec<FileMtime>, String> {
    if paths.len() > MAX_STAT_FILES {
        return Err("Too many paths".into());
    }
    Ok(paths
        .into_iter()
        .map(|path| {
            let expanded = expand_home(&path);
            let mtime_ms = std::fs::metadata(&expanded)
                .ok()
                .filter(|meta| meta.is_file())
                .and_then(|meta| file_mtime_ms(&meta));
            FileMtime { path, mtime_ms }
        })
        .collect())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathInfo {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
}

/// Metadata for files the composer is attaching (picker, drop, paste).
#[tauri::command(async)]
pub fn inspect_paths(paths: Vec<String>) -> Vec<PathInfo> {
    paths
        .into_iter()
        .filter_map(|path| inspect_path_sync(&path))
        .collect()
}

fn inspect_path_sync(path: &str) -> Option<PathInfo> {
    let path = expand_home(path);
    let meta = std::fs::metadata(&path).ok()?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(path.to_str().unwrap_or("attachment"))
        .to_string();
    Some(PathInfo {
        path: path_to_js(&path),
        name,
        size: meta.len(),
        is_dir: meta.is_dir(),
    })
}

/// Base64-encode a file so vision images can be sent inline over ACP.
#[tauri::command]
pub async fn read_file_base64(path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || read_file_base64_sync(&path))
        .await
        .map_err(|e| e.to_string())?
}

fn read_file_base64_sync(path: &str) -> Result<String, String> {
    let path = expand_home(path);
    let meta = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err("Not a file".into());
    }
    if meta.len() > MAX_ATTACHMENT_EMBED_BYTES {
        return Err(format!(
            "File is too large to attach inline (maximum {} MB).",
            MAX_ATTACHMENT_EMBED_BYTES / 1024 / 1024
        ));
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        bytes,
    ))
}

/// Video containers the chat can stream; mirrors `VIDEO_MIME_TYPES` in the UI.
const STREAMABLE_VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mov", "webm", "ogv", "avi", "mkv", "mpg", "mpeg", "3gp", "3g2",
];

/// Let the asset protocol serve one local video, so a chat can stream it with
/// range requests instead of reading the whole file into memory. Only that
/// file, and only a video container, joins the scope. Returns the canonical
/// path the caller should load.
#[tauri::command]
pub fn allow_video_stream(app: AppHandle, path: String) -> Result<String, String> {
    let path = expand_home(&path)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let video = is_streamable_video(&path);
    if !video || !path.is_file() {
        return Err("Not a video file".into());
    }
    app.asset_protocol_scope()
        .allow_file(&path)
        .map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

fn is_streamable_video(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| STREAMABLE_VIDEO_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// Read a file as raw bytes for the image viewer.
///
/// Returns an `ipc::Response`, which reaches the webview as an ArrayBuffer, so
/// previews skip the 33% base64 inflation that inline attachments pay. The
/// caller decides what the bytes are by sniffing them; this only guards size.
#[tauri::command]
pub async fn read_binary_file(path: String) -> Result<tauri::ipc::Response, String> {
    let bytes = tauri::async_runtime::spawn_blocking(move || read_binary_file_sync(&path))
        .await
        .map_err(|e| e.to_string())??;
    Ok(tauri::ipc::Response::new(bytes))
}

fn read_binary_file_sync(path: &str) -> Result<Vec<u8>, String> {
    let path = expand_home(path);
    let meta = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err("Not a file".into());
    }
    if meta.len() > MAX_PREVIEW_BYTES {
        return Err(format!(
            "File is too large to preview (maximum {} MB).",
            MAX_PREVIEW_BYTES / 1024 / 1024
        ));
    }
    std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Read an image that a Markdown document in a project shows.
///
/// The document is untrusted, so `path` must be relative to `root` and the
/// file must resolve inside it with symlinks followed: a README cannot pull in
/// a file from elsewhere on disk. The caller sniffs the bytes, as for
/// `read_binary_file`.
#[tauri::command]
pub async fn read_project_image(
    root: String,
    path: String,
) -> Result<tauri::ipc::Response, String> {
    let bytes = tauri::async_runtime::spawn_blocking(move || {
        read_project_image_sync(Path::new(&root), Path::new(&path))
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(tauri::ipc::Response::new(bytes))
}

fn read_project_image_sync(root: &Path, path: &Path) -> Result<Vec<u8>, String> {
    use std::path::Component;
    if !path
        .components()
        .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
    {
        return Err("Image path must stay inside the project".into());
    }
    let root = std::fs::canonicalize(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let joined = root.join(path);
    let file = std::fs::canonicalize(&joined).map_err(|e| format!("{}: {e}", joined.display()))?;
    if !file.starts_with(&root) {
        return Err("Image is outside the project".into());
    }
    let handle = std::fs::File::open(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    let meta = handle
        .metadata()
        .map_err(|e| format!("{}: {e}", file.display()))?;
    if !meta.is_file() {
        return Err("Not a file".into());
    }
    let too_large = || {
        format!(
            "Image is too large to show (maximum {} MB).",
            MAX_DOCUMENT_IMAGE_BYTES / 1024 / 1024
        )
    };
    if meta.len() > MAX_DOCUMENT_IMAGE_BYTES {
        return Err(too_large());
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    handle
        .take(MAX_DOCUMENT_IMAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{}: {e}", file.display()))?;
    if bytes.len() as u64 > MAX_DOCUMENT_IMAGE_BYTES {
        return Err(too_large());
    }
    Ok(bytes)
}

/// Persist a pasted blob so non-image attachments have a real path.
#[tauri::command]
pub async fn write_attachment(name: String, data: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || write_attachment_sync(&name, &data))
        .await
        .map_err(|e| e.to_string())?
}

fn write_attachment_sync(name: &str, data: &str) -> Result<String, String> {
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data)
        .map_err(|_| "Attachment data is not valid base64.".to_string())?;
    if bytes.len() as u64 > MAX_ATTACHMENT_EMBED_BYTES {
        return Err(format!(
            "File is too large to attach (maximum {} MB).",
            MAX_ATTACHMENT_EMBED_BYTES / 1024 / 1024
        ));
    }
    let dir = std::env::temp_dir().join("monocode-attachments");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = dir.join(format!(
        "{}-{}-{}",
        std::process::id(),
        stamp,
        safe_attachment_name(name)
    ));
    // A pasted screenshot can be a picture of whatever was on screen, so keep
    // it owner-only rather than at the umask default of 0644 in a shared /tmp.
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(&path)
        .and_then(|mut file| file.write_all(&bytes))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn save_generated_image(
    app: AppHandle,
    data: String,
    name: String,
) -> Result<GeneratedImageAsset, String> {
    tauri::async_runtime::spawn_blocking(move || save_generated_image_sync(&app, &data, &name))
        .await
        .map_err(|e| e.to_string())?
}

fn save_generated_image_sync(
    app: &AppHandle,
    data: &str,
    name: &str,
) -> Result<GeneratedImageAsset, String> {
    if data.len() as u64 > MAX_GENERATED_IMAGE_DATA_BYTES {
        return Err(format!(
            "Generated image is too large (maximum {} MB).",
            MAX_GENERATED_IMAGE_BYTES / 1024 / 1024
        ));
    }
    let encoded: String = data
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace())
        .collect();
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded)
        .map_err(|_| "Generated image data is not valid base64.".to_string())?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_GENERATED_IMAGE_BYTES {
        return Err(format!(
            "Generated image is too large (maximum {} MB).",
            MAX_GENERATED_IMAGE_BYTES / 1024 / 1024
        ));
    }
    if !is_png(&bytes) {
        return Err("Generated image data is not a PNG image.".into());
    }
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join(GENERATED_IMAGE_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let safe_name = safe_attachment_name(name);
    let destination = dir.join(format!("{}-{}.png", Uuid::new_v4(), safe_name));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
        .map_err(|e| format!("{}: {e}", destination.display()))?;
    if let Err(error) = file.write_all(&bytes) {
        let _ = std::fs::remove_file(&destination);
        return Err(format!("{}: {error}", destination.display()));
    }
    Ok(GeneratedImageAsset {
        path: destination.to_string_lossy().into_owned(),
        mime_type: "image/png".into(),
        size: bytes.len() as u64,
    })
}

#[tauri::command]
pub async fn delete_generated_images(app: AppHandle, paths: Vec<String>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || delete_generated_images_sync(&app, &paths))
        .await
        .map_err(|e| e.to_string())?
}

pub(crate) fn delete_generated_images_sync(
    app: &AppHandle,
    paths: &[String],
) -> Result<(), String> {
    let root = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join(GENERATED_IMAGE_DIR)
        .canonicalize()
        .map_err(|error| format!("Generated image directory is unavailable: {error}"))?;
    for path in paths {
        let candidate = match PathBuf::from(path).canonicalize() {
            Ok(candidate) => candidate,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
        };
        if !candidate.starts_with(&root) || !candidate.is_file() {
            return Err("Invalid generated image path".into());
        }
        std::fs::remove_file(candidate).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn is_png(bytes: &[u8]) -> bool {
    bytes.len() >= 8 && bytes.starts_with(&[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])
}

fn safe_attachment_name(name: &str) -> String {
    let leaf = Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("attachment");
    let cleaned: String = leaf
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('.').trim_matches('-');
    if trimmed.is_empty() {
        "attachment".into()
    } else {
        trimmed.chars().take(80).collect()
    }
}

/// Read a reasonably sized UTF-8 file for the editor.
#[tauri::command]
pub async fn read_text_file(path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || read_text_file_sync(&path))
        .await
        .map_err(|e| e.to_string())?
}

fn read_text_file_sync(path: &str) -> Result<String, String> {
    let path = expand_home(path);
    let meta = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err("Not a file".into());
    }
    if meta.len() > MAX_TEXT_FILE_BYTES {
        return Err(format!(
            "File is too large to edit (maximum {} MB).",
            MAX_TEXT_FILE_BYTES / 1024 / 1024
        ));
    }

    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.contains(&0) {
        return Err("Binary files cannot be edited.".into());
    }
    String::from_utf8(bytes).map_err(|_| "File is not valid UTF-8.".into())
}

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EditorTextFile {
    path: String,
    content: String,
    hash: String,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum EditorTextSaveResult {
    Saved { path: String, hash: String },
    Conflict { path: String, hash: Option<String> },
}

fn text_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The digest covers the exact bytes returned, before editor line normalization.
#[tauri::command]
pub async fn read_editor_text_file(path: String) -> Result<EditorTextFile, String> {
    tauri::async_runtime::spawn_blocking(move || read_editor_text_file_sync(&path))
        .await
        .map_err(|e| e.to_string())?
}

fn read_editor_text_file_sync(path: &str) -> Result<EditorTextFile, String> {
    let content = read_text_file_sync(path)?;
    Ok(EditorTextFile {
        path: path.into(),
        hash: text_hash(content.as_bytes()),
        content,
    })
}

fn disk_text_hash(path: &Path) -> Result<Option<String>, String> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Ok(None);
    }
    let mut digest = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            return Ok(Some(format!("{:x}", digest.finalize())));
        }
        digest.update(&buffer[..count]);
    }
}

// Serialize editor commands from our own windows. External programs do not
// participate: the final hash check and rename are not filesystem CAS.
static EDITOR_TEXT_SAVE_LOCK: Mutex<()> = Mutex::new(());

#[tauri::command]
pub async fn write_editor_text_file(
    path: String,
    content: String,
    expected_hash: String,
) -> Result<EditorTextSaveResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        write_editor_text_file_sync(&path, &content, &expected_hash)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn write_editor_text_file_sync(
    path: &str,
    content: &str,
    expected_hash: &str,
) -> Result<EditorTextSaveResult, String> {
    let _guard = EDITOR_TEXT_SAVE_LOCK.lock().map_err(|e| e.to_string())?;
    let requested = expand_home(path);
    let mut observed = disk_text_hash(&requested)?;
    if observed.as_deref() != Some(expected_hash) {
        return Ok(EditorTextSaveResult::Conflict {
            path: path.into(),
            hash: observed,
        });
    }
    let destination = match std::fs::canonicalize(&requested) {
        Ok(destination) => destination,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(EditorTextSaveResult::Conflict {
                path: path.into(),
                hash: None,
            });
        }
        Err(error) => return Err(error.to_string()),
    };
    let published = write_text_file_checked_sync(path, content, || {
        observed = disk_text_hash(&requested)?;
        Ok(observed.as_deref() == Some(expected_hash)
            && std::fs::canonicalize(&requested).ok().as_ref() == Some(&destination))
    })?;
    if !published {
        return Ok(EditorTextSaveResult::Conflict {
            path: path.into(),
            hash: observed,
        });
    }
    Ok(EditorTextSaveResult::Saved {
        path: path.into(),
        hash: text_hash(content.as_bytes()),
    })
}

/// Atomically replace a text file from a temporary file in the same directory.
#[tauri::command]
pub async fn write_text_file(path: String, content: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || write_text_file_sync(&path, &content))
        .await
        .map_err(|e| e.to_string())?
}

fn write_text_file_sync(path: &str, content: &str) -> Result<(), String> {
    write_text_file_checked_sync(path, content, || Ok(true)).map(|_| ())
}

fn write_text_file_checked_sync(
    path: &str,
    content: &str,
    mut before_publish: impl FnMut() -> Result<bool, String>,
) -> Result<bool, String> {
    if content.len() as u64 > MAX_TEXT_FILE_BYTES {
        return Err(format!(
            "File is too large to save (maximum {} MB).",
            MAX_TEXT_FILE_BYTES / 1024 / 1024
        ));
    }

    let requested = expand_home(path);
    let destination = if requested.exists() {
        std::fs::canonicalize(&requested).map_err(|e| format!("{}: {e}", requested.display()))?
    } else {
        requested
    };
    if destination.is_dir() {
        return Err("Cannot save text to a directory.".into());
    }

    let parent = destination
        .parent()
        .ok_or_else(|| "File has no parent directory.".to_string())?;
    let name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "Invalid file name.".to_string())?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    let mut temporary = None;
    for attempt in 0..100 {
        let candidate = parent.join(format!(
            ".{name}.monocode-{}-{stamp}-{attempt}.tmp",
            std::process::id()
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                temporary = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("{}: {error}", candidate.display())),
        }
    }

    let (temporary_path, mut file) =
        temporary.ok_or_else(|| "Could not create a temporary save file.".to_string())?;
    let write_result = (|| -> Result<bool, String> {
        file.write_all(content.as_bytes())
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        if let Ok(meta) = std::fs::metadata(&destination) {
            std::fs::set_permissions(&temporary_path, meta.permissions())
                .map_err(|e| e.to_string())?;
        }
        drop(file);
        if !before_publish()? {
            return Ok(false);
        }
        std::fs::rename(&temporary_path, &destination).map_err(|e| e.to_string())?;
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
        Ok(true)
    })();

    if !matches!(write_result, Ok(true)) {
        let _ = std::fs::remove_file(&temporary_path);
    }
    write_result
}

fn same_entry(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    let Ok(a_meta) = std::fs::metadata(a) else {
        return false;
    };
    let Ok(b_meta) = std::fs::metadata(b) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        a_meta.dev() == b_meta.dev() && a_meta.ino() == b_meta.ino()
    }
    #[cfg(not(unix))]
    {
        let _ = (a_meta, b_meta);
        match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(left), Ok(right)) => left == right,
            _ => false,
        }
    }
}

fn split_stem_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

fn unique_name_in(dir: &Path, name: &str) -> String {
    let (stem, ext) = split_stem_ext(name);
    let mut n = 0u32;
    loop {
        let candidate = match n {
            0 => name.to_string(),
            1 => format!("{stem} copy{ext}"),
            _ => format!("{stem} copy {n}{ext}"),
        };
        if !dir.join(&candidate).exists() {
            return candidate;
        }
        n += 1;
        if n > 1000 {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            return format!("{stem} copy {stamp}{ext}");
        }
    }
}

fn copy_recursive(from: &Path, to: &Path) -> Result<(), String> {
    let meta = std::fs::metadata(from).map_err(|e| format!("{}: {e}", from.display()))?;
    if meta.is_dir() {
        std::fs::create_dir(to).map_err(|e| format!("{}: {e}", to.display()))?;
        for ent in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
            let ent = ent.map_err(|e| e.to_string())?;
            copy_recursive(&ent.path(), &to.join(ent.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to)
            .map(|_| ())
            .map_err(|e| format!("{}: {e}", to.display()))
    }
}

fn rename_path_sync(path: &str, name: &str) -> Result<String, String> {
    let from = expand_home(path);
    if !from.exists() {
        return Err(format!("{}: No such file or directory", from.display()));
    }
    let parent = from
        .parent()
        .ok_or_else(|| "File has no parent directory.".to_string())?;
    let dest = resolve_under(parent, name)?;
    if same_entry(&from, &dest) {
        if from == dest {
            return Ok(from.to_string_lossy().into_owned());
        }
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = parent.join(format!(
            ".{}.monocode-rename-{stamp}",
            file_label(&from, "tmp")
        ));
        std::fs::rename(&from, &tmp).map_err(|e| e.to_string())?;
        if let Err(e) = std::fs::rename(&tmp, &dest) {
            let _ = std::fs::rename(&tmp, &from);
            return Err(e.to_string());
        }
        return Ok(dest.to_string_lossy().into_owned());
    }
    if dest.exists() {
        return Err(already_exists(&file_label(&dest, name)));
    }
    if dest.starts_with(&from) {
        return Err("Cannot move a folder into itself.".into());
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&from, &dest).map_err(|e| e.to_string())?;
    Ok(dest.to_string_lossy().into_owned())
}

/// Rename `path` to `name` (relative to the current parent; `/` nests).
#[tauri::command]
pub async fn rename_path(path: String, name: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || rename_path_sync(&path, &name))
        .await
        .map_err(|e| e.to_string())?
}

fn delete_path_sync(path: &str) -> Result<(), String> {
    let path = expand_home(path);
    if !path.exists() {
        return Err(format!("{}: No such file or directory", path.display()));
    }
    if path.is_dir() {
        std::fs::remove_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))
    } else {
        std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[tauri::command]
pub async fn delete_path(path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || delete_path_sync(&path))
        .await
        .map_err(|e| e.to_string())?
}

fn dir_contains(dir: &Path, dest_parent: &Path) -> bool {
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let dest_parent =
        std::fs::canonicalize(dest_parent).unwrap_or_else(|_| dest_parent.to_path_buf());
    dest_parent.starts_with(&dir)
}

fn copy_path_sync(from: &str, dest_parent: &str) -> Result<String, String> {
    let from = expand_home(from);
    if !from.exists() {
        return Err(format!("{}: No such file or directory", from.display()));
    }
    let dest_parent = expand_home(dest_parent);
    if !dest_parent.is_dir() {
        return Err(format!("{} is not a folder", dest_parent.display()));
    }
    if from.is_dir() && dir_contains(&from, &dest_parent) {
        return Err("Cannot paste a folder into itself.".into());
    }
    let name = unique_name_in(
        &dest_parent,
        &file_label(&from, from.to_str().unwrap_or("copy")),
    );
    let dest = dest_parent.join(&name);
    copy_recursive(&from, &dest)?;
    Ok(dest.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn copy_path(from: String, dest_parent: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || copy_path_sync(&from, &dest_parent))
        .await
        .map_err(|e| e.to_string())?
}

fn move_path_sync(from: &str, dest_parent: &str) -> Result<String, String> {
    let from = expand_home(from);
    if !from.exists() {
        return Err(format!("{}: No such file or directory", from.display()));
    }
    let dest_parent = expand_home(dest_parent);
    if !dest_parent.is_dir() {
        return Err(format!("{} is not a folder", dest_parent.display()));
    }
    if from.is_dir() && dir_contains(&from, &dest_parent) {
        return Err("Cannot paste a folder into itself.".into());
    }
    let name = file_label(&from, from.to_str().unwrap_or("item"));
    let dest = dest_parent.join(&name);
    if same_entry(&from, &dest) {
        return Ok(from.to_string_lossy().into_owned());
    }
    if dest.exists() {
        return Err(already_exists(&name));
    }
    std::fs::rename(&from, &dest).map_err(|e| e.to_string())?;
    Ok(dest.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn move_path(from: String, dest_parent: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || move_path_sync(&from, &dest_parent))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn reveal_path(path: String) -> Result<(), String> {
    // On Linux `xdg-open` can stay alive until the file manager window closes;
    // waiting on it from the main thread hangs the UI.
    tauri::async_runtime::spawn_blocking(move || reveal_path_sync(&path))
        .await
        .map_err(|e| e.to_string())?
}

fn reveal_path_sync(path: &str) -> Result<(), String> {
    let path = expand_home(path);
    if !path.exists() {
        return Err(format!("{}: No such file or directory", path.display()));
    }
    #[cfg(target_os = "macos")]
    {
        let path_str = path.to_str().ok_or_else(|| "Invalid path".to_string())?;
        let status = Command::new("open")
            .args(["-R", path_str])
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err("Could not reveal in Finder.".into());
        }
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // explorer.exe returns 1 even when it opened the folder.
        let path_str = path.to_string_lossy().replace('/', "\\");
        // `.arg` would wrap the whole `/select,...` switch in quotes when the
        // path has spaces; explorer ignores a quoted switch and opens its
        // default folder instead. Only the path itself may be quoted.
        Command::new("explorer")
            .raw_arg(format!("/select,\"{path_str}\""))
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let parent = path
            .parent()
            .ok_or_else(|| "File has no parent directory.".to_string())?;
        let status = Command::new("xdg-open")
            .arg(parent)
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err("Could not open the containing folder.".into());
        }
        Ok(())
    }
}

#[tauri::command]
pub async fn open_path_with_default_app(path: String) -> Result<(), String> {
    // The Tauri opener command needs a static path scope, and its detached
    // launcher cannot report a failing `open` process back to the UI.
    tauri::async_runtime::spawn_blocking(move || {
        let path = expand_home(&path);
        if !path.is_absolute() {
            return Err("Expected an absolute file path".to_string());
        }
        std::fs::metadata(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        open::that(&path).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests;
