/** Convert persisted feature names before any Bot consumer reads them. */
const VERSION_KEY = "vorkspace.botStorageMigration.v1";
const KEYS = [
  ["monocode:mono-roster", "monocode:bot-roster"],
  ["monocode:mono-handed", "monocode:bot-handed"],
  ["monocode:monos", "monocode:bots"],
  ["monocode:mono-profiles", "monocode:bot-profiles"],
  ["monocode:open-mono", "monocode:open-bot"],
  ["monocode:mono-declined-habits", "monocode:bot-declined-habits"],
  ["monocode:mono-channels", "monocode:bot-channels"],
  ["monocode:open-mono-channel", "monocode:open-bot-channel"],
  ["monocode:mono-rotations", "monocode:bot-rotations"],
  ["monocode:mono-intro-dismissed", "monocode:bot-intro-dismissed"],
  ["monocode.inboxMonos.v1", "monocode.inboxBots.v1"],
  ["monocode.railMonosPinned", "monocode.railBotsPinned"],
  ["monocode.monosEnabled", "monocode.botsEnabled"],
  ["monocode.monoMenuBarIcon", "monocode.botMenuBarIcon"],
] as const;

function object(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === "object" && !Array.isArray(value);
}

function transform(key: string, raw: string): string {
  if (
    key !== "monocode:mono-channels" &&
    !key.startsWith("monocode:mono-channel-messages:")
  )
    return raw;
  const value: unknown = JSON.parse(raw);
  if (!Array.isArray(value)) return raw;
  for (const item of value) {
    if (!object(item)) continue;
    if (key === "monocode:mono-channels") {
      if ("monoIds" in item && !("botIds" in item)) {
        item.botIds = item.monoIds;
        delete item.monoIds;
      }
    } else {
      for (const author of [item.from, item.author]) {
        if (!object(author)) continue;
        if (author.kind === "mono") author.kind = "bot";
        if ("monoId" in author && !("botId" in author)) {
          author.botId = author.monoId;
          delete author.monoId;
        }
      }
      if (Array.isArray(item.reactions)) {
        for (const reaction of item.reactions) {
          if (
            object(reaction) &&
            "monoId" in reaction &&
            !("botId" in reaction)
          ) {
            reaction.botId = reaction.monoId;
            delete reaction.monoId;
          }
        }
      }
    }
  }
  return JSON.stringify(value);
}

function emptyCollection(
  key: string,
  source: string,
  destination: string,
): boolean {
  const arrays = [
    "monocode:mono-roster",
    "monocode:mono-channels",
    "monocode:mono-declined-habits",
  ];
  const maps = [
    "monocode:mono-handed",
    "monocode:monos",
    "monocode:mono-profiles",
    "monocode:mono-rotations",
    "monocode.inboxMonos.v1",
  ];
  try {
    const old: unknown = JSON.parse(source);
    const current: unknown = JSON.parse(destination);
    if (
      arrays.includes(key) ||
      key.startsWith("monocode:mono-channel-messages:")
    )
      return (
        Array.isArray(old) &&
        old.length > 0 &&
        Array.isArray(current) &&
        current.length === 0
      );
    return (
      maps.includes(key) &&
      object(old) &&
      Object.keys(old).length > 0 &&
      object(current) &&
      Object.keys(current).length === 0
    );
  } catch {
    return false;
  }
}

/** Legacy collections remain available for recovery. The marker is written last,
 * so a quota failure retries safely without overwriting newer destination data. */
export function migrateBotStorage(): void {
  try {
    const storage = localStorage;
    if (storage.getItem(VERSION_KEY) === "1") return;
    const keys: [string, string][] = KEYS.map(([old, next]) => [old, next]);
    for (let index = 0; index < storage.length; index++) {
      const key = storage.key(index);
      if (key?.startsWith("monocode:mono-channel-messages:"))
        keys.push([
          key,
          key.replace(
            "monocode:mono-channel-messages:",
            "monocode:bot-channel-messages:",
          ),
        ]);
    }
    for (const [old, next] of keys) {
      const raw = storage.getItem(old);
      if (raw === null) continue;
      const destination = storage.getItem(next);
      if (destination !== null && !emptyCollection(old, raw, destination))
        continue;
      // Leave malformed legacy values untouched; readers already tolerate them.
      let converted = raw;
      try {
        converted = transform(old, raw);
      } catch {
        /* preserve the raw value */
      }
      storage.setItem(next, converted);
    }
    migrateNestedKeys(storage, "monocode:project-chat-backgrounds", (key) =>
      key.startsWith("mono:") ? `bot:${key.slice(5)}` : key,
    );
    migrateNestedKeys(storage, "monocode.keybindingOverrides", (key) =>
      key === "App: Toggle Mono" ? "App: Toggle Bot" : key,
    );
    if (storage.getItem("monocode.settingsSection") === "monos")
      storage.setItem("monocode.settingsSection", "bots");
    storage.setItem(VERSION_KEY, "1");
  } catch {
    // Storage unavailable: do not mark the conversion complete.
  }
}

function migrateNestedKeys(
  storage: Storage,
  key: string,
  rename: (name: string) => string,
) {
  const raw = storage.getItem(key);
  if (raw === null) return;
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return;
  }
  if (!object(value)) return;
  let changed = false;
  for (const name of Object.keys(value)) {
    const next = rename(name);
    if (next === name) continue;
    if (!(next in value)) value[next] = value[name];
    delete value[name];
    changed = true;
  }
  if (changed) storage.setItem(key, JSON.stringify(value));
}
