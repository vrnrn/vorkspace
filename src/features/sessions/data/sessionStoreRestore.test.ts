import { sanitizeSessionForPersist } from "./sessionStore";
import { canAutoContinue, markTurnInterrupted } from "../model/inFlight";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionRecord } from "./sessionStore";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: async (...args: unknown[]) => {
    const value = await invoke(...args);
    return args[0] === "session_upsert_delta" ? [value, 1] : value;
  },
}));

const { getSession, readSession, repairSession, applySessionRepair } =
  await import("./sessionStore");

/** A saved Codex session holding the one row this PR repairs. */
function codexRecord(): SessionRecord {
  return {
    id: "s1",
    harness: "codex",
    model: "gpt-5",
    cwd: "/repo",
    providerSessionId: "01a0e6f4-13e3-7692-9250-4befceed807b",
    blocks: [
      { id: "b0", role: "user", text: "list the agents" },
      {
        id: "b1",
        role: "tool",
        text: "Shell",
        tool: {
          callId: "exec-1",
          title: "Shell",
          kind: "execute",
          status: "completed",
          preview: {
            kind: "shell",
            title: "rg --files -g AGENTS.md -g '!node_modules'",
          },
        },
      },
    ],
  } as unknown as SessionRecord;
}

describe("restoring a session whose repair cannot be persisted", () => {
  beforeEach(() => {
    invoke.mockReset();
  });

  it("restores sidebar visibility when a hidden session is opened by id", async () => {
    const record = codexRecord();
    record.sidebarHidden = true;
    invoke.mockImplementation((cmd: string) =>
      Promise.resolve(cmd === "session_get" ? record : null),
    );
    const session = await getSession(record.id);
    expect(session?.sidebarHidden).toBe(true);
    expect(session?.blocks[0].text).toBe("list the agents");
    const repaired = invoke.mock.calls.find(
      ([cmd]) => cmd === "session_upsert_delta",
    );
    expect(repaired?.[1].session.sidebarHidden).toBe(true);
  });

  it("restores the turn's launched session links", async () => {
    const record = codexRecord();
    const launch = {
      sessionId: "app-review",
      cwd: "/repo",
      title: "Review",
      harness: "codex" as const,
      model: "gpt-6",
    };
    record.blocks[0].botSpawnedSessions = [launch];
    invoke.mockImplementation((cmd: string) =>
      Promise.resolve(cmd === "session_get" ? record : null),
    );
    expect((await getSession("s1"))?.blocks[0].botSpawnedSessions).toEqual([
      launch,
    ]);
  });

  it("still returns the repaired session when the write fails", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "session_get") return Promise.resolve(codexRecord());
      if (cmd === "session_upsert_delta")
        return Promise.reject(new Error("db locked"));
      return Promise.resolve(null);
    });

    // A failed persistence must not cost the reader the session: the repair
    // stays in memory and the next load retries the write.
    const session = await getSession("s1");
    expect(session).not.toBeNull();
    expect(session?.blocks[1].text).toBe("Find files");
  });

  it("persists the repair when the write succeeds", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "session_get") return Promise.resolve(codexRecord());
      return Promise.resolve(null);
    });

    const session = await getSession("s1");
    expect(session?.blocks[1].text).toBe("Find files");
    expect(
      invoke.mock.calls.some(([cmd]) => cmd === "session_upsert_delta"),
    ).toBe(true);
  });
});

it("retains local admission ownership through hydrate and later ordinary persistence without a receipt", async () => {
  const record = codexRecord();
  record.blocks = [
    { id: "previous-turn", role: "user", text: "Earlier request" },
    { id: "previous-reply", role: "assistant", text: "Done" },
    {
      id: "pending-turn",
      role: "user",
      text: "New request",
      localSubmissionOwned: true,
    },
  ];
  invoke.mockImplementation((cmd: string) =>
    Promise.resolve(cmd === "session_get" ? record : null),
  );
  const restored = await getSession(record.id);
  expect(restored?.blocks[2].localSubmissionOwned).toBe(true);
  const savedAgain = sanitizeSessionForPersist({
    ...restored!,
    title: "Changed after restore",
  });
  expect(savedAgain.blocks[2].localSubmissionOwned).toBe(true);
  expect(canAutoContinue(markTurnInterrupted(restored!))).toBe(false);
});

describe("progressive session restoration", () => {
  beforeEach(() => {
    invoke.mockReset();
  });

  it("returns saved text without reading provider logs or writing repairs", async () => {
    const record = { ...codexRecord(), harness: "claude" } as SessionRecord;
    invoke.mockResolvedValue(record);
    const session = await readSession("s1");
    expect(session?.blocks[1].text).toBe("Shell");
    expect(invoke.mock.calls.map(([cmd]) => cmd)).toEqual(["session_get"]);
  });

  it("does not mutate or persist a conversation while repairing it for a visible caller", async () => {
    invoke.mockResolvedValueOnce(codexRecord());
    const source = (await readSession("s1"))!;
    const repaired = await repairSession(source, { persist: false });
    expect(source.blocks[1].text).toBe("Shell");
    expect(repaired.blocks[1].text).toBe("Find files");
    expect(invoke.mock.calls.map(([cmd]) => cmd)).toEqual(["session_get"]);
    const renamed = { ...source, title: "Renamed while loading" };
    expect(applySessionRepair(renamed, source, repaired)).toMatchObject({
      title: "Renamed while loading",
      blocks: repaired.blocks,
    });
  });

  it("discards late repairs after streaming, editing, or a provider change", async () => {
    invoke.mockResolvedValueOnce(codexRecord());
    const source = (await readSession("s1"))!;
    const repaired = await repairSession(source, { persist: false });
    for (const current of [
      {
        ...source,
        blocks: [
          ...source.blocks,
          { id: "new", role: "assistant" as const, text: "Live output" },
        ],
      },
      { ...source, blocks: source.blocks.slice(0, 1) },
      { ...source, providerSessionId: "new-provider-session" },
      { ...source, harness: "claude" as const },
    ])
      expect(applySessionRepair(current, source, repaired)).toBe(current);
  });

  it.each([false, true])(
    "checks local send safety before exposing saved text (receipt read fails=%s)",
    async (fails) => {
      vi.stubGlobal("window", { __TAURI_INTERNALS__: {} });
      const record = codexRecord();
      invoke.mockImplementation((command: string) => {
        if (command === "session_get") return Promise.resolve(record);
        if (command === "local_submission_list")
          return fails
            ? Promise.reject(new Error("receipt store unavailable"))
            : Promise.resolve([
                {
                  attemptId: "pending",
                  sessionId: record.id,
                  userTurnId: "b0",
                  signature: "saved-payload",
                  phase: "uncertain",
                  outcome: null,
                  error: null,
                },
              ]);
        throw new Error(`Unexpected command: ${command}`);
      });
      try {
        const source = (await readSession(record.id))!;
        expect(source.blocks[1].text).toBe("Shell");
        expect(source.localSubmissionUncertain).toBe(true);
        expect(canAutoContinue(markTurnInterrupted(source))).toBe(false);
        const repaired = await repairSession(source, { persist: false });
        const published = applySessionRepair(source, source, repaired);
        expect(published.blocks[1].text).toBe("Find files");
        expect(published.localSubmissionUncertain).toBe(true);
        expect(canAutoContinue(markTurnInterrupted(published))).toBe(false);
        expect(invoke.mock.calls.map(([command]) => command)).toEqual([
          "session_get",
          "local_submission_list",
        ]);
      } finally {
        vi.unstubAllGlobals();
      }
    },
  );
});
