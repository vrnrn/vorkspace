import {
  limitSection,
  parseJsonObject,
  stringField,
} from "../../lib/jsonText.js";
export function buildBranchNamePrompt(message) {
  return [
    "You generate concise git branch names.",
    "Return a JSON object with key: branch.",
    "Do not call tools. Reply with JSON only.",
    "Rules:",
    "- Branch should describe the requested work from the user message.",
    "- Keep it short and specific (2-6 words).",
    "- Use plain words only, no issue prefixes and no punctuation-heavy text.",
    "",
    "User message:",
    limitSection(message, 8000),
  ].join("\n");
}
export function parseBranchName(raw) {
  const rec = parseJsonObject(raw);
  const branch = sanitizeBranchFragment(rec ? stringField(rec, "branch") : raw);
  return branch || null;
}
export function sanitizeBranchFragment(raw) {
  const normalized = raw
    .trim()
    .toLowerCase()
    .replace(/['"`]/g, "")
    .replace(/^[./\s_-]+|[./\s_-]+$/g, "");
  const fragment = normalized
    .replace(/[^a-z0-9/_-]+/g, "-")
    .replace(/\/+/g, "/")
    .replace(/-+/g, "-")
    .replace(/^[./_-]+|[./_-]+$/g, "")
    .slice(0, 64)
    .replace(/[./_-]+$/g, "");
  return fragment;
}
