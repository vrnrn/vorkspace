import {
  attachmentPathText,
  promptText,
} from "../../models/sessions/attachments.js";
import { isTaskListToolName } from "../../models/sessions/taskList.js";
import { extractToolPreview, titleFromToolInput } from "../../protocol/preview.js";
import { upstreamProviderName } from "../../protocol/providerNames.js";
import { streamTextDelta } from "../../protocol/streamText.js";
/** Images Pi RPC accepts on `prompt` / `steer`. */
export const SUPPORTED_PI_IMAGE_MIME_TYPES = new Set([
  "image/gif",
  "image/jpeg",
  "image/png",
  "image/webp",
]);
export const PI_THINKING_LEVELS = [
  "off",
  "minimal",
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
];
export function asRecord(value) {
  if (value && typeof value === "object" && !Array.isArray(value)) {
    return value;
  }
  return null;
}
export function stringField(rec, key) {
  if (!rec) return undefined;
  const value = rec[key];
  return typeof value === "string" && value.trim() ? value : undefined;
}
function toolArgsFromEvent(rec) {
  return (
    parseArgBag(rec?.args) ??
    parseArgBag(rec?.arguments) ??
    parseArgBag(rec?.input) ??
    {}
  );
}
function parseArgBag(value) {
  if (typeof value === "string" && value.trim()) {
    return tryParseJsonRecord(value);
  }
  return asRecord(value);
}
/** Later execution updates can be partial; keep keys we already have. */
export function mergeToolInput(current, next) {
  if (Object.keys(next).length === 0) return current;
  return { ...current, ...next };
}
export function parseJsonLine(line) {
  const trimmed = line.trim();
  if (!trimmed.startsWith("{")) return null;
  try {
    return asRecord(JSON.parse(trimmed));
  } catch {
    return null;
  }
}
/**
 * Spawn args for a live session. Intentionally omits `--no-extensions` so the
 * user's global Pi packages (todos, subagents, custom tools) still load.
 * Project-local `.pi` resources follow Pi's saved trust.json; RPC never prompts.
 */
export function buildPiSpawnArgs(flavor, input) {
  const args = ["--mode", "rpc"];
  if (input.isolated || input.noSession) args.push("--no-session");
  if (input.isolated || input.noExtensions) args.push("--no-extensions");
  if (input.isolated) {
    args.push(...flavor.isolateFlags);
  } else if (input.plan) {
    args.push("--tools", flavor.planTools.join(","));
  }
  if (input.resume?.trim()) {
    args.push(flavor.resumeFlag, input.resume.trim());
  }
  const model = input.model?.trim();
  if (model) args.push("--model", model);
  return args;
}
export function parsePiModelRef(nativeId) {
  if (!nativeId) return null;
  const trimmed = nativeId.trim();
  if (!trimmed) return null;
  const separator = trimmed.indexOf("/");
  if (separator <= 0 || separator === trimmed.length - 1) return null;
  return {
    provider: trimmed.slice(0, separator),
    modelId: trimmed.slice(separator + 1),
  };
}
export function piNativeId(provider, modelId) {
  return `${provider}/${modelId}`;
}
function piPromptContent(text, attachments = []) {
  const images = [];
  const body = promptText(text, attachments);
  const parts = body ? [body] : [];
  for (const attachment of attachments) {
    const mimeType = attachment.mimeType.trim().toLowerCase();
    if (
      attachment.kind === "image" &&
      attachment.data &&
      SUPPORTED_PI_IMAGE_MIME_TYPES.has(mimeType)
    ) {
      images.push({ type: "image", data: attachment.data, mimeType });
    } else {
      parts.push(attachmentPathText(attachment));
    }
  }
  return {
    message: parts.join("\n\n"),
    ...(images.length > 0 ? { images } : {}),
  };
}
export function buildPiPrompt(input) {
  const command = {
    type: "prompt",
    ...piPromptContent(input.text, input.attachments),
  };
  if (input.streaming) command.streamingBehavior = "steer";
  return command;
}
export function forkMessagesFromRpcData(data) {
  const rec = asRecord(data);
  const messages = Array.isArray(rec?.messages) ? rec.messages : [];
  const parsed = [];
  for (const item of messages) {
    const row = asRecord(item);
    const entryId = stringField(row, "entryId");
    const text = stringField(row, "text") ?? "";
    if (entryId) parsed.push({ entryId, text });
  }
  return parsed;
}
export function buildPiSteer(input) {
  return {
    type: "steer",
    ...piPromptContent(input.text, input.attachments),
  };
}
export function parseRpcResponse(rec) {
  if (stringField(rec, "type") !== "response") return null;
  const command = stringField(rec, "command") ?? "unknown";
  const id = stringField(rec, "id");
  const success = rec.success === true;
  const error = stringField(rec, "error");
  return {
    ...(id ? { id } : {}),
    command,
    success,
    ...(error ? { error } : {}),
    data: rec.data,
  };
}
export function parseExtensionUiRequest(rec) {
  if (stringField(rec, "type") !== "extension_ui_request") return null;
  const id = stringField(rec, "id");
  const method = stringField(rec, "method");
  if (!id || !method) return null;
  if (method === "select") {
    const options = Array.isArray(rec.options)
      ? rec.options.filter((item) => typeof item === "string")
      : [];
    return {
      id,
      method,
      title: stringField(rec, "title") ?? "Choose an option",
      options,
    };
  }
  if (method === "confirm") {
    return {
      id,
      method,
      title: stringField(rec, "title") ?? "Confirm",
      message: stringField(rec, "message") ?? "",
    };
  }
  if (method === "input" || method === "editor") {
    return { id, method, title: stringField(rec, "title") ?? method };
  }
  if (
    method === "notify" ||
    method === "setStatus" ||
    method === "setWidget" ||
    method === "setTitle" ||
    method === "set_editor_text"
  ) {
    const statusKey =
      method === "setStatus" ? stringField(rec, "statusKey") : undefined;
    return {
      id,
      method,
      title:
        stringField(rec, "message") ??
        stringField(rec, "statusText") ??
        stringField(rec, "title") ??
        stringField(rec, "text"),
      ...(statusKey ? { statusKey } : {}),
    };
  }
  return null;
}
export function extensionUiResponse(request, decision) {
  if (decision === "deny") {
    return { type: "extension_ui_response", id: request.id, cancelled: true };
  }
  if (request.method === "confirm") {
    return { type: "extension_ui_response", id: request.id, confirmed: true };
  }
  if (request.method === "select") {
    const value = request.options[0] ?? "";
    return { type: "extension_ui_response", id: request.id, value };
  }
  return { type: "extension_ui_response", id: request.id, cancelled: true };
}
export function extensionUiTitle(request) {
  const text =
    request.method === "confirm"
      ? [request.title, request.message].filter(Boolean).join(" — ")
      : (request.title ?? "Pi extension");
  // Pi's theme helpers emit ANSI even in RPC mode (e.g. Ponytail setStatus).
  // These labels use native UI styling. Strip CSI and OSC sequences only at
  // the display boundary: select replies must retain the original option.
  return text
    .replace(
      /(?:\u001b\]|\u009d)[^\u0007\u001b\u009c]*(?:\u0007|\u001b\\|\u009c)/g,
      "",
    )
    .replace(/(?:\u001b\[|\u009b)[0-?]*[ -/]*[@-~]/g, "");
}
export function needsExtensionUiReply(request) {
  return (
    request.method === "confirm" ||
    request.method === "select" ||
    request.method === "input" ||
    request.method === "editor"
  );
}
export function sessionFromState(data) {
  const rec = asRecord(data);
  const model = asRecord(rec?.model);
  const window = numberField(model, "contextWindow");
  return {
    sessionId: stringField(rec, "sessionId"),
    sessionFile: stringField(rec, "sessionFile"),
    ...(window && window > 0 ? { contextWindow: window } : {}),
  };
}
/**
 * Session-store ids may only be ASCII letters, digits, `-`, and `_`.
 * Pi's `sessionFile` is a filesystem path, so persist would reject the whole
 * upsert (and the sidebar would never get a git snapshot). `--session` accepts
 * the UUID `sessionId` as well, so that's what we bind.
 */
export function providerSessionIdFromState(data) {
  const sessionId = sessionFromState(data).sessionId?.trim();
  if (!sessionId || !/^[A-Za-z0-9_-]+$/.test(sessionId)) return undefined;
  return sessionId;
}
/**
 * Current Pi puts streaming usage on the frame. 0.80.x put a finished
 * assistant total on `message` and a live total on `assistantMessageEvent.partial`.
 * Tool-result messages can carry nested LLM usage for a sub-call; that is not
 * the context-window level, so only assistant `message.usage` counts.
 */
export function contextFromUsage(rec, window) {
  const usage = frameUsage(rec);
  if (!usage) return null;
  const used =
    numberField(usage, "totalTokens") ||
    (numberField(usage, "input") ?? 0) +
      (numberField(usage, "output") ?? 0) +
      (numberField(usage, "cacheRead") ?? 0) +
      (numberField(usage, "cacheWrite") ?? 0);
  if (!used) return window && window > 0 ? { window } : null;
  return window && window > 0 ? { used, window } : { used };
}
export function turnMetricsFromUsage(rec) {
  const usage = frameUsage(rec);
  if (!usage) return null;
  const inputTokens = numberField(usage, "input") ?? 0;
  const outputTokens = numberField(usage, "output") ?? 0;
  const cacheReadTokens = numberField(usage, "cacheRead") ?? 0;
  const cacheWriteTokens = numberField(usage, "cacheWrite") ?? 0;
  const cacheReported = "cacheRead" in usage || "cacheWrite" in usage;
  const cacheableInput = inputTokens + cacheReadTokens + cacheWriteTokens;
  if (!inputTokens && !outputTokens && !cacheableInput) return null;
  return {
    ...(inputTokens ? { inputTokens } : {}),
    ...(outputTokens ? { outputTokens } : {}),
    ...(cacheReadTokens ? { cacheReadTokens } : {}),
    ...(cacheWriteTokens ? { cacheWriteTokens } : {}),
    ...(cacheReported && cacheableInput
      ? { cacheHitPercent: (cacheReadTokens / cacheableInput) * 100 }
      : {}),
  };
}
const PI_USAGE_KEYS = ["input", "output", "cacheRead", "cacheWrite"];
/**
 * Pi reports usage per assistant response: streaming frames restate that
 * response's running count, `message_end` settles it, and `turn_end` repeats
 * it. A turn with tool calls makes several responses, so add each settled one
 * to the turn instead of letting the latest replace it.
 */
export function addPiTurnUsage(spent, rec) {
  if (stringField(rec, "type") === "turn_end") return null;
  const usage = frameUsage(rec);
  if (!usage) return null;
  const response = {};
  for (const key of PI_USAGE_KEYS) {
    if (key in usage) response[key] = numberField(usage, key) ?? 0;
  }
  if (stringField(rec, "type") === "message_end") {
    spent.done = sumUsage(spent.done, response);
    spent.open = {};
  } else {
    spent.open = response;
  }
  return turnMetricsFromUsage({ usage: sumUsage(spent.done, spent.open) });
}
function sumUsage(a, b) {
  const sum = { ...a };
  for (const [key, value] of Object.entries(b)) {
    sum[key] = (sum[key] ?? 0) + value;
  }
  return sum;
}
export function contextFromSessionStats(data) {
  const rec = asRecord(data);
  const usage = asRecord(rec?.contextUsage);
  if (!usage) return null;
  const used = numberField(usage, "tokens");
  const window = numberField(usage, "contextWindow");
  if (!used && !window) return null;
  return {
    ...(used ? { used } : {}),
    ...(window && window > 0 ? { window } : {}),
  };
}
export function assistantDeltaFromEvent(rec) {
  if (stringField(rec, "type") !== "message_update") return null;
  const event = asRecord(rec.assistantMessageEvent);
  const type = stringField(event, "type");
  const delta = streamTextDelta(event?.delta);
  if (!delta) return null;
  if (type === "text_delta") return { kind: "text", text: delta };
  if (type === "thinking_delta") return { kind: "thinking", text: delta };
  return null;
}
export function toolCallStartFromEvent(rec) {
  if (stringField(rec, "type") !== "message_update") return null;
  const event = asRecord(rec.assistantMessageEvent);
  if (stringField(event, "type") !== "toolcall_start") return null;
  const id = stringField(event, "id");
  const name = stringField(event, "toolName") ?? stringField(event, "name");
  if (!id || !name) return null;
  const index = numberField(event, "contentIndex") ?? -1;
  return { id, name, index };
}
export function toolCallDeltaFromEvent(rec) {
  if (stringField(rec, "type") !== "message_update") return null;
  const event = asRecord(rec.assistantMessageEvent);
  if (stringField(event, "type") !== "toolcall_delta") return null;
  const delta = streamTextDelta(event?.delta);
  if (!delta) return null;
  return {
    index: numberField(event, "contentIndex") ?? -1,
    delta,
  };
}
export function toolCallEndFromEvent(rec) {
  if (stringField(rec, "type") !== "message_update") return null;
  const event = asRecord(rec.assistantMessageEvent);
  if (stringField(event, "type") !== "toolcall_end") return null;
  const call = asRecord(event?.toolCall) ?? event;
  const id = stringField(call, "id") ?? stringField(event, "id");
  const name =
    stringField(call, "name") ??
    stringField(call, "toolName") ??
    stringField(event, "toolName");
  if (!id || !name) return null;
  const input =
    parseArgBag(call?.arguments) ??
    parseArgBag(call?.args) ??
    parseArgBag(event?.arguments) ??
    toolArgsFromEvent(call);
  return { id, name, input };
}
export function toolExecutionStartFromEvent(rec) {
  if (stringField(rec, "type") !== "tool_execution_start") return null;
  const id = stringField(rec, "toolCallId");
  const name = stringField(rec, "toolName") ?? "tool";
  if (!id) return null;
  return { id, name, input: toolArgsFromEvent(rec) };
}
export function toolExecutionUpdateFromEvent(rec) {
  if (stringField(rec, "type") !== "tool_execution_update") return null;
  const id = stringField(rec, "toolCallId");
  if (!id) return null;
  const partial = asRecord(rec.partialResult);
  return {
    id,
    name: stringField(rec, "toolName"),
    detail: textFromContent(partial?.content) || undefined,
    input: toolArgsFromEvent(rec),
  };
}
export function toolExecutionEndFromEvent(rec) {
  if (stringField(rec, "type") !== "tool_execution_end") return null;
  const id = stringField(rec, "toolCallId");
  if (!id) return null;
  const result = asRecord(rec.result);
  return {
    id,
    name: stringField(rec, "toolName"),
    detail: textFromContent(result?.content) || undefined,
    isError: rec.isError === true,
  };
}
/**
 * A failed turn is reported inside the assistant message, not as an error
 * frame: `stopReason: "error"` with the reason in `errorMessage`. Empty string
 * means "failed, no reason".
 */
export function turnErrorFromEvent(rec) {
  if (stringField(rec, "type") !== "message_end") return null;
  const message = asRecord(rec.message);
  if (stringField(message, "role") !== "assistant") return null;
  if (stringField(message, "stopReason") !== "error") return null;
  return stringField(message, "errorMessage") ?? "";
}
export function isAgentSettled(rec) {
  return stringField(rec, "type") === "agent_settled";
}
export function agentEndWillRetry(rec) {
  if (stringField(rec, "type") !== "agent_end") return null;
  return rec.willRetry === true;
}
export function statusFromPiEvent(rec) {
  const type = stringField(rec, "type");
  if (type === "compaction_start") return "Compacting context…";
  if (type === "auto_retry_start") {
    const attempt = numberField(rec, "attempt");
    const max = numberField(rec, "maxAttempts");
    if (attempt && max) return `Retrying (${attempt}/${max})…`;
    return "Retrying…";
  }
  if (type === "extension_error") {
    return stringField(rec, "error") ?? "Pi extension error";
  }
  return null;
}
export function tryParseJsonRecord(partial) {
  try {
    return asRecord(JSON.parse(partial));
  } catch {
    return null;
  }
}
export function textFromContent(content) {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  const parts = [];
  for (const item of content) {
    if (typeof item === "string") {
      parts.push(item);
      continue;
    }
    const rec = asRecord(item);
    const text = stringField(rec, "text");
    if (text) parts.push(text);
  }
  return parts.join("");
}
export function toolKindFromName(toolName) {
  const normalized = toolName.toLowerCase();
  if (isTaskListToolName(toolName)) return "tasks";
  if (
    normalized.includes("bash") ||
    normalized.includes("command") ||
    normalized.includes("shell")
  ) {
    return "execute";
  }
  if (
    normalized.includes("edit") ||
    normalized.includes("write") ||
    normalized.includes("patch") ||
    normalized.includes("replace")
  ) {
    return "edit";
  }
  if (normalized === "read" || normalized.includes("read")) return "read";
  if (
    normalized.includes("grep") ||
    normalized.includes("glob") ||
    normalized.includes("search") ||
    normalized.includes("find") ||
    normalized === "ls"
  ) {
    return "search";
  }
  if (normalized === "skill" || normalized === "skills") return "skill";
  if (
    normalized === "agent" ||
    normalized === "task" ||
    normalized === "subagent"
  ) {
    return "agent";
  }
  return toolName;
}
export function toolTitle(name, input) {
  return titleFromToolInput(name, toolKindFromName(name), input);
}
export function previewFromTool(name, input, output) {
  const kind = toolKindFromName(name);
  return extractToolPreview(
    {
      title: name,
      name,
      kind,
      input,
      rawInput: input,
      content: output,
    },
    {
      title: name,
      name,
      kind,
      rawInput: input,
    },
  );
}
export function summarizeToolRequest(toolName, input) {
  const command = stringField(input, "command") ?? stringField(input, "cmd");
  if (command) return `${toolName}: ${command.slice(0, 400)}`;
  const path =
    stringField(input, "path") ??
    stringField(input, "file_path") ??
    stringField(input, "filePath");
  if (path) return `${toolName}: ${path}`;
  try {
    const serialized = JSON.stringify(input);
    if (serialized.length <= 400) return `${toolName}: ${serialized}`;
    return `${toolName}: ${serialized.slice(0, 397)}...`;
  } catch {
    return toolName;
  }
}
/**
 * Copilot's /models endpoint also returns internal agents and dated legacy
 * snapshots that never appear in its own model picker. omp drops the
 * picker/policy flags, so hide those ids by name.
 */
const COPILOT_HIDDEN_MODELS = [
  /^(exec-agent|copilot-search)-/,
  /^trajectory-compaction$/,
  /^gpt-3\.5-turbo/,
  /^gpt-4(-0613|-o-preview)?$/,
  /^gpt-4o-mini/,
  /^gpt-4o-\d{4}-/,
  /^gpt-4\.1-\d{4}-/,
];
export function isHiddenCopilotModel(provider, modelId) {
  return (
    provider === "github-copilot" &&
    COPILOT_HIDDEN_MODELS.some((pattern) => pattern.test(modelId))
  );
}
/** Build the Pi/omp picker catalog with provider labels, sorted by model name. */
export function modelsFromRpcData(flavor, data) {
  const rec = asRecord(data);
  const list = Array.isArray(rec?.models)
    ? rec.models
    : Array.isArray(data)
      ? data
      : [];
  const models = [];
  const seen = new Set();
  for (const item of list) {
    const model = asRecord(item);
    if (!model) continue;
    const modelId = stringField(model, "id");
    const provider = stringField(model, "provider");
    if (!modelId || !provider) continue;
    if (isHiddenCopilotModel(provider, modelId)) continue;
    const nativeId = piNativeId(provider, modelId);
    if (seen.has(nativeId)) continue;
    seen.add(nativeId);
    const name = stringField(model, "name") || modelId;
    const contextWindow = numberField(model, "contextWindow");
    const settings = [
      thinkingSetting(model.reasoning === true),
      flavor.id === "omp" ? fastModeSetting() : undefined,
    ].filter((setting) => setting != null);
    models.push({
      id: `${flavor.id}:${nativeId}`,
      harness: flavor.id,
      name,
      nativeId,
      provider: { id: provider, name: upstreamProviderName(provider) },
      ...(settings.length > 0 ? { settings } : {}),
      ...(contextWindow && contextWindow > 0 ? { contextWindow } : {}),
    });
  }
  return models.sort((left, right) => left.name.localeCompare(right.name));
}
export function thinkingSetting(reasoning) {
  if (!reasoning) return undefined;
  return {
    id: "thinking",
    label: "Thinking",
    kind: "select",
    value: "medium",
    options: PI_THINKING_LEVELS.map((value) => ({
      value,
      label: thinkingLabel(value),
    })),
  };
}
export function fastModeSetting() {
  return {
    id: "fast",
    label: "Fast",
    description: "Use priority processing when the current model supports it",
    kind: "toggle",
    value: "false",
    options: [
      { value: "true", label: "On" },
      { value: "false", label: "Off" },
    ],
  };
}
export function isPiThinkingLevel(value) {
  return !!value && PI_THINKING_LEVELS.includes(value);
}
function thinkingLabel(level) {
  if (level === "xhigh") return "Extra High";
  if (level === "off") return "Off";
  return level.slice(0, 1).toUpperCase() + level.slice(1);
}
function frameUsage(rec) {
  return (
    asRecord(rec.usage) ??
    assistantMessageUsage(rec) ??
    asRecord(asRecord(asRecord(rec.assistantMessageEvent)?.partial)?.usage)
  );
}
function assistantMessageUsage(rec) {
  const message = asRecord(rec.message);
  if (stringField(message, "role") !== "assistant") return null;
  return asRecord(message?.usage);
}
function numberField(rec, key) {
  const value = rec?.[key];
  return typeof value === "number" && Number.isFinite(value)
    ? value
    : undefined;
}
