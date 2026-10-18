// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from "vitest";
import {
  loadVoxEnabled,
  saveVoxEnabled,
  subscribeVoxEnabled,
} from "./availability";
afterEach(() => {
  vi.unstubAllGlobals();
  localStorage.clear();
  vi.restoreAllMocks();
});
it("defaults off, persists opt-in and notifies other controls immediately", () => {
  expect(loadVoxEnabled()).toBe(false);
  const changed = vi.fn();
  const stop = subscribeVoxEnabled(changed);
  saveVoxEnabled(true);
  expect(loadVoxEnabled()).toBe(true);
  expect(changed).toHaveBeenCalledOnce();
  saveVoxEnabled(false);
  expect(loadVoxEnabled()).toBe(false);
  window.dispatchEvent(
    new StorageEvent("storage", { key: "vorkspace.vox.enabled" }),
  );
  expect(changed).toHaveBeenCalledTimes(3);
  stop();
});
it("does not claim an opt-in was saved when storage fails", () => {
  vi.stubGlobal("localStorage", {
    getItem: () => null,
    setItem: () => {
      throw new Error("full");
    },
  });
  expect(() => saveVoxEnabled(true)).toThrow("full");
  expect(loadVoxEnabled()).toBe(false);
});
