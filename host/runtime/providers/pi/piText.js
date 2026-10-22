import { modelsFor } from "../../models/sessions/models.js";
import {
  killChild,
  spawnChild,
  unwatchChild,
  watchChild,
} from "../../protocol/child.js";
import { PiRpc } from "./piClient.js";
import {
  agentEndWillRetry,
  asRecord,
  assistantDeltaFromEvent,
  buildPiPrompt,
  buildPiSpawnArgs,
  isAgentSettled,
  isPiThinkingLevel,
} from "./piProtocol.js";
import { abortTextPromptRace } from "../../protocol/abortTextPrompt.js";
import { mergeStream } from "../../protocol/streamText.js";
const INIT_TIMEOUT_MS = 15000;
const REQUEST_TIMEOUT_MS = 45000;
const stateByFlavor = new Map();
function stateFor(flavor) {
  let state = stateByFlavor.get(flavor.id);
  if (!state) {
    state = { live: null, turns: Promise.resolve() };
    stateByFlavor.set(flavor.id, state);
  }
  return state;
}
function pickTextModel(flavor, requested) {
  const selected = requested?.trim();
  if (selected?.includes("/")) return selected;
  const models = modelsFor(flavor.id).filter((model) =>
    Boolean(model.nativeId?.includes("/")),
  );
  const cheap = models.find((model) =>
    /haiku|mini|flash|nano|lite|luna/i.test(
      `${model.nativeId ?? ""} ${model.name} ${model.id}`,
    ),
  );
  return (cheap ?? models[0])?.nativeId?.trim() || undefined;
}
export async function runTextPrompt(flavor, input) {
  const state = stateFor(flavor);
  const run = state.turns
    .catch(() => undefined)
    .then(() => promptOnLive(flavor, input));
  state.turns = run.then(
    () => undefined,
    () => undefined,
  );
  return run;
}
async function promptOnLive(flavor, input) {
  const session = await ensureLive(
    flavor,
    input.cwd,
    input.model,
    input.modelSettings,
  );
  const timeoutMs = input.timeoutMs ?? REQUEST_TIMEOUT_MS;
  try {
    await session.rpc.request({ type: "new_session" }).catch(() => undefined);
    const thinking = input.modelSettings?.thinking;
    await session.rpc
      .request({
        type: "set_thinking_level",
        level: isPiThinkingLevel(thinking) ? thinking : "off",
      })
      .catch(() => undefined);
    const fast = input.modelSettings?.fast;
    if (flavor.id === "omp" && (fast === "true" || fast === "false")) {
      await session.rpc
        .request({ type: "set_fast_mode", enabled: fast === "true" })
        .catch(() => undefined);
    }
    session.output = "";
    session.collecting = true;
    session.onEvent = input.onEvent;
    session.turnEndPending = false;
    const turnPromise = new Promise((resolve, reject) => {
      session.turnDone = resolve;
      session.turnFailed = reject;
    });
    const abort = abortTextPromptRace(input.signal, () => {
      session.turnFailed?.(new Error("By-the-way request cancelled"));
      void session.rpc.request({ type: "abort" }).catch(() => undefined);
    });
    await session.rpc.request(buildPiPrompt({ text: input.prompt }), timeoutMs);
    if (session.turnEndPending) finishTurn(session);
    let timer;
    try {
      await Promise.race([
        turnPromise,
        new Promise((_, reject) => {
          timer = setTimeout(
            () =>
              reject(new Error(`${flavor.label} text generation timed out`)),
            timeoutMs,
          );
        }),
        ...(abort.promise ? [abort.promise] : []),
      ]);
    } finally {
      abort.detach();
      if (timer) clearTimeout(timer);
    }
    let output = session.output.trim();
    if (!output) {
      const rec = await session.rpc
        .request({ type: "get_last_assistant_text" })
        .catch(() => null);
      const text = asRecord(rec?.data)?.text;
      if (typeof text === "string") output = text.trim();
    }
    if (!output) throw new Error(`${flavor.label} returned empty output.`);
    await dropLive(flavor);
    return output;
  } catch (error) {
    session.turnDone = null;
    session.turnFailed = null;
    await session.rpc.request({ type: "abort" }).catch(() => undefined);
    await dropLive(flavor);
    throw error;
  } finally {
    session.collecting = false;
    session.onEvent = undefined;
    session.turnDone = null;
    session.turnFailed = null;
  }
}
async function ensureLive(flavor, cwd, requestedModel, modelSettings) {
  const state = stateFor(flavor);
  const model = pickTextModel(flavor, requestedModel);
  const settingsKey = modelSettingsKey(modelSettings);
  const current = state.live;
  if (
    current &&
    !current.closed &&
    current.cwd === cwd &&
    current.model === model &&
    current.settingsKey === settingsKey
  ) {
    return current;
  }
  await dropLive(flavor);
  return startLive(flavor, cwd, model, modelSettings);
}
async function startLive(flavor, cwd, model, modelSettings) {
  const state = stateFor(flavor);
  const childId = flavor.textChildId;
  const { path } = await flavor.resolveBinary();
  const liveRef = { current: null };
  const rpc = new PiRpc(
    childId,
    (rec) => {
      const current = liveRef.current;
      if (current) handleFrame(current, rec);
    },
    flavor.label,
  );
  const session = {
    rpc,
    cwd,
    model,
    settingsKey: modelSettingsKey(modelSettings),
    collecting: false,
    output: "",
    closed: false,
    turnDone: null,
    turnFailed: null,
    turnEndPending: false,
    onEvent: undefined,
  };
  liveRef.current = session;
  watchChild(
    childId,
    (line) => rpc.pushLine(line),
    () => {
      session.closed = true;
      if (state.live === session) state.live = null;
      const exited = new Error(`${flavor.label} text generator exited`);
      rpc.close(exited);
      session.turnFailed?.(exited);
      session.turnDone = null;
      session.turnFailed = null;
    },
  );
  try {
    await spawnChild(
      childId,
      path,
      buildPiSpawnArgs(flavor, {
        isolated: true,
        model,
      }),
      cwd,
      undefined,
      flavor.id,
    );
    await rpc.request({ type: "get_state" }, INIT_TIMEOUT_MS);
    state.live = session;
    return session;
  } catch (error) {
    session.closed = true;
    rpc.close(error instanceof Error ? error : new Error(String(error)));
    unwatchChild(childId);
    await killChild(childId).catch(() => undefined);
    throw error;
  }
}
async function dropLive(flavor) {
  const state = stateFor(flavor);
  const current = state.live;
  const childId = flavor.textChildId;
  state.live = null;
  if (current) {
    current.closed = true;
    current.rpc.close();
    current.turnFailed?.(new Error(`${flavor.label} text generator stopped`));
    current.turnDone = null;
    current.turnFailed = null;
  }
  unwatchChild(childId);
  await killChild(childId).catch(() => undefined);
}
function handleFrame(session, rec) {
  if (!session.collecting) return;
  const delta = assistantDeltaFromEvent(rec);
  if (delta?.kind === "text") {
    session.output = mergeStream(session.output, delta.text);
    session.onEvent?.({ type: "message.delta", text: delta.text });
  } else if (delta?.kind === "thinking") {
    session.onEvent?.({ type: "reasoning.delta", text: delta.text });
  }
  if (isAgentSettled(rec) || agentEndWillRetry(rec) === false) {
    if (session.turnDone) finishTurn(session);
    else session.turnEndPending = true;
  }
}
function finishTurn(session) {
  session.turnEndPending = false;
  const done = session.turnDone;
  session.turnDone = null;
  session.turnFailed = null;
  done?.();
}
function modelSettingsKey(settings) {
  return JSON.stringify({
    thinking: settings?.thinking,
    fast: settings?.fast,
  });
}
