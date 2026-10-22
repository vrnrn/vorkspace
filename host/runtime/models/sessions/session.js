import {
  preferredModelId,
  preferredModelSettings,
  resolveModel,
} from "./models.js";
export { HARNESSES } from "./harnessIds.js";
export const RUNTIME_MODES = [
  "supervised",
  "auto-accept-edits",
  "auto",
  "full-access",
];
export const DEFAULT_RUNTIME_MODE = "supervised";
export const HARNESS_LABEL = {
  claude: "claude",
  codex: "codex",
  pi: "pi",
  omp: "omp",
  antigravity: "antigravity",
};
export const HARNESS_TITLE = {
  claude: "Claude Code",
  codex: "Codex",
  pi: "Pi",
  omp: "omp",
  antigravity: "Antigravity",
};
export function newSession(
  harness = "claude",
  cwd = "~",
  model,
  runtimeMode = DEFAULT_RUNTIME_MODE,
  modelSettings,
) {
  const resolved = resolveModel(harness, model ?? preferredModelId(harness));
  return {
    id: crypto.randomUUID(),
    harness,
    model: resolved.id,
    modelSettings: preferredModelSettings(resolved, modelSettings),
    runtimeMode,
    title: HARNESS_LABEL[harness],
    cwd,
    blocks: [],
  };
}
/** First line of a prompt, truncated for the tab strip. */
export function titleFromPrompt(prompt, harness, attachments = []) {
  const line = prompt.trim().split(/\r?\n/)[0]?.trim() ?? "";
  const fromFiles =
    !line && attachments.length > 0
      ? attachments
          .map((file) => file.name)
          .filter(Boolean)
          .slice(0, 3)
          .join(", ")
      : "";
  const seed = line || fromFiles;
  if (!seed) return HARNESS_LABEL[harness];
  const max = 72;
  const short = seed.length > max ? `${seed.slice(0, max - 1)}…` : seed;
  return formatSessionTitle(harness, short);
}
export function formatSessionTitle(harness, title) {
  const trimmed = title.trim();
  if (!trimmed) return HARNESS_LABEL[harness];
  return `${HARNESS_LABEL[harness]} · ${trimmed}`;
}
/** True when the stored title is still a placeholder the LLM may replace. */
export function canReplaceSessionTitle(current, harness, seed) {
  return (
    current === seed ||
    current === HARNESS_LABEL[harness] ||
    current === HARNESS_TITLE[harness]
  );
}
export function hasPendingApproval(blocks) {
  return blocks.some((block) => block.approval && !block.approval.decided);
}
export function sessionNeedsInput(session) {
  return (
    !session.worktreeRemoved &&
    (hasPendingApproval(session.blocks) || session.pendingQuestion != null)
  );
}
