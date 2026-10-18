import {
  listLocalSubmissions,
  restoreLocalSubmissions,
} from "../../../integrations/harness/core/localSubmissions";
import { invoke } from "@tauri-apps/api/core";
import { isBotSession } from "../../bots/model/bot";
import { sanitizeBotSpawnedSessions } from "../../bots/model/botSpawnedSessions";
import {
  isWeakToolTitle,
  titleFromToolInput,
} from "../../../integrations/harness/core/preview";
import { codexCommandPresentation } from "../../../integrations/harness/providers/codex/codexProtocol";
import { persistableAttachment } from "../model/attachments";
import { discardComposerDraft } from "../model/draftCache";
import { ARTIFACT_KINDS } from "../../artifacts/artifacts";
import type { ContextUsage } from "../model/contextUsage";
import {
  isRemoteProjectPath,
  normalizeProjectPath,
} from "../../projects/model/recents";
import {
  claudeShellCommands,
  ompActiveAssistantTexts,
  ompSessionInterjections,
} from "../../../platform/tauri/fs";
import {
  backfillOmpInterjections,
  ompStatusSplitTexts,
} from "../model/ompInterjections";
import type {
  AgentRunMeta,
  AgentStep,
  Block,
  BtwMessage,
  BtwThread,
  GeneratedImageMeta,
  HarnessId,
  HandoffMeta,
  HandoffStatus,
  InterjectionMeta,
  LinkedWorkItem,
  RuntimeMode,
  SecondOpinionMeta,
  Session,
  TaskListMeta,
  PlanBlockMeta,
  QueuedMessage,
  MessageQueueStatus,
  TurnModel,
  TurnMetrics,
} from "../model/session";

import { defaultSessionChoice } from "../model/models";
import { isHarnessId } from "../model/harnessIds";
import { HARNESSES, RUNTIME_MODES } from "../model/session";

import { restoreOrchestrationProposal } from "../../orchestration/model/orchestrationPlan";

import type { OrchestrationSummary } from "../../orchestration/model/orchestrationSummary";

export type SessionSummary = {
  sidebarHidden?: boolean;
  orchestrationLeadId?: string;
  orchestration?: OrchestrationSummary;
  id: string;
  cwd: string;
  harness: HarnessId;
  model: string;
  runtimeMode: RuntimeMode;
  title: string;
  providerSessionId?: string;
  branch?: string;
  worktreeCwd?: string;
  worktreeRemoved?: boolean;
  repo?: string;
  additions?: number;
  deletions?: number;
  createdAt: number;
  updatedAt: number;
  archived?: boolean;
  pinned?: boolean;
  draft?: boolean;
  linkedWorkItem?: LinkedWorkItem;
  automationId?: string;
};

type SessionRecord = {
  sidebarHidden?: boolean;
  botTranscript?: Session["botTranscript"];
  orchestrationLeadId?: string;
  id: string;
  cwd: string;
  harness: string;
  model: string;
  modelSettings: Record<string, string>;
  runtimeMode: string;
  title: string;
  providerSessionId?: string | null;
  providerAccountId?: string | null;
  persistenceRevision?: number;
  blocks: Block[];
  queuedMessages?: QueuedMessage[];
  queueStatus?: MessageQueueStatus;
  contextUsed?: number | null;
  contextWindow?: number | null;
  branch?: string | null;
  worktreeCwd?: string | null;
  worktreeRemoved?: boolean;
  linkedWorkItem?: LinkedWorkItem | null;
  automationId?: string | null;
  createdAt: number;
  updatedAt: number;
};

type SessionUpsertPayload = {
  sidebarHidden?: boolean;
  id: string;
  cwd: string;
  harness: string;
  model: string;
  modelSettings: Record<string, string>;
  runtimeMode: string;
  title: string;
  providerSessionId?: string;
  providerAccountId?: string;
  blocks: Block[];
  queuedMessages?: QueuedMessage[];
  queueStatus?: MessageQueueStatus;
  contextUsed?: number;
  contextWindow?: number;
  branch?: string;
  worktreeCwd?: string;
  worktreeRemoved?: boolean;
  linkedWorkItem?: LinkedWorkItem;
  automationId?: string;
};

/** Only real chats belong in project history — blank tabs stay ephemeral. */
export function shouldPersistSession(session: Session): boolean {
  return (
    !session.ephemeral &&
    !session.inboxAsk &&
    !isRemoteProjectPath(session.cwd) &&
    session.cwd !== "~" &&
    (session.blocks.some((block) => block.role === "user") ||
      (isBotSession(session.id) &&
        (session.blocks.length > 0 || !!session.botTranscript)))
  );
}

/** Matches Rust `validate_id` — a path here fails the whole upsert. */
export function isPersistableId(value: string): boolean {
  return /^[A-Za-z0-9_-]+$/.test(value);
}

function persistableMeta(
  session: Session,
  includeQueue = true,
): Omit<SessionUpsertPayload, "blocks"> {
  const linkedWorkItem = sanitizeLinkedWorkItem(session.linkedWorkItem);
  const queuedMessages = includeQueue
    ? sanitizeQueuedMessages(session.queuedMessages)
    : [];
  return {
    id: session.id,
    cwd: normalizeProjectPath(session.cwd),
    harness: session.harness,
    model: session.model,
    modelSettings: session.modelSettings,
    runtimeMode: session.runtimeMode,
    title: session.title,
    ...(session.sidebarHidden === true ? { sidebarHidden: true } : {}),
    ...(queuedMessages.length
      ? {
          queuedMessages,
          queueStatus: session.queueStatus ?? "active",
        }
      : {}),
    ...(session.providerSessionId && isPersistableId(session.providerSessionId)
      ? { providerSessionId: session.providerSessionId }
      : {}),
    ...(session.providerAccountId && isPersistableId(session.providerAccountId)
      ? { providerAccountId: session.providerAccountId }
      : {}),
    ...(session.context ? { contextUsed: session.context.used } : {}),
    ...(session.context?.window
      ? { contextWindow: session.context.window }
      : {}),
    ...(session.branch ? { branch: session.branch } : {}),
    ...(session.worktreeCwd ? { worktreeCwd: session.worktreeCwd } : {}),
    ...(session.worktreeRemoved ? { worktreeRemoved: true } : {}),
    ...(linkedWorkItem ? { linkedWorkItem } : {}),
    ...(session.automationId && isPersistableId(session.automationId)
      ? { automationId: session.automationId }
      : {}),
  };
}

function sanitizeBotSessionCompletion(
  value: unknown,
): Block["botSessionCompletion"] {
  if (!value || typeof value !== "object") return undefined;
  const completion = value as NonNullable<Block["botSessionCompletion"]>;
  if (
    typeof completion.sessionId !== "string" ||
    !isPersistableId(completion.sessionId) ||
    typeof completion.title !== "string" ||
    !["completed", "failed", "cancelled"].includes(completion.status)
  )
    return undefined;
  return {
    sessionId: completion.sessionId,
    title: completion.title,
    status: completion.status,
    ...(typeof completion.sessionCount === "number" &&
    Number.isInteger(completion.sessionCount) &&
    completion.sessionCount > 1
      ? { sessionCount: completion.sessionCount }
      : {}),
  };
}

function sanitizeBotPeerMessage(value: unknown): Block["botPeerMessage"] {
  if (!value || typeof value !== "object") return undefined;
  const peer = value as NonNullable<Block["botPeerMessage"]>;
  if (
    typeof peer.id !== "string" ||
    !isPersistableId(peer.id) ||
    typeof peer.fromBotId !== "string" ||
    !isPersistableId(peer.fromBotId) ||
    typeof peer.fromName !== "string" ||
    typeof peer.fromMascot !== "string" ||
    typeof peer.fromColor !== "string" ||
    typeof peer.toBotId !== "string" ||
    !isPersistableId(peer.toBotId) ||
    typeof peer.toName !== "string" ||
    typeof peer.text !== "string" ||
    typeof peer.expectReply !== "boolean" ||
    typeof peer.hop !== "number" ||
    !Number.isFinite(peer.hop) ||
    typeof peer.maxHops !== "number" ||
    !Number.isFinite(peer.maxHops) ||
    typeof peer.sentAt !== "number" ||
    !Number.isFinite(peer.sentAt)
  )
    return undefined;
  return {
    id: peer.id,
    fromBotId: peer.fromBotId,
    fromName: peer.fromName,
    fromMascot: peer.fromMascot,
    fromColor: peer.fromColor,
    toBotId: peer.toBotId,
    toName: peer.toName,
    text: peer.text,
    expectReply: peer.expectReply,
    hop: peer.hop,
    maxHops: peer.maxHops,
    sentAt: peer.sentAt,
  };
}

/** Pending images need their bytes until delivery; object URLs never survive reloads. */
function sanitizeQueuedMessages(value: unknown): QueuedMessage[] {
  if (!Array.isArray(value)) return [];
  const seen = new Set<string>();
  return value.flatMap((entry) => {
    if (!entry || typeof entry !== "object") return [];
    const message = entry as QueuedMessage;
    if (
      typeof message.id !== "string" ||
      !isPersistableId(message.id) ||
      seen.has(message.id) ||
      typeof message.text !== "string"
    )
      return [];
    seen.add(message.id);
    const attachments = Array.isArray(message.attachments)
      ? message.attachments.flatMap((file) => {
          if (
            !file ||
            typeof file.id !== "string" ||
            typeof file.name !== "string" ||
            typeof file.mimeType !== "string" ||
            !["image", "audio", "file"].includes(file.kind) ||
            typeof file.size !== "number" ||
            !Number.isFinite(file.size) ||
            file.size < 0
          )
            return [];
          return [
            {
              ...persistableAttachment(file),
              ...(!file.path &&
              file.kind === "image" &&
              typeof file.data === "string"
                ? { data: file.data }
                : {}),
            },
          ];
        })
      : [];
    const noteMeta = sanitizeNoteCard(message.noteCard);
    const completion = sanitizeBotSessionCompletion(
      message.botSessionCompletion,
    );
    const peerMessage = sanitizeBotPeerMessage(message.botPeerMessage);
    const noteCard =
      noteMeta && typeof message.noteCard?.body === "string"
        ? { ...noteMeta, body: message.noteCard.body }
        : undefined;
    const handoff = message.handoffCard;
    const handoffCard =
      handoff &&
      typeof handoff.brief === "string" &&
      HARNESSES.includes(handoff.from) &&
      HARNESSES.includes(handoff.to)
        ? handoff
        : undefined;
    if (
      !message.text.trim() &&
      !attachments.length &&
      !noteCard &&
      !handoffCard
    )
      return [];
    return [
      {
        id: message.id,
        ...(typeof message.blockId === "string" &&
        isPersistableId(message.blockId)
          ? { blockId: message.blockId }
          : {}),
        text: message.text,
        attachments,
        ...(noteCard ? { noteCard } : {}),
        ...(handoffCard ? { handoffCard } : {}),
        ...(completion ? { botSessionCompletion: completion } : {}),
        ...(peerMessage ? { botPeerMessage: peerMessage } : {}),
        ...(["default", "plan", "build", "orchestrate"].includes(
          message.intent ?? "",
        )
          ? { intent: message.intent }
          : {}),
        ...(typeof message.error === "string" && message.error
          ? { error: message.error }
          : {}),
      },
    ];
  });
}

export function sanitizeLinkedWorkItem(
  value: unknown,
): LinkedWorkItem | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return undefined;
  }
  const item = value as Partial<LinkedWorkItem>;
  const kind = item.kind;
  const repo = typeof item.repo === "string" ? item.repo.trim() : "";
  const number = item.number;
  if (
    (kind !== "issue" && kind !== "pr") ||
    !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repo) ||
    typeof number !== "number" ||
    !Number.isSafeInteger(number) ||
    number <= 0
  ) {
    return undefined;
  }
  return {
    kind,
    repo,
    number,
    url: `https://github.com/${repo}/${kind === "pr" ? "pull" : "issues"}/${number}`,
  };
}

export function sanitizeSessionForPersist(
  session: Session,
): SessionUpsertPayload {
  const firstUser = session.blocks.findIndex((block) => block.role === "user");
  return {
    ...persistableMeta(session),
    blocks: session.blocks
      .map((block, index) =>
        sanitizeBlock(
          index === firstUser && session.orchestrationLeadId
            ? { ...block, orchestrationLeadId: session.orchestrationLeadId }
            : block,
        ),
      )
      .filter((block): block is Block => block != null),
  };
}

/**
 * `session_upsert` runs off the main thread, so two writes for the same
 * session could otherwise land in either order and let an older transcript
 * overwrite a newer one. Chain them per session; different sessions still
 * write concurrently.
 */
type BlockSnapshot = { deref(): Block[] | undefined };
const unavailableBlockSnapshot: BlockSnapshot = { deref: () => undefined };

/** Older webviews keep the revision fence and safely resend all block payloads. */
function weakBlockSnapshot(blocks: Block[]): BlockSnapshot {
  const WeakSnapshot = (
    globalThis as unknown as {
      WeakRef?: new (value: Block[]) => BlockSnapshot;
    }
  ).WeakRef;
  return typeof WeakSnapshot === "function"
    ? new WeakSnapshot(blocks)
    : unavailableBlockSnapshot;
}

// Revisions fence other windows/native full writers. Weak snapshots avoid keeping
// every conversation alive merely because it has been saved in this process.
const ordinarySaved = new Map<
  string,
  {
    revision: number;
    blocks: BlockSnapshot;
    persisted: WeakSet<Block>;
    lead?: string;
  }
>();
const sessionWriteQueues = new Map<string, Promise<unknown>>();
const sessionWriteLeadById = new Map<string, string>();
const deletedSessionIds = new Set<string>();

function enqueueSessionWrite<T>(
  sessionId: string,
  operation: () => Promise<T>,
): Promise<T> {
  const previous = sessionWriteQueues.get(sessionId) ?? Promise.resolve();
  const run = previous.catch(() => undefined).then(operation);
  const tail = run.then(
    () => undefined,
    () => undefined,
  );
  sessionWriteQueues.set(sessionId, tail);
  void tail.then(() => {
    if (sessionWriteQueues.get(sessionId) === tail) {
      sessionWriteQueues.delete(sessionId);
      sessionWriteLeadById.delete(sessionId);
    }
  });
  return run;
}

export async function upsertSession(
  session: Session,
): Promise<SessionSummary | null> {
  if (!shouldPersistSession(session) || deletedSessionIds.has(session.id)) {
    return null;
  }
  const bot = !!session.botTranscript || isBotSession(session.id);
  const snapshot = bot
    ? session.blocks.filter((block) => botPersistedBlock(block) !== null)
    : session.blocks;
  // Bot writes sanitize only the changed suffix inside the serialized queue.
  const payload = persistableMeta(session);
  if (session.orchestrationLeadId) {
    sessionWriteLeadById.set(session.id, session.orchestrationLeadId);
  } else {
    sessionWriteLeadById.delete(session.id);
  }
  const summary = await enqueueSessionWrite(session.id, async () => {
    if (deletedSessionIds.has(session.id)) return null;
    if (bot) {
      const previous = botSavedBlocks.get(session.id);
      let same = 0;
      if (previous) {
        while (
          same < previous.length &&
          same < snapshot.length &&
          previous[same] === snapshot[same]
        )
          same++;
      }
      const changed =
        !previous || same !== previous.length || same !== snapshot.length;
      const summary = await invoke<SessionSummary>("bot_session_upsert", {
        session: {
          ...payload,
          blocks: changed
            ? snapshot.slice(same).map((block) => botPersistedBlock(block)!)
            : [],
        },
        afterBlockId: same > 0 ? snapshot[same - 1].id : null,
        fromBlockId:
          same === 0
            ? previous !== undefined
              ? (previous[0]?.id ?? null)
              : (session.botTranscript?.firstBlockId ?? null)
            : null,
        blocksChanged: changed,
        appendOnly: previous?.length === 0,
      });
      botSavedBlocks.set(session.id, snapshot);
      return summary;
    }
    const baseline = ordinarySaved.get(session.id);
    const previous = baseline?.blocks.deref();
    const byId = new Map(previous?.map((block) => [block.id, block]));
    const firstUser = snapshot.findIndex((block) => block.role === "user");
    const ids: string[] = [];
    const blocks: Block[] = [];
    const persisted = new WeakSet<Block>();
    for (const [index, source] of snapshot.entries()) {
      const lead =
        index === firstUser &&
        session.orchestrationLeadId &&
        !deletedSessionIds.has(session.orchestrationLeadId)
          ? session.orchestrationLeadId
          : undefined;
      const same =
        baseline?.persisted.has(source) &&
        byId.get(source.id) === source &&
        (index !== firstUser || baseline?.lead === lead);
      // Identity reuse applies only to blocks actually confirmed in storage.
      // Hydration may add notices that still need their first durable write.
      if (same) {
        ids.push(source.id);
        persisted.add(source);
        continue;
      }
      const owned = lead
        ? { ...source, orchestrationLeadId: lead }
        : source.orchestrationLeadId &&
            deletedSessionIds.has(source.orchestrationLeadId)
          ? { ...source, orchestrationLeadId: undefined }
          : source;
      const clean = sanitizeBlock(owned);
      if (!clean) continue;
      ids.push(clean.id);
      persisted.add(source);
      blocks.push(clean);
    }
    const [saved, revision] = await invoke<[SessionSummary, number]>(
      "session_upsert_delta",
      {
        session: { ...payload, blocks },
        blockIds: ids,
        expectedRevision: baseline?.revision ?? null,
      },
    );
    ordinarySaved.set(session.id, {
      revision,
      blocks: weakBlockSnapshot(snapshot),
      persisted,
      lead: session.orchestrationLeadId,
    });
    return saved;
  });
  return summary ? normalizeSummary(summary) : null;
}

/**
 * Blocks are replaced, never mutated in place, so identity stands in for
 * content. Serializing the session here instead meant a full deep copy and a
 * `JSON.stringify` of the whole transcript — megabytes on a long chat — on the
 * main thread every time a save was considered. Header fields go through
 * `persistableMeta` so a new persisted column cannot be forgotten here.
 */
const blockTokens = new WeakMap<Block, number>();
const queuedMessageTokens = new WeakMap<QueuedMessage, number>();
let lastBlockToken = 0;
let lastQueuedMessageToken = 0;

function blockToken(block: Block): number {
  const seen = blockTokens.get(block);
  if (seen !== undefined) return seen;
  const token = ++lastBlockToken;
  blockTokens.set(block, token);
  return token;
}

export function persistFingerprint(session: Session): string {
  // Pasted image bytes can be megabytes. Like transcript blocks, queue rows
  // are immutable, so their identity detects edits without serializing bytes.
  const queue = (session.queuedMessages ?? []).map((message) => {
    let token = queuedMessageTokens.get(message);
    if (token === undefined) {
      token = ++lastQueuedMessageToken;
      queuedMessageTokens.set(message, token);
    }
    return token;
  });
  return `${JSON.stringify(persistableMeta(session, false))}|${queue.length ? (session.queueStatus ?? "active") : ""}:${queue.join(",")}|${session.orchestrationLeadId ?? ""}|${session.blocks
    .map(blockToken)
    .join(",")}`;
}

export async function listSessionsByProject(
  cwd: string,
): Promise<SessionSummary[]> {
  if (!cwd || cwd === "~") return [];
  const rows = await invoke<SessionSummary[]>("session_list_by_project", {
    cwd: normalizeProjectPath(cwd),
  });
  return rows.map(normalizeSummary);
}

export function rebaseProjectSessions(
  fromCwd: string,
  toCwd: string,
): Promise<void> {
  return invoke<void>("session_rebase_project", {
    fromCwd: normalizeProjectPath(fromCwd),
    toCwd: normalizeProjectPath(toCwd),
  });
}

export async function listLinkedSessions(): Promise<SessionSummary[]> {
  const rows = await invoke<SessionSummary[]>("session_list_linked");
  return rows.map(normalizeSummary);
}

export type SessionSearchHit = {
  kind: "conversation" | "message";
  sessionId: string;
  cwd: string;
  harness: string;
  title: string;
  updatedAt: number;
  blockId?: string;
  role?: string;
  preview: string;
};

export type SessionSearchResult = {
  hits: SessionSearchHit[];
  truncated: boolean;
};

export async function searchSessions(options: {
  query: string;
  searchOwner: string;
  cwd?: string;
  includeArchived?: boolean;
}): Promise<SessionSearchResult> {
  const query = options.query.trim();
  if (!query) return { hits: [], truncated: false };
  const result = await invoke<SessionSearchResult>("session_search", {
    options: {
      query,
      searchOwner: options.searchOwner,
      ...(options.cwd && options.cwd !== "~"
        ? { cwd: normalizeProjectPath(options.cwd) }
        : {}),
      ...(options.includeArchived ? { includeArchived: true } : {}),
    },
  });
  return {
    hits: Array.isArray(result?.hits) ? result.hits : [],
    truncated: !!result?.truncated,
  };
}

export function cancelSessionSearch(searchOwner: string): Promise<void> {
  return invoke<void>("cancel_session_search", { searchOwner });
}

const botSavedBlocks = new Map<string, Block[]>();
const botBlockPayloads = new WeakMap<Block, Block | null>();
function botPersistedBlock(block: Block): Block | null {
  if (botBlockPayloads.has(block)) return botBlockPayloads.get(block)!;
  const payload = sanitizeBlock(block);
  botBlockPayloads.set(block, payload);
  return payload;
}

export const BOT_PAGE_TURNS = 10;
export type BotTranscriptPage = {
  blocks: Block[];
  before: number | null;
  hasNewer: boolean;
};

export async function getBotTranscriptPage(
  sessionId: string,
  options: {
    before?: number;
    beforeBlockId?: string;
    aroundBlockId?: string;
  } = {},
): Promise<BotTranscriptPage> {
  const page = await invoke<BotTranscriptPage>("bot_session_page", {
    sessionId,
    before: options.before ?? null,
    beforeBlockId: options.beforeBlockId ?? null,
    aroundBlockId: options.aroundBlockId ?? null,
  });
  return {
    ...page,
    blocks: page.blocks
      .map((block) => sanitizeBlock(block, { hydrate: true }))
      .filter((block): block is Block => !!block),
  };
}

export function findBotTranscript(
  sessionId: string,
  query: string,
): Promise<string[]> {
  return invoke("bot_session_find", { sessionId, query });
}

export async function getSession(sessionId: string): Promise<Session | null> {
  const session = await readSession(sessionId);
  return session ? repairSession(session) : null;
}

/** Read the saved conversation without waiting for optional provider-log repairs. */
export async function readSession(sessionId: string): Promise<Session | null> {
  const record = await invoke<SessionRecord | null>(
    isBotSession(sessionId) ? "bot_session_get" : "session_get",
    {
      sessionId,
    },
  );
  if (!record) return null;
  const persisted = new WeakSet<Block>();
  let session = recordToSession(record, persisted);
  const persistedBlocks = session.blocks;
  if (
    typeof window !== "undefined" &&
    "__TAURI_INTERNALS__" in window &&
    !isRemoteProjectPath(session.cwd)
  ) {
    try {
      session = restoreLocalSubmissions(
        session,
        await listLocalSubmissions(session.id),
      );
    } catch {
      session = {
        ...session,
        localSubmissionUncertain: true,
        blocks: [
          ...session.blocks,
          {
            id: "local-submissions-unavailable",
            role: "system",
            notice: "error",
            text: "Local send receipts could not be checked. Automatic continuation is paused; review this conversation before sending another message.",
          },
        ],
      };
    }
  }
  if (session.botTranscript) botSavedBlocks.set(sessionId, persistedBlocks);
  else if (record.persistenceRevision !== undefined)
    ordinarySaved.set(sessionId, {
      revision: record.persistenceRevision,
      blocks: weakBlockSnapshot(persistedBlocks),
      persisted,
      lead: session.orchestrationLeadId,
    });
  else ordinarySaved.delete(sessionId);
  return session;
}

/** Fetched output stays off its block, so saves never ship it back. */
const TOOL_DETAIL_CACHE_CHARS = 2_000_000;
const toolDetailCache = new Map<string, string | null>();
let toolDetailCacheChars = 0;

/**
 * Output a loaded transcript omitted (`detailOmitted`), read when its row is
 * expanded. `stepId` names a step inside the block's subagent run.
 */
export async function loadToolDetail(
  sessionId: string,
  blockId: string,
  stepId?: string,
): Promise<string | null> {
  const key = `${sessionId}\0${blockId}\0${stepId ?? ""}`;
  const cached = toolDetailCache.get(key);
  if (cached !== undefined) {
    toolDetailCache.delete(key);
    toolDetailCache.set(key, cached);
    return cached;
  }
  const detail = await invoke<string | null>("session_tool_detail", {
    sessionId,
    blockId,
    stepId: stepId ?? null,
  });
  toolDetailCache.set(key, detail);
  toolDetailCacheChars += detail?.length ?? 0;
  for (const [oldest, value] of toolDetailCache) {
    if (toolDetailCacheChars <= TOOL_DETAIL_CACHE_CHARS) break;
    if (oldest === key) break;
    toolDetailCache.delete(oldest);
    toolDetailCacheChars -= value?.length ?? 0;
  }
  return detail;
}

/** A progressive caller publishes the repair against current state before persisting. */
export async function repairSession(
  source: Session,
  { persist = true }: { persist?: boolean } = {},
): Promise<Session> {
  const session = { ...source };
  if (session.harness === "claude" && session.providerSessionId) {
    const toolIds = shellPlaceholderIds(session.blocks);
    if (toolIds.length) {
      try {
        const commands = await claudeShellCommands(
          session.providerSessionId,
          session.providerAccountId,
          toolIds,
        );
        const blocks = backfillClaudeShellCommands(session.blocks, commands);
        if (blocks !== session.blocks) {
          session.blocks = blocks;
          if (persist) await upsertSession(session);
        }
      } catch {
        // A missing or unreadable Claude transcript must not block the session.
      }
    }
  }
  if (session.harness === "codex") {
    // Relabel from the command already saved on the row. Codex sends it with
    // the item and `shellCommandPreview` stores it as the preview title, so
    // this needs no disk read at all.
    const blocks = backfillCodexShellCommands(session.blocks);
    if (blocks !== session.blocks) {
      session.blocks = blocks;
      // A failed write must not cost the reader the session. The repair stays
      // in memory and the next load retries it.
      if (persist) await upsertSession(session).catch(() => undefined);
    }
  }
  if (session.harness !== "omp" || !session.providerSessionId) {
    return session;
  }
  try {
    const anchors = await ompSessionInterjections(session.providerSessionId);
    // Missing source order must not prevent the existing anchored repair.
    const source = ompStatusSplitTexts(session.blocks).length
      ? await ompActiveAssistantTexts(session.providerSessionId).catch(() => [])
      : [];
    const blocks = backfillOmpInterjections(session.blocks, anchors, source);
    if (blocks !== session.blocks) {
      session.blocks = blocks;
      // Persist before exposing the restored session to a new live turn.
      // Re-reading the source on later loads allows partial repairs to retry;
      // deterministic IDs ensure already repaired transcripts are not written.
      if (persist) await upsertSession(session);
    }
  } catch {
    // Source logs may be absent/unreadable. Even a failed write must not stop
    // restore; the recovered in-memory boundaries can still be displayed.
  }
  return session;
}

/** Never let a delayed repair replace a newer stream, edit, or provider binding. */
export function applySessionRepair(
  current: Session,
  source: Session,
  repaired: Session,
): Session {
  if (
    current.id !== source.id ||
    current.blocks !== source.blocks ||
    current.harness !== source.harness ||
    current.providerSessionId !== source.providerSessionId ||
    current.providerAccountId !== source.providerAccountId ||
    repaired.blocks === source.blocks
  )
    return current;
  return { ...current, blocks: repaired.blocks };
}

export function backfillClaudeShellCommands(
  blocks: Block[],
  commands: Record<string, string>,
): Block[] {
  let changed = false;
  const repaired = blocks.map((block) => {
    const callId = block.tool?.callId;
    const value = callId ? commands[callId] : undefined;
    const command = typeof value === "string" ? value.trim() : undefined;
    if (
      block.role !== "tool" ||
      block.tool?.kind !== "execute" ||
      block.text.trim() !== "Shell" ||
      !command
    ) {
      return block;
    }
    changed = true;
    const title = titleFromToolInput("Bash", "execute", { command });
    return {
      ...block,
      text: title,
      tool: { ...block.tool, title },
    };
  });
  return changed ? repaired : blocks;
}

/** Exec rows that were saved without their command, keyed by their tool call. */
function shellPlaceholderIds(blocks: Block[]): string[] {
  return blocks.flatMap((block) =>
    block.role === "tool" &&
    block.tool?.kind === "execute" &&
    block.text.trim() === "Shell" &&
    block.tool.callId
      ? [block.tool.callId]
      : [],
  );
}

/**
 * Relabel exec rows that were saved without their command.
 *
 * The command is already on the row: Codex sends it with the item, and
 * `shellCommandPreview` stores it as the preview title. Reading it back from
 * there keeps whatever Codex chose to show the user — including anything it
 * redacted — and never re-reads a secret off disk into the transcript store. A
 * row saved without a usable preview has no command left to recover, so it keeps
 * its placeholder label.
 */
export function backfillCodexShellCommands(blocks: Block[]): Block[] {
  let changed = false;
  const repaired = blocks.map((block) => {
    if (
      block.role !== "tool" ||
      block.tool?.kind !== "execute" ||
      block.text.trim() !== "Shell"
    ) {
      return block;
    }
    const saved = block.tool.preview?.title?.trim();
    if (!saved || isWeakToolTitle(saved)) return block;
    changed = true;
    const { title, preview } = codexCommandPresentation({}, saved);
    return {
      ...block,
      text: title,
      tool: {
        ...block.tool,
        title,
        ...(preview ? { preview } : {}),
      },
    };
  });
  return changed ? repaired : blocks;
}

export async function deleteSession(
  sessionId: string,
  imagePaths: string[] = [],
): Promise<void> {
  deletedSessionIds.add(sessionId);
  try {
    // A lead with workers still has writes in flight. Finish those before
    // the deletion transaction strips their ownership metadata.
    const pendingWrites = [...sessionWriteQueues.entries()]
      .filter(
        ([queuedSessionId]) =>
          queuedSessionId === sessionId ||
          sessionWriteLeadById.get(queuedSessionId) === sessionId,
      )
      .map(([, pending]) => pending);
    if (pendingWrites.length > 0) await Promise.all(pendingWrites);
    await enqueueSessionWrite(sessionId, () =>
      invoke<void>("session_delete", { sessionId, imagePaths }),
    );
    discardComposerDraft(sessionId);
    botSavedBlocks.delete(sessionId);
    ordinarySaved.delete(sessionId);
    const tombstone = setTimeout(
      () => deletedSessionIds.delete(sessionId),
      60_000,
    );
    if (typeof tombstone === "object") tombstone.unref();
  } catch (error) {
    deletedSessionIds.delete(sessionId);
    throw error;
  }
}

/** Delete a draft-only record while allowing its still-open blank session id to be saved later. */
export async function discardDraftSessionRecord(
  sessionId: string,
): Promise<void> {
  await enqueueSessionWrite(sessionId, () =>
    invoke<void>("session_delete", {
      sessionId,
      imagePaths: [],
      preserveComposer: true,
    }),
  );
  botSavedBlocks.delete(sessionId);
  ordinarySaved.delete(sessionId);
}

export async function setSessionArchived(
  sessionId: string,
  archived: boolean,
): Promise<void> {
  await enqueueSessionWrite(sessionId, () =>
    invoke<void>("session_set_archived", { sessionId, archived }),
  );
}

export async function setSessionPinned(
  sessionId: string,
  pinned: boolean,
): Promise<void> {
  await invoke<void>("session_set_pinned", { sessionId, pinned });
}

export async function setSessionLinkedWorkItem(
  sessionId: string,
  value: LinkedWorkItem | undefined,
): Promise<void> {
  const linkedWorkItem = sanitizeLinkedWorkItem(value);
  await enqueueSessionWrite(sessionId, () =>
    invoke<void>("session_set_linked_work_item", {
      sessionId,
      linkedWorkItem: linkedWorkItem ?? null,
    }),
  );
}

/** Drain pending saves before a worktree removal changes stored session context. */
export async function flushSessionWrites(): Promise<void> {
  await Promise.all([...sessionWriteQueues.values()]);
}

/**
 * `session_set_in_flight` runs off the main thread, so two replaces could
 * otherwise land in either order and restore a stale busy snapshot.
 */
let inFlightWrite: Promise<unknown> = Promise.resolve();

export async function replaceInFlightSessions(
  refs: { sessionId: string; cwd: string }[],
): Promise<void> {
  const run = inFlightWrite
    .catch(() => undefined)
    .then(() =>
      invoke("session_set_in_flight", {
        sessions: refs.map((ref) => ({
          sessionId: ref.sessionId,
          cwd: normalizeProjectPath(ref.cwd),
        })),
      }),
    );
  inFlightWrite = run;
  await run;
}

/** Kept across Vite reloads; boot must not delete the only copy. */
export async function listInFlightSessions(): Promise<
  { sessionId: string; cwd: string }[]
> {
  const rows = await invoke<{ sessionId: string; cwd: string }[]>(
    "session_list_in_flight",
  );
  return Array.isArray(rows) ? rows : [];
}

/** Destructive: the first window to boot after a quit owns these chats. */
export async function takeInFlightSessions(): Promise<
  { sessionId: string; cwd: string }[]
> {
  const rows = await invoke<{ sessionId: string; cwd: string }[]>(
    "session_take_in_flight",
  );
  return Array.isArray(rows) ? rows : [];
}

/**
 * `workspace_set_snapshot` runs off the main thread, so two saves could
 * otherwise finish out of order and keep an older layout.
 */
let workspaceWrite: Promise<unknown> = Promise.resolve();

export async function saveWorkspaceSnapshot(snapshot: unknown): Promise<void> {
  const run = workspaceWrite
    .catch(() => undefined)
    .then(() => invoke("workspace_set_snapshot", { snapshot }));
  workspaceWrite = run;
  await run;
}

export async function loadWorkspaceSnapshot(): Promise<unknown | null> {
  const raw = await invoke<unknown | null>("workspace_get_snapshot");
  return raw ?? null;
}

function sanitizeBlock(
  block: Block,
  options?: { hydrate?: boolean },
): Block | null {
  const next: Block = {
    id: block.id,
    role: block.role,
    text: block.text,
  };
  if (block.attachments?.length) {
    next.attachments = block.attachments.map(persistableAttachment);
  }
  const image = sanitizeGeneratedImage(block.image);
  if (block.role === "image" && !image) return null;
  if (image) next.image = image;
  if (block.startedAt != null) next.startedAt = block.startedAt;
  if (block.durationMs != null) next.durationMs = block.durationMs;
  if (
    (block.role === "assistant" || block.role === "approval") &&
    typeof block.botHabit?.id === "string" &&
    block.botHabit.id.trim() &&
    typeof block.botHabit.name === "string" &&
    block.botHabit.name.trim() &&
    typeof block.botHabit.at === "number" &&
    Number.isFinite(block.botHabit.at)
  ) {
    next.botHabit = {
      id: block.botHabit.id,
      name: block.botHabit.name,
      at: block.botHabit.at,
    };
  }
  if (block.role === "user" && typeof block.sentAt === "number")
    next.sentAt = block.sentAt;
  const turnModel = sanitizeTurnModel(block.turnModel);
  if (block.role === "user" && turnModel) next.turnModel = turnModel;
  if (block.role === "user" && block.draft) next.draft = true;
  if (block.role === "user" && block.localSubmissionOwned === true)
    next.localSubmissionOwned = true;
  if (block.role === "user" && block.monocode) next.monocode = true;
  if (
    block.role === "user" &&
    (block.intent === "plan" || block.intent === "orchestrate")
  )
    next.intent = block.intent;
  if (
    block.role === "user" &&
    typeof block.appRequestId === "string" &&
    /^[A-Za-z0-9_-]{1,512}$/.test(block.appRequestId)
  )
    next.appRequestId = block.appRequestId;
  if (
    block.role === "user" &&
    typeof block.providerTurnId === "string" &&
    isPersistableId(block.providerTurnId)
  )
    next.providerTurnId = block.providerTurnId;
  if (
    block.role === "user" &&
    typeof block.orchestrationLeadId === "string" &&
    isPersistableId(block.orchestrationLeadId)
  )
    next.orchestrationLeadId = block.orchestrationLeadId;
  // Without this the transcript would show the app's orchestration turns as
  // the user's own after a reload.
  if (block.role === "user" && block.internal) next.internal = true;
  const completion = sanitizeBotSessionCompletion(block.botSessionCompletion);
  if (block.role === "user" && block.internal && completion)
    next.botSessionCompletion = completion;
  const peerMessage = sanitizeBotPeerMessage(block.botPeerMessage);
  if (block.role === "user" && block.internal && peerMessage)
    next.botPeerMessage = peerMessage;
  const spawned = sanitizeBotSpawnedSessions(block.botSpawnedSessions);
  if (block.role === "user" && spawned.length)
    next.botSpawnedSessions = spawned;
  const turnMetrics = sanitizeTurnMetrics(block.turnMetrics);
  if (block.role === "user" && turnMetrics) next.turnMetrics = turnMetrics;
  if (block.tool) next.tool = block.tool;
  if (block.approval?.decided) {
    next.approval = {
      requestId: block.approval.requestId,
      decided: block.approval.decided,
    };
  } else if (block.approval && !block.approval.decided) {
    // Drop stale live approval prompts; request ids don't survive restarts.
    if (block.role === "approval") return null;
  }
  const agentRun = sanitizeAgentRun(block.agentRun);
  if (agentRun) next.agentRun = agentRun;
  const taskList = sanitizeTaskList(block.taskList);
  if (taskList) next.taskList = taskList;
  else if (block.role === "tasks") return null;
  const plan = sanitizePlan(block.plan, block.text);
  if (block.orchestration)
    next.orchestration = restoreOrchestrationProposal(block.orchestration);
  if (plan) next.plan = plan;
  else if (block.role === "plan") {
    next.plan = { status: "ready", originalText: block.text };
  }
  const handoff = sanitizeHandoff(block.handoff);
  if (handoff) next.handoff = handoff;
  else if (block.role === "handoff") return null;
  const secondOpinion = sanitizeSecondOpinion(block.secondOpinion);
  if (secondOpinion) next.secondOpinion = secondOpinion;
  if (block.role === "user") {
    const btwThreads = sanitizeBtwThreads(
      block.btwThreads,
      options?.hydrate === true,
    );
    if (btwThreads) next.btwThreads = btwThreads;
  }
  const noteCard = sanitizeNoteCard(block.noteCard);
  if (noteCard) next.noteCard = noteCard;
  if (Array.isArray(block.artifactCards)) {
    const cards = block.artifactCards.flatMap((card) => {
      if (
        !card ||
        !ARTIFACT_KINDS.includes(card.kind) ||
        typeof card.id !== "string" ||
        !isPersistableId(card.id) ||
        typeof card.title !== "string" ||
        !card.title.trim()
      )
        return [];
      return [
        {
          id: card.id,
          kind: card.kind,
          title: card.title.slice(0, 200),
          ...(typeof card.summary === "string" && card.summary.trim()
            ? { summary: card.summary.slice(0, 280) }
            : {}),
        },
      ];
    });
    if (cards.length) next.artifactCards = cards;
  }
  if (
    block.role === "user" &&
    typeof block.ciContext === "string" &&
    block.ciContext
  ) {
    next.ciContext = block.ciContext;
  }
  // Interjection chrome survives restarts only on system blocks; a malformed
  // payload keeps the ordinary system row rather than losing its body.
  if (block.role === "system") {
    const interjection = sanitizeInterjection(block.interjection);
    if (interjection) next.interjection = interjection;
    if (block.notice === "error" || block.notice === "interrupt") {
      next.notice = block.notice;
    }
  }
  return next;
}

function sanitizeNestedId(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined;
  const id = value.trim();
  if (!id || id.length > 256 || /[\u0000-\u001f]/.test(id)) return undefined;
  return id;
}
function sanitizeStringRecord(
  value: unknown,
): Record<string, string> | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return undefined;
  }
  const next: Record<string, string> = {};
  for (const [key, raw] of Object.entries(value)) {
    const safeKey = sanitizeNestedId(key);
    const safeValue = sanitizeNestedId(raw);
    if (safeKey && safeValue) next[safeKey] = safeValue;
  }
  return Object.keys(next).length > 0 ? next : undefined;
}

function sanitizeTimestamp(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) && value >= 0
    ? value
    : undefined;
}

function sanitizeBtwMessage(value: unknown): BtwMessage | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return undefined;
  }
  const record = value as Record<string, unknown>;
  const id = sanitizeNestedId(record.id);
  const text = typeof record.text === "string" ? record.text : undefined;
  const createdAt = sanitizeTimestamp(record.createdAt);
  const role =
    record.role === "user" || record.role === "assistant"
      ? record.role
      : undefined;
  if (!id || text == null || createdAt == null || !role) return undefined;
  const blocks = Array.isArray(record.blocks)
    ? record.blocks.flatMap((block) => {
        const next = sanitizeBlock(block as Block);
        return next ? [next] : [];
      })
    : [];
  return {
    id,
    role,
    text,
    createdAt,
    ...(blocks.length > 0 ? { blocks } : {}),
  };
}

function sanitizeBtwThreads(
  value: unknown,
  hydrate: boolean,
): BtwThread[] | undefined {
  if (!Array.isArray(value)) return undefined;
  const threads = value.flatMap((entry): BtwThread[] => {
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) {
      return [];
    }
    const record = entry as Record<string, unknown>;
    const id = sanitizeNestedId(record.id);
    const sourceEndBlockId = sanitizeNestedId(record.sourceEndBlockId);
    const createdAt = sanitizeTimestamp(record.createdAt);
    const updatedAt = sanitizeTimestamp(record.updatedAt);
    const status =
      record.status === "running" ||
      record.status === "ready" ||
      record.status === "error"
        ? record.status
        : undefined;
    const messages = Array.isArray(record.messages)
      ? record.messages.flatMap((message) => {
          const next = sanitizeBtwMessage(message);
          return next ? [next] : [];
        })
      : [];
    if (
      !id ||
      !sourceEndBlockId ||
      createdAt == null ||
      updatedAt == null ||
      !status ||
      messages.length === 0
    ) {
      return [];
    }
    const error = typeof record.error === "string" ? record.error.trim() : "";
    const model = typeof record.model === "string" ? record.model.trim() : "";
    const harness =
      typeof record.harness === "string" &&
      record.harness.trim() &&
      HARNESSES.includes(record.harness as HarnessId)
        ? (record.harness as HarnessId)
        : undefined;
    const modelSettings = sanitizeStringRecord(record.modelSettings);
    const providerThreadId = sanitizeNestedId(record.providerThreadId);
    const interrupted = hydrate && status === "running";
    return [
      {
        id,
        sourceEndBlockId,
        createdAt,
        updatedAt,
        status: interrupted ? "error" : status,
        messages,
        ...(harness ? { harness } : {}),
        ...(model ? { model } : {}),
        ...(modelSettings ? { modelSettings } : {}),
        ...(providerThreadId ? { providerThreadId } : {}),
        ...(interrupted
          ? {
              error:
                error ||
                "This by-the-way request was interrupted before reload.",
            }
          : error
            ? { error }
            : {}),
      },
    ];
  });
  return threads.length > 0 ? threads : undefined;
}

function sanitizeGeneratedImage(
  value: unknown,
): GeneratedImageMeta | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return undefined;
  }
  const record = value as Record<string, unknown>;
  const path = typeof record.path === "string" ? record.path.trim() : "";
  const name = typeof record.name === "string" ? record.name.trim() : "";
  const mimeType =
    typeof record.mimeType === "string" ? record.mimeType.trim() : "";
  const size = record.size;
  if (
    !path ||
    !name ||
    !mimeType.startsWith("image/") ||
    typeof size !== "number" ||
    !Number.isSafeInteger(size) ||
    size <= 0
  ) {
    return undefined;
  }
  const alt = typeof record.alt === "string" ? record.alt.trim() : "";
  return {
    path,
    name,
    mimeType,
    size,
    ...(alt ? { alt } : {}),
  };
}

function sanitizeTurnMetrics(value: unknown): TurnMetrics | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return undefined;
  }
  const rec = value as Record<string, unknown>;
  const number = (key: keyof TurnMetrics): number | undefined => {
    const candidate = rec[key];
    return typeof candidate === "number" &&
      Number.isFinite(candidate) &&
      candidate >= 0
      ? candidate
      : undefined;
  };
  const metrics: TurnMetrics = {
    ...(number("inputTokens") != null
      ? { inputTokens: number("inputTokens") }
      : {}),
    ...(number("outputTokens") != null
      ? { outputTokens: number("outputTokens") }
      : {}),
    ...(number("cacheReadTokens") != null
      ? { cacheReadTokens: number("cacheReadTokens") }
      : {}),
    ...(number("cacheWriteTokens") != null
      ? { cacheWriteTokens: number("cacheWriteTokens") }
      : {}),
    ...(number("cacheHitPercent") != null
      ? { cacheHitPercent: number("cacheHitPercent") }
      : {}),
  };
  return Object.keys(metrics).length > 0 ? metrics : undefined;
}

function sanitizeTurnModel(value: unknown): TurnModel | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return undefined;
  }
  const record = value as Record<string, unknown>;
  const harness = record.harness;
  const id = typeof record.id === "string" ? record.id.trim() : "";
  const name = typeof record.name === "string" ? record.name.trim() : "";
  if (
    typeof harness !== "string" ||
    !HARNESSES.includes(harness as HarnessId) ||
    !id ||
    !name
  ) {
    return undefined;
  }
  return { harness: harness as HarnessId, id, name };
}

function sanitizeInterjection(
  value: Block["interjection"],
): InterjectionMeta | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return undefined;
  }
  const record = value as Record<string, unknown>;
  const customType =
    typeof record.customType === "string" ? record.customType.trim() : "";
  if (!customType) return undefined;
  const severity = record.severity;
  return {
    customType,
    ...(severity === "nit" || severity === "concern" || severity === "blocker"
      ? { severity }
      : {}),
  };
}

function sanitizePlan(value: unknown, text: string): PlanBlockMeta | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const record = value as Record<string, unknown>;
  const status = record.status;
  if (
    status !== "streaming" &&
    status !== "ready" &&
    status !== "building" &&
    status !== "built"
  ) {
    return null;
  }
  const key = typeof record.key === "string" ? record.key.trim() : "";
  const originalText =
    typeof record.originalText === "string" ? record.originalText : text;
  const approvedText =
    typeof record.approvedText === "string" ? record.approvedText : "";
  return {
    ...(key ? { key } : {}),
    // A restarted app cannot still be executing this approval.
    status: status === "streaming" || status === "building" ? "ready" : status,
    ...(originalText ? { originalText } : {}),
    ...(approvedText ? { approvedText } : {}),
    ...(record.edited === true ? { edited: true } : {}),
  };
}

/**
 * How much of a delegated run's trail a saved session keeps. Reopening a
 * session is for reading what the subagent concluded, not for replaying every
 * call it made, and a long run would otherwise dominate the snapshot.
 */
const PERSISTED_AGENT_STEPS = 100;

function sanitizeAgentRun(value: unknown): AgentRunMeta | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const record = value as Record<string, unknown>;
  if (!Array.isArray(record.steps)) return null;
  const steps = record.steps.flatMap((entry): AgentStep[] => {
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) return [];
    const row = entry as Record<string, unknown>;
    const id = typeof row.id === "string" ? row.id : "";
    const kind = row.kind;
    if (
      !id ||
      (kind !== "tool" && kind !== "message" && kind !== "reasoning")
    ) {
      return [];
    }
    const text = typeof row.text === "string" ? row.text : "";
    return [
      {
        id,
        kind,
        text,
        ...(typeof row.toolKind === "string" ? { toolKind: row.toolKind } : {}),
        ...(typeof row.status === "string" ? { status: row.status } : {}),
        ...(typeof row.detail === "string" ? { detail: row.detail } : {}),
        ...(row.detailOmitted === true && typeof row.detail !== "string"
          ? { detailOmitted: true }
          : {}),
        ...(row.preview && typeof row.preview === "object"
          ? { preview: row.preview as AgentStep["preview"] }
          : {}),
      },
    ];
  });
  const name = typeof record.name === "string" ? record.name.trim() : "";
  if (!name && steps.length === 0) return null;
  return {
    name: name || "Subagent",
    ...(typeof record.model === "string" && record.model.trim()
      ? { model: record.model.trim() }
      : {}),
    ...(typeof record.agentType === "string" && record.agentType.trim()
      ? { agentType: record.agentType.trim() }
      : {}),
    steps: steps.slice(-PERSISTED_AGENT_STEPS),
  };
}

function sanitizeTaskList(value: unknown): TaskListMeta | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const record = value as Record<string, unknown>;
  if (!Array.isArray(record.items)) return null;
  const items = record.items.flatMap((item) => {
    if (!item || typeof item !== "object" || Array.isArray(item)) return [];
    const row = item as Record<string, unknown>;
    const text = typeof row.text === "string" ? row.text.trim() : "";
    const status = row.status;
    if (
      !text ||
      (status !== "pending" &&
        status !== "in_progress" &&
        status !== "completed" &&
        status !== "cancelled")
    ) {
      return [];
    }
    const id =
      typeof row.id === "string"
        ? row.id.trim()
        : typeof row.id === "number" && Number.isFinite(row.id)
          ? String(row.id)
          : "";
    return [
      { ...(id ? { id } : {}), text, status },
    ] satisfies TaskListMeta["items"];
  });
  if (items.length === 0) return null;
  const key = typeof record.key === "string" ? record.key.trim() : "";
  const providerSessionId =
    typeof record.providerSessionId === "string"
      ? record.providerSessionId.trim()
      : "";
  const explanation =
    typeof record.explanation === "string" ? record.explanation.trim() : "";
  return {
    ...(key ? { key } : {}),
    ...(providerSessionId ? { providerSessionId } : {}),
    ...(explanation ? { explanation } : {}),
    items,
  };
}

function normalizeSummary(summary: SessionSummary): SessionSummary {
  const linkedWorkItem = sanitizeLinkedWorkItem(summary.linkedWorkItem);
  const choice = isHarnessId(summary.harness)
    ? { harness: summary.harness, model: summary.model }
    : defaultSessionChoice(summary.cwd);
  return {
    ...summary,
    harness: choice.harness,
    model: choice.model,
    runtimeMode: asRuntimeMode(summary.runtimeMode),
    providerSessionId: isHarnessId(summary.harness)
      ? summary.providerSessionId
      : undefined,
    ...(summary.branch ? { branch: summary.branch } : {}),
    ...(summary.repo ? { repo: summary.repo } : {}),
    additions: summary.additions ?? 0,
    deletions: summary.deletions ?? 0,
    archived: summary.archived || undefined,
    pinned: summary.pinned || undefined,
    draft: summary.draft || undefined,
    sidebarHidden: summary.sidebarHidden === true || undefined,
    linkedWorkItem,
    ...(typeof summary.automationId === "string" &&
    isPersistableId(summary.automationId)
      ? { automationId: summary.automationId }
      : {}),
  };
}

function recordToSession(
  record: SessionRecord,
  persisted?: WeakSet<Block>,
): Session {
  const blocks = Array.isArray(record.blocks)
    ? record.blocks
        .map((block) => sanitizeBlock(block, { hydrate: true }))
        .filter((block): block is Block => block != null)
    : [];
  // Capture true stored identities before synthetic provider notices are added.
  for (const block of blocks) persisted?.add(block);
  const choice = isHarnessId(record.harness)
    ? { harness: record.harness, model: record.model }
    : defaultSessionChoice(record.cwd);
  const providerRemoved = !isHarnessId(record.harness);
  if (providerRemoved)
    blocks.push({
      id: `provider-removed-${record.id}`,
      role: "system",
      text: `The ${record.harness} CLI is no longer supported. Your conversation history is kept. Choose a supported provider to continue.`,
    });
  const linkedWorkItem = sanitizeLinkedWorkItem(record.linkedWorkItem);
  const queuedMessages = sanitizeQueuedMessages(record.queuedMessages);
  return {
    id: record.id,
    sidebarHidden: record.sidebarHidden === true || undefined,
    cwd: record.cwd,
    harness: choice.harness,
    model: choice.model,
    modelSettings:
      !providerRemoved &&
      record.modelSettings &&
      typeof record.modelSettings === "object"
        ? record.modelSettings
        : {},
    runtimeMode: asRuntimeMode(record.runtimeMode),
    title: record.title,
    blocks,
    // A restart interrupted the active turn. Let the user resume pending work.
    ...(queuedMessages.length
      ? { queuedMessages, queueStatus: "paused" as const }
      : {}),
    ...(record.botTranscript ? { botTranscript: record.botTranscript } : {}),
    busy: false,
    orchestrationLeadId:
      record.orchestrationLeadId ??
      blocks.find(
        (block) =>
          block.orchestrationLeadId && block.orchestrationLeadId !== record.id,
      )?.orchestrationLeadId,
    ...(!providerRemoved && record.providerSessionId
      ? { providerSessionId: record.providerSessionId }
      : {}),
    ...(!providerRemoved && record.providerAccountId
      ? { providerAccountId: record.providerAccountId }
      : {}),
    ...(record.branch ? { branch: record.branch } : {}),
    ...(record.worktreeCwd ? { worktreeCwd: record.worktreeCwd } : {}),
    ...(record.worktreeRemoved ? { worktreeRemoved: true } : {}),
    ...(linkedWorkItem ? { linkedWorkItem } : {}),
    ...(record.automationId && isPersistableId(record.automationId)
      ? { automationId: record.automationId }
      : {}),
    ...(contextFromRecord(record) ?? {}),
  };
}

/**
 * Last known reading from a stored session. The harness re-reports on the next
 * turn, so this only has to survive until then.
 */
function contextFromRecord(
  record: SessionRecord,
): { context: ContextUsage } | undefined {
  const used = record.contextUsed;
  if (typeof used !== "number" || !Number.isFinite(used) || used <= 0) {
    return undefined;
  }
  const window = record.contextWindow;
  return {
    context:
      typeof window === "number" && Number.isFinite(window) && window > 0
        ? { used, window }
        : { used },
  };
}

const HANDOFF_STATUSES: HandoffStatus[] = ["preparing", "ready"];

function sanitizeHandoff(value: Block["handoff"]): HandoffMeta | undefined {
  if (!value) return undefined;
  if (!(HARNESSES as readonly string[]).includes(value.from)) return undefined;
  if (!(HARNESSES as readonly string[]).includes(value.to)) return undefined;
  if (!HANDOFF_STATUSES.includes(value.status)) return undefined;
  const interrupted = value.status === "preparing";
  return {
    from: value.from,
    to: value.to,
    status: "ready",
    pending: interrupted || !!value.pending,
  };
}

function sanitizeSecondOpinion(
  value: Block["secondOpinion"],
): SecondOpinionMeta | undefined {
  if (!value) return undefined;
  if (!(HARNESSES as readonly string[]).includes(value.from)) return undefined;
  if (!(HARNESSES as readonly string[]).includes(value.to)) return undefined;
  const request =
    typeof value.request === "string" ? value.request.trim().slice(0, 240) : "";
  const files =
    typeof value.files === "number" && Number.isFinite(value.files)
      ? Math.max(0, Math.round(value.files))
      : 0;
  return {
    from: value.from,
    to: value.to,
    ...(request ? { request } : {}),
    ...(files > 0 ? { files } : {}),
    ...(value.kind === "handoff" ? { kind: "handoff" as const } : {}),
  };
}

function sanitizeNoteCard(value: Block["noteCard"]): Block["noteCard"] {
  if (!value || typeof value !== "object") return undefined;
  const id = typeof value.id === "string" ? value.id.trim() : "";
  if (!id) return undefined;
  const slug = typeof value.slug === "string" ? value.slug.trim() : "";
  const title = typeof value.title === "string" ? value.title.trim() : "";
  const sourceCwd =
    typeof value.sourceCwd === "string" ? value.sourceCwd.trim() : "";
  return {
    id,
    slug,
    title,
    ...(sourceCwd ? { sourceCwd } : {}),
  };
}

function asRuntimeMode(value: string): RuntimeMode {
  return (RUNTIME_MODES as string[]).includes(value)
    ? (value as RuntimeMode)
    : "supervised";
}
