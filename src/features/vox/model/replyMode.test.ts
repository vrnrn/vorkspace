// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from "vitest";
import { loadVoxSilent, saveVoxSilent, subscribeVoxSilent } from "./replyMode";

afterEach(() => {
  vi.unstubAllGlobals();
  localStorage.clear();
});

it("defaults to spoken replies and persists silent mode with same-window and cross-window notifications", () => {
  expect(loadVoxSilent()).toBe(false);
  const changed = vi.fn();
  const stop = subscribeVoxSilent(changed);
  saveVoxSilent(true);
  expect(loadVoxSilent()).toBe(true);
  window.dispatchEvent(new StorageEvent("storage", { key: "unrelated" }));
  expect(changed).toHaveBeenCalledOnce();
  window.dispatchEvent(
    new StorageEvent("storage", { key: "vorkspace.vox.silent" }),
  );
  expect(changed).toHaveBeenCalledTimes(2);
  stop();
  saveVoxSilent(false);
  expect(changed).toHaveBeenCalledTimes(2);
});

it("reports a failed preference write without publishing a new mode", () => {
  vi.stubGlobal("localStorage", {
    getItem: () => null,
    setItem: () => {
      throw new Error("full");
    },
  });
  expect(() => saveVoxSilent(true)).toThrow("full");
  expect(loadVoxSilent()).toBe(false);
});
