import { homeDir } from "../../platform/fs.js";
import {
  killChild,
  resolveCodexBinary,
  spawnChild,
  unwatchChild,
  watchChild,
} from "../../protocol/child.js";
import { asRecord, stringField } from "./codexProtocol.js";
import { JsonRpcClient } from "../../protocol/jsonRpc.js";
const PROBE_ID = "vorkspace-codex-probe";
const DISCOVERY_TIMEOUT_MS = 15000;
const REQUEST_TIMEOUT_MS = 12000;
const REASONING_LABELS = {
  none: "None",
  minimal: "Minimal",
  low: "Low",
  medium: "Medium",
  high: "High",
  xhigh: "Extra High",
  max: "Max",
  ultra: "Ultra",
};
export async function discoverCodexModels(workingDirectory) {
  const { path } = await resolveCodexBinary();
  const cwd = workingDirectory ?? (await homeDir());
  const probeId = `${PROBE_ID}-${crypto.randomUUID()}`;
  const rpc = new JsonRpcClient(
    probeId,
    {
      onRequest: (id) => {
        void rpc.respond(id, {}).catch(() => undefined);
      },
    },
    { includeJsonrpc: false, label: "codex-probe" },
  );
  const stop = async () => {
    rpc.close();
    unwatchChild(probeId);
    await killChild(probeId).catch(() => undefined);
  };
  watchChild(
    probeId,
    (line) => rpc.pushLine(line),
    () => rpc.close(new Error("Codex probe exited")),
  );
  try {
    await spawnChild(probeId, path, ["app-server"], cwd, undefined, "codex");
    return await withTimeout(
      DISCOVERY_TIMEOUT_MS,
      async () => {
        await rpc.request(
          "initialize",
          {
            clientInfo: {
              name: "vorkspace",
              title: "vorkspace",
              version: "0.1.0",
            },
            capabilities: { experimentalApi: true },
          },
          REQUEST_TIMEOUT_MS,
        );
        await rpc.notify("initialized", undefined);
        const account = await rpc
          .request("account/read", {}, REQUEST_TIMEOUT_MS)
          .catch(() => null);
        if (account && !account.account && account.requiresOpenaiAuth) {
          throw new Error(
            "Codex CLI is not authenticated. Run `codex login` and try again.",
          );
        }
        return await listAllModels(rpc);
      },
      () => {
        void stop();
      },
    );
  } finally {
    await stop();
  }
}
async function listAllModels(rpc) {
  const models = [];
  const rows = [];
  let cursor;
  do {
    const response = await rpc.request(
      "model/list",
      cursor ? { cursor } : {},
      REQUEST_TIMEOUT_MS,
    );
    const page = Array.isArray(response.data) ? response.data : [];
    rows.push(...page);
    for (const row of page) {
      const model = parseModel(row);
      if (model) models.push(model);
    }
    cursor = response.nextCursor ?? null;
  } while (cursor);
  return orderDefaultFirst(uniqueByNative(models), rows);
}
function parseModel(raw) {
  const rec = asRecord(raw);
  if (!rec) return null;
  if (rec.hidden === true) return null;
  const nativeId =
    stringField(rec, "model") ??
    stringField(rec, "slug") ??
    stringField(rec, "id");
  if (!nativeId) return null;
  const name = formatDisplayName(
    stringField(rec, "displayName") ?? stringField(rec, "name") ?? nativeId,
  );
  const settings = parseModelSettings(rec);
  return {
    id: `codex:${nativeId}`,
    harness: "codex",
    name,
    nativeId,
    ...(settings.length > 0 ? { settings } : {}),
  };
}
function parseModelSettings(rec) {
  const settings = [];
  const efforts = Array.isArray(rec.supportedReasoningEfforts)
    ? rec.supportedReasoningEfforts
    : [];
  const effortOptions = [];
  let defaultEffort;
  for (const entry of efforts) {
    if (typeof entry === "string") {
      effortOptions.push({
        value: entry,
        label: REASONING_LABELS[entry] ?? entry,
      });
      continue;
    }
    const row = asRecord(entry);
    const value = stringField(row, "reasoningEffort") ?? stringField(row, "id");
    if (!value) continue;
    effortOptions.push({
      value,
      label: REASONING_LABELS[value] ?? stringField(row, "label") ?? value,
    });
  }
  const defaultRaw = stringField(rec, "defaultReasoningEffort");
  if (defaultRaw) defaultEffort = defaultRaw;
  if (effortOptions.length > 0) {
    settings.push({
      id: "reasoningEffort",
      label: "Reasoning",
      kind: "select",
      value: defaultEffort ?? effortOptions[0].value,
      options: effortOptions,
    });
  }
  const tiersRaw =
    (Array.isArray(rec.serviceTiers) && rec.serviceTiers.length > 0
      ? rec.serviceTiers
      : Array.isArray(rec.additionalSpeedTiers)
        ? rec.additionalSpeedTiers
        : []) ?? [];
  const tierOptions = [{ value: "default", label: "Standard" }];
  for (const entry of tiersRaw) {
    if (typeof entry === "string") {
      if (entry === "default") continue;
      tierOptions.push({
        value: entry,
        label: entry === "fast" ? "Fast" : entry,
      });
      continue;
    }
    const row = asRecord(entry);
    const id = stringField(row, "id");
    if (!id || id === "default") continue;
    tierOptions.push({
      value: id,
      label: stringField(row, "name") ?? id,
    });
  }
  if (tierOptions.length > 1) {
    const defaultTier = stringField(rec, "defaultServiceTier") ?? "default";
    settings.push({
      id: "serviceTier",
      label: "Service Tier",
      kind: "select",
      value: tierOptions.some((o) => o.value === defaultTier)
        ? defaultTier
        : "default",
      options: tierOptions,
    });
  }
  return settings;
}
function formatDisplayName(name) {
  return name
    .replace(/^gpt/i, "GPT")
    .replace(/-([a-z])/g, (_, c) => `-${c.toUpperCase()}`);
}
function orderDefaultFirst(models, rows = []) {
  if (models.length <= 1) return models;
  const defaultNative = rows
    .map((row) => asRecord(row))
    .find((rec) => rec?.isDefault === true);
  const nativeId =
    stringField(defaultNative, "model") ??
    stringField(defaultNative, "slug") ??
    stringField(defaultNative, "id");
  if (!nativeId) return models;
  const index = models.findIndex((model) => model.nativeId === nativeId);
  if (index <= 0) return models;
  return [models[index], ...models.slice(0, index), ...models.slice(index + 1)];
}
function uniqueByNative(models) {
  const seen = new Set();
  return models.filter((model) => {
    if (!model.nativeId || seen.has(model.nativeId)) return false;
    seen.add(model.nativeId);
    return true;
  });
}
async function withTimeout(ms, work, onTimeout) {
  let timer;
  const pending = work();
  try {
    return await Promise.race([
      pending,
      new Promise((_, reject) => {
        timer = setTimeout(() => {
          onTimeout();
          reject(new Error("Codex model discovery timed out"));
        }, ms);
      }),
    ]);
  } finally {
    if (timer) clearTimeout(timer);
    void pending.catch(() => undefined);
  }
}
