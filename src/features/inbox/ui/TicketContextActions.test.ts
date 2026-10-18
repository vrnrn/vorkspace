// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { TicketContextActions } from "./TicketContextActions";
import { copyText } from "../../../platform/tauri/clipboard";
import type { InboxItem } from "../model/githubTasks";
import { loadInboxBots, setInboxBot } from "../model/inboxBots";

vi.mock("../../../platform/tauri/clipboard", () => ({
  copyText: vi.fn(async () => {}),
}));
vi.mock("../../../shared/ui/Popover", async () => {
  const { createElement } = await import("react");
  return {
    Popover: ({
      children,
      onKeyDown,
      role,
    }: {
      children: unknown;
      onKeyDown: () => void;
      role: string;
    }) => createElement("div", { role, onKeyDown }, children as never),
  };
});
let root: Root;
let container: HTMLDivElement;
const ticket = {
  provider: "jira",
  kind: "jira",
  title: "Demo ticket",
  url: "https://demo.atlassian.net/browse/DEMO-42",
  projectPath: "",
} as InboxItem;
const projects = [
  {
    path: "/tmp/demo",
    name: "Demo project",
    logoPath: null,
    mascotName: "crab",
    mascotColor: "blue",
  },
];
const bots = [
  { id: "bot-demo", name: "Demo Bot", mascot: "crab", color: "blue" },
];
const onStart = vi.fn();
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const store = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => store.get(key) ?? null,
    setItem: (key: string, value: string) => void store.set(key, value),
  });
  vi.clearAllMocks();
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});
function render(
  extra: Partial<Parameters<typeof TicketContextActions>[0]> = {},
) {
  act(() =>
    root.render(
      createElement(TicketContextActions, {
        item: ticket,
        cwd: "/tmp/demo",
        projects,
        bots,
        onStart,
        ...extra,
      }),
    ),
  );
}
function button(text: string) {
  return [...document.querySelectorAll<HTMLButtonElement>("button")].find(
    (b) => b.textContent?.trim() === text,
  )!;
}
async function choose(text: string) {
  await act(async () =>
    container
      .querySelector<HTMLButtonElement>(
        '[aria-label="Choose chat destination"]',
      )!
      .click(),
  );
  await act(async () => button(text).click());
}

it.each(["jira", "gitlab"] as const)(
  "preselects the saved %s Bot and sends to it",
  async (provider) => {
    setInboxBot(provider, "bot-demo");
    const item = { ...ticket, provider };
    render({ item });
    expect(
      container.querySelector('[aria-label="Choose chat destination"]')
        ?.textContent,
    ).toContain("Demo Bot");
    await act(async () => button("Send to chat").click());
    expect(onStart).toHaveBeenCalledWith(item, undefined, "bot-demo");
  },
);

it("keeps separate defaults and leaves GitHub's destination alone", async () => {
  setInboxBot("jira", "bot-demo");
  setInboxBot("gitlab", "bot-review");
  const reviewBot = { ...bots[0], id: "bot-review", name: "Review Bot" };
  render({
    item: { ...ticket, provider: "gitlab" },
    bots: [...bots, reviewBot],
  });
  expect(
    container.querySelector('[aria-label="Choose chat destination"]')
      ?.textContent,
  ).toContain("Review Bot");
  render({ item: { ...ticket, provider: "github" } });
  expect(
    container.querySelector('[aria-label="Choose chat destination"]')
      ?.textContent,
  ).toContain("Demo project");
});

it("allows a project override without replacing the saved default", async () => {
  setInboxBot("jira", "bot-demo");
  render();
  await choose("Demo project");
  render();
  await act(async () => button("Send to chat").click());
  expect(onStart).toHaveBeenCalledWith(
    expect.objectContaining({ projectPath: "/tmp/demo" }),
    undefined,
    undefined,
  );
  expect(loadInboxBots()).toEqual({ jira: "bot-demo" });
});

it("falls back when the saved Bot has been removed", async () => {
  setInboxBot("jira", "bot-deleted");
  render();
  expect(
    container.querySelector('[aria-label="Choose chat destination"]')
      ?.textContent,
  ).toContain("Demo project");
  expect(button("Send to chat").disabled).toBe(false);
});

it("follows defaults and Bot availability updates until manually overridden", async () => {
  render({ bots: [] });
  act(() => setInboxBot("jira", "bot-demo"));
  render();
  expect(
    container.querySelector('[aria-label="Choose chat destination"]')
      ?.textContent,
  ).toContain("Demo Bot");
  act(() => setInboxBot("jira", null));
  expect(
    container.querySelector('[aria-label="Choose chat destination"]')
      ?.textContent,
  ).toContain("Demo project");
});

it("resets a manual destination when switching to a different item", async () => {
  render();
  await choose("Demo Bot");
  render({
    item: { ...ticket, url: "https://demo.atlassian.net/browse/DEMO-43" },
  });
  expect(
    container.querySelector('[aria-label="Choose chat destination"]')
      ?.textContent,
  ).toContain("Demo project");
});
it("offers both kinds of destination and sends to the selected project", async () => {
  render();
  await act(async () => button("Send to chat").click());
  expect(onStart).toHaveBeenCalledWith(
    expect.objectContaining({ projectPath: "/tmp/demo" }),
    undefined,
    undefined,
  );
});
it("sends to the selected Bot without assigning its project path", async () => {
  render();
  await choose("Demo Bot");
  await act(async () => button("Send to chat").click());
  expect(onStart).toHaveBeenCalledWith(ticket, undefined, "bot-demo");
});
it("works with a Bot even when there are no projects", async () => {
  render({ projects: [] });
  expect(button("Send to chat").disabled).toBe(false);
  await act(async () => button("Send to chat").click());
  expect(onStart).toHaveBeenCalledWith(ticket, undefined, "bot-demo");
});
it("keeps copy available when there are no destinations", async () => {
  render({ projects: [], bots: [] });
  expect(button("Send to chat").disabled).toBe(true);
  await act(async () => button("Copy context").click());
  expect(copyText).toHaveBeenCalledWith(
    expect.stringContaining("vorkspace://context/jira?url="),
  );
  expect(button("Copied")).toBeDefined();
});
it("shows clipboard errors without claiming the link was copied", async () => {
  vi.mocked(copyText).mockRejectedValueOnce(new Error("Clipboard unavailable"));
  render();
  await act(async () => button("Copy context").click());
  expect(container.querySelector('[role="alert"]')!.textContent).toBe(
    "Clipboard unavailable",
  );
  expect(button("Copy context")).toBeDefined();
});
it("allows retry after an opening failure", async () => {
  onStart.mockRejectedValueOnce(new Error("Could not open Bot"));
  render();
  await choose("Demo Bot");
  await act(async () => button("Send to chat").click());
  expect(container.querySelector('[role="alert"]')!.textContent).toBe(
    "Could not open Bot",
  );
  expect(button("Send to chat").disabled).toBe(false);
});
it("blocks duplicate sends while a destination is opening", async () => {
  let finish!: () => void;
  onStart.mockReturnValueOnce(
    new Promise<void>((resolve) => {
      finish = resolve;
    }),
  );
  render();
  await act(async () => {
    button("Send to chat").click();
    button("Send to chat").click();
  });
  expect(onStart).toHaveBeenCalledTimes(1);
  await act(async () => finish());
});
