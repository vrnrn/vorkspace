import {
  buildThreadTitlePrompt,
  parseGeneratedSessionTitle,
} from "../../models/sessions/sessionTitle.js";
import { OMP_FLAVOR, PI_FLAVOR } from "./piFlavor.js";
import { runTextPrompt } from "./piText.js";
const TITLE_TIMEOUT_MS = 45000;
async function generateSessionTitle(flavor, input) {
  try {
    const output = await runTextPrompt(flavor, {
      cwd: input.cwd,
      prompt: buildThreadTitlePrompt(input.message),
      timeoutMs: TITLE_TIMEOUT_MS,
    });
    return parseGeneratedSessionTitle(output, input.message);
  } catch (error) {
    console.debug("[vorkspace] session title", error);
    return null;
  }
}
export function generatePiSessionTitle(input) {
  return generateSessionTitle(PI_FLAVOR, input);
}
export function generateOmpSessionTitle(input) {
  return generateSessionTitle(OMP_FLAVOR, input);
}
