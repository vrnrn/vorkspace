import { asRecord } from "../../providers/codex/codexProtocol.js";
export const SESSION_WINDOW_MINUTES = 300;
export const WEEKLY_WINDOW_MINUTES = 10080;
export const MONTHLY_WINDOW_MINUTES = 43200;
const WINDOW_DURATION_TOLERANCE_MINUTES = 1;
export function clampUsedPercent(value) {
  if (!Number.isFinite(value)) return 0;
  return Math.min(100, Math.max(0, value));
}
/** When a used-up window resets; the later one when several are spent. */
export function exhaustedWindowResetAt(limits) {
  let latest = null;
  for (const window of [limits.session, limits.weekly, limits.monthly]) {
    if (!window || window.usedPercent < 100 || window.resetsAt == null)
      continue;
    latest = Math.max(latest ?? 0, window.resetsAt);
  }
  return latest;
}
export function parseResetTimestamp(value) {
  if (typeof value === "number") {
    return normalizeEpochMs(value);
  }
  if (typeof value !== "string" || value.trim() === "") return null;
  const numeric = Number(value);
  if (Number.isFinite(numeric) && value.trim() !== "") {
    return normalizeEpochMs(numeric);
  }
  const parsed = Date.parse(value);
  return Number.isNaN(parsed) ? null : parsed;
}
function normalizeEpochMs(value) {
  if (!Number.isFinite(value) || value <= 0) return null;
  // 1e10 sits between seconds-epoch (<2286) and millisecond-epoch (>2001).
  return value > 10000000000 ? value : value * 1000;
}
export function parseCodexRateLimits(result) {
  const rec = asRecord(result);
  const wrapper = asRecord(rec?.rateLimits) ?? rec;
  const classified = classifyCodexWindows({
    primary: snapshotFrom(asRecord(wrapper?.primary)),
    secondary: snapshotFrom(asRecord(wrapper?.secondary)),
  });
  return {
    provider: "codex",
    session: mapCodexSnapshot(classified.session, SESSION_WINDOW_MINUTES),
    weekly: mapCodexSnapshot(classified.weekly, WEEKLY_WINDOW_MINUTES),
    monthly: mapCodexSnapshot(classified.monthly, MONTHLY_WINDOW_MINUTES),
    resetCredits: parseResetCredits(
      rec?.rateLimitResetCredits ?? rec?.rate_limit_reset_credits,
    ),
    updatedAt: Date.now(),
    error: null,
    status: "ok",
  };
}
function parseResetCredits(raw) {
  const rec = asRecord(raw);
  if (!rec) return null;
  const count =
    numberField(rec, "availableCount") ?? numberField(rec, "available_count");
  if (count == null) return null;
  const rawCredits = rec.credits;
  const credits = Array.isArray(rawCredits)
    ? rawCredits.map(parseResetCredit).filter((credit) => credit != null)
    : null;
  return {
    availableCount: Math.max(0, Math.floor(count)),
    credits,
  };
}
function parseResetCredit(raw) {
  const rec = asRecord(raw);
  if (!rec || typeof rec.id !== "string" || rec.id.trim() === "") {
    return null;
  }
  const resetType =
    rec.resetType === "codexRateLimits" ? "codexRateLimits" : "unknown";
  const status =
    rec.status === "available" ||
    rec.status === "redeeming" ||
    rec.status === "redeemed"
      ? rec.status
      : "unknown";
  return {
    id: rec.id,
    resetType,
    status,
    grantedAt: parseResetTimestamp(rec.grantedAt ?? rec.granted_at),
    expiresAt: parseResetTimestamp(rec.expiresAt ?? rec.expires_at),
    title: stringField(rec, "title"),
    description: stringField(rec, "description"),
  };
}
function snapshotFrom(rec) {
  if (!rec) return null;
  const usedPercent =
    numberField(rec, "usedPercent") ??
    numberField(rec, "used_percent") ??
    numberField(rec, "used_percentage");
  if (usedPercent == null) return null;
  return {
    usedPercent,
    windowDurationMins:
      numberField(rec, "windowDurationMins") ??
      numberField(rec, "window_duration_mins") ??
      null,
    resetsAt: rec.resetsAt ?? rec.resets_at,
  };
}
function classifyCodexWindows(input) {
  let session = null;
  let weekly = null;
  let monthly = null;
  for (const window of [input.primary, input.secondary]) {
    if (!window) continue;
    const kind = classifyWindowDuration(window.windowDurationMins);
    if (kind === "session" && !session) session = window;
    else if (kind === "weekly" && !weekly) weekly = window;
    else if (kind === "monthly" && !monthly) monthly = window;
  }
  if (
    !session &&
    input.primary &&
    classifyWindowDuration(input.primary.windowDurationMins) === null
  ) {
    session = input.primary;
  }
  if (
    !weekly &&
    input.secondary &&
    classifyWindowDuration(input.secondary.windowDurationMins) === null
  ) {
    weekly = input.secondary;
  }
  return { session, weekly, monthly };
}
function classifyWindowDuration(duration) {
  if (duration == null || !Number.isFinite(duration)) return null;
  if (
    Math.abs(duration - SESSION_WINDOW_MINUTES) <=
    WINDOW_DURATION_TOLERANCE_MINUTES
  ) {
    return "session";
  }
  if (
    Math.abs(duration - WEEKLY_WINDOW_MINUTES) <=
    WINDOW_DURATION_TOLERANCE_MINUTES
  ) {
    return "weekly";
  }
  // Free plans get a single 30-day window.
  if (
    Math.abs(duration - MONTHLY_WINDOW_MINUTES) <=
    WINDOW_DURATION_TOLERANCE_MINUTES
  ) {
    return "monthly";
  }
  return null;
}
function mapCodexSnapshot(raw, windowMinutes) {
  if (!raw) return null;
  return {
    usedPercent: clampUsedPercent(raw.usedPercent),
    windowMinutes,
    resetsAt: parseResetTimestamp(raw.resetsAt),
  };
}
function numberField(rec, key) {
  const value = rec[key];
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string" && value.trim() !== "") {
    const parsed = Number(value);
    if (Number.isFinite(parsed)) return parsed;
  }
  return null;
}
function stringField(rec, key) {
  const value = rec[key];
  return typeof value === "string" && value.trim() !== "" ? value.trim() : null;
}
