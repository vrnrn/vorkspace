// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createBot, saveBotName } from "../../features/bots/model/bot";
import {
  createBotChannel,
  findBotChannel,
} from "../../features/bots/model/botChannels";
import { shouldPersistSession } from "../../features/sessions/data/sessionStore";
import {
  newSession,
  type Session,
} from "../../features/sessions/model/session";
import { useBotChannels, type BotChannelHost } from "./useBotChannels";
import type { ControlOutcome } from "../../features/orchestration/model/orchestration";

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  localStorage.clear();
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  localStorage.clear();
  vi.unstubAllGlobals();
});

function mountHook(host: BotChannelHost) {
  let latest!: ReturnType<typeof useBotChannels>;
  function Harness() {
    latest = useBotChannels(host);
    return null;
  }
  act(() => {
    root.render(createElement(Harness));
  });
  return {
    get current() {
      return latest;
    },
  };
}

describe("useBotChannels", () => {
  it.each(["turn", "cancellation"])(
    "serializes restart when %s settles first after Stop",
    async (firstToSettle) => {
      const bot = createBot(["/code/api"]);
      const channel = createBotChannel({
        name: "restart",
        memberIds: [bot.id],
      });
      const sessions: Session[] = [];
      const settlements: ((outcome: ControlOutcome) => void)[] = [];
      let finishCancellation!: () => void;
      const cancellation = new Promise<void>((resolve) => {
        finishCancellation = resolve;
      });
      const host: BotChannelHost = {
        sessions: () => sessions,
        ensureBot: async () => newSession("codex", "/code/api"),
        openSession: async () => undefined,
        addSession: (session) => {
          sessions.push(session);
        },
        runTurn: vi.fn(
          () =>
            new Promise((resolve) => {
              settlements.push(resolve);
            }),
        ),
        stopSession: vi.fn(() => cancellation),
      };
      const hook = mountHook(host);
      await act(async () => {
        hook.current.sendUserChannelMessage(channel.id, "Original request");
      });
      const backingId = sessions[0].id;
      let stopping!: Promise<void>;
      act(() => {
        stopping = hook.current.stopChannel(channel.id);
      });
      await act(async () => {
        hook.current.sendUserChannelMessage(channel.id, "Replacement request");
      });
      expect(host.runTurn).toHaveBeenCalledTimes(1);

      const finishTurn = () =>
        settlements[0]({ status: "completed", text: "Stale reply" });
      await act(async () => {
        if (firstToSettle === "turn") finishTurn();
        else finishCancellation();
      });
      expect(host.runTurn).toHaveBeenCalledTimes(1);
      expect(
        findBotChannel(channel.id)?.memberCursors[bot.id],
      ).toBeUndefined();
      expect(
        findBotChannel(channel.id)?.messages.some(
          (m) => m.text === "Stale reply",
        ),
      ).toBe(false);

      await act(async () => {
        if (firstToSettle === "turn") finishCancellation();
        else finishTurn();
        await stopping;
      });
      expect(host.runTurn).toHaveBeenCalledTimes(2);
      expect(hook.current.busyChannelIds.has(channel.id)).toBe(true);
      expect(hook.current.currentHop(backingId)).toBe(1);

      // A later message queues behind the replacement, and Stop still targets it.
      await act(async () => {
        hook.current.sendUserChannelMessage(channel.id, "Third request");
      });
      expect(host.runTurn).toHaveBeenCalledTimes(2);
      await act(async () => {
        await hook.current.stopChannel(channel.id);
      });
      expect(host.stopSession).toHaveBeenLastCalledWith(backingId);
      expect(host.stopSession).toHaveBeenCalledTimes(2);
      await act(async () => {
        settlements[1]({ status: "cancelled", text: "" });
      });
      expect(host.runTurn).toHaveBeenCalledTimes(2);
      expect(hook.current.busyChannelIds.has(channel.id)).toBe(false);
    },
  );

  it("does not launch a stopped turn when backing-session setup finishes late", async () => {
    const bot = createBot(["/code/api"]);
    const channel = createBotChannel({ name: "setup", memberIds: [bot.id] });
    const sessions: Session[] = [];
    let finishSetup!: (session: Session) => void;
    const setup = new Promise<Session>((resolve) => {
      finishSetup = resolve;
    });
    const host: BotChannelHost = {
      sessions: () => sessions,
      ensureBot: vi.fn(() => setup),
      openSession: async () => undefined,
      addSession: (session) => {
        sessions.push(session);
      },
      runTurn: vi.fn(async () => ({
        status: "completed" as const,
        text: "Fresh reply",
      })),
      stopSession: vi.fn(async () => {}),
    };
    const hook = mountHook(host);
    await act(async () => {
      hook.current.sendUserChannelMessage(channel.id, "Original request");
    });
    await act(async () => {
      await hook.current.stopChannel(channel.id);
    });
    await act(async () => {
      hook.current.sendUserChannelMessage(channel.id, "New request");
    });
    expect(host.ensureBot).toHaveBeenCalledTimes(1);
    await act(async () => {
      finishSetup(newSession("codex", "/code/api"));
    });
    expect(sessions).toHaveLength(1);
    expect(host.runTurn).toHaveBeenCalledTimes(1);
    expect(vi.mocked(host.runTurn).mock.calls[0][1]).toContain("New request");
    expect(hook.current.busyChannelIds.has(channel.id)).toBe(false);
  });

  it.each(["failed", "cancelled", "rejected"])(
    "keeps unread messages available after a %s turn",
    async (failure) => {
      const bot = createBot(["/code/api"]);
      const channel = createBotChannel({
        name: "retry",
        memberIds: [bot.id],
      });
      const sessions: Session[] = [];
      const runTurn = vi.fn<BotChannelHost["runTurn"]>();
      if (failure === "rejected") {
        runTurn.mockRejectedValueOnce(new Error("Provider unavailable"));
      } else {
        runTurn.mockResolvedValueOnce({
          status: failure as "failed" | "cancelled",
          text: "Partial response",
        });
      }
      runTurn.mockResolvedValue({
        status: "completed",
        text: "Recovered reply",
      });
      const hook = mountHook({
        sessions: () => sessions,
        ensureBot: async () => newSession("codex", "/code/api"),
        openSession: async () => undefined,
        addSession: (session) => {
          sessions.push(session);
        },
        runTurn,
        stopSession: async () => {},
      });

      await act(async () => {
        hook.current.sendUserChannelMessage(channel.id, "Original question");
      });
      expect(runTurn).toHaveBeenCalledTimes(1);
      expect(hook.current.busyChannelIds.has(channel.id)).toBe(false);
      expect(
        findBotChannel(channel.id)?.memberCursors[bot.id],
      ).toBeUndefined();
      expect(findBotChannel(channel.id)?.messages).toHaveLength(1);

      await act(async () => {
        hook.current.sendUserChannelMessage(channel.id, "Please retry");
      });
      expect(runTurn).toHaveBeenCalledTimes(2);
      expect(runTurn.mock.calls[1][1]).toContain("Original question");
      expect(runTurn.mock.calls[1][1]).toContain("Please retry");
      expect(
        findBotChannel(channel.id)?.messages.map((message) => message.text),
      ).toEqual(["Original question", "Please retry", "Recovered reply"]);
    },
  );

  it("inherits the Bot session runtimeMode, persists backing sessions, and queues follow-up turns for an active speaker", async () => {
    const alpha = createBot(["/code/api"]);
    saveBotName(alpha.id, "Alpha");
    const alphaBotSession: Session = {
      ...newSession("claude", "/code/api", "claude-sonnet-4-6", "auto"),
      id: alpha.id,
      providerAccountId: "account-work",
    };

    const sessions: Session[] = [alphaBotSession];
    const prompts: string[] = [];
    let resolveFirstTurn!: () => void;
    const firstTurnGate = new Promise<void>((resolve) => {
      resolveFirstTurn = resolve;
    });

    const host: BotChannelHost = {
      sessions: () => sessions,
      ensureBot: async (id) =>
        id === alpha.id ? alphaBotSession : undefined,
      openSession: async (id) => sessions.find((s) => s.id === id),
      addSession: (session) => {
        sessions.push(session);
      },
      runTurn: async (sessionId, prompt) => {
        prompts.push(prompt);
        const s = sessions.find((entry) => entry.id === sessionId);
        if (s) {
          s.blocks.push({
            id: `u-${prompts.length}`,
            role: "user",
            text: prompt,
          });
        }
        if (prompts.length === 1) {
          await firstTurnGate;
          return { status: "completed", text: "First reply from Alpha." };
        }
        return { status: "completed", text: "Second reply from Alpha." };
      },
      stopSession: async () => {},
    };

    const channel = createBotChannel({
      name: "alpha-sync",
      memberIds: [alpha.id],
    });
    const hook = mountHook(host);

    // Send first user message
    act(() => {
      hook.current.sendUserChannelMessage(channel.id, "@Alpha First task");
    });

    // Wait for first turn to start
    await vi.waitFor(() => {
      expect(prompts).toHaveLength(1);
    });

    const backingId = findBotChannel(channel.id)?.memberSessions[alpha.id];
    expect(backingId).toBeDefined();
    const backing = sessions.find((s) => s.id === backingId)!;
    // Must preserve the Bot's selected permission mode ("auto", not "full-access")
    expect(backing.runtimeMode).toBe("auto");
    expect(backing.providerAccountId).toBe("account-work");
    // Must be persistable once populated (not ephemeral)
    expect(backing.ephemeral).toBeUndefined();
    expect(shouldPersistSession(backing)).toBe(true);

    // While Alpha is still running turn 1, user sends a second follow-up message
    act(() => {
      hook.current.sendUserChannelMessage(
        channel.id,
        "@Alpha Second follow-up",
      );
    });

    // Finish turn 1; turn 2 should automatically run and receive the follow-up!
    await act(async () => {
      resolveFirstTurn();
    });

    await vi.waitFor(() => {
      expect(prompts).toHaveLength(2);
    });
    expect(prompts[1]).toContain("Second follow-up");
    expect(prompts[1]).not.toContain("First reply from Alpha.");

    await vi.waitFor(() => {
      expect(hook.current.busyChannelIds.has(channel.id)).toBe(false);
    });
    expect(findBotChannel(channel.id)?.messages.map((m) => m.text)).toEqual([
      "@Alpha First task",
      "@Alpha Second follow-up",
      "First reply from Alpha.",
      "Second reply from Alpha.",
    ]);
  });

  it("clears channel active state when backing-session setup rejects so future messages still run", async () => {
    const alpha = createBot(["/code/api"]);
    saveBotName(alpha.id, "Alpha");
    const alphaBotSession: Session = {
      ...newSession("claude", "/code/api", "claude-sonnet-4-6", "supervised"),
      id: alpha.id,
    };

    const sessions: Session[] = [alphaBotSession];
    let shouldFailSetup = true;
    const prompts: string[] = [];

    const host: BotChannelHost = {
      sessions: () => sessions,
      ensureBot: async () => {
        if (shouldFailSetup) throw new Error("Transient storage error");
        return alphaBotSession;
      },
      openSession: async (id) => sessions.find((s) => s.id === id),
      addSession: (session) => {
        sessions.push(session);
      },
      runTurn: async (_sessionId, prompt) => {
        prompts.push(prompt);
        return { status: "completed", text: "Recovered reply." };
      },
      stopSession: async () => {},
    };

    const channel = createBotChannel({
      name: "recovery-room",
      memberIds: [alpha.id],
    });
    const hook = mountHook(host);

    act(() => {
      hook.current.sendUserChannelMessage(
        channel.id,
        "@Alpha Try during outage",
      );
    });

    await vi.waitFor(() => {
      expect(hook.current.busyChannelIds.has(channel.id)).toBe(false);
    });

    shouldFailSetup = false;
    act(() => {
      hook.current.sendUserChannelMessage(
        channel.id,
        "@Alpha Try after recovery",
      );
    });

    await vi.waitFor(() => {
      expect(prompts).toHaveLength(1);
    });
    expect(prompts[0]).toContain("Try after recovery");
  });
});
