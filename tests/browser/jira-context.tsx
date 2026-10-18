import { useState } from "react";
import { createRoot } from "react-dom/client";
import type { InboxItem } from "../../src/features/inbox/model/githubTasks";
import "../../src/styles/index.css";

const params = new URLSearchParams(location.search);
const provider = params.get("provider") ?? "jira";
const mergeRequest = provider === "gitlab" && params.get("kind") === "pr";
const item: InboxItem = {
  provider: "jira",
  kind: "jira",
  id: "10042",
  identifier: "DEMO-42",
  number: 42,
  title: "Keep ticket context available in every conversation",
  url: "https://demo.atlassian.net/browse/DEMO-42",
  state: "In Progress",
  stateType: "indeterminate",
  updatedAt: "2026-10-14T12:00:00Z",
  labels: [{ name: "integrations", color: "" }],
  assignees: [{ login: "Alex" }],
  draft: false,
  repo: "DEMO",
  teamName: "Demo team",
  projectPath: "",
};

if (provider === "github" || provider === "gitlab") {
  item.provider = provider;
  item.kind = mergeRequest ? "pr" : "issue";
  item.repo = "acme/demo";
  item.identifier = mergeRequest ? "!42" : "#42";
  item.url =
    provider === "github"
      ? "https://github.com/acme/demo/issues/42"
      : `https://gitlab.example.com/acme/demo/-/${mergeRequest ? "merge_requests" : "issues"}/42`;
}
let revision = 1;

Object.assign(window, {
  __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} },
  __TAURI_INTERNALS__: {
    metadata: {
      currentWindow: { label: "main" },
      currentWebview: { label: "main" },
    },
    transformCallback: () => 1,
    unregisterCallback: () => {},
    invoke: async (command: string) => {
      if (command === "gitlab_status")
        return { connected: true, url: "https://gitlab.example.com" };
      if (command === "jira_status")
        return {
          connected: true,
          site: "https://demo.atlassian.net",
          email: "demo@example.com",
        };
      if (
        [
          "jira_issue_details",
          "git_github_work_item_details",
          "gitlab_work_item_details",
        ].includes(command)
      )
        return {
          body: `Dummy description revision ${revision}: allow a ticket to be attached to project chats and Bots.`,
          author: "Alex",
          title: item.title,
          state: item.state,
          ...(mergeRequest
            ? {
                baseRefName: "main",
                headRefName: "feature/context",
                pipelineStatus: "success",
                pipelineUrl: "https://gitlab.example.com/acme/demo/-/pipelines/7",
              }
            : {}),
        };
      if (
        [
          "jira_issue_thread",
          "git_github_work_item_thread",
          "gitlab_work_item_thread",
        ].includes(command)
      )
        return {
          comments: [
            {
              id: "c1",
              kind: "comment",
              state: "",
              path: "",
              line: null,
              resolved: false,
              threadId: "",
              url: "",
              author: "Robin",
              body: `Dummy comment revision ${revision}: please include the newest comments.`,
              createdAt: "2026-10-14T12:00:00Z",
              replies: [],
            },
          ],
          truncated: false,
        };
      return null;
    },
  },
});

const { InboxDetail } = await import("../../src/features/inbox/ui/InboxView");
const { BotComposer } =
  await import("../../src/features/bots/ui/BotComposer");
const { applyTicketContextToTurn, ticketComposerCard } =
  await import("../../src/features/inbox/model/ticketContext");
function Fixture() {
  const [destination, setDestination] = useState("");
  const [card, setCard] = useState<ReturnType<typeof ticketComposerCard>>();
  const [resolved, setResolved] = useState("");
  if (destination)
    return (
      <div className="mx-auto flex h-full max-w-4xl flex-col gap-4 p-6 text-content">
        <div className="flex items-center gap-3">
          <button
            type="button"
            onClick={() => {
              setDestination("");
              setCard(undefined);
            }}
            className="rounded-md bg-content/10 px-3 py-2 text-[12px]"
          >
            Back to ticket
          </button>
          <h1 className="text-[16px] font-semibold">
            {destination === "bot-demo" ? "Demo Bot" : "Demo project chat"}
          </h1>
        </div>
        <p className="text-[12px] text-content/60">
          Dummy data only. No provider message is sent.
        </p>
        <button
          type="button"
          onClick={() => {
            revision++;
          }}
          className="self-start rounded-md bg-content/10 px-3 py-2 text-[12px]"
        >
          Update dummy ticket
        </button>
        <pre
          aria-label="Resolved ticket context"
          className="min-h-0 flex-1 overflow-auto whitespace-pre-wrap break-all text-[12px]"
        >
          {resolved ||
            "Ticket attached. Add a message, then send to inspect the context."}
        </pre>
        <BotComposer
          sessionId="dummy-context-chat"
          name="Demo Bot"
          inboxCard={card}
          onInboxCardDismiss={() => setCard(undefined)}
          onSubmit={async (text) => {
            setResolved(await applyTicketContextToTurn(text, "/tmp/demo"));
            setCard(undefined);
            return true;
          }}
        />
      </div>
    );
  return (
    <InboxDetail
      key={item.provider}
      item={item}
      cwd="/tmp/demo"
      projects={[
        {
          path: "/tmp/demo",
          name: "Demo project",
          logoPath: null,
          mascotName: "crab",
          mascotColor: "hsl(210 70% 60%)",
        },
      ]}
      bots={[
        {
          id: "bot-demo",
          name: "Demo Bot",
          mascot: "crab",
          color: "hsl(210 70% 60%)",
        },
      ]}
      revision={0}
      relatedSessions={[]}
      onStart={(ticket, _body, botId) => {
        setCard(ticketComposerCard(ticket));
        setDestination(botId || ticket.projectPath);
      }}
    />
  );
}
createRoot(document.getElementById("root")!).render(<Fixture />);
