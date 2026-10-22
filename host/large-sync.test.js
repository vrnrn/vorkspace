import { afterEach, expect, it, vi } from "vitest";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { HostEngine } from "./engine.js";
import { HostStore } from "./store.js";
import { createHostServer } from "./server.js";
import {
  loadRemoteSession,
  setSyncTransport,
} from "./test-support/sync-client.js";
// The desktop's native client (crates/vorkspace-core/src/remote.rs) rejects responses over
// 16 MiB. This routes the real renderer sync code through the real host server
// with the same cap and the same strict JSON rules.
const DESKTOP_LIMIT = 16 * 1024 * 1024;
const LONE_SURROGATE =
  /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/;
const desktop = vi.hoisted(() => ({
  url: "",
  token: "",
  environmentId: "",
  methods: [],
  largest: 0,
}));
setSyncTransport(async (_machine, method, params) => {
  const input = { method, params };
  desktop.methods.push(input.method);
  const response = await fetch(desktop.url, {
    method: "POST",
    headers: { Authorization: `Bearer ${desktop.token}` },
    body: JSON.stringify({
      version: 1,
      environmentId: desktop.environmentId,
      method: input.method,
      params: input.params,
    }),
  });
  const bytes = Buffer.from(await response.arrayBuffer());
  desktop.largest = Math.max(desktop.largest, bytes.length);
  if (bytes.length > DESKTOP_LIMIT) throw "Host response is too large";
  const text = bytes.toString("utf8");
  const value = JSON.parse(text);
  if (value.error) throw `Host rejected request: ${value.error}`;
  const data = value.result?.data;
  if (typeof data === "string" && LONE_SURROGATE.test(data))
    throw "Invalid host response";
  return value.result;
});
// Each test moves tens of megabytes through a real server.
const LARGE = 60_000;
const cleanups = [];
afterEach(async () => {
  for (const cleanup of cleanups.splice(0)) await cleanup();
});
async function setup() {
  const directory = mkdtempSync(join(tmpdir(), "vorkspace-large-sync-"));
  const store = new HostStore(join(directory, "host.db"));
  let turn;
  let finish = () => {};
  const engine = new HostEngine(store, {
    codex: {
      send: async (input) => {
        turn = input;
        await new Promise((resolve) => {
          finish = resolve;
        });
      },
      stop: async () => finish(),
      cancel: async () => finish(),
      bind: () => {},
      approve: () => {},
      answer: () => {},
    },
  });
  const project = await engine.openProject(directory);
  const server = createHostServer(engine, ["codex"]);
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  desktop.url = `http://127.0.0.1:${server.address().port}/rpc`;
  desktop.token = store.issueDevice("Laptop").token;
  desktop.environmentId = store.environmentId;
  desktop.methods = [];
  desktop.largest = 0;
  cleanups.push(async () => {
    await engine.close();
    server.closeAllConnections();
    await new Promise((resolve) => server.close(() => resolve()));
    store.close();
    rmSync(directory, { recursive: true, force: true });
  });
  const { sessionId } = engine.command({
    type: "create",
    commandId: "create",
    projectId: project.id,
    harness: "codex",
    model: "codex:test",
    runtimeMode: "supervised",
  });
  return {
    store,
    engine,
    sessionId,
    turn: () => turn,
    finish: () => finish(),
  };
}
const visible = (value) => {
  const { blockRevisions: _, ...rest } = value;
  return rest;
};
// Quotes, backslashes, newlines and control characters expand when a piece is
// embedded as a JSON string; emoji must never be split between pieces.
const text = (size, seed) =>
  `${seed} "quoted" \\path\\ \u0001 naïve 🚀\n`
    .repeat(Math.ceil(size / 40))
    .slice(0, size);
it(
  "reopens and updates a session whose transcript exceeds 16 MiB",
  async () => {
    const s = await setup();
    const created = s.store.session(s.sessionId);
    s.store.transaction(() =>
      s.store.save(
        {
          ...created,
          revision: created.revision + 1,
          session: {
            ...created.session,
            blocks: [
              ...Array.from({ length: 24 }, (_, index) => ({
                id: `history-${index}`,
                role: "assistant",
                text: text(600_000, `turn ${index}`),
              })),
              // One block alone is larger than the desktop's response cap.
              {
                id: "huge",
                role: "tool",
                text: text(17_500_000, "log"),
              },
            ],
          },
        },
        { type: "fixture" },
      ),
    );
    const full = s.store.session(s.sessionId);
    expect(Buffer.byteLength(JSON.stringify(visible(full)))).toBeGreaterThan(
      DESKTOP_LIMIT * 2,
    );
    const reopened = await loadRemoteSession("machine", s.sessionId);
    expect(reopened).toEqual(visible(full));
    expect(
      desktop.methods.filter((m) => m === "sessions.syncChunk").length,
    ).toBeGreaterThan(4);
    expect(desktop.largest).toBeLessThanOrEqual(DESKTOP_LIMIT);
    // A follow-up turn streams into the large session; only new blocks move.
    desktop.methods = [];
    desktop.largest = 0;
    s.engine.command({
      type: "send",
      commandId: "follow-up",
      sessionId: s.sessionId,
      text: "Summarize the log",
    });
    await vi.waitFor(() => expect(s.turn()).toBeTruthy());
    s.turn().onEvent({ type: "message.delta", text: "Summary" });
    s.finish();
    await vi.waitFor(() =>
      expect(s.store.session(s.sessionId).status).toBe("idle"),
    );
    const updated = await loadRemoteSession("machine", s.sessionId, reopened);
    expect(updated).toEqual(visible(s.store.session(s.sessionId)));
    expect(updated.session.blocks.at(-1)?.text).toBe("Summary");
    expect(desktop.methods).toEqual(["sessions.sync"]);
    expect(desktop.largest).toBeLessThan(64 * 1024);
  },
  LARGE,
);
it(
  "streams a changed block larger than 16 MiB as a bounded delta",
  async () => {
    const s = await setup();
    const base = await loadRemoteSession("machine", s.sessionId);
    const current = s.store.session(s.sessionId);
    s.store.transaction(() =>
      s.store.save(
        {
          ...current,
          revision: current.revision + 1,
          session: {
            ...current.session,
            blocks: [
              { id: "huge", role: "tool", text: text(17_000_000, "out") },
            ],
          },
        },
        { type: "fixture" },
      ),
    );
    desktop.methods = [];
    const next = await loadRemoteSession("machine", s.sessionId, base);
    expect(next).toEqual(visible(s.store.session(s.sessionId)));
    expect(desktop.methods[0]).toBe("sessions.sync");
    expect(desktop.methods).toContain("sessions.syncChunk");
    expect(desktop.largest).toBeLessThanOrEqual(DESKTOP_LIMIT);
  },
  LARGE,
);
