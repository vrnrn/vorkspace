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
} from "../pi/piFamily.js";
import { OMP_FLAVOR } from "../pi/piFlavor.js";
/**
 * Live omp (oh-my-pi) adapter. Spawns `omp --mode rpc` with the user's config
 * and extensions loaded (no `--no-extensions`), so plugins in `~/.omp/agent`
 * keep working; TUI-only widgets do not appear in vorkspace. omp is a fork of
 * Pi and speaks the same RPC protocol, so both run on the `piFamily` core.
 */
export function sendOmpTurn(input) {
  return sendTurn(OMP_FLAVOR, input);
}
export function compactOmpContext(input) {
  return compactContext(OMP_FLAVOR, input);
}
export function rewindOmpLastTurn(input) {
  return rewindLastTurn(OMP_FLAVOR, input);
}
export function steerOmpTurn(input) {
  return steerTurn(OMP_FLAVOR, input);
}
export function respondOmpApproval(sessionId, requestId, decision) {
  respondApproval(OMP_FLAVOR, sessionId, requestId, decision);
}
export function cancelOmpTurn(sessionId) {
  return cancelTurn(OMP_FLAVOR, sessionId);
}
export function stopOmpSession(sessionId) {
  return stopSession(OMP_FLAVOR, sessionId);
}
export function forgetOmpSession(sessionId) {
  return forgetSession(OMP_FLAVOR, sessionId);
}
export function bindOmpSession(threadId, providerSessionId, cwd) {
  bindSession(OMP_FLAVOR, threadId, providerSessionId, cwd);
}
/** Test seam. */
export function setOmpBinaryResolver(fn) {
  setFlavorBinaryResolver(OMP_FLAVOR, fn);
}
