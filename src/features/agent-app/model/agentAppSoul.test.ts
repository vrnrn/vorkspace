// @vitest-environment happy-dom
import { expect, it, vi } from "vitest";
import { newSession } from "../../sessions/model/session";
import {
  BotFileConflict,
  type AgentFilePath,
} from "../../bots/model/botFiles";
import { handleAgentApp, type AgentAppHost } from "./agentApp";

function fixture(kind: "bot" | "habit" | "session" = "bot") {
  const source = newSession("codex", "/Users/me", "codex:test");
  source.id = "chat";
  let soul = "# Soul\n\n- Be brief.\n- Check the code before answering.\n";
  let version = 0;
  const hash = () => `soul@${version}`;
  const agentFiles = vi.fn(async () => ({
    id: "bot-1",
    dir: "/data/bots/bot-1",
    soul,
    soulHash: hash(),
    memory: "",
    memoryHash: "memory@0",
    memoryPath: "/data/bots/bot-1/MEMORY.md",
    topics: [],
  }));
  const write = vi.fn(
    async (
      _botId: string,
      _path: AgentFilePath,
      text: string,
      expected: string,
    ) => {
      if (expected !== hash()) throw new BotFileConflict();
      soul = text;
      version++;
      return hash();
    },
  );
  const host = {
    isBot: (id: string) => kind === "bot" && id === source.id,
    botOf: () =>
      kind === "session" ? undefined : { id: "bot-1", projects: [] },
    agentFiles,
    writeAgentFile: write,
  } as unknown as AgentAppHost;
  const run = (action: string, input: Record<string, unknown> = {}) =>
    handleAgentApp(source, "req", action, input, host);
  return {
    run,
    write,
    agentFiles,
    soul: () => soul,
    userEdit: (text: string) => {
      soul = text;
      version++;
    },
  };
}

it("reads and updates its own soul without needing an assigned project", async () => {
  const { run, write, agentFiles, soul } = fixture();
  const current = (await run("soul.read")) as { text: string; hash: string };
  expect(current).toMatchObject({
    file: "SOUL.md",
    text: soul(),
    hash: "soul@0",
  });
  expect(agentFiles).toHaveBeenCalledWith("bot-1");
  const text = current.text.replace("Be brief.", "Use British English.");
  expect(
    await run("soul.update", { text, expectedHash: current.hash }),
  ).toEqual({
    file: "SOUL.md",
    updated: true,
    hash: "soul@1",
  });
  expect(write).toHaveBeenCalledWith("bot-1", "SOUL.md", text, current.hash);
  expect(soul()).toBe(text);
  expect(await run("soul.read")).toMatchObject({ text, hash: "soul@1" });
});

it("protects a settings edit and requires rereading before reapplying the request", async () => {
  const { run, write, userEdit, soul } = fixture();
  const current = (await run("soul.read")) as { text: string; hash: string };
  const edited = `${current.text}- Run the tests.\n`;
  userEdit(edited);
  await expect(
    run("soul.update", {
      text: current.text.replace("Be brief.", "Use British English."),
      expectedHash: current.hash,
    }),
  ).rejects.toThrow("Run soul.read and reapply");
  expect(write).toHaveBeenCalledTimes(1);
  expect(soul()).toBe(edited);

  const fresh = (await run("soul.read")) as { text: string; hash: string };
  await run("soul.update", {
    text: fresh.text.replace("Be brief.", "Use British English."),
    expectedHash: fresh.hash,
  });
  expect(soul()).toContain("Use British English.");
  expect(soul()).toContain("Run the tests.");
});

it.each(["habit", "session"] as const)(
  "refuses soul access from a %s",
  async (kind) => {
    const { run, write, agentFiles } = fixture(kind);
    await expect(run("soul.read")).rejects.toThrow(
      "Only a Bot's own conversation",
    );
    await expect(
      run("soul.update", {
        text: "# Soul\n",
        expectedHash: "soul@0",
      }),
    ).rejects.toThrow("Only a Bot's own conversation");
    expect(agentFiles).not.toHaveBeenCalled();
    expect(write).not.toHaveBeenCalled();
  },
);

it.each([
  [{ text: "# Soul" }, "expectedHash"],
  [{ text: "# Soul", expectedHash: "" }, "expectedHash"],
  [{ text: "# Soul", expectedHash: 1 }, "expectedHash"],
  [{ expectedHash: "soul@0" }, "text"],
  [{ text: null, expectedHash: "soul@0" }, "text"],
  [{ text: "x".repeat(240_001), expectedHash: "soul@0" }, "text"],
  [
    { text: "# Soul", expectedHash: "soul@0", botId: "other" },
    "Unknown soul.update fields",
  ],
  [
    { text: "# Soul", expectedHash: "soul@0", path: "MEMORY.md" },
    "Unknown soul.update fields",
  ],
])("rejects invalid soul update input (%#)", async (input, message) => {
  const { run, write } = fixture();
  await expect(run("soul.update", input)).rejects.toThrow(message);
  expect(write).not.toHaveBeenCalled();
});

it("preserves Markdown whitespace and permits clearing the standing instructions", async () => {
  const { run, soul } = fixture();
  const text = "# Soul\n\n- First line  \n  second line\n\n";
  await run("soul.update", { text, expectedHash: "soul@0" });
  expect(soul()).toBe(text);
  await run("soul.update", { text: "", expectedHash: "soul@1" });
  expect(soul()).toBe("");
});

it("passes through storage failures", async () => {
  const { run, write } = fixture();
  write.mockRejectedValueOnce(new Error("Storage unavailable"));
  await expect(
    run("soul.update", {
      text: "# Soul\n",
      expectedHash: "soul@0",
    }),
  ).rejects.toThrow("Storage unavailable");
});
