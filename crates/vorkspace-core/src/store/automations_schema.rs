//! Automation tables, created with the rest of the database.
use rusqlite::Connection;

pub(crate) fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS automations (
           id TEXT PRIMARY KEY,
           definition_json TEXT NOT NULL,
           enabled INTEGER NOT NULL,
           next_run_at INTEGER NOT NULL,
           updated_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS automations_due_idx
           ON automations (enabled, next_run_at);
         CREATE TABLE IF NOT EXISTS automation_runs (
           id TEXT PRIMARY KEY,
           automation_id TEXT NOT NULL REFERENCES automations(id) ON DELETE CASCADE,
           created_at INTEGER NOT NULL,
           run_json TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS automation_runs_history_idx
           ON automation_runs (automation_id, created_at DESC);
         CREATE TABLE IF NOT EXISTS automation_event_claims (
           automation_id TEXT NOT NULL REFERENCES automations(id) ON DELETE CASCADE,
           event_key TEXT NOT NULL,
           created_at INTEGER NOT NULL,
           PRIMARY KEY (automation_id, event_key)
         );",
    )
}
