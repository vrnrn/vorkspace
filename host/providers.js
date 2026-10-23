import * as codex from "./runtime/providers/codex/codex.js";
import * as claude from "./runtime/providers/claude/claude.js";
import * as pi from "./runtime/providers/pi/pi.js";
import * as omp from "./runtime/providers/omp/omp.js";
import * as antigravity from "./runtime/providers/antigravity/antigravity.js";
import { generateCodexSessionTitle } from "./runtime/providers/codex/codexTitle.js";
import { generateClaudeSessionTitle } from "./runtime/providers/claude/claudeTitle.js";
import { generateCodexBranchName } from "./runtime/providers/codex/codexGit.js";
import { generateClaudeBranchName } from "./runtime/providers/claude/claudeGit.js";
import {
  generatePiSessionTitle,
  generateOmpSessionTitle,
} from "./runtime/providers/pi/piTitle.js";
import {
  PI_FLAVOR,
  OMP_FLAVOR,
} from "./runtime/providers/pi/piFlavor.js";
import { respondQuestion as respondPiQuestion } from "./runtime/providers/pi/piFamily.js";
export const hostProviders = {
  codex: {
    send: codex.sendCodexTurn,
    compact: codex.compactCodexContext,
    cancel: codex.cancelCodexTurn,
    stop: codex.forgetCodexSession,
    bind: codex.bindCodexSession,
    approve: codex.respondCodexApproval,
    answer: codex.respondCodexQuestion,
    generateTitle: generateCodexSessionTitle,
    generateBranchName: generateCodexBranchName,
  },
  claude: {
    send: claude.sendClaudeTurn,
    compact: claude.compactClaudeContext,
    cancel: claude.cancelClaudeTurn,
    stop: claude.forgetClaudeSession,
    bind: claude.bindClaudeSession,
    approve: claude.respondClaudeApproval,
    answer: claude.respondClaudeQuestion,
    generateTitle: generateClaudeSessionTitle,
    generateBranchName: generateClaudeBranchName,
  },
  pi: {
    send: pi.sendPiTurn,
    compact: pi.compactPiContext,
    cancel: pi.cancelPiTurn,
    stop: pi.forgetPiSession,
    bind: pi.bindPiSession,
    approve: pi.respondPiApproval,
    answer: (id, request, reply) =>
      respondPiQuestion(PI_FLAVOR, id, request, reply),
    generateTitle: generatePiSessionTitle,
  },
  omp: {
    send: omp.sendOmpTurn,
    compact: omp.compactOmpContext,
    cancel: omp.cancelOmpTurn,
    stop: omp.forgetOmpSession,
    bind: omp.bindOmpSession,
    approve: omp.respondOmpApproval,
    answer: (id, request, reply) =>
      respondPiQuestion(OMP_FLAVOR, id, request, reply),
    generateTitle: generateOmpSessionTitle,
  },
  antigravity: {
    send: antigravity.sendAntigravityTurn,
    cancel: antigravity.cancelAntigravityTurn,
    stop: antigravity.forgetAntigravitySession,
    bind: antigravity.bindAntigravitySession,
    approve: antigravity.respondAntigravityApproval,
    answer: unsupportedQuestion,
  },
};
function unsupportedQuestion() {
  throw new Error("This provider does not support questions");
}
