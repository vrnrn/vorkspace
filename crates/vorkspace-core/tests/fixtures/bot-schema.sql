CREATE TABLE IF NOT EXISTS bot_transcripts (
           session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE
         );
         CREATE TABLE IF NOT EXISTS bot_turns (
           session_id TEXT NOT NULL REFERENCES bot_transcripts(session_id) ON DELETE CASCADE,
           seq INTEGER NOT NULL,
           PRIMARY KEY (session_id, seq)
         );
         CREATE TABLE IF NOT EXISTS bot_blocks (
           session_id TEXT NOT NULL REFERENCES bot_transcripts(session_id) ON DELETE CASCADE,
           seq INTEGER NOT NULL,
           block_id TEXT NOT NULL,
           block_json TEXT NOT NULL,
           search_text TEXT NOT NULL,
           is_user INTEGER NOT NULL,
           is_draft INTEGER NOT NULL,
           PRIMARY KEY (session_id, seq),
           UNIQUE (session_id, block_id)
         );
         CREATE INDEX IF NOT EXISTS bot_blocks_user ON bot_blocks(session_id, is_user, is_draft);
