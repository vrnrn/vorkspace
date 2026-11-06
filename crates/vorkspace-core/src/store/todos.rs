//! To-do board persistence. Items belong to a board key and keep their
//! order within each status column.
use super::Store;
use crate::todos::{
    clean_description, clean_references, clean_title, TodoBoard, TodoIssue, TodoItem,
    TodoRemoteEdit, TodoStatus, TodoSync, MAX_ATTACHMENTS, MAX_DEPENDENCIES,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::Value;

pub fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS todo_items (
           id TEXT PRIMARY KEY,
           board TEXT NOT NULL,
           title TEXT NOT NULL,
           description TEXT NOT NULL DEFAULT '',
           attachments TEXT NOT NULL DEFAULT '[]',
           dependencies TEXT NOT NULL DEFAULT '[]',
           status TEXT NOT NULL,
           position INTEGER NOT NULL,
           created_at INTEGER NOT NULL,
           updated_at INTEGER NOT NULL,
           issue TEXT,
           sync TEXT NOT NULL DEFAULT 'local',
           submission_token TEXT,
           pending_repo TEXT
         );
         CREATE INDEX IF NOT EXISTS todo_items_board_idx
           ON todo_items (board, status, position);
         CREATE TABLE IF NOT EXISTS todo_remote_edits (
           board TEXT NOT NULL,
           id TEXT NOT NULL,
           snapshot_json TEXT NOT NULL,
           input_json TEXT NOT NULL,
           attempt_id TEXT NOT NULL,
           phase TEXT NOT NULL CHECK(phase IN ('pending','uncertain')),
           updated_at INTEGER NOT NULL,
           PRIMARY KEY(board,id)
         );",
    )?;
    // Existing databases contain the original seven-column table. ALTER keeps
    // each saved row and its board identity intact, including legacy Bot rows.
    let mut statement = conn.prepare("PRAGMA table_info(todo_items)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<std::collections::HashSet<_>, _>>()?;
    drop(statement);
    for (name, definition) in [
        ("description", "TEXT NOT NULL DEFAULT ''"),
        ("attachments", "TEXT NOT NULL DEFAULT '[]'"),
        ("dependencies", "TEXT NOT NULL DEFAULT '[]'"),
        ("issue", "TEXT"),
        ("sync", "TEXT NOT NULL DEFAULT 'local'"),
        ("submission_token", "TEXT"),
        ("pending_repo", "TEXT"),
    ] {
        if !columns.contains(name) {
            conn.execute_batch(&format!(
                "ALTER TABLE todo_items ADD COLUMN {name} {definition}"
            ))?;
        }
    }
    let mut statement = conn.prepare("PRAGMA table_info(todo_remote_edits)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    if !columns.iter().any(|name| name == "attempt_id") {
        conn.execute_batch(
            "ALTER TABLE todo_remote_edits ADD COLUMN attempt_id TEXT NOT NULL DEFAULT '';",
        )?;
    }
    conn.execute(
        "UPDATE todo_remote_edits SET attempt_id=lower(hex(randomblob(16))) WHERE attempt_id=''",
        [],
    )?;
    Ok(())
}

fn now_millis() -> i64 {
    crate::time::now_ms().min(i64::MAX as u64) as i64
}

fn strings(input: &serde_json::Value, key: &str) -> Result<Option<Vec<String>>, String> {
    let Some(value) = input.get(key) else {
        return Ok(None);
    };
    let values = value
        .as_array()
        .ok_or_else(|| format!("{key} must be an array of text"))?;
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{key} must be an array of text"))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn item(row: &rusqlite::Row<'_>) -> rusqlite::Result<TodoItem> {
    fn parse<T: serde::de::DeserializeOwned>(
        row: &rusqlite::Row<'_>,
        index: usize,
    ) -> rusqlite::Result<T> {
        let raw: String = row.get(index)?;
        serde_json::from_str(&raw).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
    }
    let status: String = row.get(5)?;
    let status = TodoStatus::from_id(&status).ok_or_else(|| {
        rusqlite::Error::InvalidColumnType(5, "status".into(), rusqlite::types::Type::Text)
    })?;
    let sync: String = row.get(10)?;
    let sync = match sync.as_str() {
        "local" => TodoSync::Local,
        "pending" => TodoSync::Pending,
        "uncertain" => TodoSync::Uncertain,
        "synced" => TodoSync::Synced,
        _ => {
            return Err(rusqlite::Error::InvalidColumnType(
                10,
                "sync".into(),
                rusqlite::types::Type::Text,
            ))
        }
    };
    Ok(TodoItem {
        id: row.get(0)?,
        title: row.get(1)?,
        description: row.get(2)?,
        attachments: parse(row, 3)?,
        dependencies: parse(row, 4)?,
        status,
        position: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        issue: row
            .get::<_, Option<String>>(9)?
            .map(|raw| {
                serde_json::from_str::<TodoIssue>(&raw).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        9,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
            })
            .transpose()?,
        sync,
        pending_edit: None,
        submission_token: row.get(11)?,
        pending_repo: row.get(12)?,
    })
}

const COLUMNS: &str = "id,title,description,attachments,dependencies,status,position,created_at,updated_at,issue,sync,submission_token,pending_repo";

fn column_ids(
    tx: &Transaction<'_>,
    board: &str,
    status: TodoStatus,
) -> Result<Vec<String>, String> {
    let mut statement = tx
        .prepare("SELECT id FROM todo_items WHERE board=?1 AND status=?2 ORDER BY position,created_at,id")
        .map_err(|e| e.to_string())?;
    let ids = statement
        .query_map(params![board, status.id()], |row| row.get(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<String>, _>>()
        .map_err(|e| e.to_string());
    ids
}

fn renumber(tx: &Transaction<'_>, ids: &[String]) -> Result<(), String> {
    for (position, id) in ids.iter().enumerate() {
        tx.execute(
            "UPDATE todo_items SET position=?1 WHERE id=?2",
            params![position as i64, id],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

impl Store {
    fn todo_schema(&self) -> Result<(), String> {
        ensure_schema(&self.conn).map_err(|e| e.to_string())
    }

    fn validate_todo_dependencies(
        &self,
        board: &TodoBoard,
        id: Option<&str>,
        dependencies: &[String],
    ) -> Result<(), String> {
        for dependency in dependencies {
            if let Some(number) = dependency.strip_prefix('#') {
                if !matches!(board, TodoBoard::Project(_))
                    || number
                        .parse::<u64>()
                        .ok()
                        .filter(|number| *number > 0)
                        .is_none()
                {
                    return Err(
                        "Issue dependencies must use a positive #number on a project board".into(),
                    );
                }
                continue;
            }
            if crate::todos::github::issue_ref_from_url(dependency).is_some() {
                if !matches!(board, TodoBoard::Project(_)) {
                    return Err("GitHub issue dependencies require a project board".into());
                }
                continue;
            }
            if self.get_todo(board, dependency)?.is_none() {
                return Err(format!("Dependency {dependency} is not on this board"));
            }
        }
        let Some(id) = id else { return Ok(()) };
        let mut stack = dependencies
            .iter()
            .filter(|dependency| {
                !dependency.starts_with('#')
                    && crate::todos::github::issue_ref_from_url(dependency).is_none()
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut visited = std::collections::HashSet::new();
        while let Some(next) = stack.pop() {
            if next == id {
                return Err("To-do dependencies cannot form a cycle".into());
            }
            if !visited.insert(next.clone()) {
                continue;
            }
            if visited.len() > 10_000 {
                return Err("Dependency graph is too large to validate".into());
            }
            if let Some(item) = self.get_todo(board, &next)? {
                stack.extend(item.dependencies.into_iter().filter(|dependency| {
                    !dependency.starts_with('#')
                        && crate::todos::github::issue_ref_from_url(dependency).is_none()
                }));
            }
        }
        Ok(())
    }

    /// Every item on a board, ordered by column then position.
    pub fn list_todos(&self, board: &TodoBoard) -> Result<Vec<TodoItem>, String> {
        self.todo_schema()?;
        let mut statement = self
            .conn
            .prepare(&format!(
                "SELECT {COLUMNS} FROM todo_items WHERE board=?1 ORDER BY position,created_at,id"
            ))
            .map_err(|e| e.to_string())?;
        let mut items = statement
            .query_map([board.key()], item)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        items.sort_by_key(|item| TodoStatus::ALL.iter().position(|s| *s == item.status));
        Ok(items)
    }

    pub fn get_todo(&self, board: &TodoBoard, id: &str) -> Result<Option<TodoItem>, String> {
        self.todo_schema()?;
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM todo_items WHERE board=?1 AND id=?2"),
                params![board.key(), id],
                item,
            )
            .optional()
            .map_err(|e| e.to_string())
    }

    /// Adds an item at the end of its column.
    pub fn add_todo(
        &self,
        board: &TodoBoard,
        title: &str,
        status: TodoStatus,
    ) -> Result<TodoItem, String> {
        self.add_todo_with_details(board, title, status, "", &[], &[])
    }

    pub fn add_todo_with_details(
        &self,
        board: &TodoBoard,
        title: &str,
        status: TodoStatus,
        description: &str,
        attachments: &[String],
        dependencies: &[String],
    ) -> Result<TodoItem, String> {
        self.todo_schema()?;
        let title = clean_title(title)?;
        let description = clean_description(description)?;
        let attachments = clean_references(attachments, MAX_ATTACHMENTS, "attachment")?;
        let dependencies = clean_references(dependencies, MAX_DEPENDENCIES, "dependency")?;
        self.validate_todo_dependencies(board, None, &dependencies)?;
        let now = now_millis();
        let id = crate::session::new_id();
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        let position = column_ids(&tx, &board.key(), status)?.len() as i64;
        tx.execute(
            "INSERT INTO todo_items(id,board,title,description,attachments,dependencies,status,position,created_at,updated_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?9)",
            params![id, board.key(), title, description, serde_json::to_string(&attachments).map_err(|e|e.to_string())?, serde_json::to_string(&dependencies).map_err(|e|e.to_string())?, status.id(), position, now],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        self.get_todo(board, &id)?
            .ok_or_else(|| "The item was not saved".into())
    }

    pub fn update_todo_details(
        &self,
        board: &TodoBoard,
        id: &str,
        description: Option<&str>,
        attachments: Option<&[String]>,
        dependencies: Option<&[String]>,
    ) -> Result<TodoItem, String> {
        self.todo_schema()?;
        let current = self.get_todo(board, id)?.ok_or("To-do not found")?;
        let description = description
            .map(clean_description)
            .transpose()?
            .unwrap_or(current.description);
        let attachments = attachments
            .map(|values| clean_references(values, MAX_ATTACHMENTS, "attachment"))
            .transpose()?
            .unwrap_or(current.attachments);
        let dependencies = dependencies
            .map(|values| clean_references(values, MAX_DEPENDENCIES, "dependency"))
            .transpose()?
            .unwrap_or(current.dependencies);
        if dependencies.iter().any(|value| value == id) {
            return Err("A to-do cannot depend on itself".into());
        }
        self.validate_todo_dependencies(board, Some(id), &dependencies)?;
        self.conn.execute(
            "UPDATE todo_items SET description=?1,attachments=?2,dependencies=?3,updated_at=?4 WHERE board=?5 AND id=?6",
            params![description, serde_json::to_string(&attachments).map_err(|e|e.to_string())?, serde_json::to_string(&dependencies).map_err(|e|e.to_string())?, now_millis(), board.key(), id],
        ).map_err(|e|e.to_string())?;
        self.get_todo(board, id)?.ok_or("To-do not found".into())
    }

    fn update_todo_atomic(
        &self,
        board: &TodoBoard,
        id: &str,
        input: &Value,
        status: Option<TodoStatus>,
    ) -> Result<TodoItem, String> {
        self.todo_schema()?;
        let current = self.get_todo(board, id)?.ok_or("To-do not found")?;
        let title = input.get("title").and_then(Value::as_str);
        let description = input.get("description").and_then(Value::as_str);
        let attachments = strings(input, "attachments")?;
        let dependencies = strings(input, "dependencies")?;
        let before = input.get("before").and_then(Value::as_str);
        let title = title
            .map(clean_title)
            .transpose()?
            .unwrap_or(current.title.clone());
        let description = description
            .map(clean_description)
            .transpose()?
            .unwrap_or(current.description.clone());
        let attachments = attachments
            .as_deref()
            .map(|values| clean_references(values, MAX_ATTACHMENTS, "attachment"))
            .transpose()?
            .unwrap_or(current.attachments.clone());
        let dependencies = dependencies
            .as_deref()
            .map(|values| clean_references(values, MAX_DEPENDENCIES, "dependency"))
            .transpose()?
            .unwrap_or(current.dependencies.clone());
        self.validate_todo_dependencies(board, Some(id), &dependencies)?;
        let next_status = status.unwrap_or(current.status);
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE todo_items SET title=?1,description=?2,attachments=?3,dependencies=?4,status=?5,updated_at=?6 WHERE board=?7 AND id=?8",
            params![title, description, serde_json::to_string(&attachments).map_err(|e|e.to_string())?, serde_json::to_string(&dependencies).map_err(|e|e.to_string())?, next_status.id(), now_millis(), board.key(), id],
        ).map_err(|e|e.to_string())?;
        if status.is_some() || before.is_some() {
            let mut ids = column_ids(&tx, &board.key(), next_status)?
                .into_iter()
                .filter(|other| other != id)
                .collect::<Vec<_>>();
            let at = match before {
                Some(before) => ids
                    .iter()
                    .position(|other| other == before)
                    .ok_or("Move target is not in this column")?,
                None => ids.len(),
            };
            ids.insert(at, id.into());
            renumber(&tx, &ids)?;
            if current.status != next_status {
                renumber(&tx, &column_ids(&tx, &board.key(), current.status)?)?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        self.get_todo(board, id)?.ok_or("To-do not found".into())
    }

    /// Preserved Bot-board rows need a deliberate project owner. Assigning is
    /// transactional and appends each status column after existing project work.
    pub fn assign_legacy_bot_todos(&self, bot_id: &str, project: &str) -> Result<usize, String> {
        self.todo_schema()?;
        if bot_id.trim().is_empty() || project.trim().is_empty() {
            return Err("Choose a Bot and project".into());
        }
        let old = TodoBoard::Bot(bot_id.into()).key();
        let new = TodoBoard::Project(project.into()).key();
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        let mut count = 0;
        for status in TodoStatus::ALL {
            let existing = column_ids(&tx, &new, status)?.len() as i64;
            for (index, id) in column_ids(&tx, &old, status)?.iter().enumerate() {
                count += tx.execute("UPDATE todo_items SET board=?1,position=?2,updated_at=?3 WHERE id=?4 AND board=?5", params![new, existing + index as i64, now_millis(), id, old]).map_err(|e|e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(count)
    }

    pub fn legacy_bot_todo_counts(
        &self,
    ) -> Result<std::collections::HashMap<String, usize>, String> {
        self.todo_schema()?;
        let mut statement = self
            .conn
            .prepare(
                "SELECT board,COUNT(*) FROM todo_items WHERE board LIKE 'bot:%' GROUP BY board",
            )
            .map_err(|e| e.to_string())?;
        let result = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?
                        .trim_start_matches("bot:")
                        .to_string(),
                    row.get::<_, i64>(1)? as usize,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string());
        result
    }

    /// Reserve a stable local identity before an external issue create. A
    /// pending or uncertain row is never submitted by this method again.
    pub fn begin_todo_publish(
        &self,
        board: &TodoBoard,
        id: &str,
        repo: &str,
    ) -> Result<(TodoItem, String), String> {
        self.todo_schema()?;
        if !matches!(board, TodoBoard::Project(_)) {
            return Err("Only project to-dos can be published".into());
        }
        let item = self.get_todo(board, id)?.ok_or("To-do not found")?;
        if item.sync != TodoSync::Local {
            return Err(
                "This to-do has already been submitted or linked; reconcile it before trying again"
                    .into(),
            );
        }
        if repo.is_empty() {
            return Err("Choose a GitHub repository before publishing".into());
        }
        let token = uuid::Uuid::new_v4().to_string();
        let changed = self.conn.execute("UPDATE todo_items SET sync='pending',submission_token=?1,pending_repo=?2,updated_at=?3 WHERE board=?4 AND id=?5 AND sync='local'", params![token, repo, now_millis(), board.key(), id]).map_err(|e|e.to_string())?;
        if changed != 1 {
            return Err("To-do changed while preparing publication".into());
        }
        Ok((item, token))
    }

    pub fn uncertain_todo_publish(&self, board: &TodoBoard, id: &str) -> Result<(), String> {
        self.todo_schema()?;
        self.conn.execute("UPDATE todo_items SET sync='uncertain',updated_at=?1 WHERE board=?2 AND id=?3 AND sync='pending'", params![now_millis(), board.key(), id]).map_err(|e|e.to_string())?;
        Ok(())
    }

    /// Join a created issue to its local draft. If the draft requested a
    /// non-default state, preserve that follow-up write in the same commit.
    pub fn finish_todo_publish(
        &self,
        board: &TodoBoard,
        id: &str,
        remote: &TodoItem,
    ) -> Result<Option<TodoRemoteEdit>, String> {
        self.todo_schema()?;
        let issue = remote
            .issue
            .as_ref()
            .ok_or("Created issue has no identity")?;
        let draft = self.get_todo(board, id)?.ok_or("To-do not found")?;
        let follow_up = (draft.status != remote.status).then(|| TodoRemoteEdit {
            attempt_id: uuid::Uuid::new_v4().to_string(),
            input: serde_json::json!({"id":remote.id,"status":draft.status.id()}),
            phase: TodoSync::Pending,
        });
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        if let Some(patch) = &follow_up {
            tx.execute(
                "INSERT INTO todo_remote_edits(board,id,snapshot_json,input_json,phase,updated_at,attempt_id) VALUES(?1,?2,?3,?4,'pending',?5,?6)",
                params![board.key(), remote.id, serde_json::to_string(remote).map_err(|e|e.to_string())?, patch.input.to_string(), now_millis(), patch.attempt_id],
            ).map_err(|e| e.to_string())?;
        }
        let changed = tx.execute("UPDATE todo_items SET sync='synced',issue=?1,pending_repo=NULL,updated_at=?2 WHERE board=?3 AND id=?4 AND sync IN ('pending','uncertain')", params![serde_json::to_string(issue).map_err(|e|e.to_string())?, now_millis(), board.key(), id]).map_err(|e|e.to_string())?;
        if changed != 1 {
            return Err(
                "The submitted to-do changed; reconcile its GitHub issue before continuing".into(),
            );
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(follow_up)
    }

    /// Return a saved draft to local ownership only after GitHub has been
    /// checked. A recent pending attempt may still be running in this app.
    pub fn cancel_todo_publish(&self, board: &TodoBoard, id: &str) -> Result<TodoItem, String> {
        self.todo_schema()?;
        let item = self.get_todo(board, id)?.ok_or("To-do not found")?;
        if !matches!(item.sync, TodoSync::Pending | TodoSync::Uncertain) {
            return Err("To-do has no pending publication".into());
        }
        if item.sync == TodoSync::Pending && now_millis().saturating_sub(item.updated_at) < 60_000 {
            return Err(
                "This issue creation may still be running; wait and check GitHub again".into(),
            );
        }
        let changed = self.conn.execute("UPDATE todo_items SET sync='local',issue=NULL,submission_token=NULL,pending_repo=NULL,updated_at=?1 WHERE board=?2 AND id=?3 AND sync IN ('pending','uncertain')", params![now_millis(), board.key(), id]).map_err(|e|e.to_string())?;
        if changed != 1 {
            return Err("To-do publication changed before cancellation".into());
        }
        self.get_todo(board, id)?.ok_or("To-do not found".into())
    }

    pub fn todo_submission_token(
        &self,
        board: &TodoBoard,
        id: &str,
    ) -> Result<Option<String>, String> {
        self.todo_schema()?;
        self.conn
            .query_row(
                "SELECT submission_token FROM todo_items WHERE board=?1 AND id=?2",
                params![board.key(), id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())
            .map(Option::flatten)
    }

    /// An explicit retry reuses the original marker so duplicate creation is
    /// recognizable. The caller must first re-check GitHub and obtain a human
    /// confirmation that the original issue was not created.
    pub fn retry_todo_publish(
        &self,
        board: &TodoBoard,
        id: &str,
    ) -> Result<(TodoItem, String), String> {
        self.todo_schema()?;
        let item = self.get_todo(board, id)?.ok_or("To-do not found")?;
        if !matches!(item.sync, TodoSync::Pending | TodoSync::Uncertain) {
            return Err("Only a pending or uncertain submission can be retried".into());
        }
        if item.sync == TodoSync::Pending && now_millis().saturating_sub(item.updated_at) < 60_000 {
            return Err("This issue creation may still be running; wait before retrying and check GitHub again".into());
        }
        let token = item
            .submission_token
            .clone()
            .ok_or("Submission token is missing")?;
        let changed = self.conn.execute("UPDATE todo_items SET sync='pending',updated_at=?1 WHERE board=?2 AND id=?3 AND sync IN ('pending','uncertain')", params![now_millis(), board.key(), id]).map_err(|e|e.to_string())?;
        if changed != 1 {
            return Err("Submission changed while preparing retry".into());
        }
        Ok((item, token))
    }

    pub fn pending_todo_submissions(
        &self,
        board: &TodoBoard,
    ) -> Result<Vec<(String, String)>, String> {
        self.todo_schema()?;
        let mut statement = self.conn.prepare("SELECT id,submission_token FROM todo_items WHERE board=?1 AND sync IN ('pending','uncertain') AND submission_token IS NOT NULL").map_err(|e|e.to_string())?;
        let result = statement
            .query_map([board.key()], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string());
        result
    }

    /// Save the user's requested remote edit before any GitHub mutation. An
    /// unresolved edit blocks later writes to the same issue, including after
    /// a crash, until it is reconciled or explicitly discarded.
    pub fn begin_remote_todo_edit(
        &self,
        board: &TodoBoard,
        snapshot: &TodoItem,
        input: &serde_json::Value,
    ) -> Result<String, String> {
        self.todo_schema()?;
        if !matches!(board, TodoBoard::Project(_)) || snapshot.issue.is_none() || !input.is_object()
        {
            return Err("Invalid GitHub to-do edit".into());
        }
        let attempt = uuid::Uuid::new_v4().to_string();
        self.conn.execute("INSERT INTO todo_remote_edits(board,id,snapshot_json,input_json,phase,updated_at,attempt_id) VALUES(?1,?2,?3,?4,'pending',?5,?6)", params![board.key(), snapshot.id, serde_json::to_string(snapshot).map_err(|e|e.to_string())?, input.to_string(), now_millis(), attempt]).map_err(|error| {
            if matches!(error, rusqlite::Error::SqliteFailure(ref failure, _) if failure.code == rusqlite::ErrorCode::ConstraintViolation) {
                "This issue has an unresolved edit; reconcile it before changing it again".into()
            } else { error.to_string() }
        })?;
        Ok(attempt)
    }

    pub fn uncertain_remote_todo_edit(
        &self,
        board: &TodoBoard,
        id: &str,
        attempt: &str,
    ) -> Result<(), String> {
        self.todo_schema()?;
        let changed = self.conn.execute("UPDATE todo_remote_edits SET phase='uncertain',updated_at=?1 WHERE board=?2 AND id=?3 AND attempt_id=?4", params![now_millis(), board.key(), id, attempt]).map_err(|e|e.to_string())?;
        if changed != 1 {
            return Err("The GitHub edit attempt changed before completion".into());
        }
        Ok(())
    }

    pub fn clear_remote_todo_edit(
        &self,
        board: &TodoBoard,
        id: &str,
        attempt: &str,
    ) -> Result<(), String> {
        self.todo_schema()?;
        let changed = self
            .conn
            .execute(
                "DELETE FROM todo_remote_edits WHERE board=?1 AND id=?2 AND attempt_id=?3",
                params![board.key(), id, attempt],
            )
            .map_err(|e| e.to_string())?;
        if changed != 1 {
            return Err("The GitHub edit attempt changed before completion".into());
        }
        Ok(())
    }

    pub fn retry_remote_todo_edit(
        &self,
        board: &TodoBoard,
        id: &str,
        expected_attempt: &str,
    ) -> Result<TodoRemoteEdit, String> {
        self.todo_schema()?;
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        let (input,phase,updated_at): (String,String,i64) = tx.query_row("SELECT input_json,phase,updated_at FROM todo_remote_edits WHERE board=?1 AND id=?2 AND attempt_id=?3 AND phase IN ('pending','uncertain')", params![board.key(), id, expected_attempt], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional().map_err(|e|e.to_string())?.ok_or("The pending edit changed before retry")?;
        if phase == "pending" && now_millis().saturating_sub(updated_at) < 180_000 {
            return Err("This GitHub edit may still be running; wait before retrying and check the issue again".into());
        }
        let attempt_id = uuid::Uuid::new_v4().to_string();
        let input = serde_json::from_str(&input).map_err(|e| e.to_string())?;
        let changed = tx.execute("UPDATE todo_remote_edits SET phase='pending',updated_at=?1,attempt_id=?2 WHERE board=?3 AND id=?4 AND attempt_id=?5", params![now_millis(), attempt_id, board.key(), id, expected_attempt]).map_err(|e|e.to_string())?;
        if changed != 1 {
            return Err("The edit changed before its retry was admitted".into());
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(TodoRemoteEdit {
            attempt_id,
            input,
            phase: TodoSync::Pending,
        })
    }

    pub fn pending_remote_todo_edits(&self, board: &TodoBoard) -> Result<Vec<TodoItem>, String> {
        self.todo_schema()?;
        let mut statement = self.conn.prepare("SELECT snapshot_json,input_json,phase,attempt_id FROM todo_remote_edits WHERE board=?1 ORDER BY updated_at,id").map_err(|e|e.to_string())?;
        let rows = statement
            .query_map([board.key()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        rows.into_iter()
            .map(|(snapshot, input, phase, attempt_id)| {
                let mut item: TodoItem =
                    serde_json::from_str(&snapshot).map_err(|e| e.to_string())?;
                let input = serde_json::from_str(&input).map_err(|e| e.to_string())?;
                item.pending_edit = Some(TodoRemoteEdit {
                    attempt_id,
                    input,
                    phase: if phase == "pending" {
                        TodoSync::Pending
                    } else {
                        TodoSync::Uncertain
                    },
                });
                Ok(item)
            })
            .collect()
    }

    pub fn rename_todo(
        &self,
        board: &TodoBoard,
        id: &str,
        title: &str,
    ) -> Result<TodoItem, String> {
        self.todo_schema()?;
        let title = clean_title(title)?;
        let changed = self
            .conn
            .execute(
                "UPDATE todo_items SET title=?1,updated_at=?2 WHERE board=?3 AND id=?4",
                params![title, now_millis(), board.key(), id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err("To-do not found".into());
        }
        self.get_todo(board, id)?
            .ok_or_else(|| "To-do not found".into())
    }

    /// Moves an item into `status`, before `before` in that column or at its end.
    pub fn move_todo(
        &self,
        board: &TodoBoard,
        id: &str,
        status: TodoStatus,
        before: Option<&str>,
    ) -> Result<TodoItem, String> {
        self.todo_schema()?;
        let key = board.key();
        let current = self.get_todo(board, id)?.ok_or("To-do not found")?;
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE todo_items SET status=?1,updated_at=?2 WHERE id=?3",
            params![status.id(), now_millis(), id],
        )
        .map_err(|e| e.to_string())?;
        let mut ids: Vec<String> = column_ids(&tx, &key, status)?
            .into_iter()
            .filter(|other| other != id)
            .collect();
        let at = match before {
            Some(before) => ids
                .iter()
                .position(|other| other == before)
                .ok_or("Move target is not in this column")?,
            None => ids.len(),
        };
        ids.insert(at, id.into());
        renumber(&tx, &ids)?;
        if current.status != status {
            renumber(&tx, &column_ids(&tx, &key, current.status)?)?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        self.get_todo(board, id)?
            .ok_or_else(|| "To-do not found".into())
    }

    pub fn delete_todo(&self, board: &TodoBoard, id: &str) -> Result<(), String> {
        self.todo_schema()?;
        let current = self.get_todo(board, id)?.ok_or("To-do not found")?;
        if self.list_todos(board)?.iter().any(|item| {
            item.id != id && item.dependencies.iter().any(|dependency| dependency == id)
        }) {
            return Err("Remove dependent to-do links before deleting this task".into());
        }
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM todo_items WHERE id=?1", [id])
            .map_err(|e| e.to_string())?;
        renumber(&tx, &column_ids(&tx, &board.key(), current.status)?)?;
        tx.commit().map_err(|e| e.to_string())
    }

    /// The `todos.*` app actions on one board, which the caller has already
    /// resolved and authorized.
    pub fn todo_action(
        &self,
        board: &TodoBoard,
        action: &str,
        input: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        use serde_json::{json, Value};
        let text = |key: &str| input.get(key).and_then(Value::as_str);
        let status = |fallback: Option<TodoStatus>| -> Result<Option<TodoStatus>, String> {
            match text("status") {
                Some(id) => TodoStatus::from_id(id)
                    .map(Some)
                    .ok_or_else(|| "status must be todo, inProgress, inReview or done".into()),
                None => Ok(fallback),
            }
        };
        let id = || text("id").ok_or("Provide id");
        Ok(match action {
            "todos.list" => json!({"board": board.key(), "items": self.list_todos(board)?}),
            "todos.add" => {
                let attachments = strings(input, "attachments")?.unwrap_or_default();
                let dependencies = strings(input, "dependencies")?.unwrap_or_default();
                json!(self.add_todo_with_details(
                    board,
                    text("title").ok_or("Provide title")?,
                    status(Some(TodoStatus::Todo))?.unwrap_or_default(),
                    text("description").unwrap_or_default(),
                    &attachments,
                    &dependencies
                )?)
            }
            "todos.update" => {
                let id = id()?;
                let next = status(None)?;
                json!(self.update_todo_atomic(board, id, input, next,)?)
            }
            "todos.remove" => {
                self.delete_todo(board, id()?)?;
                json!({"removed": true})
            }
            _ => return Err("Unknown to-do action".into()),
        })
    }

    /// Open (not done) item counts per board key.
    pub fn open_todo_counts(&self) -> Result<std::collections::HashMap<String, usize>, String> {
        self.todo_schema()?;
        let mut statement = self
            .conn
            .prepare("SELECT board,COUNT(*) FROM todo_items WHERE status!='done' GROUP BY board")
            .map_err(|e| e.to_string())?;
        let counts = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string());
        counts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn store() -> Store {
        Store::open(Path::new(":memory:")).unwrap()
    }

    fn titles(store: &Store, board: &TodoBoard, status: TodoStatus) -> Vec<String> {
        store
            .list_todos(board)
            .unwrap()
            .into_iter()
            .filter(|item| item.status == status)
            .map(|item| item.title)
            .collect()
    }

    #[test]
    fn boards_are_separate_and_columns_keep_their_order() {
        let store = store();
        let project = TodoBoard::Project("/work".into());
        let bot = TodoBoard::Bot("oppy".into());
        let a = store.add_todo(&project, "a", TodoStatus::Todo).unwrap();
        let b = store.add_todo(&project, "b", TodoStatus::Todo).unwrap();
        let c = store.add_todo(&project, "c", TodoStatus::Todo).unwrap();
        store.add_todo(&bot, "mine", TodoStatus::Todo).unwrap();
        assert_eq!(titles(&store, &project, TodoStatus::Todo), ["a", "b", "c"]);
        assert_eq!(titles(&store, &bot, TodoStatus::Todo), ["mine"]);

        // Into another column, at its end; the source closes its gap.
        store
            .move_todo(&project, &b.id, TodoStatus::InProgress, None)
            .unwrap();
        assert_eq!(titles(&store, &project, TodoStatus::Todo), ["a", "c"]);
        assert_eq!(titles(&store, &project, TodoStatus::InProgress), ["b"]);
        let positions: Vec<_> = store
            .list_todos(&project)
            .unwrap()
            .into_iter()
            .filter(|i| i.status == TodoStatus::Todo)
            .map(|i| i.position)
            .collect();
        assert_eq!(positions, [0, 1]);

        // Reordered before another item in the same column.
        store
            .move_todo(&project, &c.id, TodoStatus::Todo, Some(&a.id))
            .unwrap();
        assert_eq!(titles(&store, &project, TodoStatus::Todo), ["c", "a"]);

        // Items cannot be reached through another board.
        assert!(store.rename_todo(&bot, &a.id, "x").is_err());
        assert!(store.delete_todo(&bot, &a.id).is_err());
        store.rename_todo(&project, &a.id, "  renamed ").unwrap();
        store.delete_todo(&project, &c.id).unwrap();
        assert_eq!(titles(&store, &project, TodoStatus::Todo), ["renamed"]);

        let counts = store.open_todo_counts().unwrap();
        assert_eq!(counts.get(&project.key()), Some(&2));
        assert_eq!(counts.get(&bot.key()), Some(&1));
    }

    #[test]
    fn app_actions_add_move_and_remove() {
        use serde_json::json;
        let store = store();
        let board = TodoBoard::Bot("oppy".into());
        let added = store
            .todo_action(&board, "todos.add", &json!({"title":"Write tests"}))
            .unwrap();
        let id = added["id"].as_str().unwrap().to_owned();
        assert_eq!(added["status"], "todo");
        let moved = store
            .todo_action(
                &board,
                "todos.update",
                &json!({"id":id,"status":"inProgress","title":"Write more tests"}),
            )
            .unwrap();
        assert_eq!(moved["status"], "inProgress");
        assert_eq!(moved["title"], "Write more tests");
        assert!(store
            .todo_action(&board, "todos.update", &json!({"id":id,"status":"later"}))
            .is_err());
        let listed = store.todo_action(&board, "todos.list", &json!({})).unwrap();
        assert_eq!(listed["items"].as_array().unwrap().len(), 1);
        // A validated `before` reorders by ID.
        let first = store
            .todo_action(&board, "todos.add", &json!({"title":"First"}))
            .unwrap();
        let second = store
            .todo_action(&board, "todos.add", &json!({"title":"Second"}))
            .unwrap();
        let reorder = json!({"id":second["id"],"before":first["id"]});
        crate::bot::api::validate("todos.update", &reorder).unwrap();
        store.todo_action(&board, "todos.update", &reorder).unwrap();
        let titles: Vec<_> = store
            .list_todos(&board)
            .unwrap()
            .into_iter()
            .filter(|item| item.status == TodoStatus::Todo)
            .map(|item| item.title)
            .collect();
        assert_eq!(titles, ["Second", "First"]);
        for item in [&first, &second] {
            store
                .todo_action(&board, "todos.remove", &json!({"id":item["id"]}))
                .unwrap();
        }
        // Another board cannot reach it.
        assert!(store
            .todo_action(&TodoBoard::Personal, "todos.remove", &json!({"id":id}))
            .is_err());
        store
            .todo_action(&board, "todos.remove", &json!({"id":id}))
            .unwrap();
        assert!(store.list_todos(&board).unwrap().is_empty());
    }

    #[test]
    fn listing_orders_columns_and_rejects_empty_titles() {
        let store = store();
        let board = TodoBoard::Personal;
        let done = store.add_todo(&board, "done", TodoStatus::Done).unwrap();
        store.add_todo(&board, "todo", TodoStatus::Todo).unwrap();
        store
            .add_todo(&board, "doing", TodoStatus::InProgress)
            .unwrap();
        store
            .add_todo(&board, "review", TodoStatus::InReview)
            .unwrap();
        let statuses: Vec<_> = store
            .list_todos(&board)
            .unwrap()
            .into_iter()
            .map(|i| i.status)
            .collect();
        assert_eq!(statuses, TodoStatus::ALL);
        assert!(store.add_todo(&board, " ", TodoStatus::Todo).is_err());
        assert_eq!(
            store.get_todo(&board, &done.id).unwrap().unwrap().title,
            "done"
        );
    }

    #[test]
    fn old_rows_migrate_without_losing_legacy_bot_tasks() {
        let store = store();
        store.conn.execute_batch("CREATE TABLE todo_items(id TEXT PRIMARY KEY,board TEXT NOT NULL,title TEXT NOT NULL,status TEXT NOT NULL,position INTEGER NOT NULL,created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL); INSERT INTO todo_items VALUES('old','bot:careful','Keep this','inProgress',0,1,1);").unwrap();
        let old = TodoBoard::Bot("careful".into());
        let item = store.get_todo(&old, "old").unwrap().unwrap();
        assert_eq!(item.title, "Keep this");
        assert_eq!(item.description, "");
        assert_eq!(item.sync, TodoSync::Local);
        assert_eq!(
            store.legacy_bot_todo_counts().unwrap().get("careful"),
            Some(&1)
        );
        store
            .add_todo(
                &TodoBoard::Project("/p".into()),
                "Existing",
                TodoStatus::InProgress,
            )
            .unwrap();
        assert_eq!(store.assign_legacy_bot_todos("careful", "/p").unwrap(), 1);
        assert!(store.get_todo(&old, "old").unwrap().is_none());
        let assigned = store
            .get_todo(&TodoBoard::Project("/p".into()), "old")
            .unwrap()
            .unwrap();
        assert_eq!(assigned.position, 1);
        assert_eq!(assigned.status, TodoStatus::InProgress);
    }

    #[test]
    fn details_and_submission_state_survive_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("todos.sqlite");
        let board = TodoBoard::Project("/p".into());
        let id = {
            let store = Store::open(&path).unwrap();
            let item = store
                .add_todo_with_details(
                    &board,
                    "Ship",
                    TodoStatus::InReview,
                    "Description",
                    &["https://example.com/a".into()],
                    &["#12".into()],
                )
                .unwrap();
            let (_, token) = store
                .begin_todo_publish(&board, &item.id, "acme/app")
                .unwrap();
            assert!(!token.is_empty());
            store.uncertain_todo_publish(&board, &item.id).unwrap();
            item.id
        };
        let store = Store::open(&path).unwrap();
        let item = store.get_todo(&board, &id).unwrap().unwrap();
        assert_eq!(item.status, TodoStatus::InReview);
        assert_eq!(item.description, "Description");
        assert_eq!(item.attachments, ["https://example.com/a"]);
        assert_eq!(item.dependencies, ["#12"]);
        assert_eq!(item.sync, TodoSync::Uncertain);
        assert_eq!(item.pending_repo.as_deref(), Some("acme/app"));
        assert!(store.begin_todo_publish(&board, &id, "acme/app").is_err());
        assert_eq!(store.pending_todo_submissions(&board).unwrap().len(), 1);
        let (item, token) = store.retry_todo_publish(&board, &id).unwrap();
        assert_eq!(token, item.submission_token.unwrap());
    }

    #[test]
    fn created_issue_receipt_and_initial_status_edit_commit_together() {
        let store = store();
        let board = TodoBoard::Project("/p".into());
        let local = store
            .add_todo(&board, "Review", TodoStatus::InReview)
            .unwrap();
        store
            .begin_todo_publish(&board, &local.id, "acme/app")
            .unwrap();
        let mut remote = local.clone();
        remote.id = "github:acme/app#17".into();
        remote.status = TodoStatus::Todo;
        remote.issue = Some(TodoIssue {
            repo: "acme/app".into(),
            number: 17,
            url: "https://github.com/acme/app/issues/17".into(),
            labels: vec![],
            dependencies_truncated: false,
        });
        let follow_up = store
            .finish_todo_publish(&board, &local.id, &remote)
            .unwrap()
            .unwrap();
        assert_eq!(follow_up.input["status"], "inReview");
        let linked = store.get_todo(&board, &local.id).unwrap().unwrap();
        assert_eq!(linked.sync, TodoSync::Synced);
        assert_eq!(linked.pending_repo, None);
        assert_eq!(linked.issue.unwrap().number, 17);
        let pending = store.pending_remote_todo_edits(&board).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].pending_edit.as_ref().unwrap(), &follow_up);
    }

    #[test]
    fn remote_edit_receipt_blocks_replay_and_survives_reopen() {
        use serde_json::json;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("todos.sqlite");
        let board = TodoBoard::Project("/p".into());
        let id = "github:acme/app#7";
        {
            let store = Store::open(&path).unwrap();
            let mut snapshot = store
                .add_todo(&board, "Remote issue", TodoStatus::Todo)
                .unwrap();
            snapshot.id = id.into();
            snapshot.issue = Some(TodoIssue {
                repo: "acme/app".into(),
                number: 7,
                url: "https://github.com/acme/app/issues/7".into(),
                labels: vec![],
                dependencies_truncated: false,
            });
            let attempt = store
                .begin_remote_todo_edit(
                    &board,
                    &snapshot,
                    &json!({"id":id,"description":"Keep the draft"}),
                )
                .unwrap();
            assert!(store
                .begin_remote_todo_edit(&board, &snapshot, &json!({"status":"done"}))
                .is_err());
            store
                .uncertain_remote_todo_edit(&board, id, &attempt)
                .unwrap();
        }
        let store = Store::open(&path).unwrap();
        let saved = store.pending_remote_todo_edits(&board).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].id, id);
        assert_eq!(
            saved[0].pending_edit.as_ref().unwrap().input["description"],
            "Keep the draft"
        );
        assert_eq!(
            saved[0].pending_edit.as_ref().unwrap().phase,
            TodoSync::Uncertain
        );
        let original = &saved[0].pending_edit.as_ref().unwrap().attempt_id;
        let retried = store.retry_remote_todo_edit(&board, id, original).unwrap();
        assert_eq!(retried.input["description"], "Keep the draft");
        assert_ne!(&retried.attempt_id, original);
        assert!(store.clear_remote_todo_edit(&board, id, original).is_err());
        assert!(store
            .uncertain_remote_todo_edit(&board, id, original)
            .is_err());
        assert!(store.retry_remote_todo_edit(&board, id, original).is_err());
        let current = store.pending_remote_todo_edits(&board).unwrap();
        assert_eq!(current[0].pending_edit.as_ref().unwrap(), &retried);
        store
            .clear_remote_todo_edit(&board, id, &retried.attempt_id)
            .unwrap();
        assert!(store.pending_remote_todo_edits(&board).unwrap().is_empty());
    }

    #[test]
    fn local_dependency_links_stay_on_board_and_cannot_cycle() {
        let store = store();
        let board = TodoBoard::Project("/p".into());
        let other = TodoBoard::Project("/other".into());
        let a = store.add_todo(&board, "A", TodoStatus::Todo).unwrap();
        let b = store.add_todo(&board, "B", TodoStatus::Todo).unwrap();
        let foreign = store.add_todo(&other, "Foreign", TodoStatus::Todo).unwrap();
        store
            .update_todo_details(
                &board,
                &a.id,
                None,
                None,
                Some(&[
                    b.id.clone(),
                    "#42".into(),
                    "https://github.com/acme/upstream/issues/42".into(),
                ]),
            )
            .unwrap();
        assert!(store
            .delete_todo(&board, &b.id)
            .unwrap_err()
            .contains("dependent"));
        assert!(store
            .move_todo(&board, &a.id, TodoStatus::InReview, Some(&b.id))
            .unwrap_err()
            .contains("not in this column"));
        assert_eq!(
            store.get_todo(&board, &a.id).unwrap().unwrap().status,
            TodoStatus::Todo
        );
        assert!(store
            .update_todo_details(&board, &b.id, None, None, Some(std::slice::from_ref(&a.id)))
            .unwrap_err()
            .contains("cycle"));
        assert!(store
            .update_todo_details(&board, &b.id, None, None, Some(&[foreign.id]))
            .unwrap_err()
            .contains("not on this board"));
        assert!(store
            .update_todo_details(&board, &b.id, None, None, Some(&["#0".into()]))
            .is_err());
    }

    #[test]
    fn rejected_combined_update_rolls_back_every_field() {
        use serde_json::json;
        let store = store();
        let board = TodoBoard::Personal;
        let first = store.add_todo(&board, "First", TodoStatus::Todo).unwrap();
        let second = store.add_todo(&board, "Second", TodoStatus::Todo).unwrap();
        let error = store
            .todo_action(
                &board,
                "todos.update",
                &json!({
                    "id": first.id.clone(),
                    "title": "Changed",
                    "description": "Changed body",
                    "status": "inReview",
                    "before": second.id.clone(),
                }),
            )
            .unwrap_err();
        assert!(error.contains("Move target"));
        let saved = store.get_todo(&board, &first.id).unwrap().unwrap();
        assert_eq!(saved.title, "First");
        assert_eq!(saved.description, "");
        assert_eq!(saved.status, TodoStatus::Todo);
    }
}
