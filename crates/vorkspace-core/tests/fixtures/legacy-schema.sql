CREATE TABLE IF NOT EXISTS sessions (
  id TEXT PRIMARY KEY,
  cwd TEXT NOT NULL,
  harness TEXT NOT NULL,
  model TEXT NOT NULL,
  model_settings TEXT NOT NULL DEFAULT '{}',
  runtime_mode TEXT NOT NULL,
  title TEXT NOT NULL,
  provider_session_id TEXT,
  blocks_json TEXT NOT NULL DEFAULT '[]',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS sessions_cwd_updated_idx
  ON sessions (cwd, updated_at DESC);
