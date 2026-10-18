import { projectKey, projectName } from "../../../shared/lib/paths";
import {
  loadTabGroupColors,
  loadTabGroupCustomColors,
  loadTabGroupLabels,
  loadTabGroupMascots,
  resolveTabGroupColor,
  resolveTabGroupLabel,
  resolveTabGroupMascot,
  TAB_GROUP_COLORS,
} from "../../workspace/model/tabGroups";
import {
  hasPendingApproval,
  type Block,
  type Session,
} from "../../sessions/model/session";
import {
  PROJECT_MASCOTS,
  projectMascot,
} from "../../projects/model/projectMascots";
import { removeBotBackground } from "./botBackground";
import { migrateBotStorage } from "./botStorageMigration";

/** The project palette's colors, plus indigo for a Bot's ninth preset. */
export const BOT_COLORS = [
  ...TAB_GROUP_COLORS.slice(1),
  "hsl(245 75% 65%)",
] as const;

/**
 * A Bot is a long-lived agent of its own: a name, a mascot and a color, one
 * conversation, and the projects it works on. It is not tied to any one of
 * them; it lives on the rail beside the projects and can be given more.
 */
export type Bot = {
  id: string;
  /** Its conversation; absent until it is first opened. */
  sessionId?: string;
  /** Replaces the mascot's own name. */
  name?: string;
  mascot: string;
  color: string;
  /** Project folders it works on, in the order they were added. */
  projects: string[];
  /** New sessions stay out of the project sidebar unless turned on. */
  showStartedSessionsInSidebar?: boolean;
  /** Files the sessions it starts into a sidebar folder named after it. */
  useSidebarFolders?: boolean;
  /** Superseded by SOUL.md; only read once, to seed it. */
  instructions?: string;
  /**
   * From when each project had its own Bot: the project whose agent folder
   * it inherits. The folder moves under its id the first time it is read.
   */
  legacyProject?: string;
};

const ROSTER_KEY = "monocode:bot-roster";
const BOTS_CHANGED = "monocode:bots-changed";
export const HANDED_KEY = "monocode:bot-handed";

/** Storage from when each project had one Bot, keyed by project. */
const LEGACY_AGENTS_KEY = "monocode:bots";
const LEGACY_PROFILES_KEY = "monocode:bot-profiles";
/** And from before Bots had their name. */
const LEGACY_KEYS: [string, string][] = [
  ["monocode:project-agents", LEGACY_AGENTS_KEY],
  ["monocode:project-agent-profiles", LEGACY_PROFILES_KEY],
  ["monocode:project-agent-handed", HANDED_KEY],
];

function readJson(key: string): unknown {
  try {
    const raw = localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as unknown) : undefined;
  } catch {
    return undefined;
  }
}

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

let migrated = false;

/**
 * Turns each project's Bot into one of its own that works on that project,
 * keeping its conversation, name and look. One never claimed is dropped.
 */
export function migrateLegacyBotStorage(): void {
  migrateBotStorage();
  if (migrated) return;
  migrated = true;
  try {
    for (const [legacy, key] of LEGACY_KEYS) {
      const value = localStorage.getItem(legacy);
      if (value == null) continue;
      if (localStorage.getItem(key) == null) localStorage.setItem(key, value);
      localStorage.removeItem?.(legacy);
    }
    if (localStorage.getItem(ROSTER_KEY) != null) return;
    const agents = record(readJson(LEGACY_AGENTS_KEY));
    const profiles = record(readJson(LEGACY_PROFILES_KEY));
    const mascots = loadTabGroupMascots();
    const colors = loadTabGroupColors();
    const customColors = loadTabGroupCustomColors();
    const roster: Bot[] = [];
    for (const project of new Set([
      ...Object.keys(agents),
      ...Object.keys(profiles),
    ])) {
      const sessionId = agents[project];
      const profile = record(profiles[project]);
      if (typeof sessionId !== "string" && profile.claim !== "claimed")
        continue;
      const seed = projectName(project);
      roster.push({
        id: newBotId(),
        ...(typeof sessionId === "string" ? { sessionId } : {}),
        ...(typeof profile.name === "string" && profile.name.trim()
          ? { name: profile.name }
          : {}),
        mascot: projectMascot(seed, resolveTabGroupMascot(project, mascots))
          .name,
        color: resolveTabGroupColor(project, colors, customColors, seed),
        projects: [project],
        ...(typeof profile.instructions === "string" &&
        profile.instructions.trim()
          ? { instructions: profile.instructions }
          : {}),
        legacyProject: project,
      });
    }
    localStorage.setItem(ROSTER_KEY, JSON.stringify(roster));
    localStorage.removeItem?.(LEGACY_AGENTS_KEY);
    localStorage.removeItem?.(LEGACY_PROFILES_KEY);
  } catch {
    // Storage unavailable: there is nothing to move either.
  }
}

function newBotId(): string {
  return typeof crypto !== "undefined" && "randomUUID" in crypto
    ? crypto.randomUUID()
    : `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}

function parseBot(value: unknown): Bot | undefined {
  const entry = record(value);
  if (typeof entry.id !== "string" || !/^[A-Za-z0-9-]+$/.test(entry.id))
    return undefined;
  const text = (key: string) =>
    typeof entry[key] === "string" ? (entry[key] as string) : undefined;
  const sessionId = text("sessionId");
  const name = text("name");
  const instructions = text("instructions");
  const legacyProject = text("legacyProject");
  return {
    id: entry.id,
    ...(sessionId ? { sessionId } : {}),
    ...(name ? { name } : {}),
    mascot: text("mascot") ?? PROJECT_MASCOTS[0].name,
    color: text("color") ?? TAB_GROUP_COLORS[1],
    projects: Array.isArray(entry.projects)
      ? entry.projects.filter(
          (path): path is string => typeof path === "string" && !!path,
        )
      : [],
    ...(typeof entry.showStartedSessionsInSidebar === "boolean"
      ? { showStartedSessionsInSidebar: entry.showStartedSessionsInSidebar }
      : {}),
    ...(entry.useSidebarFolders === true ? { useSidebarFolders: true } : {}),
    ...(instructions ? { instructions } : {}),
    ...(legacyProject ? { legacyProject } : {}),
  };
}

/** Every Bot, in the order the rail shows them. */
export function listBots(): Bot[] {
  migrateLegacyBotStorage();
  const parsed = readJson(ROSTER_KEY);
  if (!Array.isArray(parsed)) return [];
  return parsed.flatMap((entry) => parseBot(entry) ?? []);
}

function saveRoster(roster: readonly Bot[]): void {
  try {
    localStorage.setItem(ROSTER_KEY, JSON.stringify(roster));
  } catch {
    return;
  }
  window.dispatchEvent(new CustomEvent(BOTS_CHANGED));
}

export function findBot(id: string): Bot | undefined {
  return listBots().find((bot) => bot.id === id);
}

/** The Bot whose conversation this is. */
export function botForSession(sessionId: string): Bot | undefined {
  return listBots().find((bot) => bot.sessionId === sessionId);
}

export function isBotSession(sessionId: string): boolean {
  return !!botForSession(sessionId);
}

/**
 * The mascot and color the next Bot gets: ones the others are not using yet,
 * where any are left.
 */
export function nextBotLook(
  roster: readonly Pick<Bot, "mascot" | "color">[] = listBots(),
): { mascot: string; color: string } {
  const mascots = new Set(roster.map((bot) => bot.mascot));
  const colors = new Set(roster.map((bot) => bot.color));
  const palette = BOT_COLORS;
  return {
    mascot: (
      PROJECT_MASCOTS.find((mascot) => !mascots.has(mascot.name)) ??
      PROJECT_MASCOTS[roster.length % PROJECT_MASCOTS.length]
    ).name,
    color:
      palette.find((color) => !colors.has(color)) ??
      palette[roster.length % palette.length],
  };
}

/** A new Bot with the next look, starting with the projects given, if any. */
export function createBot(projects: readonly string[] = []): Bot {
  const roster = listBots();
  const bot: Bot = {
    id: newBotId(),
    ...nextBotLook(roster),
    projects: uniqueProjects(projects),
  };
  saveRoster([...roster, bot]);
  return bot;
}

const OPEN_BOT_KEY = "monocode:open-bot";

/** The Bot whose conversation was open, so the next launch reopens it. */
export function loadOpenBot(): string | undefined {
  try {
    return localStorage.getItem(OPEN_BOT_KEY) ?? undefined;
  } catch {
    return undefined;
  }
}

export function saveOpenBot(botId: string | undefined): void {
  try {
    if (botId) localStorage.setItem(OPEN_BOT_KEY, botId);
    else localStorage.removeItem(OPEN_BOT_KEY);
  } catch {
    // The next launch opens the workspace instead; harmless.
  }
}

function uniqueProjects(paths: readonly string[]): string[] {
  const seen = new Set<string>();
  return paths.filter((path) => {
    const key = projectKey(path);
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

export function updateBot(
  id: string,
  change: (bot: Bot) => Bot,
): Bot | undefined {
  const roster = listBots();
  const index = roster.findIndex((bot) => bot.id === id);
  if (index < 0) return undefined;
  const next = change(roster[index]);
  if (!next.name?.trim()) delete next.name;
  if (!next.instructions?.trim()) delete next.instructions;
  next.projects = uniqueProjects(next.projects);
  roster[index] = next;
  saveRoster(roster);
  return next;
}

/** Forgets the Bot and its background. Its folder of files stays on disk. */
export function removeBot(id: string): void {
  removeBotBackground(id);
  saveRoster(listBots().filter((bot) => bot.id !== id));
}

/**
 * Puts the given Bots in this order. Given only some, as a project's list
 * is, they swap among their own places and the rest stay where they are.
 */
export function reorderBots(ids: readonly string[]): void {
  const roster = listBots();
  const byId = new Map(roster.map((bot) => [bot.id, bot]));
  const ordered = ids.flatMap((id) => byId.get(id) ?? []);
  const moved = new Set(ordered.map((bot) => bot.id));
  let next = 0;
  saveRoster(
    roster.map((bot) => (moved.has(bot.id) ? ordered[next++]! : bot)),
  );
}

export function saveBotSessionId(botId: string, sessionId: string): void {
  updateBot(botId, (bot) => ({ ...bot, sessionId }));
}

export function saveBotName(botId: string, name: string): void {
  updateBot(botId, (bot) => ({ ...bot, name }));
}

export function saveBotMascot(botId: string, mascot: string): void {
  updateBot(botId, (bot) => ({ ...bot, mascot }));
}

export function addBotProject(botId: string, path: string): void {
  updateBot(botId, (bot) => ({
    ...bot,
    projects: [...bot.projects, path],
  }));
}

export function removeBotProject(botId: string, path: string): void {
  const key = projectKey(path);
  updateBot(botId, (bot) => ({
    ...bot,
    projects: bot.projects.filter((project) => projectKey(project) !== key),
  }));
}

/** Whether the Bot has been given this project. */
export function botWorksOn(bot: Pick<Bot, "projects">, path: string) {
  const key = projectKey(path);
  return bot.projects.some((project) => projectKey(project) === key);
}

export function subscribeBots(onChange: () => void): () => void {
  window.addEventListener(BOTS_CHANGED, onChange);
  return () => window.removeEventListener(BOTS_CHANGED, onChange);
}

/**
 * Snapshot for `useSyncExternalStore`; stable while storage is unchanged.
 * Project labels are part of it, since a Bot's look names its projects.
 */
export function botsSnapshot(): string {
  migrateLegacyBotStorage();
  try {
    return `${localStorage.getItem(ROSTER_KEY) ?? ""}|${JSON.stringify(
      loadTabGroupLabels(),
    )}`;
  } catch {
    return "";
  }
}

/** Mascots whose Bot name is shorter than the mascot's. */
const BOT_NAME: Record<string, string> = { mushroom: "Shroom" };

/** The name a Bot goes by until the user names it: "BotCrab", "BotShroom". */
export function defaultBotName(mascot: string): string {
  return `Bot${BOT_NAME[mascot] ?? mascot.charAt(0).toUpperCase() + mascot.slice(1)}`;
}

/** One project a Bot works on, as the rail labels it. */
export type BotProject = { path: string; name: string };

export type BotLook = {
  /** The user's name for it, or its default: "BotInvader", "BotGhost". */
  name: string;
  mascot: string;
  color: string;
  projects: BotProject[];
};

/** How a Bot looks, and the projects it works on by their rail labels. */
export function botLook(bot: Bot): BotLook {
  const labels = loadTabGroupLabels();
  return {
    name: bot.name?.trim() || defaultBotName(bot.mascot),
    mascot: bot.mascot,
    color: bot.color,
    projects: bot.projects.map((path) => ({
      path,
      name: resolveTabGroupLabel(projectKey(path), labels, projectName(path)),
    })),
  };
}

/** Its projects as a phrase: "app", "app and site", "app, site and api". */
export function botProjectsPhrase(projects: readonly BotProject[]): string {
  const names = projects.map((project) => project.name);
  if (names.length <= 1) return names[0] ?? "";
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/** Plain one-line take on a step's title, for the Bot's status. */
function plainLine(text: string): string {
  return text
    .replace(/```[\s\S]*?```/g, " ")
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(/^\s*(?:#{1,6}|>)\s*/gm, "")
    .replace(/[`*_]/g, "")
    .replace(/^\s*[-\d.]+\s+/gm, "")
    .replace(/\s+/g, " ")
    .trim();
}

/** What the agent is doing right now, as the Bots roster shows it. */
export type BotStatus = "working" | "needs-you" | "idle";

export type BotState = {
  status: BotStatus;
  /** Short present-tense note while working or waiting. */
  activity?: string;
};

export const BOT_STATUS_LABEL: Record<BotStatus, string> = {
  working: "Working",
  "needs-you": "Needs you",
  idle: "Idle",
};

/** The agent's state, with the step it is on or the call it is waiting for. */
export function botState(
  session: Pick<
    Session,
    "blocks" | "busy" | "pendingQuestion" | "worktreeRemoved" | "usageLimit"
  >,
): BotState {
  const { blocks, pendingQuestion: question } = session;
  if (!session.worktreeRemoved && question) {
    return {
      status: "needs-you",
      activity: question.title?.trim() || question.questions[0]?.prompt.trim(),
    };
  }
  if (!session.worktreeRemoved && hasPendingApproval(blocks)) {
    const approval = latest(
      blocks,
      (block) => !!block.approval && !block.approval.decided,
    );
    return {
      status: "needs-you",
      activity: approval?.tool?.title
        ? `Approve ${plainLine(approval.tool.title)}`
        : "Waiting for approval",
    };
  }
  if (!session.worktreeRemoved && session.usageLimit) {
    return { status: "needs-you", activity: "Usage limit reached" };
  }
  if (!session.busy) return { status: "idle" };
  const last = blocks[blocks.length - 1];
  if (last?.role === "assistant" && !last.tool && last.streaming) {
    return { status: "working", activity: "Writing a reply" };
  }
  // Only a step from the current turn says what it is doing now.
  for (let i = blocks.length - 1; i >= 0; i--) {
    const block = blocks[i];
    if (block.role === "user") break;
    if (block.tool?.title) {
      return { status: "working", activity: plainLine(block.tool.title) };
    }
  }
  return { status: "working", activity: "Thinking" };
}

function latest(
  blocks: readonly Block[],
  match: (block: Block) => boolean,
): Block | undefined {
  for (let i = blocks.length - 1; i >= 0; i--) {
    if (match(blocks[i])) return blocks[i];
  }
  return undefined;
}
