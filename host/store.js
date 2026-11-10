import { DatabaseSync } from "node:sqlite";
import { randomUUID, createHash, randomBytes } from "node:crypto";
import { dirname, join } from "node:path";
import { sessionNeedsInput } from "./runtime/models/sessions/session.js";
const CACHED_SESSIONS = 32;
const CACHED_SESSION_BYTES = 16 * 1024 * 1024;
const MAX_CACHED_SESSION_BYTES = 8 * 1024 * 1024;
const MIN_CACHED_SESSION_BYTES = 4 * 1024;
export class HostStore {
  db;
  environmentId;
  attachmentDir;
  // This process is the only session writer, so recently used snapshots are
  // served from memory instead of re-parsing whole transcripts. Callers must
  // treat returned values as immutable.
  cache = new Map();
  cacheBytes = 0;
  // Only store-owned, deeply frozen blocks can skip serialization. Caller-owned
  // objects are always encoded again, including nested mutations.
  encodedBlocks = new WeakMap();
  constructor(path) {
    this.attachmentDir = join(dirname(path), "attachments");
    this.db = new DatabaseSync(path);
    this.db
      .exec(`PRAGMA busy_timeout=5000; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
      CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS projects (id TEXT PRIMARY KEY, cwd TEXT NOT NULL UNIQUE, name TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS sessions (id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id), snapshot TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS receipts (id TEXT PRIMARY KEY, signature TEXT NOT NULL, receipt TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS events (session_id TEXT NOT NULL REFERENCES sessions(id), revision INTEGER NOT NULL, payload TEXT NOT NULL, PRIMARY KEY(session_id, revision));
      CREATE TABLE IF NOT EXISTS devices (id TEXT PRIMARY KEY, name TEXT NOT NULL, hash TEXT NOT NULL UNIQUE);`);
    const columns = this.db.prepare("PRAGMA table_info(sessions)").all();
    if (!columns.some((column) => column.name === "summary"))
      this.db.exec("ALTER TABLE sessions ADD COLUMN summary TEXT");
    this.db.exec(`CREATE TABLE IF NOT EXISTS session_blocks (
      session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
      id TEXT NOT NULL, ordinal INTEGER NOT NULL, revision INTEGER NOT NULL,
      digest TEXT NOT NULL, code_units INTEGER NOT NULL, payload TEXT NOT NULL,
      PRIMARY KEY(session_id, id));
      CREATE INDEX IF NOT EXISTS session_block_order ON session_blocks(session_id, ordinal);
      CREATE INDEX IF NOT EXISTS session_block_revision ON session_blocks(session_id, revision);`);
    // A summary sits after the large snapshot in the row. A covering index
    // keeps metadata reads off the transcript's SQLite overflow pages.
    this.db.exec(
      "CREATE INDEX IF NOT EXISTS session_summary_lookup ON sessions (id, project_id, summary)",
    );
    this.db
      .prepare("INSERT OR IGNORE INTO metadata VALUES ('environmentId', ?)")
      .run(randomUUID());
    this.environmentId = String(
      this.db
        .prepare("SELECT value FROM metadata WHERE key='environmentId'")
        .get().value,
    );
  }
  transaction(fn, readOnly = false) {
    this.db.exec(readOnly ? "BEGIN" : "BEGIN IMMEDIATE");
    try {
      const value = fn();
      this.db.exec("COMMIT");
      return value;
    } catch (error) {
      this.clearCache();
      try {
        this.db.exec("ROLLBACK");
      } catch (rollbackError) {
        console.error("Could not roll back host transaction:", rollbackError);
      }
      throw error;
    }
  }
  project(id) {
    const row = this.db.prepare("SELECT * FROM projects WHERE id=?").get(id);
    if (!row) throw new Error("Project is not registered on this machine");
    return row;
  }
  projects() {
    return this.db.prepare("SELECT * FROM projects ORDER BY name").all();
  }
  addProject(cwd, name) {
    this.db
      .prepare("INSERT OR IGNORE INTO projects VALUES (?, ?, ?)")
      .run(randomUUID(), cwd, name);
    return this.db.prepare("SELECT * FROM projects WHERE cwd=?").get(cwd);
  }
  removeCached(id) {
    const cached = this.cache.get(id);
    if (!cached) return;
    this.cache.delete(id);
    this.cacheBytes -= cached.weight;
  }
  clearCache() {
    this.cache.clear();
    this.cacheBytes = 0;
  }
  remember(value, snapshotLength) {
    const id = value.session.id;
    this.removeCached(id);
    // Parsed JSON retains both strings and object/array structure. Four bytes
    // per serialized UTF-16 code unit is a cheap conservative heap estimate;
    // snapshotLength is already available from the DB row or save serialization.
    const weight = Math.max(MIN_CACHED_SESSION_BYTES, snapshotLength * 4);
    if (weight > MAX_CACHED_SESSION_BYTES) return value;
    this.cache.set(id, { value, weight });
    this.cacheBytes += weight;
    while (
      this.cache.size > CACHED_SESSIONS ||
      this.cacheBytes > CACHED_SESSION_BYTES
    ) {
      const oldest = this.cache.keys().next().value;
      if (oldest === undefined) break;
      this.removeCached(oldest);
    }
    return value;
  }
  #cached(id) {
    const cached = this.cache.get(id);
    if (cached) {
      this.cache.delete(id);
      this.cache.set(id, cached);
      return cached.value;
    }
  }
  #root(id) {
    const row = this.db
      .prepare("SELECT snapshot FROM sessions WHERE id=?")
      .get(id);
    return row && String(row.snapshot);
  }
  #hydrate(id, value, length) {
    if (!Object.hasOwn(value.session, "blocks")) {
      const rows = this.db.prepare(
        "SELECT id, revision, digest, code_units, payload FROM session_blocks WHERE session_id=? ORDER BY ordinal",
      ).all(id);
      value.session.blocks = rows.map((row) => {
        const block = freezeJson(JSON.parse(String(row.payload)));
        this.encodedBlocks.set(block, {
          digest: String(row.digest), length: Number(row.code_units),
        });
        length += Number(row.code_units);
        return block;
      });
      value.blockRevisions = Object.fromEntries(rows.map((row) => [row.id, Number(row.revision)]));
    }
    return this.remember(value, length);
  }

  find(id) {
    const cached = this.#cached(id);
    if (cached) return cached;
    // Read metadata and blocks from one SQLite revision.
    if (!this.db.isTransaction)
      return this.transaction(() => this.find(id), true);
    const snapshot = this.#root(id);
    return snapshot && this.#hydrate(id, JSON.parse(snapshot), snapshot.length);
  }

  session(id) {
    const value = this.find(id);
    if (!value) throw new Error("Session not found on this machine");
    return value;
  }
  summaries(projectId) {
    const rows =
      projectId === undefined
        ? this.db
            .prepare("SELECT id, summary FROM sessions INDEXED BY session_summary_lookup")
            .all()
        : this.db
            .prepare(
              "SELECT id, summary FROM sessions INDEXED BY session_summary_lookup WHERE project_id=?",
            )
            .all(projectId);
    return rows
      .map((row) => {
        let cached;
        try {
          if (row.summary) cached = JSON.parse(String(row.summary));
        } catch {
          // Summaries are derived metadata; repair a malformed cache from the
          // durable snapshot rather than making startup depend on its validity.
        }
        if (
          cached?.model &&
          typeof cached.harness === "string" &&
          typeof cached.cwd === "string" &&
          cached.needsInput !== undefined &&
          cached.providerSessionId !== undefined
        )
          return cached;
        const fresh = summary(this.session(String(row.id)));
        this.db
          .prepare("UPDATE sessions SET summary=? WHERE id=?")
          .run(JSON.stringify(fresh), String(row.id));
        return fresh;
      })
      .sort((a, b) => b.updatedAt - a.updatedAt);
  }
  sync(id, revision) {
    const cached = this.#cached(id);
    if (cached) return syncSnapshot(cached, revision);
    if (!this.db.isTransaction)
      return this.transaction(() => this.sync(id, revision), true);

    // The covering summary index handles unchanged polls without reading the root.
    if (Number.isSafeInteger(revision)) {
      const row = this.db.prepare(
        "SELECT summary FROM sessions INDEXED BY session_summary_lookup WHERE id=?",
      ).get(id);
      try {
        const metadata = row?.summary && JSON.parse(String(row.summary));
        if (metadata?.id === id && metadata.revision === revision)
          return { kind: "unchanged", revision };
      } catch {
        // Old or malformed derived metadata falls through to the durable root.
      }
    }

    const snapshot = this.#root(id);
    if (!snapshot) throw new Error("Session not found on this machine");
    const value = JSON.parse(snapshot);
    if (revision === value.revision) return { kind: "unchanged", revision };
    if (!Number.isSafeInteger(revision) || revision > value.revision || Object.hasOwn(value.session, "blocks"))
      return syncSnapshot(this.#hydrate(id, value, snapshot.length), revision);

    // Keep the wire order for older clients. Only changed payloads need decoding.
    const ids = this.db.prepare(
      "SELECT id FROM session_blocks WHERE session_id=? ORDER BY ordinal",
    ).all(id);
    const changed = this.db.prepare(
      "SELECT payload FROM session_blocks INDEXED BY session_block_revision WHERE session_id=? AND revision>? ORDER BY ordinal",
    ).all(id, revision);
    return {
      kind: "delta",
      base: revision,
      value,
      blockIds: ids.map((row) => row.id),
      blocks: changed.map((row) => JSON.parse(String(row.payload))),
    };
  }
  sessions(projectId) {
    const rows = projectId
      ? this.db.prepare("SELECT id FROM sessions WHERE project_id=?").all(projectId)
      : this.db.prepare("SELECT id FROM sessions").all();
    return rows.map((row) => this.session(String(row.id)))
      .sort((a, b) => b.updatedAt - a.updatedAt);
  }
  advance(value, event) {
    return this.transaction(() => this.save(
      { ...value, revision: value.revision + 1, updatedAt: Date.now() },
      event,
    ));
  }
  /** Metadata, changed blocks, summary, event and caller receipts commit together.
   * Legacy full snapshots convert lazily on their next write. */
  save(input, event) {
    if (!this.db.isTransaction)
      return this.transaction(() => this.save(input, event));
    const id = input.session.id;
    const priorRow = this.db.prepare("SELECT snapshot FROM sessions WHERE id=?").get(id);
    const prior = priorRow && JSON.parse(String(priorRow.snapshot));
    const previous = new Map(this.db.prepare(
      "SELECT id, ordinal, revision, digest, code_units FROM session_blocks WHERE session_id=?",
    ).all(id).map((row) => [String(row.id), row]));
    const legacyBlocks = new Map(prior?.session.blocks?.map((block) => [block.id, block]));
    const blocks = [];
    const revisions = Object.create(null);
    const writes = [];
    const ids = new Set();
    let length = 0;
    for (const [ordinal, incoming] of input.session.blocks.entries()) {
      if (typeof incoming.id !== "string" || ids.has(incoming.id))
        throw new Error("Session blocks must have unique string IDs");
      ids.add(incoming.id);
      let encoded = this.encodedBlocks.get(incoming);
      let payload;
      let block = incoming;
      if (!encoded) {
        payload = JSON.stringify(incoming);
        encoded = {
          digest: createHash("sha256").update(payload).digest("hex"),
          length: payload.length,
        };
        // Freeze an independent JSON copy. Provider-owned preview objects remain
        // mutable, while a saved block cannot silently change behind its digest.
        block = freezeJson(JSON.parse(payload));
        this.encodedBlocks.set(block, encoded);
      }
      const old = previous.get(incoming.id);
      const unchanged = old?.digest === encoded.digest;
      // A legacy block's structural equality preserves its original sync stamp.
      const legacy = legacyBlocks.get(incoming.id);
      const legacySame = !old && legacy && JSON.stringify(legacy) === (payload ?? JSON.stringify(block));
      const revision = unchanged ? Number(old.revision)
        : legacySame ? (prior.blockRevisions?.[incoming.id] ?? input.revision) : input.revision;
      revisions[incoming.id] = revision;
      blocks.push(block);
      length += encoded.length;
      if (!unchanged) writes.push({ id: incoming.id, ordinal, revision, encoded, payload: payload ?? JSON.stringify(block) });
      else if (Number(old.ordinal) !== ordinal) writes.push({ id: incoming.id, ordinal });
    }
    const value = {
      ...input,
      session: { ...input.session, blocks },
      createdAt: input.createdAt ?? prior?.createdAt ?? prior?.updatedAt ?? input.updatedAt,
      blockRevisions: revisions,
    };
    const { blocks: _blocks, ...session } = value.session;
    const { blockRevisions: _revisions, ...metadata } = value;
    const snapshot = JSON.stringify({ ...metadata, session });
    // Publish the small root row before its FK children within this transaction;
    // no other connection can observe it until every payload and receipt commits.
    this.db.prepare(
      "INSERT INTO sessions (id, project_id, snapshot, summary) VALUES (?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET project_id=excluded.project_id, snapshot=excluded.snapshot, summary=excluded.summary",
    ).run(id, value.projectId, snapshot, JSON.stringify(summary(value)));
    const upsert = this.db.prepare(
      "INSERT INTO session_blocks VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT(session_id, id) DO UPDATE SET ordinal=excluded.ordinal, revision=excluded.revision, digest=excluded.digest, code_units=excluded.code_units, payload=excluded.payload",
    );
    for (const write of writes) {
      if (write.payload === undefined)
        this.db.prepare("UPDATE session_blocks SET ordinal=? WHERE session_id=? AND id=?").run(write.ordinal, id, write.id);
      else upsert.run(id, write.id, write.ordinal, write.revision, write.encoded.digest, write.encoded.length, write.payload);
    }
    for (const oldId of previous.keys())
      if (!ids.has(oldId)) this.db.prepare("DELETE FROM session_blocks WHERE session_id=? AND id=?").run(id, oldId);
    this.db.prepare("INSERT INTO events VALUES (?, ?, ?)").run(id, value.revision, JSON.stringify(event));
    this.db.prepare("DELETE FROM events WHERE session_id=? AND revision<?").run(id, value.revision - 2_000);
    return this.remember(value, length + snapshot.length);
  }
  updateSession(id, patch) {
    return this.transaction(() => {
      const current = this.session(id);
      if (
        patch.title !== undefined &&
        (!patch.title.trim() || patch.title.length > 200)
      )
        throw new Error("Invalid session title");
      const next = this.save(
        {
          ...current,
          revision: current.revision + 1,
          archived: patch.archived ?? current.archived,
          pinned: patch.pinned ?? current.pinned,
          session: {
            ...current.session,
            ...(patch.title === undefined ? {} : { title: patch.title.trim() }),
            ...(patch.linkedWorkItem === undefined
              ? {}
              : { linkedWorkItem: patch.linkedWorkItem ?? undefined }),
          },
        },
        { type: "session.metadata", patch },
      );
      return summary(next);
    });
  }
  deleteSession(id) {
    this.transaction(() => {
      const current = this.session(id);
      if (current.status === "running")
        throw new Error("Stop this session before deleting it");
      this.db.prepare("DELETE FROM events WHERE session_id=?").run(id);
      this.db.prepare("DELETE FROM sessions WHERE id=?").run(id);
      this.removeCached(id);
    });
  }
  receipt(id, signature) {
    const row = this.db.prepare("SELECT * FROM receipts WHERE id=?").get(id);
    if (!row) return undefined;
    if (row.signature !== signature)
      throw new Error("Command ID was already used with a different payload");
    return JSON.parse(String(row.receipt));
  }
  recordReceipt(signature, receipt) {
    this.db
      .prepare("INSERT INTO receipts VALUES (?, ?, ?)")
      .run(receipt.commandId, signature, JSON.stringify(receipt));
  }
  events(id, after) {
    if (!this.db.isTransaction)
      return this.transaction(() => this.events(id, after), true);
    const cached = this.#cached(id);
    const root = cached ? undefined : this.#root(id);
    if (!cached && !root) throw new Error("Session not found on this machine");
    const value = cached ?? JSON.parse(root);
    const revision = value.revision;
    const rows = this.db
      .prepare(
        "SELECT revision, payload FROM events WHERE session_id=? AND revision>? ORDER BY revision",
      )
      .all(id, after);
    if (
      after > revision ||
      (after < revision && Number(rows[0]?.revision) !== after + 1)
    ) {
      return { snapshot: cached ?? this.#hydrate(id, value, root.length), revision };
    }
    return {
      events: rows.map((row) => ({
        revision: row.revision,
        event: JSON.parse(String(row.payload)),
      })),
      revision,
    };
  }
  issueDevice(name) {
    const id = randomUUID();
    const token = randomBytes(32).toString("base64url");
    this.db
      .prepare("INSERT INTO devices VALUES (?, ?, ?)")
      .run(id, name, this.hash(token));
    return { id, token };
  }
  revokeDevice(id) {
    return (
      Number(
        this.db.prepare("DELETE FROM devices WHERE id=?").run(id).changes,
      ) > 0
    );
  }
  /** Lets a desktop revoke only the credential it is using. */
  revokeToken(token) {
    return (
      Number(
        this.db
          .prepare("DELETE FROM devices WHERE hash=?")
          .run(this.hash(token)).changes,
      ) > 0
    );
  }
  authenticated(token) {
    return !!this.db
      .prepare("SELECT id FROM devices WHERE hash=?")
      .get(this.hash(token));
  }
  hash(token) {
    return createHash("sha256").update(token).digest("hex");
  }
  close() {
    this.db.close();
  }
}
export function summary(value) {
  return {
    projectId: value.projectId,
    revision: value.revision,
    runId: value.runId,
    status: value.status,
    updatedAt: value.updatedAt,
    id: value.session.id,
    cwd: value.session.cwd,
    title: value.session.title,
    harness: value.session.harness,
    model: value.session.model,
    runtimeMode: value.session.runtimeMode,
    providerSessionId: value.session.providerSessionId ?? null,
    createdAt: value.createdAt ?? value.updatedAt,
    archived: value.archived,
    pinned: value.pinned,
    linkedWorkItem: value.session.linkedWorkItem,
    needsInput: sessionNeedsInput(value.session),
    draft: value.session.blocks.some(
      (block) => block.role === "user" && block.draft,
    ),
  };
}
function syncSnapshot(value, revision) {
  const { blockRevisions, ...snapshot } = value;
  if (revision === value.revision) return { kind: "unchanged", revision };
  if (!Number.isSafeInteger(revision) || revision > value.revision || !blockRevisions)
    return { kind: "snapshot", value: snapshot };
  const { session: { blocks, ...session }, ...rest } = snapshot;
  return {
    kind: "delta",
    base: revision,
    value: { ...rest, session },
    blockIds: blocks.map((block) => block.id),
    blocks: blocks.filter((block) => (blockRevisions[block.id] ?? value.revision) > revision),
  };
}

// JSON trees have no cycles after decoding. Freeze only new/decoded blocks;
// unchanged history never needs another traversal or serialization.
function freezeJson(value) {
  if (value && typeof value === "object" && !Object.isFrozen(value)) {
    for (const child of Object.values(value)) freezeJson(child);
    Object.freeze(value);
  }
  return value;
}
