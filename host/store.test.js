import { afterEach, expect, it, vi } from "vitest";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { newSession } from "./runtime/models/sessions/session.js";
import { HostStore } from "./store.js";
const cleanups = [];
afterEach(() => {
  for (const cleanup of cleanups.splice(0).reverse()) cleanup();
});
function fixture() {
  const directory = mkdtempSync(join(tmpdir(), "vorkspace-store-cache-"));
  const store = new HostStore(join(directory, "host.db"));
  const project = store.addProject(directory, "Test");
  cleanups.push(() => {
    store.close();
    rmSync(directory, { recursive: true, force: true });
  });
  return { directory, store, project };
}
function storedSession(projectId, cwd, id, text, revision = 1) {
  return {
    session: {
      ...newSession("codex", cwd),
      id,
      model: "codex:test",
      blocks: [{ id: `${id}-block`, role: "assistant", text }],
    },
    projectId,
    revision,
    status: "idle",
    createdAt: 1,
    updatedAt: revision,
  };
}
function cache(store) {
  return store;
}
it("bounds retained snapshots by estimated bytes and promotes LRU hits", () => {
  const { store, project, directory } = fixture();
  const ids = ["cache-a", "cache-b", "cache-c", "cache-d"];
  const payload = "x".repeat(1_100_000);
  for (const id of ids.slice(0, 3))
    store.save(storedSession(project.id, directory, id, payload), {
      type: "test",
    });
  expect(cache(store).cacheBytes).toBeLessThanOrEqual(16 * 1024 * 1024);
  expect(store.session(ids[0]).session.id).toBe(ids[0]);
  store.save(storedSession(project.id, directory, ids[3], payload), {
    type: "test",
  });
  expect([...cache(store).cache.keys()]).toEqual([ids[2], ids[0], ids[3]]);
  expect(cache(store).cacheBytes).toBeLessThanOrEqual(16 * 1024 * 1024);
  const parse = vi.spyOn(JSON, "parse");
  const stringify = vi.spyOn(JSON, "stringify");
  try {
    store.session(ids[0]);
    expect(parse).not.toHaveBeenCalled();
    expect(stringify).not.toHaveBeenCalled();
    store.session(ids[1]);
    // The small metadata row and the one requested block decode independently.
    expect(parse).toHaveBeenCalledTimes(2);
    expect(stringify).not.toHaveBeenCalled();
  } finally {
    parse.mockRestore();
    stringify.mockRestore();
  }
});
it("retains no more than the session entry limit for small snapshots", () => {
  const { store, project, directory } = fixture();
  for (let index = 0; index < 40; index++)
    store.save(
      storedSession(project.id, directory, `small-${index}`, "small"),
      { type: "test" },
    );
  expect(cache(store).cache.size).toBe(32);
  expect(cache(store).cacheBytes).toBeLessThanOrEqual(16 * 1024 * 1024);
});
it("skips individually oversized sessions and keeps byte accounting correct on rollback and delete", () => {
  const { store, project, directory } = fixture();
  const oversized = storedSession(
    project.id,
    directory,
    "too-large",
    "y".repeat(2_200_000),
  );
  store.save(oversized, { type: "test" });
  expect(cache(store).cache.has("too-large")).toBe(false);
  const normal = storedSession(project.id, directory, "normal", "small");
  store.save(normal, { type: "test" });
  const beforeRollback = cache(store).cacheBytes;
  expect(() =>
    store.transaction(() => {
      store.save(
        storedSession(project.id, directory, "normal", "replacement", 2),
        { type: "test" },
      );
      throw new Error("rollback");
    }),
  ).toThrow("rollback");
  expect(cache(store).cacheBytes).toBe(0);
  expect(store.session("normal").session.blocks[0].text).toBe("small");
  expect(cache(store).cacheBytes).toBe(beforeRollback);
  store.deleteSession("normal");
  expect(cache(store).cache.has("normal")).toBe(false);
  expect(cache(store).cacheBytes).toBe(0);
});
it("polls unchanged oversized sessions without reading or parsing their transcripts", () => {
  const { store, project, directory } = fixture();
  const value = storedSession(
    project.id, directory, "large", "x".repeat(2_200_000),
  );
  store.save(value, { type: "test" });
  expect(store.cache.has(value.session.id)).toBe(false);
  const plan = store.db
    .prepare(
      "EXPLAIN QUERY PLAN SELECT summary FROM sessions INDEXED BY session_summary_lookup WHERE id=?",
    )
    .all(value.session.id);
  expect(plan.some((step) => step.detail.includes("COVERING INDEX"))).toBe(true);
  const session = vi.spyOn(store, "session");
  const prepare = vi.spyOn(store.db, "prepare");
  const parse = vi.spyOn(JSON, "parse");
  try {
    for (let index = 0; index < 20; index++)
      expect(store.sync(value.session.id, value.revision)).toEqual({
        kind: "unchanged",
        revision: value.revision,
      });
    expect(session).not.toHaveBeenCalled();
    expect(
      prepare.mock.calls.every(([sql]) => !sql.includes("snapshot")),
    ).toBe(true);
    expect(parse.mock.calls.every(([json]) => json.length < 4096)).toBe(true);
    expect(store.cacheBytes).toBe(0);
  } finally {
    session.mockRestore();
    prepare.mockRestore();
    parse.mockRestore();
  }
});
it("lists project metadata without visiting transcript pages", () => {
  const { store, project, directory } = fixture();
  const other = store.addProject(join(directory, "other"), "Other");
  store.save(storedSession(project.id, directory, "first", "x".repeat(2_200_000)), {
    type: "test",
  });
  store.save(storedSession(other.id, other.cwd, "second", "y".repeat(2_200_000)), {
    type: "test",
  });
  const prepare = vi.spyOn(store.db, "prepare");
  const session = vi.spyOn(store, "session");
  try {
    expect(store.summaries().map((item) => item.id).sort()).toEqual(["first", "second"]);
    expect(store.summaries(project.id).map((item) => item.id)).toEqual(["first"]);
    expect(session).not.toHaveBeenCalled();
    for (const [sql] of [...prepare.mock.calls]) {
      const plan = store.db.prepare(`EXPLAIN QUERY PLAN ${sql}`).all(
        ...(sql.includes("project_id=?") ? [project.id] : []),
      );
      expect(plan.some((step) => step.detail.includes("COVERING INDEX"))).toBe(true);
    }
  } finally {
    prepare.mockRestore();
    session.mockRestore();
  }
});
it("keeps sync revision checks atomic through saves, rollback and deletion", () => {
  const { store, project, directory } = fixture();
  const value = storedSession(
    project.id, directory, "large", "x".repeat(2_200_000),
  );
  store.save(value, { type: "test" });
  const updated = {
    ...value,
    revision: 2,
    session: { ...value.session, title: "New title" },
  };
  expect(() =>
    store.transaction(() => {
      store.save(updated, { type: "test" });
      expect(store.sync(value.session.id, 2)).toEqual({
        kind: "unchanged", revision: 2,
      });
      throw new Error("rollback");
    }),
  ).toThrow("rollback");
  expect(store.sync(value.session.id, 1)).toEqual({
    kind: "unchanged", revision: 1,
  });
  expect(store.sync(value.session.id, 2)).toMatchObject({
    kind: "snapshot", value: { revision: 1 },
  });
  store.save(updated, { type: "test" });
  expect(store.sync(value.session.id, 1)).toMatchObject({
    kind: "delta",
    base: 1,
    value: { revision: 2, session: { title: "New title" } },
    blocks: [],
  });
  expect(store.sync(value.session.id, 2)).toEqual({
    kind: "unchanged", revision: 2,
  });
  store.deleteSession(value.session.id);
  expect(() => store.sync(value.session.id, 2)).toThrow("Session not found");
});
it.each([null, "broken json", JSON.stringify({ id: "other", revision: 1 })])(
  "falls back to the snapshot when summary metadata is unavailable: %s",
  (metadata) => {
    const { store, project, directory } = fixture();
    const value = storedSession(project.id, directory, "legacy", "history");
    store.save(value, { type: "test" });
    store.clearCache();
    store.db
      .prepare("UPDATE sessions SET summary=? WHERE id=?")
      .run(metadata, value.session.id);
    expect(store.sync(value.session.id, 1)).toEqual({
      kind: "unchanged", revision: 1,
    });
    expect(store.sync(value.session.id)).toMatchObject({
      kind: "snapshot", value: { revision: 1 },
    });
  },
);

it("writes and serializes only a changed tail while preserving large hidden payloads across reopen", () => {
  const { store, project, directory } = fixture();
  const original = storedSession(project.id, directory, "incremental", "prefix");
  original.session.blocks[0].tool = { rawOutput: "raw".repeat(800_000), nativeInput: { secret: "retained" }, preview: { detail: "full" } };
  original.session.blocks.push({ id: "tail", role: "assistant", text: "first", streaming: true });
  const saved = store.save(original, { type: "initial" });
  expect(store.cache.has("incremental")).toBe(false);
  store.db.exec(`CREATE TEMP TABLE changed_blocks (id TEXT);
    CREATE TEMP TRIGGER track_block_update AFTER UPDATE ON session_blocks BEGIN
      INSERT INTO changed_blocks VALUES (NEW.id);
    END;`);
  const prepare = vi.spyOn(store.db, "prepare");
  const stringify = vi.spyOn(JSON, "stringify");
  const updated = store.save({
    ...saved, revision: 2,
    session: { ...saved.session, blocks: [saved.session.blocks[0], { ...saved.session.blocks[1], text: "second" }] },
  }, { type: "tail" });
  expect(stringify.mock.calls.some(([value]) => value === saved.session.blocks[0])).toBe(false);
  expect(stringify.mock.results.filter((result) => typeof result.value === "string").every((result) => result.value.length < 4096)).toBe(true);
  expect(prepare.mock.calls.every(([sql]) => !sql.includes("SELECT") || !sql.includes("payload"))).toBe(true);
  expect(prepare.mock.calls.filter(([sql]) => sql.includes("INSERT INTO session_blocks"))).toHaveLength(1);
  stringify.mockRestore();
  prepare.mockRestore();
  expect(store.db.prepare("SELECT id FROM changed_blocks").all().map((row) => row.id)).toEqual(["tail"]);
  const reopened = new HostStore(join(directory, "host.db"));
  cleanups.push(() => reopened.close());
  expect(reopened.session("incremental")).toEqual(updated);
  expect(reopened.sync("incremental", 1)).toMatchObject({ kind: "delta", blocks: [{ id: "tail", text: "second" }] });
  expect(reopened.session("incremental").session.blocks[0].tool).toEqual(original.session.blocks[0].tool);
});

it("isolates caller-owned nested mutation and permits immutable reducer tool updates", async () => {
  const { applyHarnessEvent } = await import("./runtime/protocol/apply.js");
  const { store, project, directory } = fixture();
  const value = storedSession(project.id, directory, "mutations", "text");
  value.session = applyHarnessEvent(value.session, { type: "tool.started", callId: "tool", title: "Read", kind: "read", status: "running", preview: { type: "text", text: "before" } });
  const saved = store.save(value, { type: "start" });
  const tool = value.session.blocks.find((block) => block.tool?.callId === "tool");
  tool.tool.preview.text = "caller changed";
  expect(saved.session.blocks.find((block) => block.tool?.callId === "tool").tool.preview.text).toBe("before");
  const mutated = store.save({ ...value, revision: 2 }, { type: "mutated" });
  expect(mutated.session.blocks.find((block) => block.tool?.callId === "tool").tool.preview.text).toBe("caller changed");
  const session = applyHarnessEvent(mutated.session, { type: "tool.updated", callId: "tool", title: "Read", kind: "read", status: "completed", detail: "done", preview: { type: "text", text: "completed" } });
  const updated = store.save({ ...mutated, session, revision: 3 }, { type: "finish" });
  expect(updated.session.blocks.find((block) => block.tool?.callId === "tool").tool.status).toBe("completed");
  expect(mutated.session.blocks.find((block) => block.tool?.callId === "tool").tool.status).toBe("running");
});

it("commits block order, deletions, receipts and summaries atomically, retaining the previous revision after rollback", () => {
  const { store, project, directory } = fixture();
  const value = storedSession(project.id, directory, "atomic", "keep");
  value.session.blocks.push({ id: "removed", role: "assistant", text: "remove" });
  const saved = store.save(value, { type: "start" });
  const next = { ...saved, revision: 2, session: { ...saved.session, title: "Changed", blocks: [{ id: "new", role: "assistant", text: "new" }, saved.session.blocks[0]] } };
  expect(() => store.transaction(() => {
    store.save(next, { type: "changed" });
    store.recordReceipt("signature", { commandId: "atomic-receipt" });
    throw new Error("interrupt");
  })).toThrow("interrupt");
  const reopened = new HostStore(join(directory, "host.db"));
  cleanups.push(() => reopened.close());
  expect(reopened.session("atomic")).toEqual(saved);
  expect(reopened.receipt("atomic-receipt", "signature")).toBeUndefined();
  expect(reopened.summaries(project.id)[0].revision).toBe(1);
  store.transaction(() => {
    store.save(next, { type: "changed" });
    store.recordReceipt("signature", { commandId: "atomic-receipt" });
  });
  reopened.clearCache();
  expect(reopened.session("atomic").session.blocks.map((block) => block.id)).toEqual(["new", "atomic-block"]);
  expect(reopened.receipt("atomic-receipt", "signature")).toEqual({ commandId: "atomic-receipt" });
  expect(reopened.summaries(project.id)[0].revision).toBe(2);
  store.deleteSession("atomic");
  expect(store.db.prepare("SELECT COUNT(*) AS count FROM session_blocks WHERE session_id=?").get("atomic").count).toBe(0);
});

it("converts legacy snapshots without changing unchanged block revision stamps", () => {
  const { store, project, directory } = fixture();
  const value = storedSession(project.id, directory, "legacy-format", "all data", 8);
  value.blockRevisions = { "legacy-format-block": 2 };
  value.session.blocks[0].tool = { rawOutput: "hidden", preview: { full: "preserved" } };
  store.db.prepare("INSERT INTO sessions (id, project_id, snapshot) VALUES (?, ?, ?)").run(value.session.id, project.id, JSON.stringify(value));
  const read = store.session(value.session.id);
  store.save({ ...read, revision: 9, session: { ...read.session, title: "new" } }, { type: "metadata" });
  store.clearCache();
  expect(store.session(value.session.id).blockRevisions).toEqual(value.blockRevisions);
  expect(store.session(value.session.id).session.blocks).toEqual(value.session.blocks);
  expect(JSON.parse(store.db.prepare("SELECT snapshot FROM sessions WHERE id=?").get(value.session.id).snapshot).session.blocks).toBeUndefined();
});

it("reconstructs one revision when another SQLite connection commits between metadata and block reads", () => {
  const { store, project, directory } = fixture();
  const saved = store.save(storedSession(project.id, directory, "read-snapshot", "before"), { type: "before" });
  store.clearCache();
  const writer = new HostStore(join(directory, "host.db"));
  cleanups.push(() => writer.close());
  const prepare = store.db.prepare.bind(store.db);
  let committed = false;
  const spy = vi.spyOn(store.db, "prepare").mockImplementation((sql) => {
    if (sql.includes("SELECT id, revision, digest, code_units, payload") && !committed) {
      committed = true;
      writer.save({ ...saved, revision: 2, session: { ...saved.session, title: "after", blocks: [{ ...saved.session.blocks[0], text: "after" }] } }, { type: "after" });
    }
    return prepare(sql);
  });
  try {
    expect(store.session(saved.session.id)).toEqual(saved);
    expect(committed).toBe(true);
  } finally {
    spy.mockRestore();
  }
  store.clearCache();
  expect(store.session(saved.session.id)).toMatchObject({ revision: 2, session: { title: "after", blocks: [{ text: "after" }] } });
});
