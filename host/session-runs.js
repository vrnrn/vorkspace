import { randomUUID } from "node:crypto";
import { readFile } from "node:fs/promises";
import { setTimeout as delay } from "node:timers/promises";
import { isVisionImage } from "./runtime/models/sessions/attachments.js";
import { applyHarnessEvent, stopStreaming } from "./runtime/protocol/apply.js";

const FLUSH_MS = 120;
const BATCHED = new Set([
  "message.delta",
  "reasoning.delta",
  "tool.updated",
  "agent.step",
  "status",
]);
const STORAGE_FAILURE =
  "Session storage failed during this turn. Inspect its work before continuing.";

export function settledSession(value, status, message, endedAt = Date.now()) {
  const stopped = stopStreaming(value.session, endedAt);
  const blocks = stopped.blocks.map((block) =>
    block.role === "plan" && block.plan?.status === "building"
      ? {
          ...block,
          plan: {
            ...block.plan,
            status: status === "idle" && !message ? "built" : "ready",
          },
        }
      : block,
  );
  if (message) {
    blocks.push({ id: randomUUID(), role: "system", text: message, streaming: false });
  }
  return { ...value, status, session: { ...stopped, blocks } };
}

function report(message, error) {
  console.error(message, error instanceof Error ? error.message : "unknown error");
}

/** An accepted turn owns its session through provider cleanup and durable settlement. */
export class SessionRuns {
  #runs = new Map();
  #shutdown = new AbortController();
  #closeTask;

  constructor(store) {
    this.store = store;
  }

  get closing() {
    return this.#shutdown.signal.aborted;
  }

  has(id) {
    return this.#runs.has(id);
  }

  publish(value) {
    const run = this.#runs.get(value.session.id);
    if (run && run.runId === value.runId && value.status === "running") run.value = value;
  }

  start(value, provider, input) {
    const run = {
      id: value.session.id,
      runId: value.runId,
      value,
      provider,
      events: [],
      dispatched: false,
      cancelled: false,
      persistenceFailed: false,
      stopped: false,
    };
    this.#runs.set(run.id, run);
    run.done = Promise.resolve().then(() => this.#execute(run, input));
  }

  cancel(id, runId) {
    const run = this.#runs.get(id);
    if (!run || run.runId !== runId || run.cancelled || run.stopped) return;
    run.cancelled = true;
    if (run.dispatched) {
      run.cancellation = Promise.resolve()
        .then(() => {
          if (!run.stopping) return run.provider.cancel(id);
        })
        .catch(() => this.#stop(run))
        .catch((error) => report("Could not stop session provider:", error));
    }
  }

  flush(id) {
    const run = this.#runs.get(id);
    if (run) this.#flush(run);
  }

  #flush(run) {
    clearTimeout(run.flushTimer);
    run.flushTimer = undefined;
    if (!run.events.length) return;
    run.value = this.store.advance(run.value, { type: "events", events: run.events });
    run.events = [];
  }

  #scheduledFlush(run) {
    if (this.#runs.get(run.id) !== run) return;
    try {
      this.#flush(run);
    } catch (error) {
      run.persistenceFailed = true;
      report("Session persistence failed; stopping its provider:", error);
      void this.#stop(run).catch((error) => report("Could not stop session provider:", error));
    }
  }

  #event(run, event) {
    if (this.#runs.get(run.id) !== run || run.stopped) return;
    const session = applyHarnessEvent(run.value.session, event);
    if (session === run.value.session) return;
    run.value = { ...run.value, session };
    run.events.push(event);
    if (BATCHED.has(event.type)) {
      run.flushTimer ??= setTimeout(() => this.#scheduledFlush(run), FLUSH_MS);
    } else {
      this.#scheduledFlush(run);
    }
  }

  #stop(run) {
    // All cleanup paths join the same transport stop. A rejected stop can be retried.
    run.stopping ??= Promise.resolve()
      .then(() => run.provider.stop(run.id))
      .then(() => { run.stopped = true; })
      .catch((error) => {
        run.stopping = undefined;
        throw error;
      });
    return run.stopping;
  }

  async #execute(run, { prompt, intent, attachments, preparedText }) {
    const { session } = run.value;
    const canDispatch = () => !this.closing && !run.cancelled && !run.persistenceFailed;
    let error;
    try {
      if (canDispatch()) {
        const input = {
          sessionId: session.id,
          cwd: session.cwd,
          model: session.model,
          modelSettings: session.modelSettings,
          runtimeMode: session.runtimeMode,
          intent,
          onEvent: (event) => this.#event(run, event),
        };
        if (prompt === null) {
          run.dispatched = true;
          await run.provider.compact(input);
        } else {
          const resolved = [];
          for (const file of attachments) {
            if (!canDispatch()) break;
            resolved.push(
              isVisionImage(file.mimeType) && file.path && file.size <= 20 * 1024 * 1024
                ? { ...file, data: (await readFile(file.path)).toString("base64") }
                : file,
            );
          }
          if (canDispatch()) {
            run.dispatched = true;
            // Prepared context travels only to the provider. The command keeps the visible prompt.
            await run.provider.send({ ...input, text: preparedText ?? prompt, attachments: resolved });
          }
        }
      }
    } catch (reason) {
      error = reason instanceof Error ? reason.message : String(reason);
    }

    try {
      await this.#settle(run, error);
    } finally {
      clearTimeout(run.flushTimer);
      if (this.#runs.get(run.id) === run) this.#runs.delete(run.id);
    }
  }

  async #settle(run, error) {
    let endedAt;
    while (this.#runs.get(run.id) === run) {
      let saved;
      try {
        await this.#stop(run);
        await run.cancellation;
        if (this.#runs.get(run.id) !== run) return;
        endedAt ??= Date.now();
        this.#flush(run);
        const latest = this.store.session(run.id);
        if (latest.runId !== run.runId || latest.status !== "running") return;
        const message = this.closing
          ? "Host stopped. This turn was interrupted."
          : run.persistenceFailed ? STORAGE_FAILURE : run.cancelled ? "Stopped by you." : error;
        saved = this.store.advance(
          settledSession(
            latest,
            this.closing || run.persistenceFailed ? "interrupted" : "idle",
            message,
            endedAt,
          ),
          run.persistenceFailed
            ? { type: "interrupted", reason: "persistence failure" }
            : { type: "settled", error, cancelled: run.cancelled },
        );
      } catch (error) {
        report("Session settlement failed:", error);
        if (this.closing) return;
        run.persistenceFailed = true;
        // On shutdown, wake immediately for the final attempt. Startup can settle a durable running turn.
        await delay(1_000, undefined, { signal: this.#shutdown.signal, ref: false }).catch(() => {});
        continue;
      }
      const { session } = saved;
      try {
        if (session.providerSessionId) run.provider.bind(run.id, session.providerSessionId, session.cwd);
      } catch (error) {
        report("Could not restore session provider binding:", error);
      }
      return;
    }
  }

  close() {
    if (!this.#closeTask) {
      this.#shutdown.abort();
      this.#closeTask = Promise.all([...this.#runs.values()].map(async (run) => {
        await this.#stop(run);
        await run.done;
      }));
    }
    return this.#closeTask;
  }
}
