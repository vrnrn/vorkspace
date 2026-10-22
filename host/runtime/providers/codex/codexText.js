import { modelsFor } from "../../models/sessions/models.js";
import {
  killChild,
  resolveCodexBinary,
  restoreBotCodexAgentState,
  spawnChild,
  unwatchChild,
  watchChild,
} from "../../protocol/child.js";
import {
  asRecord,
  buildThreadStartParams,
  buildTurnStartParams,
  isRecoverableThreadResumeError,
  mapCodexNotification,
  stringField,
} from "./codexProtocol.js";
import { JsonRpcClient } from "../../protocol/jsonRpc.js";
import { prepareCodexBotContext } from "./codexStore.js";
import { mergeStream, streamTextDelta } from "../../protocol/streamText.js";
const TEXT_CHILD_ID = "vorkspace-codex-text";
const INIT_TIMEOUT_MS = 60000;
const REQUEST_TIMEOUT_MS = 45000;
const TEXT_RUNTIME_MODE = "supervised";
const TEXT_MODEL = "gpt-5.6-luna";
const TEXT_EFFORT = "low";
let live = null;
let turns = Promise.resolve();
function pickTextModel(requested) {
  const selected = requested?.trim();
  if (selected) return selected;
  const models = modelsFor("codex");
  const luna = models.find((model) =>
    /5\.6-luna/i.test(`${model.nativeId ?? ""} ${model.name} ${model.id}`),
  );
  return luna?.nativeId ?? TEXT_MODEL;
}
function pickTextEffort(modelId, modelSettings) {
  const requested =
    modelSettings?.reasoningEffort?.trim() || modelSettings?.effort?.trim();
  const model = modelsFor("codex").find((entry) => entry.nativeId === modelId);
  const setting = model?.settings?.find(
    (entry) => entry.id === "reasoningEffort",
  );
  const options = setting?.options?.map((option) => option.value) ?? [];
  if (requested && (options.length === 0 || options.includes(requested))) {
    return requested;
  }
  if (options.includes("low")) return "low";
  if (options.includes("none")) return "none";
  if (setting?.value && options.includes(setting.value)) return setting.value;
  return TEXT_EFFORT;
}
function pickTextServiceTier(modelSettings) {
  const value = modelSettings?.serviceTier?.trim();
  return value || undefined;
}
/** Codex app-server turn that reuses a warm process. */
export async function runCodexTextPrompt(input) {
  if (input.model !== undefined && !input.model.trim()) {
    throw new Error("The selected Codex model is unavailable.");
  }
  const run = turns.catch(() => undefined).then(() => promptOnLive(input));
  turns = run.then(
    () => undefined,
    () => undefined,
  );
  return run;
}
async function promptOnLive(input) {
  input.signal?.throwIfAborted();
  const session = await ensureLive(input);
  input.signal?.throwIfAborted();
  session.output = "";
  session.collecting = true;
  session.onEvent = input.onEvent;
  const timeoutMs = input.timeoutMs ?? REQUEST_TIMEOUT_MS;
  let abortHandler;
  const abortPromise = input.signal
    ? new Promise((_, reject) => {
        abortHandler = () => {
          void session.rpc
            .request("turn/interrupt", { threadId: session.threadId })
            .catch(() => undefined);
          reject(new Error("By-the-way request cancelled"));
        };
        input.signal.addEventListener("abort", abortHandler, { once: true });
        if (input.signal.aborted) abortHandler();
      })
    : null;
  try {
    const turnPromise = new Promise((resolve, reject) => {
      session.turnDone = resolve;
      session.turnFailed = reject;
    });
    const turnStart = session.rpc.request(
      "turn/start",
      buildTurnStartParams({
        threadId: session.threadId,
        runtimeMode: TEXT_RUNTIME_MODE,
        prompt: input.prompt,
        model: session.model || undefined,
        effort: session.effort,
        serviceTier: session.serviceTier,
        intent: input.intent,
      }),
      timeoutMs,
    );
    await Promise.race([turnStart, ...(abortPromise ? [abortPromise] : [])]);
    await Promise.race([
      turnPromise,
      new Promise((_, reject) => {
        setTimeout(
          () => reject(new Error("Codex text generation timed out")),
          timeoutMs,
        );
      }),
      ...(abortPromise ? [abortPromise] : []),
    ]);
    return session.output;
  } catch (error) {
    await session.rpc
      .request("turn/interrupt", { threadId: session.threadId })
      .catch(() => undefined);
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
async function ensureLive(input) {
  const model = pickTextModel(input.model);
  const effort = pickTextEffort(model, input.modelSettings);
  const serviceTier = pickTextServiceTier(input.modelSettings);
  // Titles, commit messages, branch names, and PR content must never become
  // saved Codex conversations. Only resumable side questions opt into storage.
  const ephemeral = input.ephemeral ?? true;
  const requestedThreadId = ephemeral
    ? undefined
    : input.threadId?.trim() || undefined;
  if (live && !live.closed) {
    if (
      live.cwd === input.cwd &&
      live.model === model &&
      live.effort === effort &&
      live.serviceTier === serviceTier &&
      live.providerAccountId === input.providerAccountId &&
      live.ephemeral === ephemeral &&
      live.codexStore === input.codexStore &&
      (!requestedThreadId || live.threadId === requestedThreadId)
    ) {
      input.onThreadId?.(live.threadId);
      return live;
    }
    if (
      live.providerAccountId !== input.providerAccountId ||
      live.codexStore !== input.codexStore
    ) {
      await dropLive();
      const started = await startLive(
        input.cwd,
        input.providerAccountId,
        model,
        effort,
        serviceTier,
        requestedThreadId,
        ephemeral,
        input.codexStore,
      );
      input.onThreadId?.(started.threadId);
      return started;
    }
    try {
      live.model = model;
      live.effort = effort;
      live.serviceTier = serviceTier;
      await openThread(live, input.cwd, requestedThreadId, ephemeral);
      input.onThreadId?.(live.threadId);
      return live;
    } catch (error) {
      await dropLive();
      throw error;
    }
  }
  const started = await startLive(
    input.cwd,
    input.providerAccountId,
    model,
    effort,
    serviceTier,
    requestedThreadId,
    ephemeral,
    input.codexStore,
  );
  input.onThreadId?.(started.threadId);
  return started;
}
async function startLive(
  cwd,
  providerAccountId,
  model = pickTextModel(),
  effort = pickTextEffort(model),
  serviceTier,
  requestedThreadId,
  ephemeral = true,
  codexStore,
) {
  await dropLive();
  const { path } = await resolveCodexBinary();
  const store =
    codexStore === "bot"
      ? await prepareCodexBotContext({
          sessionId: TEXT_CHILD_ID,
          path,
          cwd,
          providerAccountId,
          threadId: requestedThreadId,
        })
      : undefined;
  const sessionRef = { session: null };
  const rpc = new JsonRpcClient(
    TEXT_CHILD_ID,
    {
      onNotification: (method, params) => {
        handleNotification(sessionRef.session, method, params);
      },
      onRequest: (id, method, params) => {
        void handleServerRequest(rpc, id, method, params);
      },
    },
    { includeJsonrpc: false, label: "codex-text" },
  );
  const session = {
    rpc,
    cwd,
    providerAccountId,
    threadId: "",
    ephemeral,
    codexStore,
    model,
    effort,
    serviceTier,
    collecting: false,
    output: "",
    closed: false,
    turnDone: null,
    turnFailed: null,
  };
  sessionRef.session = session;
  watchChild(
    TEXT_CHILD_ID,
    (line) => rpc.pushLine(line),
    () => {
      session.closed = true;
      if (live === session) live = null;
      session.turnFailed?.(new Error("Codex text generator exited"));
      session.turnDone = null;
      session.turnFailed = null;
      rpc.close(new Error("Codex text generator exited"));
    },
  );
  try {
    await spawnChild(
      TEXT_CHILD_ID,
      path,
      ["app-server"],
      cwd,
      {
        provider: "codex",
        id: providerAccountId ?? "default",
      },
      "codex",
      codexStore,
    );
    await rpc.request(
      "initialize",
      {
        clientInfo: {
          name: "vorkspace-text",
          title: "vorkspace",
          version: "0.1.0",
        },
        capabilities: { experimentalApi: true },
      },
      INIT_TIMEOUT_MS,
    );
    await rpc.notify("initialized", undefined);
    await openThread(session, cwd, requestedThreadId, ephemeral, store);
    live = session;
    return session;
  } catch (error) {
    session.closed = true;
    rpc.close(error instanceof Error ? error : new Error(String(error)));
    unwatchChild(TEXT_CHILD_ID);
    await killChild(TEXT_CHILD_ID).catch(() => undefined);
    throw error;
  }
}
async function openThread(
  session,
  cwd,
  requestedThreadId,
  ephemeral,
  preparedStore,
) {
  let opened;
  let hasSavedContext = false;
  if (session.codexStore === "bot") {
    const { path } = await resolveCodexBinary();
    const store =
      preparedStore ??
      (await prepareCodexBotContext({
        sessionId: TEXT_CHILD_ID,
        path,
        cwd,
        providerAccountId: session.providerAccountId,
        threadId: requestedThreadId,
      }));
    session.storeConfig = store.config;
    hasSavedContext = store.hasThread;
    if (requestedThreadId && hasSavedContext)
      await restoreBotCodexAgentState(
        session.providerAccountId,
        requestedThreadId,
      );
  }
  const params = buildThreadStartParams({
    cwd,
    runtimeMode: TEXT_RUNTIME_MODE,
    model: session.model || undefined,
    serviceTier: session.serviceTier,
  });
  if (session.storeConfig)
    params.config = {
      ...session.storeConfig,
      ...params.config,
    };
  if (requestedThreadId) {
    try {
      opened = await session.rpc.request(
        "thread/resume",
        {
          threadId: requestedThreadId,
          ...params,
        },
        INIT_TIMEOUT_MS,
      );
    } catch (error) {
      if (hasSavedContext) throw error;
      if (!isRecoverableThreadResumeError(error)) throw error;
      opened = undefined;
    }
    if (!opened?.thread?.id?.trim()) opened = undefined;
  }
  if (!opened) {
    opened = await session.rpc.request(
      "thread/start",
      {
        ...params,
        ...(ephemeral ? { ephemeral: true } : {}),
      },
      INIT_TIMEOUT_MS,
    );
  }
  const threadId = opened.thread?.id?.trim();
  if (!threadId) throw new Error("Codex did not return a thread id");
  session.cwd = cwd;
  session.threadId = threadId;
  session.ephemeral = ephemeral;
}
async function dropLive() {
  const current = live;
  live = null;
  if (current) {
    current.closed = true;
    current.rpc.close();
  }
  unwatchChild(TEXT_CHILD_ID);
  await killChild(TEXT_CHILD_ID).catch(() => undefined);
}
function handleNotification(session, method, params) {
  if (!session || !session.collecting) {
    if (session && method === "turn/completed") {
      session.turnDone?.();
      session.turnDone = null;
      session.turnFailed = null;
    }
    return;
  }
  const mapped = mapCodexNotification(method, params);
  for (const event of mapped.events) {
    session.onEvent?.(event);
  }
  if (method === "item/agentMessage/delta") {
    const delta = streamTextDelta(asRecord(params)?.delta);
    if (delta) session.output = mergeStream(session.output, delta);
    return;
  }
  if (method === "turn/completed") {
    const turn = asRecord(asRecord(params)?.turn);
    const status = stringField(turn, "status") ?? "completed";
    if (status === "failed") {
      const message =
        stringField(asRecord(turn?.error), "message") ?? "Codex turn failed";
      session.turnFailed?.(new Error(message));
    } else {
      session.turnDone?.();
    }
    session.turnDone = null;
    session.turnFailed = null;
  }
}
async function handleServerRequest(rpc, id, method, _params) {
  if (
    method === "item/commandExecution/requestApproval" ||
    method === "item/fileChange/requestApproval"
  ) {
    await rpc.respond(id, { decision: "decline" }).catch(() => undefined);
    return;
  }
  if (method === "item/permissions/requestApproval") {
    await rpc.respond(id, { permissions: {} }).catch(() => undefined);
    return;
  }
  await rpc.respond(id, {}).catch(() => undefined);
}
