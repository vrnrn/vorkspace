import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  applyTicketContextToTurn,
  hasTicketContextLinks,
  parseTicketContextLink,
  supportsTicketContext,
  ticketContextLink,
  ticketComposerCard,
} from "./ticketContext";
import { clearInboxCache, type InboxItem } from "./githubTasks";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => vi.unstubAllGlobals());

const urls = {
  jira: "https://demo.atlassian.net/browse/DEMO-42",
  github: "https://github.com/acme/demo/issues/42",
  gitlab: "https://gitlab.example.com/acme/nested/demo/-/issues/42",
};
const link = (provider: keyof typeof urls) =>
  ticketContextLink({ provider, url: urls[provider] });
let body = "Initial description";
let comment = "Initial comment";
beforeEach(() => {
  clearInboxCache();
  body = "Initial description";
  comment = "Initial comment";
  vi.mocked(invoke)
    .mockReset()
    .mockImplementation(async (command) => {
      if (command === "jira_status")
        return { connected: true, site: "https://demo.atlassian.net" };
      if (command === "gitlab_status")
        return { connected: true, url: "https://gitlab.example.com" };
      if (String(command).endsWith("details")) return { body, author: "Alex" };
      if (String(command).endsWith("thread"))
        return {
          comments: [
            {
              body: comment,
              author: "Robin",
              createdAt: "2026-10-14T12:00:00Z",
            },
          ],
          truncated: false,
        };
      throw new Error(`Unexpected command: ${command}`);
    });
});

it.each(["jira", "github", "gitlab"] as const)(
  "round trips %s identity without embedding ticket contents",
  (provider) => {
    const ref = parseTicketContextLink(link(provider))!;
    expect(ref.provider).toBe(provider);
    expect(ref.url).toBe(urls[provider]);
    expect(ref.number).toBe(42);
    expect(link(provider)).not.toContain(body);
  },
);
it.each(["jira", "github", "gitlab"] as const)(
  "fetches fresh %s description and comments on every turn",
  async (provider) => {
    const prompt = `Explain this: ${link(provider)}`;
    expect(await applyTicketContextToTurn(prompt, "/tmp/demo")).toContain(
      "Initial description",
    );
    body = "Updated description";
    comment = "Newest decision";
    const resolved = await applyTicketContextToTurn(prompt, "/tmp/demo");
    expect(resolved).toContain("Updated description");
    expect(resolved).toContain("Newest decision");
    expect(resolved).not.toContain("Initial description");
  },
);
it("deduplicates links in one turn while retaining the user's message", async () => {
  const prompt = `Please compare [ticket](${link("jira")}) and ${link("jira")}`;
  const resolved = await applyTicketContextToTurn(prompt, "/tmp/demo");
  expect(resolved.startsWith(prompt)).toBe(true);
  expect(
    vi.mocked(invoke).mock.calls.filter(([c]) => c === "jira_issue_details"),
  ).toHaveLength(1);
});
it("does not contact integrations for ordinary text", async () => {
  expect(await applyTicketContextToTurn("hello", "/tmp/demo")).toBe("hello");
  expect(invoke).not.toHaveBeenCalled();
});
it.each([
  "vorkspace://context/jira?url=https%3A%2F%2Fuser%3Asecret%40demo.atlassian.net%2Fbrowse%2FDEMO-42",
  "vorkspace://context/github?url=https%3A%2F%2Fevil.example%2Facme%2Fdemo%2Fissues%2F42",
  "vorkspace://context/jira?url=http%3A%2F%2Fdemo.atlassian.net%2Fbrowse%2FDEMO-42",
  "vorkspace://context/github?url=https%3A%2F%2Fgithub.com%2Facme%2Fdemo%2Fpull%2F42",
  "vorkspace://context/gitlab?url=https%3A%2F%2Fgitlab.example.com%2Facme%2Fdemo%2F-%2Fmerge_requests%2F42%2Fdiffs",
  "vorkspace://context/jira?url=https%3A%2F%2Fdemo.atlassian.net%2Fbrowse%2FDEMO-42%3Ftoken%3Dsecret",
])("rejects unsupported or credential-bearing references: %s", (value) =>
  expect(parseTicketContextLink(value)).toBeNull(),
);
const mergeRequestUrl =
  "https://gitlab.example.com/acme/nested/demo/-/merge_requests/42";
it("round trips a GitLab merge request reference", () => {
  const mergeRequest = {
    provider: "gitlab",
    kind: "pr",
    url: mergeRequestUrl,
  } as const;
  expect(parseTicketContextLink(ticketContextLink(mergeRequest))).toMatchObject(
    {
      provider: "gitlab",
      kind: "pr",
      key: "!42",
      repo: "acme/nested/demo",
      number: 42,
    },
  );
  expect(supportsTicketContext(mergeRequest)).toBe(true);
  expect(supportsTicketContext({ provider: "github", kind: "pr" })).toBe(false);
});
it("fetches a GitLab merge request with its branches", async () => {
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "gitlab_status")
      return { connected: true, url: "https://gitlab.example.com" };
    if (command === "gitlab_work_item_details")
      return {
        body: "Merge request body",
        author: "Alex",
        baseRefName: "main",
        headRefName: "feature",
      };
    if (command === "gitlab_work_item_thread")
      return { comments: [], truncated: false };
    throw new Error(`Unexpected command: ${command} ${JSON.stringify(args)}`);
  });
  const resolved = await applyTicketContextToTurn(
    ticketContextLink({ provider: "gitlab", url: mergeRequestUrl }),
    "/tmp/demo",
  );
  expect(resolved).toContain("Merge request body");
  expect(resolved).toContain('"sourceBranch":"feature"');
  expect(resolved).toContain('"targetBranch":"main"');
  expect(invoke).toHaveBeenCalledWith(
    "gitlab_work_item_details",
    expect.objectContaining({ kind: "pr", number: 42 }),
  );
});
it("asks the agent to work on a GitLab merge request", () => {
  const card = ticketComposerCard({
    provider: "gitlab",
    kind: "pr",
    number: 42,
    title: "Add retries",
    url: mergeRequestUrl,
    labels: [],
  } as unknown as InboxItem);
  expect(card.prompt).toContain("Work on this GitLab merge request:");
  expect(card.prompt).toContain("vorkspace://context/gitlab?url=");
});
it("fails closed for a different Jira site", async () => {
  const other = ticketContextLink({
    provider: "jira",
    url: "https://other.atlassian.net/browse/DEMO-42",
  });
  await expect(applyTicketContextToTurn(other, "/tmp/demo")).rejects.toThrow(
    "Connect this Jira site",
  );
  expect(invoke).toHaveBeenCalledTimes(1);
});
it("fails closed for a different GitLab host", async () => {
  const other = ticketContextLink({
    provider: "gitlab",
    url: "https://other.example.com/acme/demo/-/issues/42",
  });
  await expect(applyTicketContextToTurn(other, "/tmp/demo")).rejects.toThrow(
    "Connect this GitLab host",
  );
  expect(invoke).toHaveBeenCalledTimes(1);
});
it("reports fetch failures instead of sending an empty context", async () => {
  vi.mocked(invoke).mockRejectedValue(new Error("Permission denied"));
  await expect(
    applyTicketContextToTurn(link("github"), "/tmp/demo"),
  ).rejects.toThrow("Could not load github #42 context: Permission denied");
});
it("limits context and reports omitted comments", async () => {
  vi.mocked(invoke).mockImplementation(async (command) =>
    String(command).endsWith("details")
      ? { body: "x".repeat(70_000) }
      : {
          comments: Array.from({ length: 50 }, (_, i) => ({
            body: `Comment ${i}`,
            author: "Alex",
            createdAt: "",
          })),
          truncated: false,
        },
  );
  const resolved = await applyTicketContextToTurn(link("github"), "/tmp/demo");
  expect(resolved).toContain('"truncated":true');
  expect(resolved).not.toContain('"body":"Comment 0"');
  expect(resolved).toContain('"body":"Comment 49"');
});
it("creates the same portable context card for project and Bot composers", () => {
  const item = {
    provider: "jira",
    kind: "jira",
    identifier: "DEMO-42",
    title: "Demo ticket",
    url: urls.jira,
    labels: [],
    repo: "DEMO",
    number: 42,
  } as unknown as InboxItem;
  const card = ticketComposerCard(item, "Older body");
  expect(card.prompt).toContain(link("jira"));
  expect(card.prompt).toContain("Demo ticket");
  expect(card.prompt).not.toContain("Older body");
});

it("emits vorkspace links while accepting legacy monocode references", async () => {
  const current = link("jira");
  const legacy = current.replace("vorkspace:", "monocode:");
  expect(current).toMatch(/^vorkspace:\/\/context\//);
  expect(parseTicketContextLink(legacy)).toEqual(
    parseTicketContextLink(current),
  );
  expect(
    await applyTicketContextToTurn(`Review ${legacy}`, "/tmp/demo"),
  ).toContain("Initial description");
});

it("generates and resolves development links using the actual native profile", async () => {
  vi.stubGlobal("__VORKSPACE_DEVELOPMENT__", true);
  const development = link("jira");
  expect(development).toMatch(/^vorkspace-dev:\/\/context\//);
  expect(hasTicketContextLinks(`Review ${development}`)).toBe(true);
  expect(parseTicketContextLink(development)?.key).toBe("DEMO-42");
  expect(
    await applyTicketContextToTurn(`Review ${development}`, "/tmp/demo"),
  ).toContain("Initial description");
  vi.stubGlobal("__VORKSPACE_DEVELOPMENT__", false);
  expect(link("jira")).toMatch(/^vorkspace:\/\/context\//);
});
