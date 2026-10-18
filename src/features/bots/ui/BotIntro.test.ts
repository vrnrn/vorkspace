// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { BotIntroCard } from "./BotIntro";

let root: Root;
let container: HTMLElement;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

function button(label: string) {
  return [...document.body.querySelectorAll("button")].find(
    (entry) => entry.textContent === label,
  )!;
}

it("creates the first bot or hides bots", () => {
  const onCreate = vi.fn();
  const onHide = vi.fn();
  act(() =>
    root.render(
      createElement(BotIntroCard, {
        onCreate,
        onHide,
      }),
    ),
  );

  expect(document.body.textContent).toContain("Meet Bots");

  act(() => button("Create bot").click());
  expect(onCreate).toHaveBeenCalledOnce();
  act(() =>
    document.body.querySelector<HTMLElement>('[role="switch"]')!.click(),
  );
  expect(onHide).toHaveBeenCalledOnce();
});
