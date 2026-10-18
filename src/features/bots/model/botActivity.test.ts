import { expect, it } from "vitest";
import type { Block } from "../../sessions/model/session";
import {
  resolveBotActivity,
  type BotActivitySelection,
} from "./botActivity";

const blocks: Block[] = [
  { id: "user", role: "user", text: "Inspect" },
  {
    id: "call",
    role: "tool",
    text: "Inspect file",
    tool: { status: "in_progress" },
  },
];
const selection: BotActivitySelection = {
  sessionId: "bot",
  turnId: "user",
  blocks,
};

it("follows the selected live turn, including new steps and settled tool state", () => {
  const updated: Block[] = [
    blocks[0],
    { ...blocks[1], tool: { status: "completed" } },
    { id: "reply", role: "assistant", text: "Done" },
  ];
  expect(
    resolveBotActivity(selection, { id: "bot", blocks: updated, busy: true }),
  ).toEqual({
    turnId: "user",
    blocks: updated,
    live: true,
  });
  expect(
    resolveBotActivity(selection, { id: "bot", blocks: updated, busy: false })
      ?.live,
  ).toBe(false);
  expect(
    resolveBotActivity(selection, {
      id: "bot",
      blocks: [
        ...updated,
        { id: "next", role: "user", text: "Another request" },
      ],
      busy: true,
    })?.live,
  ).toBe(false);
});

it("retains an archived turn without borrowing another turn or Bot's activity", () => {
  expect(
    resolveBotActivity(selection, {
      id: "bot",
      blocks: [{ id: "new", role: "user", text: "Latest" }],
      busy: true,
    }),
  ).toEqual({
    turnId: "user",
    blocks,
    live: false,
  });
  expect(
    resolveBotActivity(selection, { id: "another-bot", blocks }),
  ).toBeNull();
  expect(resolveBotActivity(selection, undefined)).toBeNull();
  expect(resolveBotActivity(null, { id: "bot", blocks })).toBeNull();
});

it("keeps completion activity separate from the previous reply before any output arrives", () => {
  const notification: Block = {
    id: "notification",
    role: "user",
    text: "Hidden completion prompt",
    internal: true,
    startedAt: 2_000,
    botSessionCompletion: {
      sessionId: "worker",
      title: "Review",
      status: "completed",
    },
  };
  const session = { id: "bot", blocks: [...blocks, notification], busy: true };
  expect(resolveBotActivity(selection, session)?.live).toBe(false);
  const completionSelection = {
    sessionId: "bot",
    turnId: notification.id,
    blocks: [notification],
  };
  const step: Block = {
    id: "check-report",
    role: "tool",
    text: "Read the review session",
    tool: { status: "in_progress" },
  };
  const updated = { ...session, blocks: [...session.blocks, step] };
  expect(resolveBotActivity(completionSelection, updated)).toEqual({
    turnId: notification.id,
    blocks: [notification, step],
    live: true,
  });
});
