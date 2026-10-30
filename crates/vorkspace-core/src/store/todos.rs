//! To-do board persistence. Items belong to a board key and keep their
//! order within each status column.
use super::Store;
use crate::todos::{clean_title, TodoBoard, TodoItem, TodoStatus};
use rusqlite::{params, Connection, OptionalExtension, Transaction};

pub fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS todo_items (
           id TEXT PRIMARY KEY,
           board TEXT NOT NULL,
           title TEXT NOT NULL,
           status TEXT NOT NULL,
           position INTEGER NOT NULL,
           created_at INTEGER NOT NULL,
           updated_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS todo_items_board_idx
           ON todo_items (board, status, position);",
    )
}

fn now_millis() -> i64 {
    crate::time::now_ms().min(i64::MAX as u64) as i64
}

fn item(row: &rusqlite::Row<'_>) -> rusqlite::Result<TodoItem> {
    let status: String = row.get(2)?;
    Ok(TodoItem {
        id: row.get(0)?,
        title: row.get(1)?,
        status: TodoStatus::from_id(&status).unwrap_or_default(),
        position: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

const COLUMNS: &str = "id,title,status,position,created_at,updated_at";

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
        self.todo_schema()?;
        let title = clean_title(title)?;
        let now = now_millis();
        let id = crate::bot::new_id();
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        let position = column_ids(&tx, &board.key(), status)?.len() as i64;
        tx.execute(
            "INSERT INTO todo_items(id,board,title,status,position,created_at,updated_at)
             VALUES(?1,?2,?3,?4,?5,?6,?6)",
            params![id, board.key(), title, status.id(), position, now],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        self.get_todo(board, &id)?
            .ok_or_else(|| "The item was not saved".into())
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
        let at = before
            .and_then(|before| ids.iter().position(|other| other == before))
            .unwrap_or(ids.len());
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
                    .ok_or_else(|| "status must be todo, inProgress or done".into()),
                None => Ok(fallback),
            }
        };
        let id = || text("id").ok_or("Provide id");
        Ok(match action {
            "todos.list" => json!({"board": board.key(), "items": self.list_todos(board)?}),
            "todos.add" => json!(self.add_todo(
                board,
                text("title").ok_or("Provide title")?,
                status(Some(TodoStatus::Todo))?.unwrap_or_default(),
            )?),
            "todos.update" => {
                let id = id()?;
                let mut item = self.get_todo(board, id)?.ok_or("To-do not found")?;
                if let Some(title) = text("title") {
                    item = self.rename_todo(board, id, title)?;
                }
                if let Some(next) = status(None)? {
                    item = self.move_todo(board, id, next, text("before"))?;
                } else if let Some(before) = text("before") {
                    item = self.move_todo(board, id, item.status, Some(before))?;
                }
                json!(item)
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
}
