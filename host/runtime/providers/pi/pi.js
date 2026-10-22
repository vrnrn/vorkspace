import {
  bindSession,
  cancelTurn,
  compactContext,
  forgetSession,
  respondApproval,
  rewindLastTurn,
  sendTurn,
  setPiBinaryResolver as setFlavorBinaryResolver,
  steerTurn,
  stopSession,
} from "./piFamily.js";
import { PI_FLAVOR } from "./piFlavor.js";
/**
 * Live Pi adapter. Spawns `pi --mode rpc` with the user's config and extensions
 * loaded (no `--no-extensions`). Todos/subagents packages in `~/.pi/agent`
 * keep working; TUI-only widgets do not appear in vorkspace.
 */
export function sendPiTurn(input) {
  return sendTurn(PI_FLAVOR, input);
}
export function compactPiContext(input) {
  return compactContext(PI_FLAVOR, input);
}
export function rewindPiLastTurn(input) {
  return rewindLastTurn(PI_FLAVOR, input);
}
export function steerPiTurn(input) {
  return steerTurn(PI_FLAVOR, input);
}
export function respondPiApproval(sessionId, requestId, decision) {
  respondApproval(PI_FLAVOR, sessionId, requestId, decision);
}
export function cancelPiTurn(sessionId) {
  return cancelTurn(PI_FLAVOR, sessionId);
}
export function stopPiSession(sessionId) {
  return stopSession(PI_FLAVOR, sessionId);
}
export function forgetPiSession(sessionId) {
  return forgetSession(PI_FLAVOR, sessionId);
}
export function bindPiSession(threadId, providerSessionId, cwd) {
  bindSession(PI_FLAVOR, threadId, providerSessionId, cwd);
}
/** Test seam. */
export function setPiBinaryResolver(fn) {
  setFlavorBinaryResolver(PI_FLAVOR, fn);
}
