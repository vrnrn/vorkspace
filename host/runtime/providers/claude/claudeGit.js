import {
  buildBranchNamePrompt,
  parseBranchName,
} from "../../models/source-control/gitText.js";
import { runClaudeTextPrompt } from "./claudeText.js";
const GIT_TIMEOUT_MS = 90000;
export async function generateClaudeBranchName(cwd, message) {
  try {
    const output = await runClaudeTextPrompt({
      cwd,
      prompt: buildBranchNamePrompt(message),
      timeoutMs: GIT_TIMEOUT_MS,
    });
    return parseBranchName(output);
  } catch (error) {
    console.debug("[vorkspace] branch name", error);
    return null;
  }
}
