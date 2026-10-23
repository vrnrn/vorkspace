import {
  buildBranchNamePrompt,
  parseBranchName,
} from "../../models/source-control/gitText.js";
import { runCodexTextPrompt } from "./codexText.js";
const GIT_TIMEOUT_MS = 90000;
export async function generateCodexBranchName(cwd, message) {
  try {
    const output = await runCodexTextPrompt({
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
