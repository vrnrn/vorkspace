import { createHash, randomUUID } from "node:crypto";
import { realpath, stat } from "node:fs/promises";
import { basename, isAbsolute } from "node:path";
import {
  renameHostWorktreeBranch,
  resolveHostWorktree,
} from "./git-worktrees.js";
import { applyHarnessEvent } from "./runtime/protocol/apply.js";
import { resolveModel } from "./runtime/models/sessions/models.js";
import {
  HARNESS_LABEL,
  RUNTIME_MODES,
  canReplaceSessionTitle,
  formatSessionTitle,
  titleFromPrompt,
} from "./runtime/models/sessions/session.js";
import { namedWorktreeBranch } from "./runtime/models/source-control/worktrees.js";
import { isRemoteProvider } from "./runtime/models/connections/protocol.js";
import { parseRemoteAttachments, resolveAttachments } from "./attachments.js";
import { SessionRuns, settledSession } from "./session-runs.js";
// Visible text remains small; resolved provider context has its own byte cap.
export const MAX_PREPARED_TEXT_BYTES = 5 * 1024 * 1024;
const text = (value, label, max = 128) => {
  if (
    typeof value !== "string" ||
    !value.trim() ||
    value.length > max ||
    value.includes("\0")
  )
    throw new Error(`Invalid ${label}`);
  return value;
};
function modelSettings(value) {
  if (value == null) return {};
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error("Invalid model settings");
  const entries = Object.entries(value);
  if (
    entries.length > 20 ||
    entries.some(
      ([key, setting]) =>
        !/^[a-zA-Z][a-zA-Z0-9]{0,63}$/.test(key) ||
        typeof setting !== "string" ||
        setting.length > 128 ||
        setting.includes("\0"),
    )
  )
    throw new Error("Invalid model settings");
  return Object.fromEntries(entries);
}
export function parseCommand(input) {
  if (!input || typeof input !== "object" || Array.isArray(input))
    throw new Error("Invalid command");
  const v = input;
  const commandId = text(v.commandId, "command ID");
  if (
    v.preparedText !== undefined &&
    (v.type !== "send" ||
      typeof v.preparedText !== "string" ||
      !v.preparedText.trim() ||
      v.preparedText.includes("\0") ||
      Buffer.byteLength(v.preparedText, "utf8") > MAX_PREPARED_TEXT_BYTES)
  )
    throw new Error("Invalid prepared prompt (5 MiB UTF-8 maximum)");
  if (v.type === "create") {
    if (!isRemoteProvider(v.harness) || !RUNTIME_MODES.includes(v.runtimeMode))
      throw new Error("Invalid provider or permission mode");
    if (
      v.autoWorktreeBranch !== undefined &&
      (v.worktreeCwd === undefined ||
        typeof v.autoWorktreeBranch !== "string" ||
        !/^mc\/[a-z0-9]{8}$/.test(v.autoWorktreeBranch))
    )
      throw new Error("Invalid automatically created worktree branch");
    return {
      type: "create",
      commandId,
      projectId: text(v.projectId, "project ID"),
      ...(v.worktreeCwd !== undefined
        ? { worktreeCwd: text(v.worktreeCwd, "working copy", 4096) }
        : {}),
      ...(v.autoWorktreeBranch !== undefined
        ? { autoWorktreeBranch: v.autoWorktreeBranch }
        : {}),
      harness: v.harness,
      model: text(v.model, "model", 200),
      ...(v.modelSettings !== undefined
        ? { modelSettings: modelSettings(v.modelSettings) }
        : {}),
      runtimeMode: v.runtimeMode,
    };
  }
  const sessionId = text(v.sessionId, "session ID");
  if (v.type === "configure") {
    if (!RUNTIME_MODES.includes(v.runtimeMode))
      throw new Error("Invalid permission mode");
    return {
      type: "configure",
      commandId,
      sessionId,
      model: text(v.model, "model", 200),
      modelSettings: modelSettings(v.modelSettings),
      runtimeMode: v.runtimeMode,
    };
  }
  if (v.type === "compact") return { type: "compact", commandId, sessionId };
  if (v.type === "send" || v.type === "draft") {
    const attachments = parseRemoteAttachments(v.attachments);
    if (
      typeof v.text !== "string" ||
      v.text.length > 256_000 ||
      v.text.includes("\0") ||
      (!v.text.trim() &&
        attachments.length === 0 &&
        !(v.type === "send" && v.draftBlockId !== undefined))
    )
      throw new Error("Invalid prompt");
    if (
      v.type === "send" &&
      v.intent !== undefined &&
      !["default", "plan", "build"].includes(String(v.intent))
    )
      throw new Error("Invalid turn intent");
    if (
      v.planBlockId !== undefined &&
      (v.type !== "send" || v.intent !== "build")
    )
      throw new Error("Invalid plan build");
    return {
      type: v.type,
      commandId,
      sessionId,
      text: v.text,
      ...(v.preparedText !== undefined ? { preparedText: v.preparedText } : {}),
      ...(attachments.length ? { attachments } : {}),
      ...(v.type === "send" && v.intent ? { intent: v.intent } : {}),
      ...(v.type === "send" && v.draftBlockId !== undefined
        ? { draftBlockId: text(v.draftBlockId, "draft block ID") }
        : {}),
      ...(v.type === "send" && v.planBlockId !== undefined
        ? { planBlockId: text(v.planBlockId, "plan block ID") }
        : {}),
    };
  }
  if (v.type === "removeDraft")
    return {
      type: "removeDraft",
      commandId,
      sessionId,
      draftBlockId: text(v.draftBlockId, "draft block ID"),
    };
  const runId = text(v.runId, "run ID");
  if (v.type === "cancel")
    return { type: "cancel", commandId, sessionId, runId };
  if (!Number.isSafeInteger(v.requestId) || Number(v.requestId) < 0)
    throw new Error("Invalid request ID");
  const requestId = Number(v.requestId);
  if (v.type === "approve" && (v.decision === "allow" || v.decision === "deny"))
    return {
      type: "approve",
      commandId,
      sessionId,
      runId,
      requestId,
      decision: v.decision,
    };
  if (v.type === "answer") {
    const reply = v.reply;
    if (reply?.kind === "skipped")
      return {
        type: "answer",
        commandId,
        sessionId,
        runId,
        requestId,
        reply: { kind: "skipped" },
      };
    if (
      reply?.kind === "answered" &&
      reply.answers &&
      typeof reply.answers === "object" &&
      !Array.isArray(reply.answers)
    ) {
      const entries = Object.entries(reply.answers);
      if (
        entries.length > 50 ||
        entries.some(
          ([key, value]) =>
            key.length > 200 ||
            !Array.isArray(value) ||
            value.length > 50 ||
            value.some((x) => typeof x !== "string" || x.length > 10_000),
        )
      )
        throw new Error("Invalid question answers");
      if (
        reply.custom != null &&
        (typeof reply.custom !== "object" ||
          Array.isArray(reply.custom) ||
          Object.values(reply.custom).some(
            (x) => typeof x !== "string" || x.length > 10_000,
          ))
      )
        throw new Error("Invalid custom answers");
      return {
        type: "answer",
        commandId,
        sessionId,
        runId,
        requestId,
        reply: {
          kind: "answered",
          answers: Object.fromEntries(entries),
          ...(reply.custom ? { custom: reply.custom } : {}),
        },
      };
    }
  }
  throw new Error("Unsupported command");
}
export class HostEngine {
  store;
  providers;
  switchingProjects = new Set();
  #runs;
  constructor(store, providers) {
    this.store = store;
    this.providers = providers;
    this.#runs = new SessionRuns(store);
    // Provider dispatch is not transactional with SQLite. Never replay a send
    // automatically after a crash; its external effects may already exist.
    // Idle history needs only binding metadata. Loading every transcript here
    // made startup heap and JSON parsing scale with all saved conversation text.
    for (const item of store.summaries()) {
      if (item.status === "running") {
        const value = store.session(item.id);
        this.store.advance(
          settledSession(
            value,
            "interrupted",
            "Host restarted. This turn was interrupted; inspect its work before continuing.",
            value.updatedAt,
          ),
          { type: "interrupted" },
        );
      }
      // Retain history for providers no longer installed on this host. Session
      // commands still report them as unavailable, but must not block startup.
      if (item.providerSessionId)
        this.providers[item.harness]?.bind(
          item.id,
          item.providerSessionId,
          item.cwd,
        );
    }
  }
  async openProject(path) {
    if (!isAbsolute(path) || path.includes("\0"))
      throw new Error("Choose an absolute directory path on the host");
    const cwd = await realpath(path);
    if (!(await stat(cwd)).isDirectory())
      throw new Error("Project path is not a directory");
    return this.store.addProject(cwd, basename(cwd));
  }
  async withIdleProject(projectId, action) {
    if (this.switchingProjects.has(projectId))
      throw new Error("A branch switch is already in progress");
    if (
      this.store
        .summaries(projectId)
        .some((session) => session.status === "running")
    )
      throw new Error(
        "Wait for running host sessions before switching branches",
      );
    this.switchingProjects.add(projectId);
    try {
      return await action();
    } finally {
      this.switchingProjects.delete(projectId);
    }
  }
  provider(id) {
    const provider = this.providers[id];
    if (!provider) throw new Error(`${id} is not available on this host`);
    return provider;
  }
  updateSession(id, patch) {
    this.#runs.flush(id);
    const summary = this.store.updateSession(id, patch);
    if (this.#runs.has(id)) this.#runs.publish(this.store.session(id));
    return summary;
  }
  command(raw) {
    if (this.#runs.closing) throw new Error("Host is stopping");
    const command = parseCommand(raw);
    const signature = createHash("sha256")
      .update(JSON.stringify(command))
      .digest("hex");
    const previous = this.store.receipt(command.commandId, signature);
    if (previous) return previous;
    // Commands apply to the latest state, including batched stream output.
    if (command.type !== "create") this.#runs.flush(command.sessionId);
    let effect;
    const { receipt, saved } = this.store.transaction(() => {
      let value;
      if (command.type === "create") {
        const project = this.store.project(command.projectId);
        if (this.switchingProjects.has(project.id))
          throw new Error("Wait for the branch switch to finish");
        this.provider(command.harness);
        const cwd = resolveHostWorktree(project.cwd, command.worktreeCwd);
        const now = Date.now();
        value = {
          projectId: project.id,
          autoWorktreeBranch: command.autoWorktreeBranch,
          revision: 0,
          status: "idle",
          createdAt: now,
          updatedAt: now,
          session: {
            id: randomUUID(),
            cwd,
            harness: command.harness,
            model: command.model,
            runtimeMode: command.runtimeMode,
            modelSettings: command.modelSettings ?? {},
            title: "New remote session",
            ...(command.autoWorktreeBranch
              ? { branch: command.autoWorktreeBranch, worktreeCwd: cwd }
              : {}),
            blocks: [],
          },
        };
      } else {
        value = this.store.session(command.sessionId);
        if (
          (command.type === "send" || command.type === "compact") &&
          this.switchingProjects.has(value.projectId)
        )
          throw new Error("Wait for the branch switch to finish");
        const provider = this.provider(value.session.harness);
        if (command.type === "configure") {
          if (value.status === "running")
            throw new Error(
              "Wait for the current turn before changing settings",
            );
          value = {
            ...value,
            session: {
              ...value.session,
              model: command.model,
              modelSettings: command.modelSettings,
              runtimeMode: command.runtimeMode,
            },
          };
        } else if (command.type === "draft") {
          if (
            value.status === "running" ||
            value.session.blocks.some((block) => block.draft)
          )
            throw new Error("This session cannot save another draft right now");
          const attachments = resolveAttachments(
            this.store,
            command.attachments ?? [],
          );
          value = {
            ...value,
            session: {
              ...value.session,
              title: value.session.blocks.length
                ? value.session.title
                : titleFromPrompt(
                    command.text,
                    value.session.harness,
                    attachments,
                  ),
              blocks: [
                ...value.session.blocks,
                {
                  id: command.commandId,
                  role: "user",
                  text: command.text,
                  ...(attachments.length ? { attachments } : {}),
                  draft: true,
                },
              ],
            },
          };
        } else if (command.type === "removeDraft") {
          const draft = value.session.blocks.find(
            (block) => block.id === command.draftBlockId && block.draft,
          );
          if (!draft) throw new Error("Draft not found");
          value = {
            ...value,
            session: {
              ...value.session,
              blocks: value.session.blocks.filter(
                (block) => block.id !== draft.id,
              ),
            },
          };
        } else if (command.type === "send" || command.type === "compact") {
          if (value.status === "running" || this.#runs.has(command.sessionId))
            throw new Error("This session is already running");
          if (command.type === "compact" && !provider.compact)
            throw new Error(
              "Context compaction is unavailable for this provider",
            );
          const draft =
            command.type === "send" && command.draftBlockId
              ? value.session.blocks.find(
                  (block) => block.id === command.draftBlockId && block.draft,
                )
              : undefined;
          if (command.type === "send" && command.draftBlockId && !draft)
            throw new Error("Draft not found");
          const plan =
            command.type === "send" && command.planBlockId
              ? value.session.blocks.find(
                  (block) =>
                    block.id === command.planBlockId && block.role === "plan",
                )
              : undefined;
          if (
            command.type === "send" &&
            command.planBlockId &&
            (!plan ||
              !plan.text.trim() ||
              plan.streaming ||
              plan.plan?.status === "building" ||
              plan.plan?.status === "built")
          )
            throw new Error("Plan is not ready to build");
          const attachments =
            command.type === "send"
              ? (draft?.attachments ??
                resolveAttachments(this.store, command.attachments ?? []))
              : [];
          const runId = randomUUID();
          const firstTurn =
            command.type === "send" &&
            !value.session.blocks.some((block) => !block.draft);
          const placeholderTitle =
            value.session.title === "New remote session" ||
            canReplaceSessionTitle(
              value.session.title,
              value.session.harness,
              HARNESS_LABEL[value.session.harness],
            );
          const model = resolveModel(
            value.session.harness,
            value.session.model,
          );
          value = {
            ...value,
            status: "running",
            runId,
            session: {
              ...value.session,
              busy: true,
              pendingQuestion: undefined,
              title:
                firstTurn && placeholderTitle
                  ? titleFromPrompt(
                      command.text,
                      value.session.harness,
                      attachments,
                    )
                  : value.session.title,
              blocks: [
                ...value.session.blocks
                  .filter((block) => !block.draft)
                  .map((block) =>
                    block === plan
                      ? {
                          ...block,
                          plan: {
                            ...(block.plan ?? { status: "ready" }),
                            status: "building",
                            approvedText: block.text,
                          },
                        }
                      : block,
                  ),
                {
                  id: command.commandId,
                  role: "user",
                  text: command.type === "compact" ? "/compact" : command.text,
                  ...(attachments.length ? { attachments } : {}),
                  startedAt: Date.now(),
                  turnModel: {
                    harness: value.session.harness,
                    id: value.session.model,
                    name:
                      model.id === value.session.model
                        ? model.name
                        : value.session.model.replace(/^[^:]+:/, ""),
                  },
                },
              ],
            },
          };
          effect = (saved) => {
            this.#runs.start(saved, provider, {
              prompt: command.type === "compact" ? null : command.text,
              intent: command.type === "send" ? command.intent : undefined,
              attachments,
              preparedText: command.preparedText,
            });
            if (firstTurn && command.type === "send") {
              this.generateFirstTurnNames(
                saved,
                command.text,
                placeholderTitle,
              );
            }
          };
        } else {
          if (value.runId !== command.runId || value.status !== "running")
            throw new Error(
              "This request belongs to a finished or replaced turn",
            );
          if (command.type === "cancel") {
            effect = () => this.#runs.cancel(command.sessionId, command.runId);
          } else if (command.type === "approve") {
            const pending = value.session.blocks.some(
              (block) =>
                block.approval?.requestId === command.requestId &&
                !block.approval.decided,
            );
            if (!pending) throw new Error("Approval is already resolved");
            value = {
              ...value,
              session: applyHarnessEvent(value.session, {
                type: "approval.resolved",
                requestId: command.requestId,
                decision: command.decision,
              }),
            };
            effect = () =>
              provider.approve(
                command.sessionId,
                command.requestId,
                command.decision,
              );
          } else {
            if (value.session.pendingQuestion?.requestId !== command.requestId)
              throw new Error("Question is already resolved");
            value = {
              ...value,
              session: { ...value.session, pendingQuestion: undefined },
            };
            effect = () =>
              provider.answer(
                command.sessionId,
                command.requestId,
                command.reply,
              );
          }
        }
      }
      const saved = this.store.save(
        {
          ...value,
          revision: value.revision + 1,
          // Creation already initialized both timestamps from the same clock read.
          updatedAt: command.type === "create" ? value.updatedAt : Date.now(),
        },
        { type: "command", command },
      );
      const result = {
        commandId: command.commandId,
        sessionId: saved.session.id,
        revision: saved.revision,
      };
      this.store.recordReceipt(signature, result);
      return { receipt: result, saved };
    });
    this.#runs.publish(saved);
    // A receipt means durable host acceptance, not provider completion.
    effect?.(saved);
    return receipt;
  }
  generateFirstTurnNames(value, message, generateTitle) {
    const provider = this.provider(value.session.harness);
    const { id, cwd, harness, title } = value.session;
    if (generateTitle && provider.generateTitle) {
      void provider
        .generateTitle({ sessionId: id, cwd, message })
        .then((generated) => {
          if (!generated) return;
          this.#runs.flush(id);
          const current = this.store.session(id);
          if (current.session.title !== title) return;
          const saved = this.store.advance(
            {
              ...current,
              session: {
                ...current.session,
                title: formatSessionTitle(harness, generated.title),
              },
            },
            { type: "session.generatedTitle" },
          );
          this.#runs.publish(saved);
        })
        .catch((error) =>
          console.debug("[vorkspace] remote session title", error),
        );
    }
    const temporary = value.autoWorktreeBranch;
    if (temporary && provider.generateBranchName) {
      void provider
        .generateBranchName(cwd, message)
        .then(async (fragment) => {
          const branch = fragment ? namedWorktreeBranch(fragment) : null;
          if (!branch) return;
          // A title/branch request may finish after the conversation was deleted.
          const currentBeforeRename = this.store.session(id);
          if (currentBeforeRename.autoWorktreeBranch !== temporary) return;
          const project = this.store.project(value.projectId);
          await renameHostWorktreeBranch(
            project.cwd,
            cwd,
            temporary,
            branch,
            () => this.store.session(id).autoWorktreeBranch === temporary,
          );
          this.#runs.flush(id);
          const current = this.store.session(id);
          const saved = this.store.advance(
            {
              ...current,
              autoWorktreeBranch: undefined,
              session: { ...current.session, branch },
            },
            { type: "session.generatedBranch", branch },
          );
          this.#runs.publish(saved);
        })
        .catch((error) =>
          console.debug("[vorkspace] remote worktree branch", error),
        );
    }
  }
  close() {
    return this.#runs.close();
  }
}
