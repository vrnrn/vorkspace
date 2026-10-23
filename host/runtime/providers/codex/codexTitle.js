import {
  buildThreadTitlePrompt,
  parseGeneratedSessionTitle,
} from "../../models/sessions/sessionTitle.js";
import { runCodexTextPrompt } from "./codexText.js";
const TITLE_TIMEOUT_MS = 45000;
/** Codex app-server turn that returns a sidebar title, or null on failure. */
export async function generateCodexSessionTitle(input) {
  try {
    const output = await runCodexTextPrompt({
      cwd: input.cwd,
      providerAccountId: input.providerAccountId,
      prompt: buildThreadTitlePrompt(input.message),
      timeoutMs: TITLE_TIMEOUT_MS,
    });
    return parseGeneratedSessionTitle(output, input.message);
  } catch (error) {
    console.debug("[vorkspace] session title", error);
    return null;
  }
}
