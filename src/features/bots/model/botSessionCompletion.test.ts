import { describe, expect, it } from "vitest";
import { submitWithSettlement } from "../../../app/model/managedSubmission";
import type { ControlOutcome } from "../../orchestration/model/orchestration";
import {
  canDispatchQueuedHead,
  canSteerQueuedHead,
  dequeueQueuedMessage,
  queuedMessageForSubmit,
} from "../../sessions/model/messageQueue";
import {
  newSession,
  type QueuedMessage,
  type Session,
} from "../../sessions/model/session";
import {
  dismissQueuedBotSessionCompletion,
  enqueueBotSessionCompletion,
  botSessionCompletionMessage,
  botSessionCompletionResult,
  BotSessionCompletionBatches,
} from "./botSessionCompletion";

const options = {
  requestId: "app-bot-request",
  sessionId: "worker",
  project: "/code/project",
  prompt: "Fix the API and run tests",
  outcome: {
    status: "completed",
    text: "The API is fixed and tests pass.",
  } as ControlOutcome,
};

function groupedMessage() {
  let message!: QueuedMessage;
  const batches = new BotSessionCompletionBatches((_, ready) => {
    message = ready;
  });
  for (const [sessionId, status] of [
    ["stopped", "cancelled"],
    ["failed", "failed"],
    ["completed", "completed"],
  ] as const) {
    batches.watch(
      { botId: "bot", turn: 1 },
      sessionId,
    )(
      botSessionCompletionResult({
        ...options,
        sessionId,
        requestId: sessionId,
        outcome: { status, text: `Result for ${sessionId}` },
      }),
    );
  }
  batches.closeInactive(() => false);
  return message;
}

describe("Bot session completion notifications", () => {
  it("accepts immediately, then queues completion behind chat without steering a busy Bot", async () => {
    let bot: Session = {
      ...newSession("codex", "/code/project"),
      busy: true,
      turnReady: true,
      queuedMessages: [
        { id: "chat", text: "Also explain the design", attachments: [] },
      ],
    };
    let settle!: (outcome: ControlOutcome) => void;
    await expect(
      submitWithSettlement({
        submit: (done) => {
          settle = done;
          return true;
        },
        onSettled: (outcome) => {
          bot = enqueueBotSessionCompletion(
            bot,
            botSessionCompletionMessage({ ...options, outcome }),
          );
        },
        rejectionMessage: "Could not start",
      }),
    ).resolves.toBe(true);
    expect(bot.queuedMessages).toHaveLength(1);
    settle(options.outcome);
    settle(options.outcome);
    expect(bot.queuedMessages?.map((message) => message.id)).toEqual([
      "chat",
      "bot-completion-app-bot-request",
    ]);
    const next = dequeueQueuedMessage(bot, "chat");
    expect(canSteerQueuedHead(next)).toBe(false);
    expect(
      queuedMessageForSubmit(next, "bot-completion-app-bot-request", "steer"),
    ).toBeUndefined();
    expect(canDispatchQueuedHead(next)).toBe(false);
    expect(canDispatchQueuedHead({ ...next, busy: false })).toBe(true);
  });

  it("uses the final answer from the watched turn even after a later turn begins", () => {
    const session = newSession("codex", options.project);
    session.title = "API fix";
    session.blocks = [
      {
        id: "u",
        role: "user",
        text: options.prompt,
        appRequestId: options.requestId,
      },
      { id: "progress", role: "assistant", text: "I am checking" },
      {
        id: "tool",
        role: "tool",
        text: "private tool output",
        tool: { kind: "shell" },
      },
      {
        id: "reply",
        role: "assistant",
        text: "Fixed the endpoint; 12 tests pass.",
      },
      { id: "later", role: "user", text: "New task" },
      { id: "later-reply", role: "assistant", text: "Unrelated answer" },
    ];
    const message = botSessionCompletionMessage({ ...options, session });
    expect(message.text).toContain("Fixed the endpoint; 12 tests pass.");
    expect(message.text).toContain(options.project);
    expect(message.text).toContain(options.prompt);
    expect(message.text).not.toContain("Unrelated answer");
    expect(message.text).not.toContain("private tool output");
    expect(message.botSessionCompletion).toEqual({
      sessionId: "worker",
      title: "API fix",
      status: "completed",
    });
  });

  it.each(["failed", "cancelled"] as const)(
    "reports %s with available output and an error",
    (status) => {
      const message = botSessionCompletionMessage({
        ...options,
        outcome: { status, text: "Partial work", error: "Agent stopped" },
      });
      expect(message.botSessionCompletion?.status).toBe(status);
      expect(message.text).toContain("Partial work");
      expect(message.text).toContain("Agent stopped");
    },
  );

  it("deduplicates both pending and consumed receipts and preserves a paused queue", () => {
    const bot = newSession("codex", options.project);
    const message = botSessionCompletionMessage(options);
    const next = enqueueBotSessionCompletion(
      { ...bot, queueStatus: "paused" },
      message,
    );
    expect(next.queueStatus).toBe("paused");
    expect(enqueueBotSessionCompletion(next, message)).toBe(next);
    const delivered = {
      ...bot,
      blocks: [
        {
          id: "notice",
          role: "user" as const,
          text: message.text,
          appRequestId: message.id,
          internal: true,
        },
      ],
    };
    expect(enqueueBotSessionCompletion(delivered, message)).toBe(delivered);
  });

  it("dismisses a queued session report while keeping chat, other reports and the consumed transcript", () => {
    const target = botSessionCompletionMessage(options);
    const other = botSessionCompletionMessage({
      ...options,
      requestId: "other-request",
      sessionId: "other",
    });
    const chat = { id: "chat", text: "Explain the change", attachments: [] };
    const bot: Session = {
      ...newSession("codex", options.project),
      queueStatus: "paused",
      queuedMessages: [chat, target, other],
      blocks: [
        {
          id: "old-notice",
          role: "user",
          text: target.text,
          appRequestId: target.id,
          internal: true,
        },
      ],
    };
    const next = dismissQueuedBotSessionCompletion(bot, "worker");
    expect(next.queuedMessages).toEqual([chat, other]);
    expect(next.queueStatus).toBe("paused");
    expect(next.blocks).toBe(bot.blocks);
    expect(dismissQueuedBotSessionCompletion(next, "worker")).toBe(next);
  });

  it("removes reports from a saved batch and updates its identity, count and aggregate status", () => {
    // A save/load keeps the JSON text and metadata; it has no in-memory watches.
    const message = JSON.parse(
      JSON.stringify(groupedMessage()),
    ) as QueuedMessage;
    const bot: Session = {
      ...newSession("codex", options.project),
      queuedMessages: [message],
    };
    const next = dismissQueuedBotSessionCompletion(bot, "failed");
    const remaining = next.queuedMessages![0];
    expect(remaining.id).toBe(message.id);
    expect(remaining.botSessionCompletion).toEqual({
      sessionId: "stopped",
      title: "2 session results",
      status: "cancelled",
      sessionCount: 2,
    });
    expect(
      JSON.parse(remaining.text.split("\n\n")[1]).sessions.map(
        (entry: { sessionId: string }) => entry.sessionId,
      ),
    ).toEqual(["stopped", "completed"]);

    const last = dismissQueuedBotSessionCompletion(next, "stopped");
    expect(last.queuedMessages![0].id).toBe(message.id);
    expect(last.queuedMessages![0].botSessionCompletion).toEqual({
      sessionId: "completed",
      title: "Agent session",
      status: "completed",
    });
    expect(
      JSON.parse(last.queuedMessages![0].text.split("\n\n")[1]).sessionId,
    ).toBe("completed");
    expect(
      dismissQueuedBotSessionCompletion(last, "completed").queuedMessages,
    ).toEqual([]);
  });

  it("leaves unrelated or unrecognized saved batches untouched", () => {
    const message = groupedMessage();
    const bot: Session = {
      ...newSession("codex", options.project),
      queuedMessages: [message],
    };
    expect(dismissQueuedBotSessionCompletion(bot, "unrelated")).toBe(bot);
    const malformed: Session = {
      ...bot,
      queuedMessages: [{ ...message, text: "Incomplete saved report" }],
    };
    expect(dismissQueuedBotSessionCompletion(malformed, "stopped")).toBe(
      malformed,
    );
  });

  it("bounds long results and points the Bot to the source conversation", () => {
    const message = botSessionCompletionMessage({
      ...options,
      outcome: { status: "completed", text: "x".repeat(15_000) },
    });
    expect(message.text).toContain('"truncated":true');
    expect(message.text).toContain("app sessions.read");
    expect(message.text.length).toBeLessThan(13_000);
  });
});
