export const MODELS = [
  {
    id: "claude:sonnet-5",
    harness: "claude",
    name: "Claude Sonnet 5",
    nativeId: "claude-sonnet-5",
  },
  {
    id: "claude:opus-5",
    harness: "claude",
    name: "Claude Opus 5",
    nativeId: "claude-opus-5",
  },
  {
    id: "claude:opus-5-5",
    harness: "claude",
    name: "Claude Opus 5.5",
    nativeId: "claude-opus-5-5",
  },
  {
    id: "claude:fable-5",
    harness: "claude",
    name: "Claude Fable 5",
    nativeId: "claude-fable-5",
  },
  {
    id: "claude:opus-4.6",
    harness: "claude",
    name: "Opus 4.6",
    nativeId: "claude-opus-4-6",
  },
  {
    id: "claude:sonnet-4.6",
    harness: "claude",
    name: "Sonnet 4.6",
    nativeId: "claude-sonnet-4-6",
  },
  {
    id: "claude:haiku-4.5",
    harness: "claude",
    name: "Haiku 4.5",
    nativeId: "claude-haiku-4-5",
  },
  {
    id: "claude:opus-4.5",
    harness: "claude",
    name: "Opus 4.5",
    nativeId: "claude-opus-4-5",
  },
  {
    id: "pi:default",
    harness: "pi",
    name: "Default",
    nativeId: "",
  },
  {
    id: "omp:default",
    harness: "omp",
    name: "Default",
    nativeId: "",
  },
  {
    id: "antigravity:gemini-3.8-flash-high",
    harness: "antigravity",
    name: "Gemini 3.8 Flash (High)",
    nativeId: "gemini-3.8-flash-high",
  },
];
export const DEFAULT_MODEL_ID = {
  claude: "claude:sonnet-5",
  codex: "",
  pi: "pi:default",
  omp: "omp:default",
  antigravity: "antigravity:gemini-3.8-flash-high",
};
const HARNESS_ORDER = ["claude", "codex", "pi", "omp", "antigravity"];
const EMPTY_MODELS = [];
let overlays = {};
let overlayDefaults = {};
let catalogVersion = 0;
const listeners = new Set();
function emit() {
  catalogVersion += 1;
  baseByHarness = null;
  indexById = null;
  allCache = null;
  for (const listener of listeners) listener();
}
export function setHarnessModels(harness, models) {
  if (models.length === 0) return;
  overlays = { ...overlays, [harness]: models };
  overlayDefaults = {
    ...overlayDefaults,
    [harness]: pickDefaultId(harness, models),
  };
  emit();
}
/** True after a live CLI catalog has replaced the built-in fallback list. */
export function hasLiveCatalog(harness) {
  return overlays[harness] != null;
}
export function defaultModelId(harness) {
  return overlayDefaults[harness] ?? DEFAULT_MODEL_ID[harness];
}
// `modelsFor`/`findModel` sit in render bodies (every session card, every
// provider row, the picker itself), so they must not rebuild the catalog on
// each call. These caches are dropped in `emit()` whenever an overlay lands.
let baseByHarness = null;
let allCache = null;
let indexById = null;
function baseModelsFor(harness) {
  if (!baseByHarness) {
    const grouped = {};
    for (const model of MODELS) {
      (grouped[model.harness] ??= []).push(model);
    }
    baseByHarness = grouped;
  }
  return baseByHarness[harness] ?? EMPTY_MODELS;
}
export function modelsFor(harness) {
  return overlays[harness] ?? baseModelsFor(harness);
}
export function allModels() {
  return (allCache ??= HARNESS_ORDER.flatMap(modelsFor));
}
export function findModel(id) {
  if (!indexById) {
    const index = new Map();
    // First writer wins, matching the previous `allModels().find(...)` order.
    for (const model of allModels()) {
      if (!index.has(model.id)) index.set(model.id, model);
    }
    indexById = index;
  }
  return indexById.get(id);
}
/** The bundled list never changes, so index it once for `lookupModel`. */
const bundledById = new Map(MODELS.map((model) => [model.id, model]));
/**
 * Catalog entry for a saved model id, live list first and bundled list second.
 *
 * A live overlay replaces the bundled list rather than adding to it, so a
 * model the CLI stops advertising misses `findModel` entirely. Falling back to
 * the bundle keeps the full native id (`claude:opus-5-5` → `claude-opus-5-5`)
 * instead of re-deriving one from the key, which strips the provider prefix
 * and hands the CLI an id it rejects.
 */
function lookupModel(id) {
  return findModel(id) ?? bundledById.get(id);
}
export function resolveModel(harness, id) {
  const available = modelsFor(harness);
  if (id) {
    const exact = findModel(id);
    if (exact && exact.harness === harness) return exact;
    const slug = nativeIdFrom(id);
    const byNative = available.find(
      (model) => (model.nativeId ?? nativeIdFrom(model.id)) === slug,
    );
    if (byNative) return byNative;
    // Keep moving Claude aliases as aliases until the CLI advertises them.
    if (harness === "claude" && /^(opus|sonnet|haiku)$/.test(slug)) {
      return {
        id: `claude:${slug}`,
        harness,
        name: `Claude ${slug.charAt(0).toUpperCase()}${slug.slice(1)}`,
        nativeId: slug,
      };
    }
    // A versioned id may differ only by Claude's provider prefix. Do not
    // resolve it to a moving alias or another version via a prefix match.
    const comparableSlug = comparableNativeId(harness, slug);
    const hits = available.filter((model) => {
      const native = model.nativeId ?? nativeIdFrom(model.id);
      const comparableNative = comparableNativeId(harness, native);
      return (
        comparableNative.startsWith(comparableSlug) ||
        comparableSlug.startsWith(comparableNative)
      );
    });
    if (/\d/.test(comparableSlug)) {
      const same = hits.find(
        (model) =>
          comparableNativeId(
            harness,
            model.nativeId ?? nativeIdFrom(model.id),
          ) === comparableSlug,
      );
      if (same) return same;
      if (
        harness !== "claude" &&
        hits.length === 1 &&
        !/\d/.test(
          comparableNativeId(
            harness,
            hits[0].nativeId ?? nativeIdFrom(hits[0].id),
          ),
        )
      )
        return hits[0];
    } else if (hits.length === 1) {
      return hits[0];
    } else if (hits.length > 0) {
      return hits[0];
    }
    const bundled = bundledById.get(id);
    if (bundled && bundled.harness === harness) {
      return withAliasSettings(bundled, available);
    }
    // A saved Claude version or provider-qualified ID can be absent from both catalogs.
    // Preserve the requested ID until discovery supplies its model metadata.
    const requested = id.trim();
    if (
      harness === "claude" &&
      /^claude:[a-z][a-z0-9-]*(?:-\d|\/)/.test(requested)
    ) {
      const nativeId = nativeIdForUnknownKey(requested);
      return withAliasSettings(
        { id: requested, harness, name: nativeId, nativeId },
        available,
      );
    }
  }
  // Codex has no built-in catalog. During startup, retain the saved model
  // until discovery finishes instead of borrowing another provider's model.
  if (available.length === 0) {
    const requested = id?.trim() ?? "";
    const modelId =
      requested &&
      (!requested.includes(":") || requested.startsWith(`${harness}:`))
        ? requested
        : "";
    const nativeId = nativeIdFrom(modelId);
    return {
      id: modelId,
      harness,
      name: nativeId
        ? nativeId
            .replace(/^gpt/i, "GPT")
            .replace(/-([a-z])/g, (_, letter) => `-${letter.toUpperCase()}`)
        : harness.charAt(0).toUpperCase() + harness.slice(1),
      nativeId,
    };
  }
  const fallbackId = defaultModelId(harness);
  return (fallbackId ? findModel(fallbackId) : undefined) ?? available[0];
}
/**
 * A concrete version missing from the live catalog borrows the settings of the
 * alias that launches it. Claude lists Opus 5.5 only as `opus`, so a saved
 * `claude:opus-5-5` would otherwise lose its effort and fast options.
 */
function withAliasSettings(model, available) {
  if (model.settings) return model;
  const native = comparableNativeId(
    model.harness,
    model.nativeId ?? nativeIdFrom(model.id),
  );
  const alias = available.find(
    (item) =>
      item.settings &&
      item.aliasTarget != null &&
      comparableNativeId(item.harness, item.aliasTarget) === native,
  );
  return alias ? { ...model, settings: alias.settings } : model;
}
export function nativeModelId(model) {
  if (typeof model !== "string") {
    return claudeNativeId(
      model.harness,
      model.nativeId ?? nativeIdFrom(model.id),
    );
  }
  const found = lookupModel(model);
  if (found) {
    return claudeNativeId(
      found.harness,
      found.nativeId ?? nativeIdFrom(found.id),
    );
  }
  return nativeIdForUnknownKey(model);
}
/**
 * Last-resort native id for a saved key no catalog knows.
 *
 * Short Claude version keys need the `claude-` prefix.
 * Provider-qualified IDs are opaque and retain their spelling.
 */
function nativeIdForUnknownKey(id) {
  const trimmed = id.trim();
  const slug = nativeIdFrom(trimmed);
  const colon = trimmed.indexOf(":");
  const harness = colon >= 0 ? trimmed.slice(0, colon).toLowerCase() : "";
  // Picker keys can use dotted versions (`opus-4.8`), while Claude's CLI
  // expects hyphenated native ids (`claude-opus-4-8`).
  return harness === "claude" && !slug.includes("/")
    ? claudeNativeId("claude", slug.replace(/\.(?=\d)/g, "-"))
    : slug;
}
/**
 * Claude's CLI rejects digit-bearing slugs without the `claude-` prefix
 * (`opus-5-5` is invalid; `claude-opus-5-5` and the alias `opus` both work).
 * Live `list_models` can still advertise the short form as `value`.
 */
function claudeNativeId(harness, native) {
  if (
    harness !== "claude" ||
    !native ||
    native.startsWith("claude-") ||
    native.includes("/")
  ) {
    return native;
  }
  return /\d/.test(native) ? `claude-${native}` : native;
}
export function defaultModelSettings(model) {
  const settings = {};
  for (const setting of model.settings ?? []) {
    settings[setting.id] = setting.value;
  }
  return settings;
}
export function mergeModelSettings(model, current) {
  // Preserve saved settings until provider-qualified model metadata arrives.
  if (
    modelsFor(model.harness).length === 0 ||
    (!model.settings &&
      model.nativeId?.includes("/") &&
      !hasLiveCatalog(model.harness))
  )
    return { ...current };
  const next = defaultModelSettings(model);
  if (!current) return next;
  for (const setting of model.settings ?? []) {
    const value = compatibleSettingValue(setting, current[setting.id]);
    if (value != null) next[setting.id] = value;
  }
  return next;
}
export function preferredModelSettings(model, current) {
  return mergeModelSettings(model, current);
}
export function preferredModelId(harness) {
  return defaultModelId(harness);
}
/** Normalize provider-specific spellings of extra-high reasoning. */
const SETTING_VALUE_ALIASES = {
  "extra-high": "xhigh",
  xhigh: "extra-high",
};
function compatibleSettingValue(setting, value) {
  if (value == null) return undefined;
  if (setting.options.some((option) => option.value === value)) return value;
  const alias = SETTING_VALUE_ALIASES[value];
  if (alias && setting.options.some((option) => option.value === alias)) {
    return alias;
  }
  return undefined;
}
function nativeIdFrom(id) {
  const trimmed = id.trim();
  const colon = trimmed.indexOf(":");
  const slug = colon >= 0 ? trimmed.slice(colon + 1) : trimmed;
  const bracket = slug.indexOf("[");
  return bracket >= 0 ? slug.slice(0, bracket) : slug;
}
/** Match Claude ids with and without the provider prefix. */
function comparableNativeId(harness, id) {
  return harness === "claude" ? id.replace(/^claude-/, "") : id;
}
function pickDefaultId(harness, models) {
  if (harness === "claude") {
    return (
      models.find((model) => model.nativeId === "claude-sonnet-5")?.id ??
      models.find((model) => model.nativeId === "sonnet")?.id ??
      models.find((model) => model.id === DEFAULT_MODEL_ID.claude)?.id ??
      models[0]?.id ??
      DEFAULT_MODEL_ID.claude
    );
  }
  if (harness === "codex") {
    return models[0]?.id ?? "";
  }
  return (
    models.find((model) => model.id === DEFAULT_MODEL_ID[harness])?.id ??
    models[0]?.id ??
    DEFAULT_MODEL_ID[harness]
  );
}
