import { homeDir } from "../../platform/fs.js";
import {
  execChild,
  killChild,
  listCopilotClaudeModels,
  resolveClaudeBinary,
  spawnChild,
  unwatchChild,
  watchChild,
  writeChild,
} from "../../protocol/child.js";
import {
  asRecord,
  buildClaudeSpawnArgs,
  buildControlRequest,
  compareSemver,
  isClaudeInitMessage,
  listModelsFromControlResponse,
  MINIMUM_CLAUDE_FABLE_5_VERSION,
  MINIMUM_CLAUDE_OPUS_4_7_VERSION,
  MINIMUM_CLAUDE_OPUS_4_8_VERSION,
  MINIMUM_CLAUDE_OPUS_5_5_VERSION,
  MINIMUM_CLAUDE_OPUS_5_VERSION,
  MINIMUM_CLAUDE_SONNET_5_VERSION,
  parseClaudeVersion,
  parseControlResponse,
  parseJsonLine,
  stringField,
} from "./claudeProtocol.js";
const EFFORT_LOW_TO_ULTRATHINK = {
  id: "effort",
  label: "Reasoning",
  kind: "select",
  value: "high",
  options: [
    { value: "low", label: "Low" },
    { value: "medium", label: "Medium" },
    { value: "high", label: "High" },
    { value: "max", label: "Max" },
    { value: "ultrathink", label: "Ultrathink" },
  ],
};
const EFFORT_WITH_XHIGH = {
  id: "effort",
  label: "Reasoning",
  kind: "select",
  value: "high",
  options: [
    { value: "low", label: "Low" },
    { value: "medium", label: "Medium" },
    { value: "high", label: "High" },
    { value: "xhigh", label: "Extra High" },
    { value: "max", label: "Max" },
    {
      value: "ultracode",
      label: "Ultracode",
    },
    { value: "ultrathink", label: "Ultrathink" },
  ],
};
const EFFORT_OPUS_47 = {
  id: "effort",
  label: "Reasoning",
  kind: "select",
  value: "xhigh",
  options: [
    { value: "low", label: "Low" },
    { value: "medium", label: "Medium" },
    { value: "high", label: "High" },
    { value: "xhigh", label: "Extra High" },
    { value: "max", label: "Max" },
    { value: "ultrathink", label: "Ultrathink" },
  ],
};
const FAST_MODE = {
  id: "fast",
  label: "Fast",
  kind: "toggle",
  value: "false",
  options: [
    { value: "true", label: "On" },
    { value: "false", label: "Off" },
  ],
};
const THINKING = {
  id: "thinking",
  label: "Thinking",
  kind: "toggle",
  value: "false",
  options: [
    { value: "true", label: "On" },
    { value: "false", label: "Off" },
  ],
};
function contextWindow(defaultValue) {
  return {
    id: "context",
    label: "Context",
    kind: "select",
    value: defaultValue,
    options: [
      { value: "200k", label: "200k" },
      { value: "1m", label: "1M" },
    ],
  };
}
/** Fallback catalog when `list_models` is unavailable. */
export const CLAUDE_MODEL_CATALOG = [
  {
    id: "claude:fable-5",
    harness: "claude",
    name: "Claude Fable 5",
    nativeId: "claude-fable-5",
    settings: [EFFORT_WITH_XHIGH, contextWindow("1m")],
  },
  {
    id: "claude:opus-5",
    harness: "claude",
    name: "Claude Opus 5",
    nativeId: "claude-opus-5",
    settings: [EFFORT_WITH_XHIGH, FAST_MODE, contextWindow("1m")],
  },
  {
    id: "claude:opus-5-5",
    harness: "claude",
    name: "Claude Opus 5.5",
    nativeId: "claude-opus-5-5",
    settings: [EFFORT_WITH_XHIGH, FAST_MODE, contextWindow("1m")],
  },
  {
    id: "claude:sonnet-5",
    harness: "claude",
    name: "Claude Sonnet 5",
    nativeId: "claude-sonnet-5",
    settings: [EFFORT_WITH_XHIGH, contextWindow("200k")],
  },
  {
    id: "claude:opus-4.8",
    harness: "claude",
    name: "Claude Opus 4.8",
    nativeId: "claude-opus-4-8",
    settings: [EFFORT_WITH_XHIGH, FAST_MODE],
  },
  {
    id: "claude:opus-4.7",
    harness: "claude",
    name: "Claude Opus 4.7",
    nativeId: "claude-opus-4-7",
    settings: [EFFORT_OPUS_47, FAST_MODE],
  },
  {
    id: "claude:opus-4.6",
    harness: "claude",
    name: "Claude Opus 4.6",
    nativeId: "claude-opus-4-6",
    settings: [EFFORT_LOW_TO_ULTRATHINK, FAST_MODE, contextWindow("1m")],
  },
  {
    id: "claude:sonnet-4.6",
    harness: "claude",
    name: "Claude Sonnet 4.6",
    nativeId: "claude-sonnet-4-6",
    settings: [EFFORT_LOW_TO_ULTRATHINK, contextWindow("200k")],
  },
  {
    id: "claude:opus-4.5",
    harness: "claude",
    name: "Claude Opus 4.5",
    nativeId: "claude-opus-4-5",
    settings: [
      {
        id: "effort",
        label: "Reasoning",
        kind: "select",
        value: "high",
        options: [
          { value: "low", label: "Low" },
          { value: "medium", label: "Medium" },
          { value: "high", label: "High" },
          { value: "max", label: "Max" },
        ],
      },
      FAST_MODE,
    ],
  },
  {
    id: "claude:haiku-4.5",
    harness: "claude",
    name: "Claude Haiku 4.5",
    nativeId: "claude-haiku-4-5",
    settings: [THINKING],
  },
];
const PROBE_ID = "vorkspace-claude-probe";
const LIST_MODELS_REQUEST_ID = "vorkspace_list_models";
const INIT_REQUEST_ID = "vorkspace_init";
const DISCOVERY_TIMEOUT_MS = 15000;
const EFFORT_LABELS = {
  low: "Low",
  medium: "Medium",
  high: "High",
  xhigh: "Extra High",
  max: "Max",
};
export async function discoverClaudeModels(workingDirectory) {
  const copilot = listCopilotClaudeModels().catch(() => []);
  const listed = await discoverViaListModels(workingDirectory).catch(
    (error) => {
      console.debug("[vorkspace] claude list_models catalog failed", error);
      return [];
    },
  );
  const claude =
    listed.length > 0 ? listed : await discoverViaVersion(workingDirectory);
  const rows = await copilot;
  if (!Array.isArray(rows)) return claude;
  return claude.concat(
    rows.flatMap((row) => {
      const rec = asRecord(row);
      const id = stringField(rec, "id");
      const name = stringField(rec, "name");
      const levels = advertisedEffortLevels(rec ?? {});
      return id && name
        ? [
            {
              id: `claude:copilot/${id}`,
              harness: "claude",
              name,
              nativeId: `copilot/${id}`,
              provider: { id: "github-copilot", name: "GitHub Copilot" },
              contextWindow:
                typeof rec?.contextWindow === "number"
                  ? rec.contextWindow
                  : undefined,
              settings:
                levels.length > 0
                  ? [
                      {
                        id: "effort",
                        label: "Reasoning",
                        kind: "select",
                        value: levels.includes("medium") ? "medium" : levels[0],
                        options: levels.map((value) => ({
                          value,
                          label:
                            value === "none"
                              ? "None"
                              : (EFFORT_LABELS[value] ?? value),
                        })),
                      },
                    ]
                  : undefined,
            },
          ]
        : [];
    }),
  );
}
async function discoverViaListModels(workingDirectory) {
  const { path } = await resolveClaudeBinary();
  const cwd = workingDirectory ?? (await homeDir());
  const sessionId = crypto.randomUUID();
  const probeId = `${PROBE_ID}-${sessionId}`;
  let listed = null;
  let failed = null;
  const pending = new Promise((resolve, reject) => {
    listed = resolve;
    failed = reject;
  });
  let asked = false;
  const ask = () => {
    if (asked) return;
    asked = true;
    void writeChild(
      probeId,
      JSON.stringify(
        buildControlRequest(LIST_MODELS_REQUEST_ID, { subtype: "list_models" }),
      ),
    ).catch((error) => {
      failed?.(error instanceof Error ? error : new Error(String(error)));
    });
  };
  const stop = async () => {
    unwatchChild(probeId);
    await killChild(probeId).catch(() => undefined);
  };
  watchChild(
    probeId,
    (line) => {
      const rec = parseJsonLine(line);
      if (!rec) return;
      if (isClaudeInitMessage(rec)) ask();
      const init = parseControlResponse(rec);
      if (init?.ok && init.requestId === INIT_REQUEST_ID) ask();
      const rows = listModelsFromControlResponse(rec, LIST_MODELS_REQUEST_ID);
      if (rows) listed?.(modelsFromClaudeListModels(rows));
    },
    () => failed?.(new Error("Claude Code catalog probe exited")),
  );
  try {
    await spawnChild(
      probeId,
      path,
      buildClaudeSpawnArgs({ isolated: true, sessionId }),
      cwd,
      undefined,
      "claude",
    );
    await writeChild(
      probeId,
      JSON.stringify(
        buildControlRequest(INIT_REQUEST_ID, { subtype: "initialize" }),
      ),
    );
    return await withTimeout(DISCOVERY_TIMEOUT_MS, pending, () => {
      void stop();
    });
  } finally {
    await stop();
  }
}
async function discoverViaVersion(workingDirectory) {
  const { path } = await resolveClaudeBinary();
  const cwd = workingDirectory ?? (await homeDir());
  const versionOut = await execChild(path, ["--version"], cwd, "claude");
  const version = parseClaudeVersion(versionOut);
  return modelsForClaudeVersion(version);
}
function withTimeout(ms, promise, onTimeout) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      onTimeout();
      reject(new Error("Claude Code catalog probe timed out"));
    }, ms);
    promise.then(resolve, reject).finally(() => clearTimeout(timer));
  });
}
/** Map a `list_models` payload into the picker catalog. */
export function modelsFromClaudeListModels(raw) {
  const rows = Array.isArray(raw)
    ? raw
    : Array.isArray(asRecord(raw)?.models)
      ? asRecord(raw)?.models
      : [];
  const models = [];
  const seen = new Set();
  for (const item of rows) {
    const model = modelFromListRow(item);
    if (!model) continue;
    const key = model.nativeId ?? model.id;
    if (seen.has(key)) continue;
    seen.add(key);
    models.push(model);
  }
  return models;
}
function modelFromListRow(raw) {
  const rec = asRecord(raw);
  if (!rec) return null;
  if (rec.disabled === true) return null;
  const value = stringField(rec, "value") ?? "";
  if (!value || value === "default" || value.startsWith("cc-update-required")) {
    return null;
  }
  const resolved = stringField(rec, "resolvedModel") ?? "";
  const fromValue = splitClaudeModelValue(value);
  const fromResolved = splitClaudeModelValue(resolved);
  const nativeId = claudeLaunchId(fromValue.id, fromResolved.id);
  if (!nativeId) return null;
  const displayName = stringField(rec, "displayName") ?? "";
  const description = stringField(rec, "description") ?? "";
  const name = pickerName(displayName, description, nativeId, fromResolved.id);
  const settings = settingsFromListRow(
    rec,
    fromValue.context1m || fromResolved.context1m,
  );
  return {
    id: claudeCatalogId(nativeId),
    harness: "claude",
    name,
    nativeId,
    ...(fromResolved.id && fromResolved.id !== nativeId
      ? { aliasTarget: fromResolved.id }
      : {}),
    ...(settings.length > 0 ? { settings } : {}),
  };
}
function settingsFromListRow(rec, context1m) {
  const settings = [];
  const levels = advertisedEffortLevels(rec);
  if (rec.supportsEffort === true || levels.length > 0) {
    settings.push(effortSetting(levels));
  } else if (rec.supportsAdaptiveThinking === true) {
    settings.push(THINKING);
  }
  if (rec.supportsFastMode === true) settings.push(FAST_MODE);
  if (context1m) settings.push(contextWindow("1m"));
  return settings;
}
function advertisedEffortLevels(rec) {
  const raw = rec.supportedEffortLevels;
  if (!Array.isArray(raw)) return [];
  return raw.filter(
    (level) => typeof level === "string" && level.trim() !== "",
  );
}
function effortSetting(levels) {
  const known = levels.filter((level) => EFFORT_LABELS[level]);
  const options = (
    known.length > 0 ? known : ["low", "medium", "high", "max"]
  ).map((value) => ({ value, label: EFFORT_LABELS[value] ?? value }));
  if (options.some((option) => option.value === "xhigh")) {
    options.push({ value: "ultracode", label: "Ultracode" });
  }
  options.push({ value: "ultrathink", label: "Ultrathink" });
  const defaultValue = options.some((option) => option.value === "high")
    ? "high"
    : (options[0]?.value ?? "high");
  return {
    id: "effort",
    label: "Reasoning",
    kind: "select",
    value: defaultValue,
    options,
  };
}
function pickerName(displayName, description, fallback, resolvedModel) {
  const name = displayName.trim();
  const head = description.split("·")[0]?.trim() ?? "";
  let picked = name || head || fallback;
  if (
    head &&
    name &&
    head.toLowerCase().startsWith(name.toLowerCase()) &&
    head.length > name.length
  ) {
    picked = head;
  }
  return qualifyClaudeAliasName(picked, resolvedModel);
}
/**
 * Claude's live catalog can name a moving alias only as "Opus" while also
 * reporting its concrete target as `claude-opus-5-5`. Keep the alias for CLI
 * launches, but include the resolved version in the label so model releases do
 * not silently look like the previous generation.
 */
function qualifyClaudeAliasName(name, resolvedModel) {
  const resolved = resolvedClaudeModelName(resolvedModel);
  if (!resolved) return name;
  const family = escapeRegExp(resolved.family);
  const match = new RegExp(`^(Claude\\s+)?(${family})(.*)$`, "i").exec(name);
  if (!match) return name;
  const suffix = match[3] ?? "";
  // A catalog-supplied version is more authoritative than our interpretation
  // of the concrete id. Otherwise, enrich any generic alias variation.
  if (/^\s+v?\d+(?:[.-]\d+)*/i.test(suffix)) return name;
  return `${match[1] ?? ""}${match[2]} ${resolved.version}${suffix}`;
}
function resolvedClaudeModelName(model) {
  const { id } = splitClaudeModelValue(model);
  if (!id.toLowerCase().startsWith("claude-")) return null;
  const parts = id.slice("claude-".length).split("-");
  const versionStart = parts.findIndex(
    (part) => /^\d+(?:\.\d+)*$/.test(part) && !/^\d{8}$/.test(part),
  );
  if (versionStart <= 0) return null;
  const version = [];
  for (const part of parts.slice(versionStart)) {
    if (!/^\d+(?:\.\d+)*$/.test(part) || /^\d{8}$/.test(part)) break;
    version.push(...part.split("."));
  }
  if (version.length === 0) return null;
  const family = parts
    .slice(0, versionStart)
    .map(
      (part) => `${part[0]?.toUpperCase() ?? ""}${part.slice(1).toLowerCase()}`,
    )
    .join(" ");
  return { family, version: version.join(".") };
}
function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
function splitClaudeModelValue(value) {
  const match = /^(.*)\[1m\]$/i.exec(value.trim());
  if (match?.[1]?.trim()) return { id: match[1].trim(), context1m: true };
  return { id: value.trim(), context1m: false };
}
function claudeCatalogId(nativeId) {
  const slug = nativeId.startsWith("claude-")
    ? nativeId.slice("claude-".length)
    : nativeId;
  return `claude:${slug}`;
}
/**
 * `--model` argument for a `list_models` row.
 *
 * Claude advertises family aliases (`opus`) that must stay bare, and concrete
 * ids that need the `claude-` prefix. A versioned `value` of `opus-5-5` is
 * not a valid CLI model name; prefer `resolvedModel` when it is the full id,
 * otherwise restore the prefix.
 */
function claudeLaunchId(valueId, resolvedId) {
  const nativeId = valueId || resolvedId;
  if (!nativeId) return "";
  if (
    nativeId.startsWith("claude-") ||
    nativeId.includes("/") ||
    !/\d/.test(nativeId)
  )
    return nativeId;
  return resolvedId.startsWith("claude-") ? resolvedId : `claude-${nativeId}`;
}
export function modelsForClaudeVersion(version) {
  return CLAUDE_MODEL_CATALOG.filter((model) => {
    const slug = model.nativeId ?? "";
    if (slug === "claude-opus-5-5") {
      return version
        ? compareSemver(version, MINIMUM_CLAUDE_OPUS_5_5_VERSION) >= 0
        : false;
    }
    if (slug === "claude-opus-5") {
      return version
        ? compareSemver(version, MINIMUM_CLAUDE_OPUS_5_VERSION) >= 0
        : false;
    }
    if (slug === "claude-sonnet-5") {
      return version
        ? compareSemver(version, MINIMUM_CLAUDE_SONNET_5_VERSION) >= 0
        : false;
    }
    if (slug === "claude-fable-5") {
      return version
        ? compareSemver(version, MINIMUM_CLAUDE_FABLE_5_VERSION) >= 0
        : false;
    }
    if (slug === "claude-opus-4-8") {
      return version
        ? compareSemver(version, MINIMUM_CLAUDE_OPUS_4_8_VERSION) >= 0
        : false;
    }
    if (slug === "claude-opus-4-7") {
      return version
        ? compareSemver(version, MINIMUM_CLAUDE_OPUS_4_7_VERSION) >= 0
        : false;
    }
    return true;
  });
}
