import { runtimeProviderBinaryPath } from "../models/providers/providerBinaryPaths.js";
let backend;
export function configureChildBackend(next) {
  if (bridge || users)
    throw new Error("Configure the child backend before starting the bridge");
  backend = next;
}
export function prepareBotCodexStore(providerAccountId, threadId) {
  return invoke("codex_bot_store_prepare", { providerAccountId, threadId });
}
export function copyBotCodexThreads(
  providerAccountId,
  threadId,
  paths,
  sqliteHome,
) {
  return invoke("codex_bot_store_copy", {
    providerAccountId,
    threadId,
    paths,
    sqliteHome,
  });
}
export function restoreBotCodexAgentState(providerAccountId, threadId) {
  return invoke("codex_bot_store_restore_agent_state", {
    providerAccountId,
    threadId,
  });
}
function invoke(command, args) {
  if (!backend) throw new Error("Configure the host child backend before use");
  return backend.invoke(command, args);
}
function listen(event, handler) {
  if (!backend) throw new Error("Configure the host child backend before use");
  return backend.listen(event, handler);
}
const lineHandlers = new Map();
const exitHandlers = new Map();
const lineBuffer = new Map();
const outputFailures = new Map();
const stderrHandlers = new Map();
// Output is broadcast to every window. Only ids this window spawned or opened
// may buffer while unwatched; anything else would be held until the bridge
// is torn down, since nothing here ever watches or unwatches it.
const ownedChildren = new Set();
const livePid = new Map();
const pendingExit = new Map();
/** True when this exit belongs to the child we currently have spawned. */
export function isCurrentChildExit(expectedPid, exitedPid) {
  if (expectedPid == null || expectedPid <= 0) return false;
  if (exitedPid == null || exitedPid <= 0) return false;
  return exitedPid === expectedPid;
}
const PREWATCH_OUTPUT_LINES = 1000;
const PREWATCH_OUTPUT_BYTES = 16 * 1024 * 1024;
/** Pre-watch protocol output is a prefix, never a silently truncated suffix.
 * Once admission fails the bridge stops the run and reports a failed exit. */
export class ChildOutputBuffer {
  maxLines;
  maxBytes;
  lines = [];
  bytes = 0;
  constructor(
    maxLines = PREWATCH_OUTPUT_LINES,
    maxBytes = PREWATCH_OUTPUT_BYTES,
  ) {
    this.maxLines = maxLines;
    this.maxBytes = maxBytes;
  }
  push(line) {
    const bytes = new TextEncoder().encode(line).byteLength;
    if (
      this.lines.length >= this.maxLines ||
      bytes > this.maxBytes - this.bytes
    )
      return false;
    this.lines.push(line);
    this.bytes += bytes;
    return true;
  }
}
let bridge = null;
let bridgeAttempt = null;
let users = 0;
let teardownTimer;
function deliverOutputFailure(sessionId) {
  const failure = outputFailures.get(sessionId);
  const onExit = exitHandlers.get(sessionId);
  if (!failure || failure.delivered || !onExit) return;
  failure.delivered = true;
  livePid.delete(sessionId);
  pendingExit.delete(sessionId);
  const onStderr = stderrHandlers.get(sessionId);
  if (onStderr) onStderr(failure.message);
  else console.error(failure.message);
  onExit(1);
}
function failPrewatchOutput(sessionId) {
  outputFailures.set(sessionId, {
    message: `Provider output exceeded the pre-watch buffer (${PREWATCH_OUTPUT_BYTES} bytes / ${PREWATCH_OUTPUT_LINES} records); run stopped`,
    delivered: false,
  });
  // Do not use killChild: the admitted prefix and failure must survive until
  // the caller installs its watcher. This direct stop never waits on replay.
  void invoke("harness_kill", { sessionId }).catch((error) => {
    console.error("Could not stop provider after output overflow", error);
  });
}
function deliverExit(sessionId, exit) {
  livePid.delete(sessionId);
  pendingExit.delete(sessionId);
  if (exit.error) {
    outputFailures.set(sessionId, { message: exit.error, delivered: false });
    deliverOutputFailure(sessionId);
  } else {
    exitHandlers.get(sessionId)?.(exit.code);
  }
}
function ensureBridge() {
  if (bridge) return;
  let failed = false;
  const installed = [];
  const register = (pending) =>
    pending.then((unlisten) => {
      if (failed) {
        unlisten();
        return () => undefined;
      }
      installed.push(unlisten);
      return unlisten;
    });
  const attempt = Symbol("bridge-installation");
  bridgeAttempt = attempt;
  const installation = Promise.all([
    register(
      listen("harness-stdout", (event) => {
        const { payload } = event;
        const { sessionId } = payload;
        const lines = "lines" in payload ? payload.lines : [payload.line];
        for (const line of lines) {
          // A handler may detach mid-batch; later lines follow the old path.
          if (outputFailures.has(sessionId)) continue;
          const handler = lineHandlers.get(sessionId);
          if (handler) handler(line);
          else if (ownedChildren.has(sessionId)) {
            const queued = lineBuffer.get(sessionId) ?? new ChildOutputBuffer();
            if (!queued.push(line)) failPrewatchOutput(sessionId);
            lineBuffer.set(sessionId, queued);
          }
        }
      }),
    ),
    register(
      listen("harness-stderr", (event) => {
        const { sessionId, line } = event.payload;
        stderrHandlers.get(sessionId)?.(line);
      }),
    ),
    register(
      listen("harness-exit", (event) => {
        const { sessionId, code, pid, error } = event.payload;
        if (outputFailures.has(sessionId)) return;
        const handler = exitHandlers.get(sessionId);
        if (
          (!handler && !ownedChildren.has(sessionId)) ||
          pid == null ||
          pid <= 0
        )
          return;
        const currentPid = livePid.get(sessionId);
        if (isCurrentChildExit(currentPid, pid) && handler) {
          deliverExit(sessionId, { code, error });
          return;
        }
        if (currentPid != null && currentPid !== pid) return;
        const exits = pendingExit.get(sessionId) ?? [];
        exits.push({ code, pid, error });
        if (exits.length > 8) exits.splice(0, exits.length - 8);
        pendingExit.set(sessionId, exits);
      }),
    ),
  ]).catch((error) => {
    failed = true;
    installed.splice(0).forEach((unlisten) => unlisten());
    if (bridgeAttempt === attempt) {
      bridge = null;
      bridgeAttempt = null;
    }
    throw error;
  });
  void installation.catch(() => undefined);
  bridge = installation;
}
function teardownBridge() {
  const pending = bridge;
  bridge = null;
  bridgeAttempt = null;
  lineHandlers.clear();
  exitHandlers.clear();
  lineBuffer.clear();
  outputFailures.clear();
  stderrHandlers.clear();
  ownedChildren.clear();
  livePid.clear();
  pendingExit.clear();
  void pending?.then((fns) => fns.forEach((fn) => fn())).catch(() => undefined);
}
export function startHarnessBridge() {
  users += 1;
  if (teardownTimer) {
    clearTimeout(teardownTimer);
    teardownTimer = undefined;
  }
  ensureBridge();
  return () => {
    users -= 1;
    if (users > 0) return;
    users = 0;
    teardownTimer = setTimeout(() => {
      teardownTimer = undefined;
      if (users === 0) teardownBridge();
    }, 0);
  };
}
export async function acquireHarnessBridge() {
  const release = startHarnessBridge();
  const installation = bridge;
  try {
    await installation;
    return release;
  } catch (error) {
    release();
    throw error;
  }
}
export function watchChild(sessionId, onLine, onExit, onStderr) {
  const queued = lineBuffer.get(sessionId);
  lineBuffer.delete(sessionId);
  lineHandlers.set(sessionId, onLine);
  exitHandlers.set(sessionId, onExit);
  if (onStderr) stderrHandlers.set(sessionId, onStderr);
  if (queued) queued.lines.forEach(onLine);
  if (outputFailures.has(sessionId)) {
    deliverOutputFailure(sessionId);
    return;
  }
  const exited = pendingExit
    .get(sessionId)
    ?.find((exit) => exit.pid === livePid.get(sessionId));
  if (exited) deliverExit(sessionId, exited);
}
export function unwatchChild(sessionId) {
  lineHandlers.delete(sessionId);
  exitHandlers.delete(sessionId);
  lineBuffer.delete(sessionId);
  outputFailures.delete(sessionId);
  ownedChildren.delete(sessionId);
  stderrHandlers.delete(sessionId);
  pendingExit.delete(sessionId);
}
export async function spawnChild(
  sessionId,
  command,
  args,
  cwd,
  account,
  binaryProvider,
  codexStore,
) {
  livePid.delete(sessionId);
  pendingExit.delete(sessionId);
  outputFailures.delete(sessionId);
  lineBuffer.delete(sessionId);
  ownedChildren.add(sessionId);
  const binaryPath = binaryProvider
    ? runtimeProviderBinaryPath(binaryProvider)
    : undefined;
  const pid = await invoke("harness_spawn", {
    sessionId,
    command,
    args,
    cwd,
    account,
    binaryProvider,
    binaryPath,
    ...(codexStore ? { codexStore } : {}),
  });
  if (typeof pid !== "number" || pid <= 0) return;
  if (outputFailures.has(sessionId)) return;
  livePid.set(sessionId, pid);
  const exits = pendingExit.get(sessionId);
  const exited = exits?.find((event) => event.pid === pid);
  if (exited && exitHandlers.has(sessionId)) deliverExit(sessionId, exited);
}
export function writeChild(sessionId, line) {
  return invoke("harness_write", { sessionId, line });
}
export function killChild(sessionId) {
  livePid.delete(sessionId);
  pendingExit.delete(sessionId);
  unwatchChild(sessionId);
  return invoke("harness_kill", { sessionId });
}
async function resolveHarnessBinary(provider, binaryPath) {
  const configuredPath =
    binaryPath === undefined
      ? runtimeProviderBinaryPath(provider)
      : binaryPath?.trim();
  if (configuredPath) {
    return invoke("harness_resolve_configured", {
      provider,
      binaryPath: configuredPath,
    });
  }
  const command = {
    claude: "harness_resolve_claude",
    codex: "harness_resolve_codex",
    pi: "harness_resolve_pi",
    omp: "harness_resolve_omp",
    antigravity: "harness_resolve_antigravity",
  };
  return invoke(command[provider]);
}
export function resolveCodexBinary(binaryPath) {
  return resolveHarnessBinary("codex", binaryPath);
}
export function resolveClaudeBinary(binaryPath) {
  return resolveHarnessBinary("claude", binaryPath);
}
export function listCopilotClaudeModels() {
  return invoke("harness_copilot_models");
}
export function resolvePiBinary(binaryPath) {
  return resolveHarnessBinary("pi", binaryPath);
}
export function resolveOmpBinary(binaryPath) {
  return resolveHarnessBinary("omp", binaryPath);
}
export function resolveAntigravityBinary(binaryPath) {
  return resolveHarnessBinary("antigravity", binaryPath);
}
export function execChild(
  command,
  args,
  cwd,
  binaryProvider,
  binaryPathOverride,
) {
  const binaryPath =
    binaryPathOverride === undefined && binaryProvider
      ? runtimeProviderBinaryPath(binaryProvider)
      : binaryPathOverride;
  return invoke("harness_exec", {
    command,
    args,
    cwd,
    binaryProvider,
    binaryPath,
  });
}
