import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { AgentTranscript } from "../../src/features/sessions/ui/AgentTranscript";
import { BotComposer } from "../../src/features/bots/ui/BotComposer";
import { MessageQueue } from "../../src/features/sessions/ui/MessageQueue";
import { visibleBotCompletionMessages } from "../../src/features/bots/model/botMessaging";
import {
  newSession,
  type Block,
  type QueuedMessage,
} from "../../src/features/sessions/model/session";
import {
  saveTranscriptAnchor,
  saveTranscriptLayout,
} from "../../src/features/settings/model/appearance";
import "../../src/styles/index.css";

saveTranscriptAnchor(false);
saveTranscriptLayout("chat");

// Settled plain text isolates layout during scrolling from streaming, image
// loading and asynchronous syntax highlighting. Turns deliberately vary in size.
const blocks: Block[] = Array.from({ length: 20 }, (_, index): Block[] => [
  {
    id: `user-${index}`,
    role: "user",
    text: `Prompt ${index}: explain this behavior and preserve my place while reading.`,
  },
  {
    id: `reply-${index}`,
    role: "assistant",
    text: Array.from(
      { length: 8 + (index % 5) * 8 },
      (_, paragraph) =>
        `Paragraph ${paragraph}. This is a settled answer with enough text to exercise variable message heights.`,
    ).join("\n\n"),
  },
]).flat();

// Like the transcript pool, parking detaches the transcript from the page.
const root = document.getElementById("root")!;
const host = document.createElement("div");
host.className = "contents";
root.appendChild(host);

function Fixture() {
  const [parked, setParked] = useState(false);
  useEffect(() => {
    Object.assign(window, {
      parkTranscript: () => {
        host.remove();
        setParked(true);
      },
      showTranscript: () => {
        root.appendChild(host);
        setParked(false);
      },
    });
  }, []);
  return (
    <AgentTranscript
      blocks={blocks}
      initialTurns={20}
      visible={!parked}
      parked={parked}
      onNavigateReady={(navigate) => {
        Object.assign(window, { findTranscript: navigate });
      }}
    />
  );
}

const botAnswer =
  "A reply arriving in chunks that should glide smoothly as the conversation grows. ".repeat(
    35,
  );

function BotFixture() {
  const history = blocks.slice(-12);
  const prompt: Block = {
    id: "bot-prompt",
    role: "user",
    text: "Explain this in detail",
  };
  const [busy, setBusy] = useState(true);
  const [conversation, setConversation] = useState<Block[]>([
    ...history,
    prompt,
    { id: "bot-answer", role: "assistant", text: botAnswer, streaming: true },
  ]);
  useEffect(() => {
    Object.assign(window, {
      completeBotReply: (text = botAnswer) => {
        setConversation([
          ...history,
          prompt,
          { id: "bot-answer", role: "assistant", text, streaming: false },
        ]);
        setBusy(false);
      },
    });
  }, []);
  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="min-h-0 flex-1">
        <AgentTranscript
          blocks={conversation}
          busy={busy}
          agentName="Bot"
          agentMascot={{ mascot: "crab", color: "#aaf" }}
          bottomAligned
          inlineWork
        />
      </div>
      <BotComposer
        sessionId="browser-bot"
        name="Bot"
        enabled={false}
        onSubmit={() => false}
        controls={<span className="h-6.5">Model · Effort · Context</span>}
      />
    </div>
  );
}

function BotCompletionFixture() {
  const history = new URLSearchParams(location.search).has("short")
    ? []
    : blocks.slice(-12);
  const start = Date.now();
  const launched = {
    sessionId: "app-bot-review",
    cwd: "/repo",
    title: "Review the changes",
    harness: "codex" as const,
    model: "gpt-6",
  };
  const original: Block[] = [
    ...history,
    {
      id: "bot-request",
      role: "user",
      text: "Start a review session and report back",
      startedAt: start,
      durationMs: 1000,
      botSpawnedSessions: [launched],
    },
    {
      id: "bot-launch",
      role: "tool",
      text: "Start a session",
      tool: { kind: "execute", status: "completed" },
    },
    {
      id: "bot-handoff",
      role: "assistant",
      text: "I started the review session. I’ll report back when it finishes.",
    },
  ];
  const completion: Block = {
    id: "bot-completion",
    role: "user",
    text: "Review complete",
    internal: true,
    appRequestId: "bot-completion-batch-review",
    startedAt: start + 5000,
  };
  const [conversation, setConversation] = useState(original);
  const [notifications, setNotifications] = useState<QueuedMessage[]>([]);
  const [session] = useState(() => newSession("codex", "/repo"));
  const [busy, setBusy] = useState(false);
  const [state, setState] = useState<"working" | "done">("working");
  useEffect(() => {
    Object.assign(window, {
      queueBotCompletion: () => {
        setNotifications([
          {
            id: completion.id,
            text: completion.text,
            attachments: [],
            botSessionCompletion: {
              sessionId: launched.sessionId,
              title: launched.title,
              status: "completed",
            },
          },
        ]);
        setState("done");
      },
      beginBotCompletion: () => {
        setNotifications([]);
        setConversation([...original, completion]);
        setBusy(true);
        setState("done");
      },
      finishBotCompletion: () => {
        setConversation([
          ...original,
          { ...completion, durationMs: 1000 },
          {
            id: "bot-report",
            role: "assistant",
            text: "The review is complete. Here are the results. ".repeat(40),
          },
        ]);
        setBusy(false);
      },
    });
  }, []);
  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="min-h-0 flex-1">
        <AgentTranscript
          blocks={conversation}
          busy={busy}
          agentName="Bot"
          agentMascot={{ mascot: "crab", color: "#aaf" }}
          bottomAligned
          inlineWork
          onOpenSession={() => {}}
          onShowWork={() => {}}
          launchedSessions={
            new Map([[launched.sessionId, { state, title: launched.title }]])
          }
        />
      </div>
      <MessageQueue
        messages={visibleBotCompletionMessages({
          ...session,
          blocks: conversation,
          busy,
          queuedMessages: notifications,
          queueStatus: "active",
        })}
        variant="messages"
        onResume={() => {}}
      />
      <BotComposer
        sessionId="completion-bot"
        name="Bot"
        enabled={false}
        onSubmit={() => false}
        controls={<span className="h-6.5">Model · Effort · Context</span>}
      />
    </div>
  );
}

const params = new URLSearchParams(location.search);
const bot = params.has("bot");
const completion = params.has("botCompletion");
if (bot || completion) root.style.width = "540px";
createRoot(host).render(
  completion ? <BotCompletionFixture /> : bot ? <BotFixture /> : <Fixture />,
);
