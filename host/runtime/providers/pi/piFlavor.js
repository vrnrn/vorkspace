import { resolveOmpBinary, resolvePiBinary } from "../../protocol/child.js";
export const PI_FLAVOR = {
  id: "pi",
  label: "Pi",
  resolveBinary: resolvePiBinary,
  resumeFlag: "--session",
  isolateFlags: ["--no-tools", "--no-skills", "--no-context-files"],
  planTools: ["read", "grep", "find", "ls"],
  probeChildId: "vorkspace-pi-probe",
  textChildId: "vorkspace-pi-text",
};
/**
 * omp renamed two of Pi's flags: sessions resume through `--resume` (Pi uses
 * `--session`), and project context is stripped with `--no-rules` (Pi uses
 * `--no-context-files`). Verified against omp 18.0.6 `--help`.
 */
export const OMP_FLAVOR = {
  id: "omp",
  label: "omp",
  resolveBinary: resolveOmpBinary,
  resumeFlag: "--resume",
  isolateFlags: ["--no-tools", "--no-skills", "--no-rules"],
  planTools: ["read", "grep", "glob", "lsp"],
  probeChildId: "vorkspace-omp-probe",
  textChildId: "vorkspace-omp-text",
};
