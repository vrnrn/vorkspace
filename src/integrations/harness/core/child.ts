import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  runtimeProviderBinaryPath,
  type ConfigurableBinaryProvider,
} from "../../../features/providers/model/providerBinaryPaths";

/** Process I/O is supplied by the desktop or a headless host. Provider
 * protocols never need to know which process owns their children. */
export interface ChildBackend {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  listen<T>(
    event: string,
    handler: (event: { payload: T }) => void,
  ): Promise<UnlistenFn>;
}

let backend: ChildBackend | undefined;

export function configureChildBackend(next: ChildBackend): void {
  if (bridge || users)
    throw new Error("Configure the child backend before starting the bridge");
  backend = next;
}

export function hasHeadlessChildBackend(): boolean {
  return backend !== undefined;
}

/** Provider-owned transcript files are read on the machine running the child. */
export function readHarnessTextFile(path: string): Promise<string> {
  return invoke<string>(backend ? "harness_read_text_file" : "read_text_file", {
    path,
  });
}

export function prepareMonoCodexStore(
  providerAccountId?: string,
  threadId?: string,
): Promise<{ home: string; hasThread: boolean }> {
  return invoke("codex_mono_store_prepare", { providerAccountId, threadId });
}

export function copyMonoCodexThreads(
  providerAccountId: string | undefined,
  threadId: string,
  paths: string[],
  sqliteHome?: string,
): Promise<void> {
  return invoke("codex_mono_store_copy", {
    providerAccountId,
    threadId,
    paths,
    sqliteHome,
  });
}

/** Copy a native Claude or Codex conversation into another account's home,
 * so that account resumes the same provider session id. */
export function transferProviderSession(
  provider: "claude" | "codex",
  fromAccountId: string | undefined,
  toAccountId: string | undefined,
  providerSessionId: string,
): Promise<void> {
  return invoke("provider_session_transfer", {
    provider,
    fromAccountId,
    toAccountId,
    providerSessionId,
  });
}

export function restoreMonoCodexAgentState(
  providerAccountId: string | undefined,
  threadId: string,
): Promise<void> {
  return invoke("codex_mono_store_restore_agent_state", {
    providerAccountId,
    threadId,
  });
}

function invoke<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  return backend
    ? backend.invoke<T>(command, args)
    : tauriInvoke<T>(command, args);
}

function listen<T>(
  event: string,
  handler: (event: { payload: T }) => void,
): Promise<UnlistenFn> {
  return backend ? backend.listen(event, handler) : tauriListen(event, handler);
}

type LinePayload = { sessionId: string; line: string };
// The desktop batches a frame's stdout lines into one event;
// the remote host still sends them one at a time.
type LinesPayload = LinePayload | { sessionId: string; lines: string[] };
type ExitPayload = {
  sessionId: string;
  code: number | null;
  pid?: number;
  error?: string;
};

type LineHandler = (line: string) => void;
type ExitHandler = (code: number | null) => void;

const lineHandlers = new Map<string, LineHandler>();
const exitHandlers = new Map<string, ExitHandler>();
const lineBuffer = new Map<string, ChildOutputBuffer>();
const outputFailures = new Map<
  string,
  { message: string; delivered: boolean }
>();
const stderrHandlers = new Map<string, LineHandler>();
// Output is broadcast to every window. Only ids this window spawned or opened
// may buffer while unwatched; anything else would be held until the bridge
// is torn down, since nothing here ever watches or unwatches it.
const ownedChildren = new Set<string>();
const livePid = new Map<string, number>();
const pendingExit = new Map<
  string,
  Array<{ code: number | null; pid: number; error?: string }>
>();

/** True when this exit belongs to the child we currently have spawned. */
export function isCurrentChildExit(
  expectedPid: number | undefined,
  exitedPid: number | undefined,
): boolean {
  if (expectedPid == null || expectedPid <= 0) return false;
  if (exitedPid == null || exitedPid <= 0) return false;
  return exitedPid === expectedPid;
}

const PREWATCH_OUTPUT_LINES = 1000;
const PREWATCH_OUTPUT_BYTES = 16 * 1024 * 1024;

/** Pre-watch protocol output is a prefix, never a silently truncated suffix.
 * Once admission fails the bridge stops the run and reports a failed exit. */
export class ChildOutputBuffer {
  readonly lines: string[] = [];
  private bytes = 0;
  constructor(
    private readonly maxLines = PREWATCH_OUTPUT_LINES,
    private readonly maxBytes = PREWATCH_OUTPUT_BYTES,
  ) {}

  push(line: string): boolean {
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

let bridge: Promise<UnlistenFn[]> | null = null;
let bridgeAttempt: symbol | null = null;
let users = 0;
let teardownTimer: ReturnType<typeof setTimeout> | undefined;

function deliverOutputFailure(sessionId: string) {
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

function failPrewatchOutput(sessionId: string) {
  outputFailures.set(sessionId, {
    message: `Provider output exceeded the pre-watch buffer (${PREWATCH_OUTPUT_BYTES} bytes / ${PREWATCH_OUTPUT_LINES} records); run stopped`,
    delivered: false,
  });
  // Do not use killChild: the admitted prefix and failure must survive until
  // the caller installs its watcher. This direct stop never waits on replay.
  void invoke("harness_kill", { sessionId }).catch((error: unknown) => {
    console.error("Could not stop provider after output overflow", error);
  });
}

function deliverExit(
  sessionId: string,
  exit: { code: number | null; error?: string },
) {
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
  const installed: UnlistenFn[] = [];
  const register = (pending: Promise<UnlistenFn>) =>
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
      listen<LinesPayload>("harness-stdout", (event) => {
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
      listen<LinePayload>("harness-stderr", (event) => {
        const { sessionId, line } = event.payload;
        stderrHandlers.get(sessionId)?.(line);
      }),
    ),
    register(
      listen<ExitPayload>("harness-exit", (event) => {
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
  ]).catch((error: unknown) => {
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

export function startHarnessBridge(): () => void {
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

export async function acquireHarnessBridge(): Promise<() => void> {
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

if (import.meta.hot) {
  import.meta.hot.dispose(() => {
    users = 0;
    if (teardownTimer) {
      clearTimeout(teardownTimer);
      teardownTimer = undefined;
    }
    teardownBridge();
  });
}

export function watchChild(
  sessionId: string,
  onLine: LineHandler,
  onExit: ExitHandler,
  onStderr?: LineHandler,
) {
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

export function unwatchChild(sessionId: string) {
  lineHandlers.delete(sessionId);
  exitHandlers.delete(sessionId);
  lineBuffer.delete(sessionId);
  outputFailures.delete(sessionId);
  ownedChildren.delete(sessionId);
  stderrHandlers.delete(sessionId);
  pendingExit.delete(sessionId);
}

export async function spawnChild(
  sessionId: string,
  command: string,
  args: string[],
  cwd: string,
  account?: { provider: "claude" | "codex"; id: string },
  binaryProvider?: ConfigurableBinaryProvider,
  codexStore?: "mono",
): Promise<void> {
  livePid.delete(sessionId);
  pendingExit.delete(sessionId);
  outputFailures.delete(sessionId);
  lineBuffer.delete(sessionId);
  ownedChildren.add(sessionId);
  const binaryPath = binaryProvider
    ? runtimeProviderBinaryPath(binaryProvider)
    : undefined;
  const pid = await invoke<number>("harness_spawn", {
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

export function writeChild(sessionId: string, line: string): Promise<void> {
  return invoke("harness_write", { sessionId, line });
}

export function killChild(sessionId: string): Promise<void> {
  livePid.delete(sessionId);
  pendingExit.delete(sessionId);
  unwatchChild(sessionId);
  return invoke("harness_kill", { sessionId });
}

export function killAllChildren(): Promise<void> {
  lineHandlers.clear();
  exitHandlers.clear();
  lineBuffer.clear();
  outputFailures.clear();
  stderrHandlers.clear();
  ownedChildren.clear();
  livePid.clear();
  pendingExit.clear();
  return invoke("harness_kill_all");
}

type ResolvedHarnessBinary = { path: string; args?: string[] };

async function resolveHarnessBinary(
  provider: ConfigurableBinaryProvider,
  binaryPath?: string | null,
): Promise<ResolvedHarnessBinary> {
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
  const command: Record<ConfigurableBinaryProvider, string> = {
    claude: "harness_resolve_claude",
    codex: "harness_resolve_codex",
    pi: "harness_resolve_pi",
    omp: "harness_resolve_omp",
    antigravity: "harness_resolve_antigravity",
  };
  return invoke(command[provider]);
}

export function resolveCodexBinary(
  binaryPath?: string | null,
): Promise<{ path: string }> {
  return resolveHarnessBinary("codex", binaryPath);
}

export function resolveClaudeBinary(
  binaryPath?: string | null,
): Promise<{ path: string }> {
  return resolveHarnessBinary("claude", binaryPath);
}

export function listCopilotClaudeModels(): Promise<unknown> {
  return invoke("harness_copilot_models");
}

export function resolvePiBinary(
  binaryPath?: string | null,
): Promise<{ path: string }> {
  return resolveHarnessBinary("pi", binaryPath);
}

export function resolveOmpBinary(
  binaryPath?: string | null,
): Promise<{ path: string }> {
  return resolveHarnessBinary("omp", binaryPath);
}

export function resolveAntigravityBinary(
  binaryPath?: string | null,
): Promise<{ path: string; args: string[] }> {
  return resolveHarnessBinary("antigravity", binaryPath) as Promise<{
    path: string;
    args: string[];
  }>;
}

export type HarnessBinaryInspection = {
  path: string;
  version?: string;
  error?: string;
};

export function inspectHarnessBinary(
  provider: ConfigurableBinaryProvider,
  binaryPath?: string | null,
): Promise<HarnessBinaryInspection> {
  return resolveHarnessBinary(provider, binaryPath).then(async (resolved) => {
    if (provider === "antigravity") {
      return { path: resolved.path, version: "ACP server" };
    }
    try {
      const version = (
        await execChild(
          resolved.path,
          ["--version"],
          undefined,
          provider,
          binaryPath,
        )
      ).trim();
      return /\d+\.\d+\.\d+/.test(version)
        ? { path: resolved.path, version }
        : { path: resolved.path, error: "CLI returned no valid version." };
    } catch (error) {
      return {
        path: resolved.path,
        error: error instanceof Error ? error.message : String(error),
      };
    }
  });
}

/** Runs the CLI's own self-update against the binary vorkspace uses. */
export async function updateHarnessCli(
  provider: ConfigurableBinaryProvider,
): Promise<void> {
  const resolved = await resolveHarnessBinary(provider);
  await invoke("harness_update", {
    command: resolved.path,
    binaryProvider: provider,
    binaryPath: runtimeProviderBinaryPath(provider),
  });
}

export function execChild(
  command: string,
  args: string[],
  cwd?: string,
  binaryProvider?: ConfigurableBinaryProvider,
  binaryPathOverride?: string | null,
): Promise<string> {
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
