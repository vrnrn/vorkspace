CREATE TABLE IF NOT EXISTS ordinary_transcripts (
        session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
        revision INTEGER NOT NULL DEFAULT 0, normalized INTEGER NOT NULL DEFAULT 0);
      CREATE TABLE IF NOT EXISTS ordinary_blocks (
        session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
        block_id TEXT NOT NULL, position INTEGER NOT NULL, block_json TEXT NOT NULL,
        has_user INTEGER NOT NULL, is_draft INTEGER NOT NULL,
        PRIMARY KEY(session_id, block_id));
      CREATE INDEX IF NOT EXISTS ordinary_blocks_order ON ordinary_blocks(session_id, position);
      CREATE INDEX IF NOT EXISTS ordinary_blocks_flags ON ordinary_blocks(session_id,has_user,is_draft);
      CREATE TABLE IF NOT EXISTS ordinary_detached_leads(session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,lead_id TEXT NOT NULL,PRIMARY KEY(session_id,lead_id));
