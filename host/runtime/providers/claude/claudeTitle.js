import {
  buildThreadTitlePrompt,
  parseGeneratedSessionTitle,
} from "../../models/sessions/sessionTitle.js";
import { runClaudeTextPrompt } from "./claudeText.js";
const TITLE_TIMEOUT_MS = 45000;
export async function generateClaudeSessionTitle(input) {
  try {
    const output = await runClaudeTextPrompt({
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
