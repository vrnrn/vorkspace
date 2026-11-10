import { modelsFor } from "../../models/sessions/models.js";
import { killChild, spawnChild, watchChild } from "../../protocol/child.js";
import { settleTextExchange } from "../../protocol/textExchange.js";
import { PiRpc } from "./piClient.js";
import {
  agentEndWillRetry,
  asRecord,
  assistantDeltaFromEvent,
  buildPiPrompt,
  buildPiSpawnArgs,
  isAgentSettled,
} from "./piProtocol.js";

const INIT_TIMEOUT_MS = 15_000;
const REQUEST_TIMEOUT_MS = 45_000;
const turnsByFlavor = new Map();

function pickTextModel(flavor) {
  const models = modelsFor(flavor.id).filter((model) => model.nativeId?.includes("/"));
  const cheap = models.find((model) =>
    /haiku|mini|flash|nano|lite|luna/i.test(
      `${model.nativeId ?? ""} ${model.name} ${model.id}`,
    ),
  );
  return (cheap ?? models[0])?.nativeId?.trim() || undefined;
}

/** One isolated Pi-family request for naming a session. */
export function runTextPrompt(flavor, input) {
  const previous = turnsByFlavor.get(flavor.id) ?? Promise.resolve();
  const run = previous.then(() => runPrompt(flavor, input));
  const tail = run.then(() => undefined, () => undefined);
  turnsByFlavor.set(flavor.id, tail);
  return run;
}

async function runPrompt(
  flavor,
  { cwd, prompt, timeoutMs = REQUEST_TIMEOUT_MS, signal },
) {
  signal?.throwIfAborted();
  const { path } = await flavor.resolveBinary();
  signal?.throwIfAborted();
  const childId = flavor.textChildId;
  let output = "";
  let finish;
  let fail;
  const rpc = new PiRpc(childId, (rec) => {
    if (!finish) return;
    const delta = assistantDeltaFromEvent(rec);
    if (delta?.kind === "text") output += delta.text;
    if (isAgentSettled(rec) || agentEndWillRetry(rec) === false) {
      finish();
      finish = undefined;
    }
  }, flavor.label);
  watchChild(childId, (line) => rpc.pushLine(line), () => {
    const error = new Error(`${flavor.label} text generator exited`);
    rpc.close(error);
    fail?.(error);
  });
  try {
    await spawnChild(childId, path, buildPiSpawnArgs(flavor, {
      isolated: true,
      model: pickTextModel(flavor),
    }), cwd, undefined, flavor.id);
    await rpc.request({ type: "get_state" }, INIT_TIMEOUT_MS);
    signal?.throwIfAborted();
    await rpc.request({ type: "new_session" }).catch(() => undefined);
    await rpc.request({ type: "set_thinking_level", level: "off" }).catch(() => undefined);
    const completed = new Promise((resolve, reject) => {
      finish = resolve;
      fail = reject;
    });
    await settleTextExchange(
      () => rpc.request(buildPiPrompt({ text: prompt }), timeoutMs),
      completed,
      { label: flavor.label, timeoutMs, signal },
    );
    if (output.trim()) return output.trim();
    const reply = await rpc.request({ type: "get_last_assistant_text" }).catch(() => null);
    const text = asRecord(reply?.data)?.text;
    if (typeof text === "string" && text.trim()) return text.trim();
    throw new Error(`${flavor.label} returned empty output.`);
  } finally {
    rpc.close();
    await killChild(childId).catch(() => undefined);
  }
}
