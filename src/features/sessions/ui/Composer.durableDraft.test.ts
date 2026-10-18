// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Composer } from "./Composer";
import { flushComposerDraft } from "../model/draftCache";
import { writeComposer } from "../model/composerControl";

const ipc = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", async (original) => ({
  ...(await original<typeof import("@tauri-apps/api/core")>()),
  invoke: ipc.invoke,
}));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ onDragDropEvent: async () => () => {} }),
}));
vi.mock("../../source-control/hooks/useProjectBranches", () => ({
  useProjectBranchesState: () => ({
    branches: { current: "main", detached: false, branches: [] },
    settled: true,
  }),
}));
let root: Root;
let container: HTMLDivElement;
let owner: string;
let count = 0;
let onSubmit: ReturnType<typeof vi.fn>;
const savedText = "A😀中\ne\u0301";
const stored = () => ({
  sessionId: owner,
  revision: 3,
  payload: {
    cwd: "/repo",
    text: savedText,
    selectionStart: 1,
    selectionEnd: 3,
    attachments: [],
    mcpTags: [],
  },
});
function render(initialDraft = "", persistDraft = true) {
  root.render(
    createElement(Composer, {
      sessionId: owner,
      cwd: "/repo",
      executionCwd: "/repo",
      harness: "codex",
      model: "test",
      runtimeMode: "supervised",
      initialDraft,
      persistDraft,
      onSubmit,
      onStop: vi.fn(),
    }),
  );
}
beforeEach(() => {
  onSubmit = vi.fn();
  owner = `durable-ui-${++count}`;
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    configurable: true,
    value: {},
  });
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      disconnect() {}
    },
  );
  ipc.invoke.mockReset();
  ipc.invoke.mockImplementation(async (command, args) => {
    if (command === "composer_draft_get") return stored();
    if (command === "composer_draft_set")
      return { ...args.request, revision: args.request.expectedRevision + 1 };
    if (
      [
        "mcp_discover",
        "claude_mcp_list",
        "list_project_files",
        "project_files",
        "inspect_paths",
      ].includes(command)
    )
      return [];
    return undefined;
  });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(async () => {
  await act(async () => {
    root.unmount();
    await flushComposerDraft(owner).catch(() => undefined);
  });
  container.remove();
  Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
  vi.unstubAllGlobals();
});

it("restores exact UTF-16 selection and preserves it when the parent echoes initialDraft", async () => {
  await act(async () => render());
  const input = container.querySelector("textarea")!;
  expect(input.value).toBe(savedText);
  expect([input.selectionStart, input.selectionEnd]).toEqual([1, 3]);
  await act(async () => render(savedText));
  expect([input.selectionStart, input.selectionEnd]).toEqual([1, 3]);
  expect(
    ipc.invoke.mock.calls.filter(
      ([command]) => command === "composer_draft_set",
    ),
  ).toHaveLength(0);
});

it("keeps temporary composers in memory while the native bridge exists", async () => {
  await act(async () => render("temporary text", false));
  expect(container.querySelector("textarea")?.value).toBe("temporary text");
  expect(
    ipc.invoke.mock.calls.filter(([command]) =>
      command.startsWith("composer_draft_"),
    ),
  ).toHaveLength(0);
});

it("fills a restored draft without sending and persists through the existing draft owner", async () => {
  const file = {
    id: "keep-file",
    name: "keep.txt",
    kind: "file",
    mimeType: "text/plain",
    size: 3,
    path: "/tmp/keep.txt",
  };
  ipc.invoke.mockImplementation(async (command, args) => {
    if (command === "composer_draft_get")
      return {
        ...stored(),
        payload: { ...stored().payload, attachments: [file] },
      };
    if (command === "inspect_paths") return [{ path: file.path }];
    if (command === "composer_draft_set")
      return { ...args.request, revision: args.request.expectedRevision + 1 };
    return [];
  });
  await act(async () => render());
  await act(async () => {
    await writeComposer(owner, { text: "Review this before sending" });
  });
  const field = container.querySelector("textarea")!;
  const expected = savedText + "\n\nReview this before sending";
  expect(field.value).toBe(expected);
  expect(field.selectionStart).toBe(expected.length);
  expect(onSubmit).not.toHaveBeenCalled();
  await act(async () => flushComposerDraft(owner));
  const writes = ipc.invoke.mock.calls.filter(
    ([command]) => command === "composer_draft_set",
  );
  expect(writes.at(-1)?.[1].request.payload).toMatchObject({
    text: expected,
    attachments: [file],
  });
  await act(async () => root.unmount());
  root = createRoot(container);
  await act(async () => render());
  expect(container.querySelector("textarea")?.value).toBe(expected);
  expect(container.textContent).toContain("keep.txt");
});

it("keeps typing during hydration and displays both versions for an explicit recovery choice", async () => {
  let resolve!: (value: ReturnType<typeof stored>) => void;
  const pending = new Promise<ReturnType<typeof stored>>((done) => {
    resolve = done;
  });
  ipc.invoke.mockImplementation(async (command) =>
    command === "composer_draft_get" ? pending : [],
  );
  await act(async () => render());
  const input = container.querySelector("textarea")!;
  await act(async () => {
    input.value = "new typing";
    input.setSelectionRange(10, 10);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await act(async () => resolve(stored()));
  expect(input.value).toBe("new typing");
  expect(container.textContent).toContain("Compare drafts");
  expect(container.textContent).toContain("new typing");
  expect(container.textContent).toContain(savedText);
  expect(container.textContent).toContain("Restore saved draft");
});
