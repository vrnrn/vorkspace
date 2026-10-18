// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { migrateBotStorage } from "./botStorageMigration";

let stored: Map<string, string>;
beforeEach(() => {
  vi.resetModules();
  stored = new Map();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => stored.get(key) ?? null,
    setItem: (key: string, value: string) => stored.set(key, value),
    removeItem: (key: string) => stored.delete(key),
    key: (index: number) => [...stored.keys()][index] ?? null,
    get length() {
      return stored.size;
    },
  });
});
afterEach(() => vi.unstubAllGlobals());

function put(key: string, value: unknown) {
  stored.set(key, JSON.stringify(value));
}

it("opens the previous roster and channels with the same identities and settings", async () => {
  const roster = [
    {
      id: "resident",
      sessionId: "saved-chat",
      mascot: "crab",
      color: "red",
      projects: ["/repo"],
    },
  ];
  put("monocode:mono-roster", roster);
  put("monocode:mono-channels", [
    {
      id: "team",
      name: "Team",
      monoIds: ["resident"],
      memberSessions: { resident: "channel-chat" },
    },
  ]);
  put("monocode:mono-channel-messages:team", [
    {
      id: "msg",
      text: "Hello",
      from: { kind: "mono", monoId: "resident", name: "Helper" },
      reactions: [{ monoId: "resident", name: "Helper", emoji: "👍" }],
    },
  ]);
  put("monocode:mono-rotations", {
    "saved-chat": { at: 1, reason: "context", earlier: ["Saved context"] },
  });
  put("monocode.inboxMonos.v1", { jira: "resident" });
  stored.set("monocode:open-mono", "resident");
  stored.set("monocode.monosEnabled", "0");
  stored.set("monocode.railMonosPinned", "1");
  stored.set("monocode.monoMenuBarIcon", "0");
  stored.set("monocode.settingsSection", "monos");
  const { listBots, loadOpenBot } = await import("./bot");
  expect(listBots()).toEqual(roster);
  expect(loadOpenBot()).toBe("resident");
  const { listBotChannels } = await import("./botChannels");
  expect(listBotChannels()[0]).toMatchObject({
    memberIds: ["resident"],
    memberSessions: { resident: "channel-chat" },
    messages: [
      {
        from: { kind: "bot", botId: "resident" },
        reactions: [{ botId: "resident" }],
      },
    ],
  });
  const { loadBotRotation } = await import("./botRotation");
  expect(loadBotRotation("saved-chat")?.earlier).toEqual(["Saved context"]);
  const { loadInboxBots } = await import("../../inbox/model/inboxBots");
  expect(loadInboxBots()).toEqual({ jira: "resident" });
  const { loadBotsEnabled, loadBotMenuBarIcon } =
    await import("../../settings/model/settings");
  const { loadRailBotsPinned } =
    await import("../../settings/model/displayPrefs");
  expect(loadBotsEnabled()).toBe(false);
  expect(loadBotMenuBarIcon()).toBe(false);
  expect(loadRailBotsPinned()).toBe(true);
  expect(stored.get("monocode.settingsSection")).toBe("bots");
  expect(JSON.parse(stored.get("monocode:mono-roster")!)).toEqual(roster);
});

it("recovers launch-initialized empty collections without replacing populated or scalar choices", () => {
  put("monocode:mono-roster", [{ id: "old" }]);
  put("monocode:bot-roster", []);
  put("monocode:mono-channels", [{ id: "old" }]);
  put("monocode:bot-channels", [{ id: "new" }]);
  put("monocode:mono-handed", { left: "old" });
  put("monocode:bot-handed", {});
  stored.set("monocode.monosEnabled", "0");
  stored.set("monocode.botsEnabled", "1");
  migrateBotStorage();
  expect(JSON.parse(stored.get("monocode:bot-roster")!)).toEqual([
    { id: "old" },
  ]);
  expect(JSON.parse(stored.get("monocode:bot-channels")!)).toEqual([
    { id: "new" },
  ]);
  expect(JSON.parse(stored.get("monocode:bot-handed")!)).toEqual({
    left: "old",
  });
  expect(stored.get("monocode.botsEnabled")).toBe("1");
  put("monocode:bot-roster", []);
  migrateBotStorage();
  expect(stored.get("monocode:bot-roster")).toBe("[]");
});

it("migrates backgrounds and shortcuts while preserving newer overrides", async () => {
  put("monocode:project-chat-backgrounds", {
    "mono:resident": { path: "/old.png", scope: "all" },
    "mono:other": { path: "/other.png" },
    "bot:other": { path: "/new.png" },
    "/repo": { path: "/repo.png" },
  });
  put("monocode.keybindingOverrides", {
    "App: Toggle Mono": { disabled: true },
    "App: Search": { shortcut: "Command+KeyY" },
  });
  const { botChatBackground } = await import("./botBackground");
  expect(botChatBackground("resident")?.path).toBe("/old.png");
  expect(botChatBackground("other")?.path).toBe("/new.png");
  const { loadKeybindingOverrides } =
    await import("../../settings/model/settings");
  expect(loadKeybindingOverrides()).toEqual({
    "App: Toggle Bot": { disabled: true },
    "App: Search": { shortcut: "Command+KeyY" },
  });
  expect(
    JSON.parse(stored.get("monocode:project-chat-backgrounds")!)["/repo"],
  ).toEqual({ path: "/repo.png" });
});

it("preserves a newly rebound shortcut when both command names exist", async () => {
  put("monocode.keybindingOverrides", {
    "App: Toggle Mono": { disabled: true },
    "App: Toggle Bot": { shortcut: "Command+Shift+KeyM" },
  });
  const { loadKeybindingOverrides } =
    await import("../../settings/model/settings");
  expect(loadKeybindingOverrides()).toEqual({
    "App: Toggle Bot": { shortcut: "Command+Shift+KeyM" },
  });
});

it("retries failed writes and publishes completion only after all collections", () => {
  put("monocode:mono-roster", [{ id: "old" }]);
  put("monocode:mono-channels", [{ id: "team", monoIds: ["old"] }]);
  const write = vi.spyOn(localStorage, "setItem");
  write.mockImplementationOnce(() => {
    throw new Error("quota");
  });
  migrateBotStorage();
  expect(stored.has("vorkspace.botStorageMigration.v1")).toBe(false);
  migrateBotStorage();
  expect(JSON.parse(stored.get("monocode:bot-roster")!)).toEqual([
    { id: "old" },
  ]);
  expect(JSON.parse(stored.get("monocode:bot-channels")!)[0].botIds).toEqual([
    "old",
  ]);
  expect(write.mock.calls.at(-1)?.[0]).toBe("vorkspace.botStorageMigration.v1");
});
