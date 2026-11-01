//! Exact native transcript copy, including Codex fork/subagent dependencies.
use super::Profile;
use crate::kind::Kind;
use std::path::{Path, PathBuf};
mod codex;
use codex::*;
pub(super) fn transfer(from: &Profile, to: &Profile, kind: Kind, id: &str) -> Result<(), String> {
    validate_thread_id(id)?;
    let from = from.home(kind)?;
    let to = to.home(kind)?;
    if same_path(&from, &to) {
        return Ok(());
    }
    match kind {
        Kind::Claude => transfer_claude(&from, &to, id),
        Kind::Codex => transfer_codex(&from, &to, id),
        _ => Err("Provider accounts are not supported for this provider".into()),
    }
}
fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Claude keeps `projects/<cwd>/<id>.jsonl`, with subagent transcripts and
/// large tool results in `projects/<cwd>/<id>/` and edit checkpoints in
/// `file-history/<id>/`. The transcript is copied last: it is what `--resume`
/// looks for, so a partial copy is never mistaken for a complete one.
fn transfer_claude(from: &Path, to: &Path, id: &str) -> Result<(), String> {
    let name = format!("{id}.jsonl");
    let project = std::fs::read_dir(from.join("projects"))
        .ok()
        .and_then(|projects| {
            projects
                .flatten()
                .map(|entry| entry.path())
                .find(|dir| dir.join(&name).is_file())
        })
        .ok_or("Could not find the Claude conversation to carry over")?;
    let dest_project = to.join("projects").join(
        project
            .file_name()
            .ok_or("Invalid Claude project directory")?,
    );
    std::fs::create_dir_all(&dest_project).map_err(|e| e.to_string())?;
    copy_tree(&project.join(id), &dest_project.join(id))?;
    copy_tree(
        &from.join("file-history").join(id),
        &to.join("file-history").join(id),
    )?;
    copy_durable(&project.join(&name), &dest_project.join(&name))
        .map_err(|e| format!("Could not carry the Claude conversation over: {e}"))
}

fn copy_tree(source: &Path, dest: &Path) -> Result<(), String> {
    let entries = match std::fs::read_dir(source) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        let target = dest.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            copy_durable(&entry.path(), &target).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Codex resolves a thread id to its rollout file. Spawned agents and forked
/// history live in their own rollouts, so those are carried along, and the
/// root is copied last for the same reason as Claude's transcript.
fn transfer_codex(from: &Path, to: &Path, id: &str) -> Result<(), String> {
    let roots = [from.join("sessions"), from.join("archived_sessions")]
        .into_iter()
        .filter(|root| root.is_dir())
        .collect::<Vec<_>>();
    let find = |thread: &str| -> Result<Option<PathBuf>, String> {
        for root in &roots {
            if let Some(path) = find_thread(root, thread)? {
                return Ok(Some(path));
            }
        }
        Ok(None)
    };
    let root = find(id)?.ok_or("Could not find the Codex conversation to carry over")?;
    let sqlite_home = std::env::var_os("CODEX_SQLITE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| from.to_path_buf());

    // A custom shared SQLite location has no account-local destination contract.
    // Use the transcript handoff rather than claiming a complete native transfer.
    if !same_path(&sqlite_home, from) {
        return Err("Native transfer with CODEX_SQLITE_HOME requires a transcript handoff".into());
    }
    let edges = closed_agent_edges(&sqlite_home, id)?;
    let edge_db = if edges.is_empty() {
        None
    } else {
        Some(state_db(to)?.ok_or(
            "Destination Codex account must be initialized before carrying closed subagents",
        )?)
    };

    let mut files = vec![root];
    let mut index = 0;
    // Spawned agents may never have written a rollout; inherited history is
    // required to reconstruct a fork and must never be silently omitted.
    let mut pending: Vec<(String, bool)> = descendant_threads(&sqlite_home, id)?
        .into_iter()
        .map(|thread| (thread, false))
        .collect();
    loop {
        while index < files.len() {
            // Compressed rollouts can retain a plain JSONL append tail.
            let path = files[index].clone();
            let sibling = if path.extension().is_some_and(|ext| ext == "zst") {
                path.with_extension("")
            } else {
                let mut compressed = path.as_os_str().to_os_string();
                compressed.push(".zst");
                PathBuf::from(compressed)
            };
            if sibling.is_file() && !files.contains(&sibling) {
                files.push(sibling);
            }
            if let Some(base) = history_base(&path)? {
                validate_thread_id(&base)?;
                pending.push((base, true));
            }
            index += 1;
        }
        let Some((thread, required)) = pending.pop() else {
            break;
        };
        if files.iter().any(|path| rollout_matches(path, &thread)) {
            continue;
        }
        // A spawned agent that never wrote a rollout has nothing to resume.
        if let Some(path) = find(&thread)? {
            files.push(path);
        } else if required {
            return Err(format!(
                "Could not find required Codex history base {thread}"
            ));
        }
    }

    // Commit closure state before publishing the resumable root. Every retry
    // revalidates/reapplies it, including when an older root already exists.
    if let Some(db) = edge_db {
        apply_agent_edges(&db, edges)?;
    }
    for path in files.iter().rev() {
        let dest = rollout_dest(to, path)?;
        copy_durable(path, &dest)
            .map_err(|e| format!("Could not carry the Codex conversation over: {e}"))?;
    }
    Ok(())
}

fn descendant_threads(sqlite_home: &Path, id: &str) -> Result<Vec<String>, String> {
    let Some(path) = state_db(sqlite_home)? else {
        return Ok(Vec::new());
    };
    let db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| e.to_string())?;
    db.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    let exists: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='thread_spawn_edges')",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if !exists {
        return Ok(Vec::new());
    }
    let mut statement = db
        .prepare("WITH RECURSIVE subtree(child) AS (SELECT child_thread_id FROM thread_spawn_edges WHERE parent_thread_id = ?1 UNION SELECT edge.child_thread_id FROM thread_spawn_edges edge JOIN subtree ON edge.parent_thread_id = subtree.child) SELECT child FROM subtree")
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([id], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    let mut threads = Vec::new();
    for row in rows {
        let thread = row.map_err(|e| e.to_string())?;
        if validate_thread_id(&thread).is_ok() {
            threads.push(thread);
        }
    }
    Ok(threads)
}

#[cfg(test)]
mod tests;
