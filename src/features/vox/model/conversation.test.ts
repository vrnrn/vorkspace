import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  loadVoxConversation,
  resetSavedVoxConversation,
  saveVoxConversation,
  VOX_CONVERSATION_KEY,
} from "./conversation";
let data: Map<string, string>;
beforeEach(() => {
  data = new Map();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => data.set(key, value),
  });
});
afterEach(() => vi.unstubAllGlobals());
it("retains one native conversation identity across reloads", () => {
  const first = loadVoxConversation();
  saveVoxConversation(first, "native-thread");
  expect(loadVoxConversation()).toEqual({
    ...first,
    threadId: "native-thread",
  });
});
it("reset retires a conversation and prevents late publication from restoring it", () => {
  const old = loadVoxConversation();
  saveVoxConversation(old, "old-thread");
  const saved = loadVoxConversation();
  data.set("unrelated-memory", "keep this");
  resetSavedVoxConversation();
  const fresh = loadVoxConversation();
  expect(fresh.threadId).toBeUndefined();
  expect(fresh.revision).not.toBe(old.revision);
  expect(() => saveVoxConversation(saved, "old-thread")).toThrow(
    "conversation changed",
  );
  expect(data.get("unrelated-memory")).toBe("keep this");
});
it("does not silently replace an unreadable saved identity or ignore a failed save", () => {
  data.set(VOX_CONVERSATION_KEY, "broken json");
  expect(() => loadVoxConversation()).toThrow("could not be read");
  expect(data.get(VOX_CONVERSATION_KEY)).toBe("broken json");
  data.clear();
  const empty = loadVoxConversation();
  vi.stubGlobal("localStorage", {
    getItem: () => null,
    setItem: () => {
      throw new Error("full");
    },
  });
  expect(() => saveVoxConversation(empty, "native-thread")).toThrow(
    "could not be saved",
  );
});
