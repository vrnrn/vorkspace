// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createBot, saveBotName } from "./bot";
import {
  addChannelReaction,
  buildChannelTurnPrompt,
  channelForMemberSession,
  cleanBotCollaborativeReply,
  createBotChannel,
  enqueueBotPeerMessage,
  findBotChannel,
  isBotPassReply,
  listBotChannels,
  listBotPeerSummaries,
  markChannelCursor,
  MAX_CHANNEL_AUTONOMOUS_HOPS,
  MAX_PEER_AUTONOMOUS_HOPS,
  botPeersAndChannelsBrief,
  parseChannelMentions,
  postChannelMessage,
  removeBotChannel,
  resolveBotPeer,
  setChannelMemberSession,
} from "./botChannels";
import { canSteerQueuedHead } from "../../sessions/model/messageQueue";
import { newSession } from "../../sessions/model/session";
import {
  handleAgentApp as handleAgentAppRaw,
  type AgentAppHost,
} from "../../agent-app/model/agentApp";

const handleAgentApp: typeof handleAgentAppRaw = async (
  session,
  requestId,
  action,
  input,
  host,
) => {
  const mappedAction = action.startsWith("bots.")
    ? action.replace(/^bots\./, "bots.")
    : action;
  const mappedInput =
    input && typeof input === "object" && "bot" in input
      ? { ...input, bot: input.bot }
      : input;
  let res: unknown;
  try {
    res = await handleAgentAppRaw(
      session,
      requestId,
      action,
      input,
      host,
    );
  } catch (err) {
    if (String(err).includes("Unknown action") && mappedAction !== action) {
      res = await handleAgentAppRaw(
        session,
        requestId,
        mappedAction,
        mappedInput as Record<string, unknown>,
        host,
      );
    } else {
      throw err;
    }
  }
  if (res && typeof res === "object") {
    if ("bots" in res && !("bots" in res)) {
      return { ...res, bots: (res as Record<string, unknown>).bots };
    }
    if ("bot" in res && !("bot" in res)) {
      return { ...res, bot: (res as Record<string, unknown>).bot };
    }
  }
  return res;
};

beforeEach(() => {
  const stored = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => stored.get(key) ?? null,
    setItem: (key: string, value: string) => stored.set(key, value),
    removeItem: (key: string) => stored.delete(key),
  });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("botChannels & peer Bot collaboration", () => {
  it("resolves peer Bots by id or case-insensitive name and lists summaries", () => {
    const skull = createBot(["/code/api"]);
    saveBotName(skull.id, "Skull");
    const cypress = createBot(["/code/web"]);
    saveBotName(cypress.id, "Cypress");

    expect(resolveBotPeer("skull")?.id).toBe(skull.id);
    expect(resolveBotPeer("@Cypress")?.id).toBe(cypress.id);
    expect(resolveBotPeer(cypress.id)?.id).toBe(cypress.id);
    expect(resolveBotPeer("Skull", skull.id)).toBeUndefined();

    const peersForSkull = listBotPeerSummaries({ excludeBotId: skull.id });
    expect(peersForSkull).toHaveLength(1);
    expect(peersForSkull[0]).toMatchObject({
      id: cypress.id,
      name: "Cypress",
      projects: [{ name: "web", path: "/code/web" }],
    });
  });

  it("queues direct peer Bot messages idempotently and never steers mid-turn", () => {
    const skull = createBot(["/code/api"]);
    saveBotName(skull.id, "Skull");
    const cypress = createBot(["/code/web"]);
    saveBotName(cypress.id, "Cypress");

    const session = {
      ...newSession("claude", "/code/web"),
      id: cypress.id,
      busy: true,
      turnReady: true,
    };

    const first = enqueueBotPeerMessage(session, {
      fromBotId: skull.id,
      toBotId: cypress.id,
      text: "What auth header does web send?",
      requestId: "req-1",
      expectReply: true,
      hop: 1,
    });
    expect(first.alreadyQueued).toBe(false);
    expect(first.session.queuedMessages).toHaveLength(1);
    expect(first.session.queuedMessages?.[0]?.botPeerMessage).toMatchObject({
      fromBotId: skull.id,
      fromName: "Skull",
      expectReply: true,
      hop: 1,
      requestId: "req-1",
    });

    // Peer messages must wait for an idle turn rather than steering mid-reply.
    expect(canSteerQueuedHead(first.session)).toBe(false);

    // Retrying with the same requestId is idempotent.
    const duplicate = enqueueBotPeerMessage(first.session, {
      fromBotId: skull.id,
      toBotId: cypress.id,
      text: "What auth header does web send?",
      requestId: "req-1",
      expectReply: true,
      hop: 1,
    });
    expect(duplicate.alreadyQueued).toBe(true);
    expect(duplicate.session.queuedMessages).toHaveLength(1);
  });

  it("parses @mentions and routes user vs Bot channel messages with hop limits", () => {
    const skull = createBot(["/code/api"]);
    saveBotName(skull.id, "Skull");
    const cypress = createBot(["/code/web"]);
    saveBotName(cypress.id, "Cypress");
    const boba = createBot(["/code/infra"]);
    saveBotName(boba.id, "Boba");

    const channel = createBotChannel({
      name: "Backend Sync",
      topic: "API & Web coordination",
      memberIds: [skull.id, cypress.id, boba.id],
    });
    expect(channel.name).toBe("backend-sync");
    expect(listBotChannels()).toHaveLength(1);

    // 1. User message without @mentions wakes all member Bots in roster order.
    const broadcast = postChannelMessage({
      channelId: channel.id,
      from: { kind: "user" },
      text: "Status check everyone!",
    });
    expect(broadcast.wakeBotIds).toEqual([skull.id, cypress.id, boba.id]);

    // 2. User message with specific @mention wakes only the mentioned Bot(s).
    const targeted = postChannelMessage({
      channelId: channel.id,
      from: { kind: "user" },
      text: "Hey @Cypress can you check the login form?",
    });
    expect(targeted.wakeBotIds).toEqual([cypress.id]);

    // 3. Bot message wakes only explicitly @mentioned peers (never itself).
    const botToBot = postChannelMessage({
      channelId: channel.id,
      from: { kind: "bot", botId: cypress.id },
      text: "@Skull I need the new `/v2/session` schema.",
      hop: 1,
    });
    expect(botToBot.wakeBotIds).toEqual([skull.id]);

    // 4. Bot message without @mentions does not wake other Bots (prevents loops).
    const botSummary = postChannelMessage({
      channelId: channel.id,
      from: { kind: "bot", botId: skull.id },
      text: "Here is the schema summary for the room.",
      hop: 2,
    });
    expect(botSummary.wakeBotIds).toEqual([]);

    // 5. Bot message at MAX_CHANNEL_AUTONOMOUS_HOPS does not wake further Bots.
    const capped = postChannelMessage({
      channelId: channel.id,
      from: { kind: "bot", botId: skull.id },
      text: "@Boba please deploy this.",
      hop: MAX_CHANNEL_AUTONOMOUS_HOPS,
    });
    expect(capped.wakeBotIds).toEqual([]);
  });

  it("tracks per-Bot unread channel cursors and backing sessions", () => {
    const skull = createBot(["/code/api"]);
    saveBotName(skull.id, "Skull");
    const cypress = createBot(["/code/web"]);
    saveBotName(cypress.id, "Cypress");

    const channel = createBotChannel({
      name: "release-room",
      memberIds: [skull.id, cypress.id],
    });
    setChannelMemberSession(channel.id, skull.id, "backing-skull-1");
    expect(channelForMemberSession("backing-skull-1")).toEqual({
      channel: expect.objectContaining({ id: channel.id }),
      botId: skull.id,
    });

    const first = postChannelMessage({
      channelId: channel.id,
      from: { kind: "user" },
      text: "@Skull check tests",
    });
    const turn1 = buildChannelTurnPrompt(
      findBotChannel(channel.id)!,
      skull.id,
      1,
    );
    expect(turn1.prompt).toContain("#release-room");
    expect(turn1.prompt).toContain("@Skull check tests");
    expect(turn1.lastMessageId).toBe(first.message.id);

    markChannelCursor(channel.id, skull.id, first.message.id);
    const second = postChannelMessage({
      channelId: channel.id,
      from: { kind: "bot", botId: cypress.id },
      text: "@Skull frontend is green",
      hop: 1,
    });

    const turn2 = buildChannelTurnPrompt(
      findBotChannel(channel.id)!,
      skull.id,
      2,
    );
    expect(turn2.prompt).toContain("@Skull frontend is green");
    expect(turn2.lastMessageId).toBe(second.message.id);

    addChannelReaction(channel.id, second.message.id, skull.id, "👍");
    expect(
      findBotChannel(channel.id)?.messages.find(
        (m) => m.id === second.message.id,
      )?.reactions,
    ).toEqual([{ emoji: "👍", botId: skull.id, name: "Skull" }]);

    removeBotChannel(channel.id);
    expect(findBotChannel(channel.id)).toBeUndefined();
  });

  it("recognizes <pass/> replies and builds the collaboration brief", () => {
    expect(isBotPassReply("<pass/>")).toBe(true);
    expect(isBotPassReply("PASS")).toBe(true);
    expect(cleanBotCollaborativeReply("<pass/>")).toBe("");
    expect(cleanBotCollaborativeReply("Ship it <pass/>")).toBe("Ship it");

    const skull = createBot(["/code/api"]);
    saveBotName(skull.id, "Skull");
    const cypress = createBot(["/code/web"]);
    saveBotName(cypress.id, "Cypress");
    createBotChannel({
      name: "arch",
      topic: "Architecture",
      memberIds: [skull.id, cypress.id],
    });

    const brief = botPeersAndChannelsBrief(skull.id);
    expect(brief).toContain("<bot_collaboration_rules>");
    expect(brief).toContain("Cypress");
    expect(brief).toContain("#arch");
    expect(brief).toContain("app bots.send");
    expect(brief).toContain("app channels.post");
  });

  it("handles bots.* and channels.* CLI actions through handleAgentApp", async () => {
    const skull = createBot(["/code/api"]);
    saveBotName(skull.id, "Skull");
    const cypress = createBot(["/code/web"]);
    saveBotName(cypress.id, "Cypress");

    const skullSession = {
      ...newSession("claude", "/code/api"),
      id: skull.id,
    };
    const cypressSession = {
      ...newSession("codex", "/code/web"),
      id: cypress.id,
      blocks: [
        { id: "u1", role: "user" as const, text: "Check web build" },
        { id: "a1", role: "assistant" as const, text: "Web build passed." },
      ],
    };

    const sentPeerMessages: unknown[] = [];
    const postedChannels: unknown[] = [];

    const host: AgentAppHost = {
      start: async () => {},
      sessions: async () => [],
      session: async (id) => (id === cypress.id ? cypressSession : null),
      send: async () => ({ alreadySubmitted: false }),
      draft: async () => ({ alreadySaved: false, draft: true }),
      stop: async () => {},
      remove: async () => {},
      worktrees: async () => ({ worktrees: [] }),
      createWorktree: async () => ({
        path: "/code/web",
        branch: "main",
        head: "abc",
        main: true,
        locked: false,
        missing: false,
      }),
      notes: async () => [],
      note: async () => null,
      saveNote: async (n) => ({
        id: n.id ?? "n1",
        title: n.title ?? "",
        body: n.body,
        tags: n.tags ?? [],
        createdAt: 1,
        updatedAt: 1,
      }),
      isBot: (id) => id === skull.id || id === cypress.id,
      botOf: (id) =>
        id === skull.id
          ? { id: skull.id, projects: ["/code/api"] }
          : id === cypress.id
            ? { id: cypress.id, projects: ["/code/web"] }
            : undefined,
      bots: {
        session: async (id) => (id === cypress.id ? cypressSession : null),
        send: async (input) => {
          sentPeerMessages.push(input);
          return { alreadyQueued: false };
        },
      },
      channels: {
        currentHop: () => 0,
        onPosted: (channel, message, wakeBotIds) => {
          postedChannels.push({
            channelId: channel.id,
            messageId: message.id,
            wakeBotIds,
          });
        },
      },
      agentFiles: async (id) => ({
        id,
        dir: `/data/${id}`,
        soul: "",
        soulHash: "h1",
        memory: "",
        memoryHash: "m1",
        memoryPath: `/data/${id}/MEMORY.md`,
        topics: [],
      }),
      readAgentFile: async () => ({ text: "", hash: "h1" }),
      writeAgentFile: async () => "h2",
    };

    const listed = (await handleAgentApp(
      skullSession,
      "req-list",
      "bots.list",
      {},
      host,
    )) as { bots: { id: string; name: string }[] };
    expect(listed.bots).toHaveLength(1);
    expect(listed.bots[0]?.name).toBe("Cypress");

    const readResult = (await handleAgentApp(
      skullSession,
      "req-read",
      "bots.read",
      { bot: "Cypress", limit: 2 },
      host,
    )) as {
      bot: { name: string };
      turns: { assistant: { text: string; truncated: boolean } | null }[];
    };
    expect(readResult.bot.name).toBe("Cypress");
    expect(readResult.turns[0]?.assistant?.text).toContain("Web build passed.");

    const sendResult = (await handleAgentApp(
      skullSession,
      "req-send",
      "bots.send",
      { to: "Cypress", message: "Can you verify the CORS origin?" },
      host,
    )) as { queued: boolean; expectReply: boolean; hop: number };
    expect(sendResult).toMatchObject({
      queued: true,
      expectReply: true,
      hop: 1,
    });
    expect(sentPeerMessages).toHaveLength(1);

    // Rejects self-messaging
    await expect(
      handleAgentApp(
        skullSession,
        "req-self",
        "bots.send",
        { to: "Skull", message: "Hi me" },
        host,
      ),
    ).rejects.toThrow(/yourself/);

    // Rejects when hop limit is exceeded
    const exhaustedSession = {
      ...skullSession,
      blocks: [
        {
          id: "p1",
          role: "user" as const,
          text: "Peer prompt",
          botPeerMessage: {
            fromBotId: cypress.id,
            fromName: "Cypress",
            fromMascot: cypress.mascot,
            fromColor: cypress.color,
            text: "hop 4",
            expectReply: false,
            hop: MAX_PEER_AUTONOMOUS_HOPS,
            requestId: "r4",
          },
        },
      ],
    };
    await expect(
      handleAgentApp(
        exhaustedSession,
        "req-over-hop",
        "bots.send",
        { to: "Cypress", message: "One more hop" },
        host,
      ),
    ).rejects.toThrow(/hop limit/);

    // Channels create, post, read, list
    const createdChan = (await handleAgentApp(
      skullSession,
      "req-chan-create",
      "channels.create",
      {
        name: "release-coordination",
        members: ["Cypress"],
        topic: "Release v2",
        message: "@Cypress ready for cut?",
      },
      host,
    )) as {
      channel: { id: string; name: string };
      posted: { wokeBots: { name: string }[] };
    };
    expect(createdChan.channel.name).toBe("release-coordination");
    expect(createdChan.posted.wokeBots).toEqual([
      { id: cypress.id, name: "Cypress" },
    ]);
    expect(postedChannels).toHaveLength(1);

    const postedChan = (await handleAgentApp(
      skullSession,
      "req-chan-post",
      "channels.post",
      {
        channel: "release-coordination",
        message: "Tag v2.0.0 is pushed.",
        to: ["Cypress"],
      },
      host,
    )) as { wokeBots: { name: string }[] };
    expect(postedChan.wokeBots).toEqual([{ id: cypress.id, name: "Cypress" }]);

    const readChan = (await handleAgentApp(
      skullSession,
      "req-chan-read",
      "channels.read",
      { channel: "release-coordination", limit: 10 },
      host,
    )) as { messages: { text: string }[] };
    expect(readChan.messages).toHaveLength(2);

    // Calling channels.create on an existing channel merges new members & updates topic
    const bramble = createBot(["/code/infra"]);
    saveBotName(bramble.id, "Bramble");
    const brambleSession = {
      ...newSession("claude", "/code/infra"),
      id: bramble.id,
    };
    const mergedHost: AgentAppHost = {
      ...host,
      isBot: (id) => id === skull.id || id === cypress.id || id === bramble.id,
      botOf: (id) =>
        id === bramble.id
          ? { id: bramble.id, projects: ["/code/infra"] }
          : host.botOf?.(id),
    };
    const mergedChan = (await handleAgentApp(
      brambleSession,
      "req-chan-merge",
      "channels.create",
      {
        name: "release-coordination",
        topic: "Release v2 + Infra",
      },
      mergedHost,
    )) as {
      alreadyExisted: boolean;
      channel: { topic: string; members: { id: string }[] };
    };
    expect(mergedChan.alreadyExisted).toBe(true);
    expect(mergedChan.channel.topic).toBe("Release v2 + Infra");
    expect(mergedChan.channel.members.map((m) => m.id)).toEqual([
      skull.id,
      cypress.id,
      bramble.id,
    ]);

    // Enforces hop limit on channels.create opening message
    const exhaustedChannelHost: AgentAppHost = {
      ...mergedHost,
      channels: {
        currentHop: () => MAX_CHANNEL_AUTONOMOUS_HOPS,
        onPosted: () => {},
      },
    };
    await expect(
      handleAgentApp(
        skullSession,
        "req-chan-create-over-hop",
        "channels.create",
        {
          name: "release-coordination",
          message: "@Cypress try to bypass hop cap",
        },
        exhaustedChannelHost,
      ),
    ).rejects.toThrow(/hop limit/);

    // Switching between channels and DMs must not reset the autonomy budget.
    await expect(
      handleAgentApp(
        skullSession,
        "req-channel-to-dm-over-hop",
        "bots.send",
        { to: "Cypress", message: "Continue in a DM" },
        exhaustedChannelHost,
      ),
    ).rejects.toThrow(/hop limit/);
    const beforeMessages = findBotChannel(createdChan.channel.id)?.messages
      .length;
    for (const action of ["channels.post", "channels.create"]) {
      await expect(
        handleAgentApp(
          exhaustedSession,
          `req-peer-to-${action.replace(".", "-")}`,
          action,
          action === "channels.post"
            ? {
                channel: "release-coordination",
                message: "Continue in channel",
              }
            : {
                name: "peer-budget-bypass",
                members: ["Cypress"],
                message: "Continue here",
              },
          host,
        ),
      ).rejects.toThrow(/hop limit/);
    }
    expect(findBotChannel("peer-budget-bypass")).toBeUndefined();
    expect(findBotChannel(createdChan.channel.id)?.messages).toHaveLength(
      beforeMessages!,
    );
    const bridged = await handleAgentApp(
      skullSession,
      "req-channel-to-peer-hop",
      "bots.send",
      { to: "Cypress", message: "Continue with remaining budget" },
      { ...host, channels: { ...host.channels, currentHop: () => 2 } },
    );
    expect(bridged).toMatchObject({ hop: 3 });
  });

  it("parses multi-word display names emitted by mention chips without matching shorter prefixes", () => {
    const build = createBot(["/code/core"]);
    saveBotName(build.id, "Build");
    const buildBot = createBot(["/code/ci"]);
    saveBotName(buildBot.id, "Build Bot");

    const parsed = parseChannelMentions(
      "@Build Bot please verify the artifacts.",
      [build, buildBot],
    );
    expect(parsed.botIds).toEqual([buildBot.id]);

    const parsedBoth = parseChannelMentions(
      "@Build and @Build Bot please sync.",
      [build, buildBot],
    );
    expect(parsedBoth.botIds).toEqual([build.id, buildBot.id]);
  });

  it("preserves unread messages that arrive while a Bot turn is in flight", () => {
    const alpha = createBot(["/code/api"]);
    saveBotName(alpha.id, "Alpha");
    const channel = createBotChannel({
      name: "alpha-room",
      memberIds: [alpha.id],
    });

    // 1. User posts m1
    const m1 = postChannelMessage({
      channelId: channel.id,
      from: { kind: "user" },
      text: "First question",
    });

    // 2. Alpha starts turn 1 and reads up to m1
    const turn1 = buildChannelTurnPrompt(
      findBotChannel(channel.id)!,
      alpha.id,
      1,
    );
    expect(turn1.lastMessageId).toBe(m1.message.id);
    expect(turn1.prompt).toContain("First question");

    // 3. While Alpha is running turn 1, User posts m2
    const m2 = postChannelMessage({
      channelId: channel.id,
      from: { kind: "user" },
      text: "@Alpha Second follow-up question",
    });

    // 4. Turn 1 finishes: markChannelCursor records what Turn 1 actually read (m1),
    // and then Alpha posts its Turn 1 reply (m3).
    markChannelCursor(channel.id, alpha.id, turn1.lastMessageId!);
    postChannelMessage({
      channelId: channel.id,
      from: { kind: "bot", botId: alpha.id },
      text: "Answer to first question.",
      hop: 1,
    });

    // Cursor must remain at m1 (not jump to m3), so Turn 2 sees m2!
    const afterTurn1 = findBotChannel(channel.id)!;
    expect(afterTurn1.memberCursors[alpha.id]).toBe(m1.message.id);

    // 5. Turn 2 builds its prompt: sees m2 (and excludes Alpha's own m3 reply)
    const turn2 = buildChannelTurnPrompt(afterTurn1, alpha.id, 1);
    expect(turn2.lastMessageId).toBe(m2.message.id);
    expect(turn2.prompt).toContain("Second follow-up question");
    expect(turn2.prompt).not.toContain("Answer to first question.");
  });
});
