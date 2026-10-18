// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { BotWorkStatus } from "../model/botWorkStatus";
import { BotWorkTicker } from "./BotWorkTicker";

let root: Root;
let container: HTMLDivElement;
const running: BotWorkStatus = {
  key: "command",
  label: "Running command…",
  kind: "run",
  active: true,
};
const thinking: BotWorkStatus = {
  key: "note",
  label: "Thinking…",
  kind: "think",
  active: true,
};
const editing: BotWorkStatus = {
  key: "edit",
  label: "Editing file…",
  kind: "edit",
  active: true,
};

beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("matchMedia", () => ({ matches: false }));
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

function render(status: BotWorkStatus) {
  act(() => root.render(createElement(BotWorkTicker, { status })));
}

function advance(ms: number) {
  act(() => vi.advanceTimersByTime(ms));
}

it("pushes the old row upward with the new one below, then removes the old row", () => {
  render(running);
  render(thinking);
  expect(container.querySelector(".sr-only")?.textContent).toBe(running.label);
  advance(850);
  expect(container.querySelector('[data-moving="true"]')).not.toBeNull();
  expect(
    Array.from(container.querySelectorAll(".bot-work-ticker-row")).map(
      (row) => row.textContent,
    ),
  ).toEqual([running.label, thinking.label]);
  expect(container.querySelector(".sr-only")?.textContent).toBe(thinking.label);
  advance(340);
  expect(container.querySelectorAll(".bot-work-ticker-row")).toHaveLength(1);
  expect(container.querySelector('[data-moving="true"]')).toBeNull();
});

it("coalesces a burst into the latest activity without building an animation backlog", () => {
  render(running);
  render(thinking);
  advance(100);
  render(editing);
  advance(750);
  expect(container.querySelector(".sr-only")?.textContent).toBe(editing.label);
  expect(container.textContent).not.toContain(thinking.label);
  advance(340);
  expect(container.querySelectorAll(".bot-work-ticker-row")).toHaveLength(1);
});

it("ticks for another command with the same label, while streamed chunks remain steady", () => {
  render(running);
  advance(850);
  render({ ...running });
  expect(container.querySelector('[data-moving="true"]')).toBeNull();
  render({ ...running, key: "next-command" });
  expect(container.querySelectorAll(".bot-work-ticker-row")).toHaveLength(2);
  advance(340);
  expect(container.querySelectorAll(".bot-work-ticker-row")).toHaveLength(1);
});

it("changes immediately without a rolling animation when reduced motion is enabled", () => {
  vi.stubGlobal("matchMedia", () => ({ matches: true }));
  render(running);
  render(thinking);
  expect(container.querySelector(".sr-only")?.textContent).toBe(thinking.label);
  expect(container.querySelector('[data-moving="true"]')).toBeNull();
  expect(container.querySelectorAll(".bot-work-ticker-row")).toHaveLength(1);
});
