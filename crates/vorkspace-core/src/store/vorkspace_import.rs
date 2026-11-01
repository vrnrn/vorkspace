//! One-time copies of saved desktop conversations into the native store.
use super::Store;
use crate::{
    agent::Settings,
    tool::{ToolCall, ToolState},
    transcript::Transcript,
};
use rusqlite::{params, Connection, OpenFlags};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

/// Tauri's application-data location, keyed by its bundle identifier.
pub use crate::compatibility::vorkspace_database;

#[derive(Debug, Default)]
pub struct ImportReport {
    pub imported: usize,
    pub existing: usize,
    pub skipped: usize,
    pub latest_project: Option<String>,
}

impl Store {
    /// Import each source session once. Existing native conversations, including
    /// previously imported copies, are never overwritten by a later launch.
    pub fn import_vorkspace(&mut self, path: &Path) -> Result<ImportReport, String> {
        let path = path
            .canonicalize()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let mut source = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| e.to_string())?;
        source
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        // A read transaction includes committed WAL records and holds a consistent
        // snapshot even when the desktop app is still saving conversations.
        let source = source.transaction().map_err(|e| e.to_string())?;
        let result = import_sessions(self, &source, &path);
        result.map_err(|e| format!("Cannot import {}: {e}", path.display()))
    }
}

fn invalid(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(error.into())
}

fn import_sessions(
    store: &mut Store,
    source: &Connection,
    path: &Path,
) -> rusqlite::Result<ImportReport> {
    let columns = source
        .prepare("PRAGMA table_info(sessions)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    for required in [
        "id",
        "cwd",
        "harness",
        "model",
        "model_settings",
        "runtime_mode",
        "title",
        "provider_session_id",
        "blocks_json",
        "updated_at",
    ] {
        if !columns.contains(required) {
            return Err(invalid(format!(
                "Not a vorkspace session database (missing {required})"
            )));
        }
    }
    let mut fields = "id,cwd,harness,model,model_settings,runtime_mode,title,provider_session_id,blocks_json,updated_at".to_owned();
    for optional in [
        "archived",
        "pinned",
        "worktree_cwd",
        "worktree_removed",
        "context_used",
        "context_window",
        "sidebar_hidden",
        "inbox_ask",
        "linked_work_item_json",
    ] {
        fields.push(',');
        fields.push_str(if columns.contains(optional) {
            optional
        } else {
            "NULL"
        });
    }
    // Sort only small metadata rows. Sorting full transcripts can spill hundreds
    // of megabytes to disk, and repeat launches shouldn't load imported bodies.
    let summaries = source
        .prepare("SELECT id,cwd FROM sessions ORDER BY updated_at DESC,id")?
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut sessions = source.prepare(&format!("SELECT {fields} FROM sessions WHERE id=?1"))?;
    let tx = store.conn.transaction()?;
    let mut report = ImportReport::default();
    let source_key = path.to_string_lossy();
    for (source_id, cwd) in summaries {
        let key = format!("import:vorkspace:{}", json!([source_key, source_id]));
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM native_settings WHERE key=?1)",
            [&key],
            |r| r.get(0),
        )?;
        if exists {
            report.existing += 1;
            report.latest_project.get_or_insert(cwd);
            continue;
        }
        let mut rows = sessions.query([&source_id])?;
        let session = rows.next()?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        let provider: String = session.get(2)?;
        // Internal workers and remote connections need their own migration;
        // don't expose them as ordinary local chats.
        if session.get::<_, Option<bool>>(16)?.unwrap_or(false)
            || session.get::<_, Option<String>>(17)?.is_some()
            || !Path::new(&cwd).is_absolute()
        {
            report.skipped += 1;
            continue;
        }
        let normalized = super::shared::has_table(source,"ordinary_transcripts").map_err(invalid)?
            && source.query_row("SELECT EXISTS(SELECT 1 FROM ordinary_transcripts WHERE session_id=?1 AND normalized=1)",[&source_id],|row|row.get::<_,bool>(0))?;
        let blocks: Vec<Value> = if normalized {
            let mut query = source.prepare(
                "SELECT block_json FROM ordinary_blocks WHERE session_id=?1 ORDER BY position",
            )?;
            let values = query.query_map([&source_id], |row| row.get::<_, String>(0))?;
            values
                .map(|raw| serde_json::from_str(&raw?).map_err(invalid))
                .collect::<rusqlite::Result<_>>()?
        } else {
            let raw: String = session.get(8)?;
            serde_json::from_str(&raw).map_err(invalid)?
        };
        // Empty/Bot-only histories and unsent drafts aren't completed chats.
        if !blocks
            .iter()
            .any(|b| b["role"] == "user" && b["draft"] != true && b["internal"] != true)
        {
            report.skipped += 1;
            continue;
        }
        let model_settings: String = session.get(4)?;
        let model_settings: HashMap<String, String> =
            serde_json::from_str(&model_settings).map_err(invalid)?;
        let mut settings = Settings {
            model: crate::session::native_model(&session.get::<_, String>(3)?, &provider),
            effort: ["effort", "reasoningEffort", "reasoning_effort", "variant"]
                .into_iter()
                .find_map(|key| model_settings.get(key).cloned()),
            access: crate::agent::AccessMode::from_id(&session.get::<_, String>(5)?),
            fast: model_settings.get("fast").is_some_and(|v| v == "true"),
            service_tier: model_settings.get("serviceTier").cloned(),
            thinking: model_settings.get("thinking").and_then(|v| v.parse().ok()),
            model_settings,
            worktree_cwd: session.get(12)?,
            worktree_removed: session.get::<_, Option<bool>>(13)?.unwrap_or(false),
            ..Settings::default()
        };
        let transcript = transcript(&blocks)?;
        let mut offset = 0;
        for block in &blocks {
            let rows = self::transcript(std::slice::from_ref(block))?.rows;
            if let Some(prompt) = rows.iter().rposition(crate::transcript::Row::is_prompt) {
                if let Some(info) = super::shared::project_turns(std::slice::from_ref(block))
                    .map_err(invalid)?
                    .into_values()
                    .next()
                {
                    settings.turns.insert(offset + prompt + 1, info);
                }
            }
            offset += rows.len();
        }
        tx.execute("INSERT INTO native_sessions(title,provider,cwd,provider_id,updated_at,archived,pinned,work_item) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![session.get::<_, String>(6)?, provider, cwd, session.get::<_, Option<String>>(7)?,
                session.get::<_, i64>(9)?, session.get::<_, Option<bool>>(10)?.unwrap_or(false),
                session.get::<_, Option<bool>>(11)?.unwrap_or(false), session.get::<_, Option<String>>(18)?])?;
        let id = tx.last_insert_rowid();
        {
            let mut insert = tx.prepare("INSERT INTO native_rows(session_id,row_ix,role,body,first,sent_at) VALUES(?1,?2,?3,?4,?5,?6)")?;
            for (ix, row) in transcript.rows.iter().enumerate() {
                insert.execute(params![
                    id,
                    ix as i64,
                    row.role,
                    row.text,
                    row.first,
                    row.sent_at.map(|at| at.min(i64::MAX as u64) as i64)
                ])?;
            }
        }
        tx.execute(
            "INSERT INTO native_settings(key,value) VALUES(?1,?2)",
            params![
                format!("agent:{id}"),
                serde_json::to_string(&settings).map_err(invalid)?
            ],
        )?;
        if let Some(used) = session.get::<_, Option<i64>>(14)?.filter(|used| *used >= 0) {
            tx.execute(
                "INSERT INTO native_settings(key,value) VALUES(?1,?2)",
                params![
                    format!("context:{id}"),
                    json!({"used":used,"window":session.get::<_, Option<i64>>(15)?.filter(|window| *window > 0)}).to_string()
                ],
            )?;
        }
        tx.execute(
            "INSERT INTO native_settings(key,value) VALUES(?1,?2)",
            params![
                key,
                json!({"session_id":id,"source_id":source_id,"source_path":source_key}).to_string()
            ],
        )?;
        report.imported += 1;
        report.latest_project.get_or_insert(cwd);
    }
    tx.commit()?;
    Ok(report)
}

fn transcript(blocks: &[Value]) -> rusqlite::Result<Transcript> {
    let mut transcript = Transcript::default();
    for block in blocks {
        if block["draft"] == true || block["internal"] == true {
            continue;
        }
        let role = block["role"].as_str().unwrap_or("system");
        let text = block["text"].as_str().unwrap_or("");
        if block["interjection"].is_object() {
            let note = crate::transcript::Interjection {
                text: text.into(),
                custom_type: block["interjection"]["customType"]
                    .as_str()
                    .unwrap_or("custom")
                    .into(),
                severity: block["interjection"]["severity"]
                    .as_str()
                    .map(str::to_owned),
            };
            transcript.begin("interjection");
            transcript.append(&serde_json::to_string(&note).map_err(invalid)?);
        } else if role == "tool" {
            let tool = &block["tool"];
            let preview = &tool["preview"];
            let name = match tool["kind"].as_str().unwrap_or("") {
                "read" => "Read",
                "write" => "Write",
                "edit" => "Edit",
                "search" => "Grep",
                "execute" => "Bash",
                _ => tool["title"].as_str().unwrap_or(text),
            };
            let mut call = ToolCall {
                id: tool["callId"].as_str().or(block["id"].as_str()).unwrap_or("imported-tool").into(),
                name: name.into(),
                input: json!({"command":preview["title"].as_str().or(tool["title"].as_str()).unwrap_or(text),
                    "path":preview["path"],"query":preview["query"]}).to_string(),
                output: tool["detail"].as_str().filter(|s| !s.is_empty()).or(preview["output"].as_str()).unwrap_or("").into(),
                panel: crate::tool::tasks::from_desktop(block),
                display: None,
                // Imported activity is historical, never an active native job.
                state: match tool["status"].as_str() {
                    Some("failed" | "error") => ToolState::Failed,
                    Some("cancelled" | "canceled" | "aborted") => ToolState::Cancelled,
                    _ => ToolState::Completed,
                },
            };
            call.bound_output();
            transcript.begin("tool");
            transcript.rows.last_mut().unwrap().text =
                serde_json::to_string(&call).map_err(invalid)?;
        } else if role == "tasks" && crate::tool::tasks::from_desktop(block).is_some() {
            let mut call = ToolCall::new(block["id"].as_str().unwrap_or(""), "update_plan");
            call.input = block["taskList"].to_string();
            call.panel = crate::tool::tasks::from_desktop(block);
            call.state = ToolState::Completed;
            transcript.begin("tool");
            transcript.rows.last_mut().unwrap().text =
                serde_json::to_string(&call).map_err(invalid)?;
        } else {
            if text.is_empty() {
                continue;
            }
            transcript.begin(match role {
                "user" | "assistant" | "reasoning" => role,
                "plan" | "tasks" => "assistant",
                _ => "system",
            });
            transcript.append(text);
        }
        // Imported history must not acquire today's send time.
        if let Some(row) = transcript.rows.iter_mut().rev().find(|r| r.first) {
            row.sent_at = block["sentAt"].as_u64().or(block["startedAt"].as_u64());
        }
    }
    Ok(transcript)
}

#[cfg(test)]
mod tests;
