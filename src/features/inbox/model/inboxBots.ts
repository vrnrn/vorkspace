import { useSyncExternalStore } from "react";

/** Inbox sources that can preselect a Bot in Send to chat. */
export const INBOX_BOT_SOURCES = ["jira", "gitlab"] as const;

export type InboxBotSource = (typeof INBOX_BOT_SOURCES)[number];

export type InboxBots = Partial<Record<InboxBotSource, string>>;

const KEY = "monocode.inboxBots.v1";
const CHANGE = "monocode:inbox-bots-change";
const NONE: InboxBots = Object.freeze({});

let lastRaw: string | null | undefined;
let lastBots: InboxBots = NONE;

function parseInboxBots(raw: string | null): InboxBots {
  try {
    const parsed: unknown = JSON.parse(raw ?? "{}");
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed))
      return NONE;
    const values = parsed as Record<string, unknown>;
    const bots: InboxBots = {};
    for (const source of INBOX_BOT_SOURCES) {
      const botId = values[source];
      if (typeof botId === "string" && botId.trim())
        bots[source] = botId.trim();
    }
    return bots;
  } catch {
    return NONE;
  }
}

/** Saved default Bot IDs. The same object returns until the saved value changes. */
export function loadInboxBots(): InboxBots {
  let raw: string | null;
  try {
    raw = localStorage.getItem(KEY);
  } catch {
    return NONE;
  }
  if (raw !== lastRaw) {
    lastRaw = raw;
    lastBots = parseInboxBots(raw);
  }
  return lastBots;
}

/** Save or clear the Bot preselected as a source's chat destination. */
export function setInboxBot(
  source: InboxBotSource,
  botId: string | null,
): void {
  const bots: InboxBots = { ...loadInboxBots() };
  if (botId?.trim()) bots[source] = botId.trim();
  else delete bots[source];
  try {
    localStorage.setItem(KEY, JSON.stringify(bots));
  } catch {
    return;
  }
  if (typeof window !== "undefined") window.dispatchEvent(new Event(CHANGE));
}

function subscribeInboxBots(listener: () => void): () => void {
  if (typeof window === "undefined") return () => {};
  const onStorage = (event: StorageEvent) => {
    if (event.key === KEY || event.key === null) listener();
  };
  window.addEventListener(CHANGE, listener);
  window.addEventListener("storage", onStorage);
  return () => {
    window.removeEventListener(CHANGE, listener);
    window.removeEventListener("storage", onStorage);
  };
}

export function useInboxBots(): InboxBots {
  return useSyncExternalStore(subscribeInboxBots, loadInboxBots, () => NONE);
}
