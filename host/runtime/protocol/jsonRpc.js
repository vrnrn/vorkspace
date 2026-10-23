import { writeChild } from "./child.js";
// A child wedged hard enough to block stdin writes is unrecoverable; the
// write deadline fails the request so the caller can recycle the generation.
const WRITE_TIMEOUT_MS = 15000;
/**
 * Bidirectional JSON-RPC / JSONL client over a harness child process stdin/stdout.
 * Supports numeric and string request ids; Codex app-server uses headerless frames.
 */
export class JsonRpcClient {
  sessionId;
  handlers;
  nextId = 1;
  pending = new Map();
  closed = false;
  includeJsonrpc;
  label;
  constructor(sessionId, handlers, options = {}) {
    this.sessionId = sessionId;
    this.handlers = handlers;
    this.includeJsonrpc = options.includeJsonrpc !== false;
    this.label = options.label ?? "rpc";
  }
  pushLine(line) {
    const trimmed = line.trim();
    if (!trimmed) return;
    let msg;
    try {
      msg = JSON.parse(trimmed);
    } catch {
      console.debug(
        `[${this.label} ${this.sessionId}] non-json`,
        trimmed.slice(0, 200),
      );
      return;
    }
    this.handle(msg);
  }
  close(error) {
    if (this.closed) return;
    this.closed = true;
    const err = error ?? new Error("Harness process exited");
    this.rejectPending(err);
  }
  /** Drop in-flight client requests so a cancelled turn can unwind. */
  rejectPending(error) {
    const err = error ?? new Error("cancelled");
    for (const [, pending] of this.pending) pending.reject(err);
    this.pending.clear();
  }
  get isClosed() {
    return this.closed;
  }
  async request(method, params, timeoutMs = 0) {
    if (this.closed) throw new Error("Harness process is not running");
    const id = this.nextId++;
    const key = String(id);
    // Register before writing. A local harness can answer quickly enough for
    // Tauri to deliver its stdout event before harness_write resolves; adding
    // the pending entry after send() silently discarded that response.
    const response = new Promise((resolve, reject) => {
      const timer =
        timeoutMs > 0
          ? setTimeout(() => {
              this.pending.delete(key);
              reject(new Error(`${method} timed out`));
            }, timeoutMs)
          : undefined;
      this.pending.set(key, {
        resolve: (value) => {
          if (timer) clearTimeout(timer);
          resolve(value);
        },
        reject: (error) => {
          if (timer) clearTimeout(timer);
          reject(error);
        },
      });
    });
    // The deadline may settle `response` while we are still suspended in
    // send() below; mark it handled so that window can't surface as an
    // unhandled rejection. The promise returned to the caller still settles.
    response.catch(() => undefined);
    try {
      await this.send({
        ...(this.includeJsonrpc ? { jsonrpc: "2.0" } : {}),
        id,
        method,
        ...(params !== undefined ? { params } : {}),
      });
    } catch (error) {
      const pending = this.pending.get(key);
      if (pending) {
        this.pending.delete(key);
        pending.reject(
          error instanceof Error ? error : new Error(String(error)),
        );
      }
    }
    return response;
  }
  async notify(method, params) {
    if (this.closed) return;
    await this.send({
      ...(this.includeJsonrpc ? { jsonrpc: "2.0" } : {}),
      method,
      ...(params !== undefined ? { params } : {}),
    });
  }
  async respond(id, result) {
    await this.send({
      ...(this.includeJsonrpc ? { jsonrpc: "2.0" } : {}),
      id,
      result,
    });
  }
  async respondError(id, error) {
    await this.send({
      ...(this.includeJsonrpc ? { jsonrpc: "2.0" } : {}),
      id,
      error,
    });
  }
  async send(payload) {
    // Bound the write: a child that stops draining stdin must not let a
    // blocked harness_write outlive the request's own deadline (or wedge a
    // cancellation waiting on the session/cancel notify).
    const line = JSON.stringify(payload);
    let timer;
    try {
      await Promise.race([
        writeChild(this.sessionId, line),
        new Promise((_, reject) => {
          timer = setTimeout(
            () => reject(new Error("harness write timed out")),
            WRITE_TIMEOUT_MS,
          );
        }),
      ]);
    } finally {
      if (timer) clearTimeout(timer);
    }
  }
  handle(msg) {
    if (msg.id != null && (msg.result !== undefined || msg.error)) {
      const key = String(msg.id);
      const pending = this.pending.get(key);
      if (!pending) return;
      this.pending.delete(key);
      if (msg.error) {
        pending.reject(
          new Error(
            msg.error.message || `${this.label} error ${msg.error.code ?? ""}`,
          ),
        );
        return;
      }
      pending.resolve(msg.result);
      return;
    }
    if (msg.method && msg.id != null) {
      void this.handlers.onRequest?.(msg.id, msg.method, msg.params);
      return;
    }
    if (msg.method) {
      this.handlers.onNotification?.(msg.method, msg.params);
    }
  }
}
