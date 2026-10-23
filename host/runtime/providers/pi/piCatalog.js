import { homeDir } from "../../platform/fs.js";
import {
  killChild,
  spawnChild,
  unwatchChild,
  watchChild,
} from "../../protocol/child.js";
import { PiRpc } from "./piClient.js";
import { OMP_FLAVOR, PI_FLAVOR } from "./piFlavor.js";
import { buildPiSpawnArgs, modelsFromRpcData } from "./piProtocol.js";
const DISCOVERY_TIMEOUT_MS = 45000;
// Pi can answer RPC before asynchronous extension providers register models.
// Identical early responses are not a readiness signal: observe startup for at
// least five seconds, then require a quiet second, within the overall deadline.
const PI_STARTUP_WINDOW_MS = 5000;
const PI_CATALOG_QUIET_MS = 1000;
const PI_CATALOG_POLL_MS = 500;
/**
 * Probe models in the supplied directory, or the user's home directory.
 * Load extensions for both flavors: they can register providers and models,
 * and stripping them can prevent a configured default provider from loading.
 * Pi merges startup snapshots until the five-second minimum and one-second
 * quiet period are met; this heuristic can miss providers registering later.
 * omp returns its first snapshot. The 45-second deadline starts after spawn
 * and returns collected models if any; other failures reject. Every outcome
 * closes RPC requests, cancels polling, and tears down the child probe.
 */
async function discoverModels(flavor, workingDirectory) {
  const { path } = await flavor.resolveBinary();
  const cwd = workingDirectory ?? (await homeDir());
  const probeId = `${flavor.probeChildId}-${crypto.randomUUID()}`;
  const rpc = new PiRpc(probeId, () => undefined, flavor.label);
  const models = new Map();
  const collectedModels = () =>
    [...models.values()].sort((left, right) =>
      left.name.localeCompare(right.name),
    );
  let stopped = false;
  let pollTimer;
  let wakePoll;
  let exitError;
  let rejectExit;
  const stop = async () => {
    stopped = true;
    if (pollTimer !== undefined) clearTimeout(pollTimer);
    wakePoll?.();
    rpc.close();
    unwatchChild(probeId);
    await killChild(probeId).catch(() => undefined);
  };
  watchChild(
    probeId,
    (line) => rpc.pushLine(line),
    () => {
      exitError = new Error(`${flavor.label} catalog probe exited`);
      rpc.close(exitError);
      rejectExit?.(exitError);
    },
  );
  let timeout;
  const timeoutError = new Error(`${flavor.label} model discovery timed out`);
  try {
    await spawnChild(
      probeId,
      path,
      buildPiSpawnArgs(flavor, {
        noSession: true,
      }),
      cwd,
      undefined,
      flavor.id,
    );
    if (exitError) throw exitError;
    const started = Date.now();
    const timedOut = new Promise((_, reject) => {
      timeout = setTimeout(() => reject(timeoutError), DISCOVERY_TIMEOUT_MS);
    });
    const exited = new Promise((_, reject) => {
      rejectExit = reject;
    });
    const pollModels = async () => {
      let previousSnapshot = "";
      let lastChange = started;
      while (!stopped) {
        const response = await rpc.request(
          { type: "get_available_models" },
          // The outer timer owns the discovery deadline. A shorter RPC timer
          // could win the race and discard models collected by earlier polls.
          DISCOVERY_TIMEOUT_MS,
        );
        if (stopped) break;
        const snapshot = modelsFromRpcData(flavor, response.data);
        const signature = JSON.stringify(snapshot);
        if (signature !== previousSnapshot) lastChange = Date.now();
        previousSnapshot = signature;
        for (const model of snapshot) models.set(model.id, model);
        if (
          flavor.id !== "pi" ||
          (Date.now() - started >= PI_STARTUP_WINDOW_MS &&
            Date.now() - lastChange >= PI_CATALOG_QUIET_MS)
        ) {
          break;
        }
        await new Promise((resolve) => {
          wakePoll = resolve;
          pollTimer = setTimeout(resolve, PI_CATALOG_POLL_MS);
        });
      }
      return collectedModels();
    };
    return await Promise.race([pollModels(), timedOut, exited]);
  } catch (error) {
    // A slow or continually changing extension must not discard useful models
    // already received, or keep the probe alive beyond the discovery deadline.
    if (error === timeoutError && models.size > 0) return collectedModels();
    throw error;
  } finally {
    if (timeout !== undefined) clearTimeout(timeout);
    await stop();
  }
}
export function discoverPiModels(workingDirectory) {
  return discoverModels(PI_FLAVOR, workingDirectory);
}
export function discoverOmpModels(workingDirectory) {
  return discoverModels(OMP_FLAVOR, workingDirectory);
}
