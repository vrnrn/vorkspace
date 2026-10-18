// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { createMono, updateMono } from "../../monos/model/mono";
import { registerComposerEditor } from "../../sessions/model/composerControl";
import { newSession } from "../../sessions/model/session";
import { createVoxController } from "./controller";
import type { VoxRequest } from "./protocol";

const cleanups: Array<() => void> = [];
beforeEach(() => localStorage.clear());
afterEach(() => cleanups.splice(0).forEach((clean) => clean()));

function fixture() {
  const session = newSession("codex", "/local/project");
  const snapshot = {
    selected: session,
    projects: [
      session.cwd,
      "/local/other",
      session.cwd,
      "remote://remote/project",
      "~",
    ],
    cwd: session.cwd,
  };
  const host = {
    snapshot: vi.fn(() => snapshot),
    getSession: vi.fn(async (_id: string) => session),
    openMono: vi.fn(async (_id: string) => {}),
    openSession: vi.fn(async (_id: string) => {}),
    dispatch: vi.fn(async (_request: VoxRequest, _context: unknown) => ({
      accepted: true,
    })),
  };
  const abort = new AbortController();
  const run = createVoxController(host);
  const request = (action: string, input: Record<string, unknown> = {}) =>
    run({
      sessionId: "vox-controller",
      requestId: crypto.randomUUID(),
      action,
      input,
      isCurrent: () => !abort.signal.aborted,
      signal: abort.signal,
    });
  return { session, snapshot, host, abort, request };
}

function editor(sessionId: string, value: string) {
  const field = document.createElement("textarea");
  field.value = value;
  document.body.append(field);
  const unregister = registerComposerEditor(sessionId, {
    field,
    write: (text) => {
      field.value = text;
    },
  });
  cleanups.push(() => {
    unregister();
    field.remove();
  });
  return field;
}

it("discovers current metadata without reading history, opening chats, or dispatching work", async () => {
  const f = fixture();
  const result = await f.request("workspace.current");
  expect(result).toMatchObject({
    session: { id: f.session.id },
    projects: ["/local/project", "/local/other"],
  });
  expect(JSON.stringify(result)).not.toContain("blocks");
  expect(f.host.getSession).not.toHaveBeenCalled();
  expect(f.host.openSession).not.toHaveBeenCalled();
  expect(f.host.dispatch).not.toHaveBeenCalled();
});

it("opens a named Mono and fills its actual composer without dispatching a send", async () => {
  const f = fixture();
  const mono = createMono();
  updateMono(mono.id, (entry) => ({
    ...entry,
    name: "Build Helper",
    sessionId: "mono-draft-test",
  }));
  const field = editor("mono-draft-test", "Keep my draft");
  const result = await f.request("composer.write", {
    mono: "@build-helper",
    text: "Review this change",
  });
  expect(f.host.openMono).toHaveBeenCalledExactlyOnceWith(mono.id);
  expect(result).toMatchObject({
    sessionId: "mono-draft-test",
    updated: true,
    sent: false,
  });
  expect(field.value).toBe("Keep my draft\n\nReview this change");
  expect(f.host.dispatch).not.toHaveBeenCalled();
});

it("pins the intended session across a delayed lookup and a selection change", async () => {
  const f = fixture();
  let finish!: () => void;
  f.host.getSession.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = () => resolve(f.session);
      }),
  );
  const field = editor(f.session.id, "Original target");
  const pending = f.request("composer.write", { text: "Append here" });
  f.snapshot.selected = newSession("codex", "/local/other");
  finish();
  await pending;
  expect(f.host.openSession).toHaveBeenCalledExactlyOnceWith(f.session.id);
  expect(field.value).toBe("Original target\n\nAppend here");
});

it("does not navigate or edit after a stopped lookup finishes", async () => {
  const f = fixture();
  let finish!: () => void;
  f.host.getSession.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = () => resolve(f.session);
      }),
  );
  const field = editor(f.session.id, "Untouched");
  const pending = f.request("composer.write", { text: "Never insert" });
  f.abort.abort();
  finish();
  await expect(pending).rejects.toThrow("Vox stopped");
  expect(f.host.openSession).not.toHaveBeenCalled();
  expect(field.value).toBe("Untouched");
  await expect(
    f.request("sessions.send", { prompt: "Never send" }),
  ).rejects.toThrow("Vox stopped");
  expect(f.host.dispatch).not.toHaveBeenCalled();
});

it("rejects unavailable, remote, worker, ambiguous, and malformed targets before navigation", async () => {
  const f = fixture();
  f.host.getSession.mockResolvedValueOnce({ ...f.session, cwd: "/unknown" });
  await expect(
    f.request("workspace.open", { sessionId: f.session.id }),
  ).rejects.toThrow("unavailable");
  f.host.getSession.mockResolvedValueOnce({
    ...f.session,
    cwd: "remote://remote/project",
  });
  await expect(
    f.request("workspace.open", { sessionId: f.session.id }),
  ).rejects.toThrow("unavailable");
  f.host.getSession.mockResolvedValueOnce({
    ...f.session,
    orchestrationLeadId: "lead",
  });
  await expect(
    f.request("workspace.open", { sessionId: f.session.id }),
  ).rejects.toThrow("unavailable");
  const a = createMono();
  const b = createMono();
  updateMono(a.id, (entry) => ({ ...entry, name: "Helper" }));
  updateMono(b.id, (entry) => ({ ...entry, name: "Helper" }));
  await expect(f.request("composer.read", { mono: "Helper" })).rejects.toThrow(
    "Several Monos match",
  );
  await expect(f.request("composer.read", { mono: "Missing" })).rejects.toThrow(
    "No Mono matches",
  );
  await expect(
    f.request("composer.write", {
      mono: "Helper",
      sessionId: "other",
      text: "No",
    }),
  ).rejects.toThrow("not both");
  await expect(f.request("composer.read", { surprise: true })).rejects.toThrow(
    "Unsupported composer field",
  );
  await expect(f.request("sessions.delete")).rejects.toThrow(
    "Unsupported Vox action",
  );
  expect(f.host.openMono).not.toHaveBeenCalled();
  expect(f.host.openSession).not.toHaveBeenCalled();
  expect(f.host.dispatch).not.toHaveBeenCalled();
});

it("delegates app operations with an explicit ephemeral controller scope", async () => {
  const f = fixture();
  await f.request("monos.send", { to: "Helper", message: "Review this" });
  expect(f.host.dispatch).toHaveBeenCalledWith(
    expect.objectContaining({ action: "monos.send" }),
    expect.objectContaining({
      projects: ["/local/project", "/local/other"],
      source: expect.objectContaining({
        id: "vox-controller",
        title: "Vox",
        ephemeral: true,
        blocks: [],
      }),
    }),
  );
  expect(f.host.openMono).not.toHaveBeenCalled();
});
