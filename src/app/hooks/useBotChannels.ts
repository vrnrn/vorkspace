import { useCallback, useMemo, useRef, useState } from "react";
import type { ControlOutcome } from "../../features/orchestration/model/orchestration";
import { artifactCards } from "../../features/artifacts/artifacts";
import { findBot, botLook } from "../../features/bots/model/bot";
import {
  addChannelReaction,
  buildChannelTurnPrompt,
  cleanBotCollaborativeReply,
  clearChannelCursor,
  extractBotTurnReply,
  findBotChannel,
  markChannelCursor,
  MAX_CHANNEL_AUTONOMOUS_HOPS,
  postChannelMessage,
  setChannelMemberSession,
  type BotChannel,
  type BotChannelMessage,
} from "../../features/bots/model/botChannels";
import { botReaction as parseBotReaction } from "../../features/bots/model/botReaction";
import { botSpawnedSessions } from "../../features/bots/model/botSpawnedSessions";
import { newSession, type Session } from "../../features/sessions/model/session";

export type BotChannelHost = {
  sessions(): Session[];
  ensureBot(botId: string): Promise<Session | undefined>;
  openSession(sessionId: string): Promise<Session | undefined>;
  addSession(session: Session): void;
  runTurn(sessionId: string, prompt: string): Promise<ControlOutcome>;
  stopSession(sessionId: string): Promise<void>;
};

type QueuedSpeaker = {
  botId: string;
  hop: number;
};

export function useBotChannels(host: BotChannelHost) {
  const hostRef = useRef(host);
  hostRef.current = host;

  const queuesRef = useRef(new Map<string, QueuedSpeaker[]>());
  // Keep ownership until both the turn and any provider cancellation settle.
  const workersRef = useRef(new Set<string>());
  const generationsRef = useRef(new Map<string, number>());
  const stoppingRef = useRef(new Map<string, Promise<void>>());
  const runningSpeakerRef = useRef(new Map<string, string>());
  const activeSessionByChannelRef = useRef(new Map<string, string>());
  const activeHopBySessionRef = useRef(new Map<string, number>());
  const cliPostedInTurnRef = useRef(new Set<string>());

  const [activeByChannel, setActiveByChannel] = useState<
    ReadonlyMap<string, readonly string[]>
  >(() => new Map());

  const syncChannelActiveState = useCallback((channelId: string) => {
    const current = runningSpeakerRef.current.get(channelId);
    const queued = (queuesRef.current.get(channelId) ?? []).map(
      (item) => item.botId,
    );
    const combined = [
      ...(current ? [current] : []),
      ...queued.filter((id) => id !== current),
    ];
    setActiveByChannel((prev) => {
      const next = new Map(prev);
      if (combined.length === 0) next.delete(channelId);
      else next.set(channelId, combined);
      return next;
    });
  }, []);

  const ensureBackingSession = useCallback(
    async (channel: BotChannel, botId: string): Promise<Session | undefined> => {
      const h = hostRef.current;
      const bot = findBot(botId);
      if (!bot) return undefined;
      const botSession = await h.ensureBot(botId);
      if (!botSession) return undefined;

      const existingId = channel.memberSessions[botId];
      if (existingId) {
        const existing =
          h.sessions().find((s) => s.id === existingId) ??
          (await h.openSession(existingId));
        if (existing) return existing;
        // Backing session record was missing from storage; reset the cursor so
        // the newly created backing session replays the channel context.
        clearChannelCursor(channel.id, botId);
      }

      const look = botLook(bot);
      const backing: Session = {
        ...newSession(
          botSession.harness,
          bot.projects[0] ?? botSession.cwd,
          botSession.model,
          botSession.runtimeMode,
          botSession.modelSettings,
        ),
        providerAccountId: botSession.providerAccountId,
        title: `${look.name} · #${channel.name}`,
        sidebarHidden: true,
      };
      setChannelMemberSession(channel.id, botId, backing.id);
      h.addSession(backing);
      return backing;
    },
    [],
  );

  const enqueueChannelTurns = useCallback(
    (channelId: string, botIds: readonly string[], hop: number) => {
      if (hop > MAX_CHANNEL_AUTONOMOUS_HOPS || botIds.length === 0) return;
      const queue = queuesRef.current.get(channelId) ?? [];
      for (const botId of botIds) {
        if (!findBot(botId)) continue;
        // Allow at most one pending follow-up in the queue even if botId is
        // currently the active speaker so concurrent messages are never lost.
        if (queue.some((entry) => entry.botId === botId)) {
          continue;
        }
        queue.push({ botId, hop });
      }
      queuesRef.current.set(channelId, queue);
      syncChannelActiveState(channelId);

      if (workersRef.current.has(channelId)) return;
      workersRef.current.add(channelId);

      void (async () => {
        try {
          while (true) {
            const stopping = stoppingRef.current.get(channelId);
            if (stopping) await stopping.catch(() => undefined);
            const pending = queuesRef.current.get(channelId);
            const next = pending?.shift();
            if (!next) break;
            const generation = generationsRef.current.get(channelId) ?? 0;

            const channel = findBotChannel(channelId);
            const bot = findBot(next.botId);
            if (
              !channel ||
              !bot ||
              !channel.memberIds.includes(next.botId)
            ) {
              syncChannelActiveState(channelId);
              continue;
            }

            runningSpeakerRef.current.set(channelId, next.botId);
            syncChannelActiveState(channelId);

            let backing: Session | undefined;
            try {
              backing = await ensureBackingSession(channel, next.botId);
            } catch {
              backing = undefined;
            }
            if (
              !backing ||
              generation !== (generationsRef.current.get(channelId) ?? 0)
            ) {
              syncChannelActiveState(channelId);
              continue;
            }

            const freshChannel = findBotChannel(channelId);
            if (!freshChannel) break;
            const { prompt, lastMessageId } = buildChannelTurnPrompt(
              freshChannel,
              next.botId,
              next.hop,
            );
            if (!lastMessageId) {
              syncChannelActiveState(channelId);
              continue;
            }

            activeSessionByChannelRef.current.set(channelId, backing.id);
            activeHopBySessionRef.current.set(backing.id, next.hop);
            cliPostedInTurnRef.current.delete(`${backing.id}:${channelId}`);

            const beforeBlockCount =
              hostRef.current
                .sessions()
                .find((s) => s.id === backing.id)?.blocks.length ?? 0;

            let outcome: ControlOutcome;
            try {
              outcome = await hostRef.current.runTurn(backing.id, prompt);
            } catch (error) {
              outcome = {
                status: "failed",
                text: "",
                error: error instanceof Error ? error.message : String(error),
              };
            } finally {
              activeHopBySessionRef.current.delete(backing.id);
              activeSessionByChannelRef.current.delete(channelId);
            }

            // Stop invalidates this turn even if a new message is already queued.
            // A cancelled turn must not post a reply or advance an unread cursor.
            if (generation !== (generationsRef.current.get(channelId) ?? 0)) {
              cliPostedInTurnRef.current.delete(`${backing.id}:${channelId}`);
              continue;
            }

            if (outcome.status !== "completed") {
              cliPostedInTurnRef.current.delete(`${backing.id}:${channelId}`);
              continue;
            }

            // Record the last message this turn actually saw before posting its
            // reply; if new messages arrived concurrently, postChannelMessage
            // preserves this cursor so the queued follow-up turn sees them.
            markChannelCursor(channelId, next.botId, lastMessageId);

            if (
              cliPostedInTurnRef.current.has(`${backing.id}:${channelId}`)
            ) {
              cliPostedInTurnRef.current.delete(`${backing.id}:${channelId}`);
              continue;
            }

            const latestBacking = hostRef.current
              .sessions()
              .find((s) => s.id === backing.id);
            const rawReply =
              extractBotTurnReply(latestBacking) || outcome.text || "";
            const reaction = parseBotReaction(rawReply);
            if (reaction) {
              addChannelReaction(
                channelId,
                lastMessageId,
                next.botId,
                reaction,
              );
              continue;
            }

            const cleaned = cleanBotCollaborativeReply(rawReply);
            if (!cleaned) continue;

            const newBlocks =
              latestBacking?.blocks.slice(beforeBlockCount) ?? [];
            const newArtifacts = artifactCards(newBlocks);
            const newLaunches = botSpawnedSessions(newBlocks);

            const postResult = postChannelMessage({
              channelId,
              from: { kind: "bot", botId: next.botId },
              text: cleaned,
              hop: next.hop,
              turnSessionId: backing.id,
              ...(newArtifacts.length ? { artifacts: newArtifacts } : {}),
              ...(newLaunches.length ? { spawnedSessions: newLaunches } : {}),
            });

            if (
              postResult.wakeBotIds.length > 0 &&
              next.hop + 1 <= MAX_CHANNEL_AUTONOMOUS_HOPS
            ) {
              const currentQueue = queuesRef.current.get(channelId) ?? [];
              for (const targetBotId of postResult.wakeBotIds) {
                if (
                  !currentQueue.some((entry) => entry.botId === targetBotId)
                ) {
                  currentQueue.push({
                    botId: targetBotId,
                    hop: next.hop + 1,
                  });
                }
              }
              queuesRef.current.set(channelId, currentQueue);
              syncChannelActiveState(channelId);
            }
          }
        } finally {
          workersRef.current.delete(channelId);
          runningSpeakerRef.current.delete(channelId);
          activeSessionByChannelRef.current.delete(channelId);
          syncChannelActiveState(channelId);
        }
      })();
    },
    [ensureBackingSession, syncChannelActiveState],
  );

  const sendUserChannelMessage = useCallback(
    (channelId: string, text: string) => {
      const result = postChannelMessage({
        channelId,
        from: { kind: "user" },
        text,
        hop: 0,
      });
      if (result.wakeBotIds.length > 0) {
        enqueueChannelTurns(channelId, result.wakeBotIds, 1);
      }
    },
    [enqueueChannelTurns],
  );

  const stopChannel = useCallback(
    async (channelId: string) => {
      generationsRef.current.set(
        channelId,
        (generationsRef.current.get(channelId) ?? 0) + 1,
      );
      queuesRef.current.delete(channelId);
      const activeSessionId = activeSessionByChannelRef.current.get(channelId);
      runningSpeakerRef.current.delete(channelId);
      activeSessionByChannelRef.current.delete(channelId);
      syncChannelActiveState(channelId);
      if (activeSessionId) {
        const stopping = hostRef.current
          .stopSession(activeSessionId)
          .finally(() => {
            if (stoppingRef.current.get(channelId) === stopping) {
              stoppingRef.current.delete(channelId);
            }
          });
        stoppingRef.current.set(channelId, stopping);
        await stopping;
      }
    },
    [syncChannelActiveState],
  );

  const currentHop = useCallback((sourceSessionId: string): number => {
    return activeHopBySessionRef.current.get(sourceSessionId) ?? 0;
  }, []);

  const onCliChannelPosted = useCallback(
    (
      channel: BotChannel,
      message: BotChannelMessage,
      wakeBotIds: string[],
      sourceSessionId: string,
    ) => {
      cliPostedInTurnRef.current.add(`${sourceSessionId}:${channel.id}`);
      if (wakeBotIds.length > 0 && message.hop <= MAX_CHANNEL_AUTONOMOUS_HOPS) {
        enqueueChannelTurns(channel.id, wakeBotIds, message.hop);
      }
    },
    [enqueueChannelTurns],
  );

  const busyChannelIds = useMemo(
    () => new Set(activeByChannel.keys()),
    [activeByChannel],
  );

  const activeBotIdsForChannel = useCallback(
    (channelId: string): readonly string[] =>
      activeByChannel.get(channelId) ?? [],
    [activeByChannel],
  );

  return {
    busyChannelIds,
    activeBotIdsForChannel,
    sendUserChannelMessage,
    stopChannel,
    currentHop,
    onCliChannelPosted,
    enqueueChannelTurns,
  };
}
