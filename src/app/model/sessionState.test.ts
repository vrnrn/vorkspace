import { ownLocalSubmissionTurn } from "../../integrations/harness/core/localSubmissions";
import { sanitizeSessionForPersist } from "../../features/sessions/data/sessionStore";
// @vitest-environment happy-dom
import { act, createElement, StrictMode, useRef, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  newSession,
  type Session,
} from "../../features/sessions/model/session";
import {
  applyHarnessEvents,
  stopStreaming,
} from "../../integrations/harness/core/apply";
import {
  createBotPeerQueuedMessage,
  enqueueBotPeerMessage,
} from "../../features/bots/model/botChannels";
import { botState } from "../../features/bots/model/bot";
import { canDispatchQueuedHead } from "../../features/sessions/model/messageQueue";
import { HarnessEventQueue } from "./harnessFlush";
import { updateSessionState } from "./sessionState";

let root: Root;
let container: HTMLDivElement;

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

function mountSessions(strict = false) {
  const blank: Session = {
    ...newSession("claude", "/repo"),
    id: "blank",
    providerAccountId: "default",
  };
  const background: Session = {
    ...newSession("claude", "/repo"),
    id: "background",
    busy: true,
  };
  let snapshot!: {
    sessions: Session[];
    update: (change: (sessions: Session[]) => Session[]) => void;
    rename: (title: string) => void;
    queue: HarnessEventQueue;
    current: () => Session[];
  };

  function Workspace() {
    const [sessions, setSessions] = useState([blank, background]);
    const sessionsRef = useRef(sessions);
    sessionsRef.current = sessions;
    const [queue] = useState(
      () =>
        new HarnessEventQueue(
          () => false,
          (batches) => {
            updateSessionState(sessionsRef, setSessions, (current) =>
              current.map((session) => {
                const events = batches.get(session.id);
                return events ? applyHarnessEvents(session, events) : session;
              }),
            );
          },
        ),
    );
    snapshot = {
      sessions,
      update: (change) => {
        updateSessionState(sessionsRef, setSessions, change);
      },
      rename: (title) => {
        setSessions((current) =>
          current.map((session) =>
            session.id === "blank" ? { ...session, title } : session,
          ),
        );
      },
      queue,
      current: () => sessionsRef.current,
    };
    return null;
  }

  act(() =>
    root.render(
      strict
        ? createElement(StrictMode, null, createElement(Workspace))
        : createElement(Workspace),
    ),
  );
  return () => snapshot;
}

function chooseAccount(sessions: Session[]): Session[] {
  return sessions.map((session) =>
    session.id === "blank"
      ? { ...session, providerAccountId: "account-work" }
      : session,
  );
}

function flushBackground(queue: HarnessEventQueue) {
  queue.enqueue("background", {
    type: "message.delta",
    text: "Still working",
  });
  queue.flush();
}

it("keeps a fresh account choice when another session flushes before render", () => {
  const state = mountSessions();
  act(() => {
    state().update(chooseAccount);
    expect(state().current()[0].providerAccountId).toBe("account-work");
    flushBackground(state().queue);
  });

  expect(state().sessions[0].providerAccountId).toBe("account-work");
  expect(state().sessions[1].blocks[0].text).toBe("Still working");
});

it("keeps background output when the account changes later in the same batch", () => {
  const state = mountSessions();
  act(() => {
    flushBackground(state().queue);
    state().update(chooseAccount);
  });

  expect(state().sessions[0].providerAccountId).toBe("account-work");
  expect(state().sessions[1].blocks[0].text).toBe("Still working");
});

it("preserves other pending React changes while switching accounts and flushing", () => {
  const state = mountSessions();
  act(() => {
    state().rename("Draft title");
    state().update(chooseAccount);
    flushBackground(state().queue);
  });

  expect(state().sessions[0].title).toBe("Draft title");
  expect(state().sessions[0].providerAccountId).toBe("account-work");
  expect(state().sessions[1].blocks[0].text).toBe("Still working");
});

it("retains a newly opened account session during a background flush", () => {
  const state = mountSessions();
  const opened = {
    ...newSession("claude", "/repo"),
    providerAccountId: "account-work",
  };
  act(() => {
    state().update((current) => [...current, opened]);
    flushBackground(state().queue);
  });

  expect(state().sessions).toHaveLength(3);
  expect(state().sessions[2]).toBe(opened);
  expect(state().sessions[0].providerAccountId).toBe("default");
  expect(state().sessions[1].blocks[0].text).toBe("Still working");
});

it("preserves the account and a single copy of streamed output in Strict Mode", () => {
  const state = mountSessions(true);
  act(() => {
    state().rename("Draft title");
    state().update(chooseAccount);
    flushBackground(state().queue);
  });

  expect(state().sessions[0].title).toBe("Draft title");
  expect(state().sessions[0].providerAccountId).toBe("account-work");
  expect(state().sessions[1].blocks).toHaveLength(1);
  expect(state().sessions[1].blocks[0].text).toBe("Still working");
});

it.each([false, true])(
  "settles a DM recipient before delivering its reply in the same React batch (strict=%s)",
  (strict) => {
    const state = mountSessions(strict);
    const reply = createBotPeerQueuedMessage({
      fromBotId: "recipient",
      toBotId: "sender",
      text: "Here is the answer",
      requestId: "peer-reply-request",
      expectReply: false,
      hop: 2,
    });
    const followUp = createBotPeerQueuedMessage({
      fromBotId: "sender",
      toBotId: "recipient",
      text: "One more question",
      requestId: "next-request",
    });

    act(() => {
      flushBackground(state().queue);
      state().update((sessions) =>
        sessions.map((session) =>
          session.id === "background" ? stopStreaming(session) : session,
        ),
      );
      // The onSettled callback runs before React renders. Its snapshot must
      // already contain the finished reply and idle recipient.
      const recipient = state().current()[1];
      expect(botState(recipient).status).toBe("idle");
      expect(recipient.blocks[0]).toMatchObject({
        text: "Still working",
        streaming: false,
      });

      // A pending UI change must also survive the automatic reply delivery.
      state().rename("Sender conversation");
      state().update((sessions) =>
        sessions.map((session) =>
          session.id === "blank"
            ? enqueueBotPeerMessage(session, reply).session
            : session,
        ),
      );
      state().update((sessions) =>
        sessions.map((session) =>
          session.id === "background"
            ? enqueueBotPeerMessage(session, followUp).session
            : session,
        ),
      );
    });

    const [sender, recipient] = state().sessions;
    expect(sender.title).toBe("Sender conversation");
    expect(sender.queuedMessages).toEqual([reply]);
    expect(canDispatchQueuedHead(sender)).toBe(true);
    expect(botState(recipient).status).toBe("idle");
    expect(recipient.blocks[0].streaming).toBe(false);
    expect(recipient.queuedMessages).toEqual([followUp]);
    expect(canDispatchQueuedHead(recipient)).toBe(true);
  },
);

it.each([false, true])(
  "keeps admission ownership in ref and rendered state across concurrent updates (strict=%s)",
  (strict) => {
    const state = mountSessions(strict);
    act(() => {
      state().update((sessions) =>
        sessions.map((session) =>
          session.id === "blank"
            ? {
                ...session,
                blocks: [
                  {
                    id: "previous-turn",
                    role: "user",
                    text: "Original request",
                  },
                ],
              }
            : session,
        ),
      );
      state().rename("Pending title");
      // An edited replacement is not visible yet. Marking only a storage copy
      // would lose this guard when the ordinary rendered state saves again.
      state().update((sessions) =>
        sessions.map((session) =>
          session.id === "blank"
            ? ownLocalSubmissionTurn(session, "edited-replacement")
            : session,
        ),
      );
      const checkpoint = sanitizeSessionForPersist(state().current()[0]);
      expect(checkpoint.blocks[0].localSubmissionOwned).toBe(true);
      flushBackground(state().queue);
    });
    expect(state().sessions[0].title).toBe("Pending title");
    expect(state().sessions[0].blocks[0].localSubmissionOwned).toBe(true);
    expect(
      sanitizeSessionForPersist(state().sessions[0]).blocks[0]
        .localSubmissionOwned,
    ).toBe(true);
  },
);
