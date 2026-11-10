import { modelsFor } from "../../models/sessions/models.js";
import {
  killChild,
  resolveCodexBinary,
  spawnChild,
  watchChild,
} from "../../protocol/child.js";
import { JsonRpcClient } from "../../protocol/jsonRpc.js";
import { snapshotRemainder, streamTextDelta } from "../../protocol/streamText.js";
import { settleTextExchange } from "../../protocol/textExchange.js";
import {
  asRecord,
  buildThreadStartParams,
  buildTurnStartParams,
  stringField,
} from "./codexProtocol.js";

const CHILD_ID = "vorkspace-codex-text";
const INIT_TIMEOUT_MS = 60_000;
const REQUEST_TIMEOUT_MS = 45_000;
const RUNTIME_MODE = "supervised";
let turns = Promise.resolve();

function modelDefaults() {
  const catalog = modelsFor("codex");
  const model = catalog.find((entry) =>
    /5\.6-luna/i.test(`${entry.nativeId ?? ""} ${entry.name} ${entry.id}`),
  )?.nativeId ?? "gpt-5.6-luna";
  const setting = catalog
    .find((entry) => entry.nativeId === model)?.settings
    ?.find((entry) => entry.id === "reasoningEffort");
  const options = setting?.options?.map((option) => option.value) ?? [];
  const effort = ["low", "none", setting?.value].find((value) =>
    options.includes(value),
  ) ?? "low";
  return { model, effort };
}

/** One ephemeral Codex request for naming a session or branch. */
export function runCodexTextPrompt(input) {
  const run = turns.then(() => runPrompt(input));
  turns = run.then(() => undefined, () => undefined);
  return run;
}

async function runPrompt({
  cwd,
  providerAccountId,
  prompt,
  timeoutMs = REQUEST_TIMEOUT_MS,
  signal,
}) {
  signal?.throwIfAborted();
  const { path } = await resolveCodexBinary();
  signal?.throwIfAborted();
  const session = {
    threadId: "", messages: new Map(), output: "", finish: null, fail: null,
  };
  const rpc = new JsonRpcClient(CHILD_ID, {
    onNotification: (method, params) => collectText(session, method, params),
    onRequest: (id, method) => {
      let result = {};
      if (method === "item/commandExecution/requestApproval" ||
        method === "item/fileChange/requestApproval"
      ) {
        result = { decision: "decline" };
      } else if (method === "item/permissions/requestApproval") {
        result = { permissions: {} };
      } else if (method === "currentTime/read") {
        result = { currentTimeAt: Math.floor(Date.now() / 1000) };
      }
      void rpc.respond(id, result).catch(() => undefined);
    },
  }, { includeJsonrpc: false, label: "codex-text" });
  watchChild(CHILD_ID, (line) => rpc.pushLine(line), () => {
    const error = new Error("Codex text generator exited");
    rpc.close(error);
    session.fail?.(error);
  });
  try {
    await spawnChild(
      CHILD_ID, path, ["app-server"], cwd,
      { provider: "codex", id: providerAccountId ?? "default" }, "codex",
    );
    signal?.throwIfAborted();
    await rpc.request("initialize", {
      clientInfo: { name: "vorkspace-text", title: "vorkspace", version: "0.1.0" },
      capabilities: { experimentalApi: true },
    }, INIT_TIMEOUT_MS);
    await rpc.notify("initialized", undefined);
    signal?.throwIfAborted();
    const { model, effort } = modelDefaults();
    const opened = await rpc.request("thread/start", {
      ...buildThreadStartParams({ cwd, runtimeMode: RUNTIME_MODE, model }),
      ephemeral: true,
    }, INIT_TIMEOUT_MS);
    session.threadId = opened.thread?.id?.trim();
    if (!session.threadId) throw new Error("Codex did not return a thread id");
    const params = buildTurnStartParams({
      threadId: session.threadId,
      runtimeMode: RUNTIME_MODE,
      prompt,
      model,
      effort,
    });
    const completed = new Promise((resolve, reject) => {
      session.finish = resolve;
      session.fail = reject;
    });
    await settleTextExchange(
      () => rpc.request("turn/start", params, timeoutMs),
      completed,
      { label: "Codex", timeoutMs, signal },
    );
    return session.output;
  } finally {
    rpc.close();
    await killChild(CHILD_ID).catch(() => undefined);
  }
}

function collectText(session, method, params) {
  if (!session.finish) return;
  const rec = asRecord(params);
  const threadId = stringField(rec, "threadId");
  if (threadId && threadId !== session.threadId) return;

  const item = asRecord(rec?.item);
  const snapshot = method === "item/completed" && item?.type === "agentMessage";
  if (snapshot || method === "item/agentMessage/delta") {
    const id = (snapshot ? stringField(item, "id") : stringField(rec, "itemId")) ?? "";
    const current = session.messages.get(id) ?? "";
    const text = streamTextDelta(snapshot ? item.text : rec?.delta);
    const delta = snapshot ? snapshotRemainder(current, text) : text;
    if (delta) {
      session.messages.set(id, current + delta);
      session.output += delta;
    }
    return;
  }
  if (method !== "turn/completed" && method !== "turn/aborted") return;
  const turn = asRecord(rec?.turn);
  if (turn?.status === "failed") {
    const message = stringField(asRecord(turn.error), "message");
    session.fail(new Error(message ?? "Codex turn failed"));
  } else {
    session.finish();
  }
  session.finish = null;
  session.fail = null;
}
