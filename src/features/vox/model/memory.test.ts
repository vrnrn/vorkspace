// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { handleVoxMemory, voxMemoryContext } from "./memory";
import type { VoxRequest } from "./protocol";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
let files: Map<string, string>;
const run = (
  action: string,
  input: Record<string, unknown> = {},
  guard = () => {},
) => handleVoxMemory({ action, input } as VoxRequest, guard);
beforeEach(() => {
  files = new Map([
    ["MEMORY.md", ""],
    ["SOUL.md", ""],
  ]);
  vi.mocked(invoke)
    .mockReset()
    .mockImplementation(async (command, args) => {
      const value = args as {
        mono: string;
        path: string;
        text: string;
        expectedHash: string;
      };
      expect(value.mono).toBe("vox");
      const hash = (path: string) => `version:${files.get(path) ?? ""}`;
      if (command === "mono_read")
        return { text: files.get(value.path) ?? null, hash: hash(value.path) };
      if (command === "mono_save") {
        if (value.expectedHash !== hash(value.path)) throw "conflict";
        files.set(value.path, value.text);
        return hash(value.path);
      }
      if (command === "mono_load")
        return {
          id: "vox",
          dir: "/fixture/vox",
          soul: "",
          soulHash: "",
          memory: files.get("MEMORY.md"),
          memoryHash: hash("MEMORY.md"),
          memoryPath: "/fixture/vox/MEMORY.md",
          topics: [],
        };
      throw new Error(`unexpected ${command}`);
    });
});
afterEach(() => vi.restoreAllMocks());
it("uses the Mono memory owner for durable add, correction, recall and deletion", async () => {
  await run("memory.add", { fact: "Uses npm" });
  expect(await voxMemoryContext()).toContain("Uses npm");
  await run("memory.replace", { find: "Uses npm", fact: "Uses pnpm" });
  expect(await run("memory.search", { query: "pnpm" })).toMatchObject({
    hits: [
      expect.objectContaining({ line: expect.stringContaining("Uses pnpm") }),
    ],
  });
  await run("memory.remove", { find: "Uses pnpm" });
  expect(files.get("MEMORY.md")).not.toContain("Uses pnpm");
});
it("cannot redirect memory to another Mono and revalidates cancellation before writing", async () => {
  await expect(
    run("memory.add", { mono: "other", fact: "wrong owner" }),
  ).rejects.toThrow();
  let checks = 0;
  await expect(
    run("memory.add", { fact: "stale request" }, () => {
      if (++checks > 1) throw new Error("stopped");
    }),
  ).rejects.toThrow("stopped");
  expect(files.get("MEMORY.md")).toBe("");
});
it("loads memory using the same bounded budget as Monos", async () => {
  files.set(
    "MEMORY.md",
    Array.from({ length: 500 }, (_, n) => `${n} ${"x".repeat(300)}`).join("\n"),
  );
  const context = await voxMemoryContext();
  expect(context.length).toBeLessThan(25_000);
  expect(context).not.toContain("499 ");
});
