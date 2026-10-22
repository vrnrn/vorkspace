import { modelsFor } from "../../models/sessions/models.js";
import {
  killChild,
  resolveClaudeBinary,
  spawnChild,
  unwatchChild,
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
let live = null;
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
    key: JSON.stringify({ effort, context, thinking, fast, readOnly }),
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
  const run = turns.catch(() => undefined).then(() => promptOnLive(input));
  turns = run.then(
    () => undefined,
    () => undefined,
  );
  return run;
}
async function promptOnLive(input) {
  input.signal?.throwIfAborted();
  const model = pickTextModel(input.model);
  const settings = textSettings(model, input.modelSettings, input.intent);
  const session = await ensureLive(
    input.cwd,
    input.providerAccountId,
    model,
    settings,
  );
  input.signal?.throwIfAborted();
  session.output = "";
  session.collecting = true;
  session.onEvent = input.onEvent;
  session.toolsByIndex = new Map();
  session.toolsById = new Map();
  const timeoutMs = input.timeoutMs ?? REQUEST_TIMEOUT_MS;
  let abortHandler;
  const abortPromise = input.signal
    ? new Promise((_, reject) => {
        const cancel = () => {
          session.turnFailed?.(new Error("By-the-way request cancelled"));
          reject(new Error("By-the-way request cancelled"));
        };
        abortHandler = cancel;
        input.signal.addEventListener("abort", cancel, { once: true });
        if (input.signal.aborted) cancel();
      })
    : null;
  try {
    const turnPromise = new Promise((resolve, reject) => {
      session.turnDone = resolve;
      session.turnFailed = reject;
    });
    await writeChild(
      TEXT_CHILD_ID,
      JSON.stringify(
        buildClaudeUserMessage({
          text: input.prompt,
          effort: settings.promptEffort,
        }),
      ),
    );
    await Promise.race([
      turnPromise,
      new Promise((_, reject) => {
        setTimeout(
          () => reject(new Error("Claude text generation timed out")),
          timeoutMs,
        );
      }),
      ...(abortPromise ? [abortPromise] : []),
    ]);
    const output = session.output.trim();
    if (!output) throw new Error("Claude returned empty output.");
    return output;
  } catch (error) {
    if (session.closed) await dropLive();
    throw error;
  } finally {
    if (abortHandler && input.signal) {
      input.signal.removeEventListener("abort", abortHandler);
    }
    session.collecting = false;
    session.turnDone = null;
    session.turnFailed = null;
    await dropLive();
  }
}
async function ensureLive(
  cwd,
  providerAccountId,
  requestedModel,
  requestedSettings,
) {
  const model = pickTextModel(requestedModel);
  const settings = requestedSettings ?? textSettings(model);
  if (
    live &&
    !live.closed &&
    live.cwd === cwd &&
    live.providerAccountId === providerAccountId &&
    live.model === model &&
    live.settingsKey === settings.key
  ) {
    return live;
  }
  await dropLive();
  return startLive(cwd, providerAccountId, model, settings);
}
async function startLive(
  cwd,
  providerAccountId,
  model = pickTextModel(),
  settings = textSettings(model),
) {
  const { path } = await resolveClaudeBinary();
  const session = {
    cwd,
    providerAccountId,
    model,
    settingsKey: settings.key,
    collecting: false,
    output: "",
    closed: false,
    ready: false,
    turnDone: null,
    turnFailed: null,
    readyDone: null,
    toolsByIndex: new Map(),
    toolsById: new Map(),
  };
  watchChild(
    TEXT_CHILD_ID,
    (line) => handleLine(session, line),
    () => {
      session.closed = true;
      if (live === session) live = null;
      session.turnFailed?.(new Error("Claude text generator exited"));
      session.readyDone?.();
      session.turnDone = null;
      session.turnFailed = null;
      session.readyDone = null;
    },
  );
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
      cwd,
      { provider: "claude", id: providerAccountId ?? "default" },
      "claude",
    );
    live = session;
    await waitForReady(session, INIT_TIMEOUT_MS);
    return session;
  } catch (error) {
    session.closed = true;
    unwatchChild(TEXT_CHILD_ID);
    await killChild(TEXT_CHILD_ID).catch(() => undefined);
    throw error;
  }
}
async function dropLive() {
  const current = live;
  live = null;
  if (current) {
    current.closed = true;
    current.readyDone?.();
    current.turnFailed?.(new Error("Claude text generator stopped"));
    current.turnDone = null;
    current.turnFailed = null;
    current.readyDone = null;
  }
  unwatchChild(TEXT_CHILD_ID);
  await killChild(TEXT_CHILD_ID).catch(() => undefined);
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
  if (!session.collecting) return;
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
    session.toolsById.set(started.id, tool);
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
