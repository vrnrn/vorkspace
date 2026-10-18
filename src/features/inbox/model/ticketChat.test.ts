import { expect, it, vi } from "vitest";
import { openTicketInBot } from "./ticketChat";
import type { InboxItem } from "./githubTasks";
import type { Session } from "../../sessions/model/session";

const item = {
  provider: "jira",
  kind: "jira",
  identifier: "DEMO-42",
  number: 42,
  title: "Demo ticket",
  url: "https://demo.atlassian.net/browse/DEMO-42",
  labels: [],
  repo: "DEMO",
} as unknown as InboxItem;
function host() {
  const session = {
    id: "existing-chat",
    blocks: [{ id: "earlier", role: "user", text: "Existing conversation" }],
  } as Session;
  return {
    ensure: vi.fn(async () => session as Session | undefined),
    current: vi.fn(() => session),
    attach: vi.fn(),
    reveal: vi.fn(async () => {}),
    session,
  };
}
it("attaches to the existing Bot without replacing its conversation", async () => {
  const h = host();
  await openTicketInBot(item, "bot-demo", h);
  expect(h.attach).toHaveBeenCalledWith(
    "existing-chat",
    expect.objectContaining({
      title: "Demo ticket",
      prompt: expect.stringContaining("vorkspace://context/jira"),
    }),
  );
  expect(h.session.blocks[0].text).toBe("Existing conversation");
  expect(h.reveal).toHaveBeenCalledWith("bot-demo");
  expect(h.attach.mock.invocationCallOrder[0]).toBeLessThan(
    h.reveal.mock.invocationCallOrder[0],
  );
});
it("preserves an existing pending ticket", async () => {
  const h = host();
  h.session.inboxCard = { title: "Earlier ticket" } as Session["inboxCard"];
  await expect(openTicketInBot(item, "bot-demo", h)).rejects.toThrow(
    "already has a ticket",
  );
  expect(h.attach).not.toHaveBeenCalled();
});
it("reports a deleted Bot without opening another chat", async () => {
  const h = host();
  h.ensure.mockResolvedValue(undefined);
  await expect(openTicketInBot(item, "gone", h)).rejects.toThrow(
    "no longer available",
  );
  expect(h.attach).not.toHaveBeenCalled();
  expect(h.reveal).not.toHaveBeenCalled();
});
