import {
  extractJsonObject,
  limitSection,
} from "../../lib/jsonText.js";
const MESSAGE_LIMIT = 8000;
const TITLE_LIMIT = 50;
const THREAD_TITLE_PROMPT = `Generate a title that will help the user recognize this coding session weeks later.
Also identify one GitHub issue or pull request only when the user explicitly refers to it by number or URL.
Return JSON with exactly two keys: title and workItem.
workItem must be null or an object with exactly two keys: kind ("issue" or "pr") and number (a positive integer copied from the user message).
Never invent a work item number. If the reference is ambiguous or has no number, return null.
Do not call tools. Reply with JSON only.

Before answering, silently reduce the request to:
- Subject: What system, feature, or problem is this really about?
- Outcome: What does the user ultimately want to understand or change?
- Incidental instructions: What only describes how the agent should do the work?

Title the subject and outcome. Discard incidental instructions.

Editorial rules:
- 3-8 words, fewer than 40 characters.
- Use a compact noun phrase or clear action phrase.
- Capture the umbrella goal when the request lists several symptoms or steps.
- Name the product change, not the mock, plan, report, branch, or PR used to produce it.
- Models, subagents, tools, and output formats do not belong in the title unless they are themselves the topic.
- Do not claim the work is complete.
- Do not copy and truncate the user's message.
- Avoid quotes, labels, filler, and trailing punctuation.`;
export function buildThreadTitlePrompt(message) {
  return `${THREAD_TITLE_PROMPT}\n\nUser message:\n${limitSection(message, MESSAGE_LIMIT)}`;
}
export function sanitizeThreadTitle(raw) {
  const normalized = raw
    .trim()
    .split(/\r?\n/g)[0]
    ?.trim()
    .replace(/^['"`]+|['"`]+$/g, "")
    .trim()
    .replace(/\s+/g, " ");
  if (!normalized) return "";
  if (normalized.length <= TITLE_LIMIT) return normalized;
  return `${normalized.slice(0, TITLE_LIMIT - 3).trimEnd()}...`;
}
function referencedNumber(message, number) {
  return new RegExp(`(^|\\D)${number}(?=\\D|$)`).test(message);
}
export function parseGeneratedSessionTitle(raw, message) {
  const json = extractJsonObject(raw);
  if (json) {
    try {
      const parsed = JSON.parse(json);
      if (parsed && typeof parsed === "object" && "title" in parsed) {
        const title = sanitizeThreadTitle(String(parsed.title));
        if (title) {
          const candidate = parsed.workItem;
          const workItem =
            candidate && typeof candidate === "object" ? candidate : null;
          const kind = workItem?.kind;
          const number = workItem?.number;
          const validWorkItem =
            (kind === "issue" || kind === "pr") &&
            typeof number === "number" &&
            Number.isSafeInteger(number) &&
            number > 0 &&
            referencedNumber(message, number)
              ? { kind, number }
              : null;
          return { title, workItem: validWorkItem };
        }
      }
    } catch {
      // Fall through to a bare-title parse when the model skipped JSON.
    }
  }
  const fallback = sanitizeThreadTitle(raw);
  if (!fallback || /[{}]/.test(fallback)) return null;
  const words = fallback.split(" ").filter(Boolean).length;
  if (words < 2 || words > 10) return null;
  return { title: fallback, workItem: null };
}
