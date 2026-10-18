export { startHarnessBridge, killAllChildren } from "./core/child";
export {
  harnessLoginArgs,
  isHarnessAuthError,
  latestTurnNeedsHarnessLogin,
  loginHarness,
  supportsHarnessLogin,
} from "./core/auth";
export {
  applyHarnessEvent,
  applyHarnessEvents,
  appendUser,
  appendSteerUser,
  promoteLastAssistantToPlan,
  stopStreaming,
} from "./core/apply";

export {
  sendCodexTurn,
  compactCodexContext,
  rewindCodexLastTurn,
  cancelCodexTurn,
  respondCodexApproval,
  stopCodexSession,
  forgetCodexSession,
  bindCodexSession,
  hasLiveCodexSession,
  migrateMonoCodexSession,
} from "./providers/codex/codex";

export {
  sendClaudeTurn,
  compactClaudeContext,
  cancelClaudeTurn,
  respondClaudeApproval,
  stopClaudeSession,
  forgetClaudeSession,
  bindClaudeSession,
} from "./providers/claude/claude";
export {
  sendPiTurn,
  compactPiContext,
  rewindPiLastTurn,
  cancelPiTurn,
  respondPiApproval,
  stopPiSession,
  forgetPiSession,
  bindPiSession,
} from "./providers/pi/pi";
export {
  sendOmpTurn,
  compactOmpContext,
  rewindOmpLastTurn,
  cancelOmpTurn,
  respondOmpApproval,
  stopOmpSession,
  forgetOmpSession,
  bindOmpSession,
} from "./providers/omp/omp";

export {
  sendAntigravityTurn,
  steerAntigravityTurn,
  cancelAntigravityTurn,
  stopAntigravitySession,
  forgetAntigravitySession,
  respondAntigravityApproval,
  bindAntigravitySession,
} from "./providers/antigravity/antigravity";

export { generateCodexSessionTitle } from "./providers/codex/codexTitle";

export { generateClaudeSessionTitle } from "./providers/claude/claudeTitle";
export {
  generatePiSessionTitle,
  generateOmpSessionTitle,
} from "./providers/pi/piTitle";

export {
  generateCodexCommitMessage,
  generateCodexPrContent,
} from "./providers/codex/codexGit";

export {
  generateClaudeCommitMessage,
  generateClaudePrContent,
} from "./providers/claude/claudeGit";

export {
  generateCommitMessage,
  generatePrContent,
  pickTextHarness,
  warmupText,
} from "./core/textHarness";

export { warmupClaudeText } from "./providers/claude/claudeText";
export { warmupPiText, warmupOmpText } from "./providers/pi/piText";

export { refreshCodexCatalog } from "./providers/codex/codexCatalog";

export { refreshClaudeCatalog } from "./providers/claude/claudeCatalog";
export { refreshPiCatalog, refreshOmpCatalog } from "./providers/pi/piCatalog";

export { refreshAntigravityCatalog } from "./providers/antigravity/antigravityCatalog";
export { registerBuiltinHarnesses } from "./core/register";
export {
  getHarnessAvailabilitySnapshot,
  hasProbedHarnessAvailability,
  harnessUnavailableHint,
  isHarnessAvailable,
  probeHarnessAvailability,
  subscribeHarnessAvailability,
} from "./core/availability";
export {
  getHarness,
  requireHarness,
  isLiveHarness,
  sendHarnessTurn,
  compactHarnessContext,
  canCompactHarnessContext,
  steerHarnessTurn,
  canSteerHarness,
  canRewindHarnessLastTurn,
  rewindHarnessLastTurn,
  cancelHarnessTurn,
  respondHarnessApproval,
  respondHarnessQuestion,
  keepHarnessQuestionOpen,
  stopHarnessSession,
  configureHarnessIdlePark,
  enforceHarnessIdleLimit,
  forgetHarnessSession,
  bindHarnessSession,
  transferHarnessSessionAccount,
  refreshHarnessCatalogs,
  generateHarnessTitle,
  generateHarnessCommitMessage,
  generateHarnessPrContent,
  generateHarnessBranchName,
  canRunHarnessTextPrompt,
  runHarnessTextPrompt,
  stopHarnessTextPrompts,
} from "./core/registry";
export type {
  ApprovalDecision,
  CompactContextInput,
  HarnessEvent,
  SteerTurnInput,
} from "./core/types";
export { TurnNotReadyError } from "./core/types";
export type {
  UserQuestion,
  UserQuestionPrompt,
  UserQuestionReply,
} from "../../features/sessions/model/userQuestion";
export type { HarnessAdapter, TextPromptInput } from "./core/registry";
