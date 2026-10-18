import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { loadInboxBots, setInboxBot } from "./inboxBots";

const KEY = "monocode.inboxBots.v1";
let store: Map<string, string>;

beforeEach(() => {
  store = new Map();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => store.get(key) ?? null,
    setItem: (key: string, value: string) => void store.set(key, value),
  });
});

afterEach(() => vi.unstubAllGlobals());

it("starts without a default Bot for either source", () => {
  expect(loadInboxBots()).toEqual({});
});

it("keeps only Jira and GitLab entries with nonempty Bot IDs", () => {
  store.set(
    KEY,
    JSON.stringify({ jira: " bot-jira ", gitlab: 42, github: "bot-other" }),
  );
  expect(loadInboxBots()).toEqual({ jira: "bot-jira" });
});

it.each(["null", "[]", "broken", '{"jira":" "}'])(
  "ignores invalid settings: %s",
  (raw) => {
    store.set(KEY, raw);
    expect(loadInboxBots()).toEqual({});
  },
);

it("saves and clears separate default Bots", () => {
  setInboxBot("gitlab", "bot-reviews");
  setInboxBot("jira", "bot-tickets");
  expect(loadInboxBots()).toEqual({
    gitlab: "bot-reviews",
    jira: "bot-tickets",
  });
  setInboxBot("gitlab", null);
  expect(loadInboxBots()).toEqual({ jira: "bot-tickets" });
});

it("does not interpret the old provider defaults as Bot IDs", () => {
  store.set(
    "monocode.inboxAgents.v1",
    JSON.stringify({ jira: "codex", gitlab: "claude" }),
  );
  expect(loadInboxBots()).toEqual({});
});

it("returns a stable snapshot until saved settings change", () => {
  setInboxBot("jira", "bot-tickets");
  const initial = loadInboxBots();
  expect(loadInboxBots()).toBe(initial);
  setInboxBot("jira", "bot-other");
  expect(loadInboxBots()).not.toBe(initial);
});
