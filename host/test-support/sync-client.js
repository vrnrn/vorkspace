// Test client for exercising host chunked-sync responses over real HTTP.
import { applySessionSync } from "../runtime/models/connections/protocol.js";
let transport;
export function setSyncTransport(next) {
  transport = next;
}
const remoteRequest = (...args) => transport(...args);
/** Reads one sync, assembling it from bounded pieces when the host chunks it. */
async function syncRemoteSession(machineId, sessionId, revision) {
  const response = await remoteRequest(machineId, "sessions.sync", {
    sessionId,
    revision,
  });
  if (response.kind !== "chunked") return response;
  const pieces = [];
  let offset = 0;
  while (offset < response.length) {
    const { data } = await remoteRequest(machineId, "sessions.syncChunk", {
      sessionId,
      transfer: response.transfer,
      offset,
    });
    if (!data) throw new Error("Session transfer ended early");
    pieces.push(data);
    offset += data.length;
  }
  if (offset !== response.length)
    throw new Error("Session transfer has an unexpected length");
  return JSON.parse(pieces.join(""));
}
/** Fetches only what changed since `known`; falls back to a full snapshot. */
export async function loadRemoteSession(machineId, sessionId, known) {
  const sync = (revision) => syncRemoteSession(machineId, sessionId, revision);
  const update = await sync(known?.revision);
  let snapshot;
  try {
    snapshot = applySessionSync(known, update);
  } catch {
    snapshot = applySessionSync(undefined, await sync());
  }
  return snapshot;
}
