import { modelsFor } from "../../models/sessions/models.js";
import {
  killChild,
  resolveClaudeBinary,
  spawnChild,
  watchChild,
  writeChild,
} from "../../protocol/child.js";
import { isAgentToolName } from "../../protocol/preview.js";
import {
  assistantTextBlocks,
  buildClaudeSpawnArgs,
  buildClaudeUserMessage,
  inputJsonDeltaFromEvent,
  isClaudeUltracodeEffort,
  normalizeClaudeCliEffort,
  parseJsonLine,
  previewFromTool,
  resolveClaudeApiModelId,
  streamDeltaFromEvent,
  stringField,
  summarizeToolRequest,
  toolKindFromName,
  toolStartFromEvent,
  toolTitle,
  tryParseJsonRecord,
  turnStatusFromResult,
} from "./claudeProtocol.js";
import { mergeStream } from "../../protocol/streamText.js";
const TEXT_CHILD_ID = "vorkspace-claude-text";
const INIT_TIMEOUT_MS = 8000;
const REQUEST_TIMEOUT_MS = 45000;
const TEXT_MODEL = "claude-haiku-4-5";
let turns = Promise.resolve();
function textSettings(model, modelSettings, intent) {
  const effort = modelSettings?.effort?.trim() || undefined;
  const context = modelSettings?.context?.trim() || undefined;
  const thinking = modelSettings?.thinking === "true";
  const fast = modelSettings?.fast === "true";
  const readOnly = intent === "plan";
  const settings = {};
  if (thinking) settings.alwaysThinkingEnabled = true;
  if (fast) settings.fastMode = true;
  if (isClaudeUltracodeEffort(effort)) settings.ultracode = true;
  return {
    launchModel: resolveClaudeApiModelId(model, context),
    effort: normalizeClaudeCliEffort(effort, model),
    promptEffort: effort,
    settings,
    ...(readOnly ? { permissionMode: "plan", maxTurns: 1 } : {}),
  };
}
function pickTextModel(requested) {
  const selected = requested?.trim();
  if (selected) return selected;
  const models = modelsFor("claude");
  const haiku = models.find((model) =>
    /haiku/i.test(`${model.nativeId ?? ""} ${model.name} ${model.id}`),
  );
  return haiku?.nativeId ?? TEXT_MODEL;
}
export async function runClaudeTextPrompt(input) {
  const run = turns.then(() => runPrompt(input));
  turns = run.then(
    () => undefined,
    () => undefined,
  );
  return run;
}
async function runPrompt(input) {
  input.signal?.throwIfAborted();
  const model = pickTextModel(input.model);
  const settings = textSettings(model, input.modelSettings, input.intent);
  const { path } = await resolveClaudeBinary();
  input.signal?.throwIfAborted();
  const session = {
    onEvent: input.onEvent,
    output: "",
    closed: false,
    ready: false,
    turnDone: null,
    turnFailed: null,
    readyDone: null,
    toolsByIndex: new Map(),
  };
  watchChild(
    TEXT_CHILD_ID,
    (line) => handleLine(session, line),
    () => {
      session.closed = true;
      session.turnFailed?.(new Error("Claude text generator exited"));
      session.readyDone?.();
      session.turnDone = null;
      session.turnFailed = null;
      session.readyDone = null;
    },
  );
  let timeout;
  let failRequest;
  const abort = () => {
    failRequest?.(new Error("By-the-way request cancelled"));
  };
  try {
    await spawnChild(
      TEXT_CHILD_ID,
      path,
      buildClaudeSpawnArgs({
        isolated: true,
        model: settings.launchModel,
        effort: settings.effort,
        settings: settings.settings,
        permissionMode: settings.permissionMode,
        maxTurns: settings.maxTurns,
      }),
      input.cwd,
      { provider: "claude", id: input.providerAccountId ?? "default" },
      "claude",
    );
    await waitForReady(session, INIT_TIMEOUT_MS);
    input.signal?.throwIfAborted();
    const message = buildClaudeUserMessage({
      text: input.prompt,
      effort: settings.promptEffort,
    });
    const completion = new Promise((resolve, reject) => {
      session.turnDone = resolve;
      session.turnFailed = reject;
    });
    const deadline = new Promise((_, reject) => {
      failRequest = reject;
      timeout = setTimeout(
        () => reject(new Error("Claude text generation timed out")),
        input.timeoutMs ?? REQUEST_TIMEOUT_MS,
      );
    });
    input.signal?.addEventListener("abort", abort, { once: true });
    await Promise.race([
      // The child can finish or exit before its stdin write is acknowledged.
      Promise.all([
        completion,
        Promise.resolve().then(() => writeChild(TEXT_CHILD_ID, JSON.stringify(message))),
      ]),
      deadline,
    ]);
    const output = session.output.trim();
    if (!output) throw new Error("Claude returned empty output.");
    return output;
  } finally {
    clearTimeout(timeout);
    input.signal?.removeEventListener("abort", abort);
    await killChild(TEXT_CHILD_ID).catch(() => undefined);
  }
}
function handleLine(session, line) {
  const rec = parseJsonLine(line);
  if (!rec) return;
  const type = stringField(rec, "type");
  if (
    type === "system" &&
    (stringField(rec, "subtype") === "init" ||
      stringField(rec, "subtype") === "initialized")
  ) {
    session.ready = true;
    session.readyDone?.();
    session.readyDone = null;
  }
  if (!session.turnDone) return;
  if (type === "assistant") {
    const snapshot = assistantTextBlocks(rec).join("");
    if (snapshot) session.output = mergeStream(session.output, snapshot);
    return;
  }
  if (type === "stream_event") {
    handleStreamEvent(session, rec);
    return;
  }
  if (type === "result") {
    const result = turnStatusFromResult(rec);
    if (result.status === "failed") {
      session.turnFailed?.(new Error(result.error ?? "Claude turn failed"));
    } else {
      session.turnDone?.();
    }
    session.turnDone = null;
    session.turnFailed = null;
  }
}
function handleStreamEvent(session, rec) {
  const delta = streamDeltaFromEvent(rec);
  if (delta) {
    if (delta.kind === "assistant") {
      session.output = mergeStream(session.output, delta.text);
      session.onEvent?.({ type: "message.delta", text: delta.text });
    } else {
      session.onEvent?.({ type: "reasoning.delta", text: delta.text });
    }
    return;
  }
  const started = toolStartFromEvent(rec);
  if (started) {
    const tool = {
      id: started.id,
      name: started.name,
      input: started.input,
      partialJson: "",
      title: toolTitle(started.name, started.input),
    };
    if (started.index >= 0) session.toolsByIndex.set(started.index, tool);
    session.onEvent?.({
      type: "tool.started",
      callId: tool.id,
      title: tool.title,
      kind: toolKindFromName(tool.name),
      ...(isAgentToolName(tool.name) && stringField(tool.input, "model")
        ? { agentModel: stringField(tool.input, "model") }
        : {}),
      status: isAgentToolName(tool.name) ? "in_progress" : "pending",
      preview: previewFromTool(tool.name, tool.input),
    });
    return;
  }
  const jsonDelta = inputJsonDeltaFromEvent(rec);
  if (!jsonDelta) return;
  const tool = session.toolsByIndex.get(jsonDelta.index);
  if (!tool) return;
  tool.partialJson += jsonDelta.partial;
  const parsed = tryParseJsonRecord(tool.partialJson);
  if (!parsed) return;
  tool.input = parsed;
  tool.title = toolTitle(tool.name, parsed);
  session.onEvent?.({
    type: "tool.updated",
    callId: tool.id,
    title: tool.title,
    kind: toolKindFromName(tool.name),
    ...(isAgentToolName(tool.name) && stringField(tool.input, "model")
      ? { agentModel: stringField(tool.input, "model") }
      : {}),
    status: "pending",
    detail: summarizeToolRequest(tool.name, parsed),
    preview: previewFromTool(tool.name, parsed),
  });
}
function waitForReady(session, timeoutMs) {
  if (session.closed) return Promise.reject(new Error("Claude text generator exited"));
  if (session.ready) return Promise.resolve();
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      session.readyDone = null;
      resolve();
    }, timeoutMs);
    session.readyDone = () => {
      clearTimeout(timer);
      if (session.closed) {
        reject(new Error("Claude text generator exited"));
        return;
      }
      resolve();
    };
  });
}
