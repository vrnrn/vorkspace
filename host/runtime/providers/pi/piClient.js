import { writeChild } from "../../protocol/child.js";
import { parseJsonLine, parseRpcResponse, stringField } from "./piProtocol.js";
/**
 * JSONL request/response multiplexer for `pi --mode rpc` and the identical
 * `omp --mode rpc`. Agent events and extension UI frames go to `onFrame`.
 */
export class PiRpc {
  sessionId;
  onFrame;
  label;
  nextId = 1;
  pending = new Map();
  closed = false;
  constructor(sessionId, onFrame, label = "Pi") {
    this.sessionId = sessionId;
    this.onFrame = onFrame;
    this.label = label;
  }
  pushLine(line) {
    const rec = parseJsonLine(line);
    if (!rec) return;
    const response = parseRpcResponse(rec);
    if (response?.id && this.pending.has(response.id)) {
      const pending = this.pending.get(response.id);
      this.pending.delete(response.id);
      if (!pending) return;
      if (!response.success) {
        pending.reject(
          new Error(
            response.error || `${this.label} ${response.command} failed`,
          ),
        );
        return;
      }
      pending.resolve(rec);
      return;
    }
    this.onFrame(rec);
  }
  async request(command, timeoutMs = 15000) {
    if (this.closed) throw new Error(`${this.label} process is not running`);
    const id = stringField(command, "id") ?? `mc_${this.nextId++}`;
    if (this.pending.has(id))
      throw new Error(`Duplicate RPC request id: ${id}`);
    const payload = { ...command, id };
    const pending = new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        const type = stringField(command, "type") ?? "command";
        reject(new Error(`${this.label} ${type} timed out`));
      }, timeoutMs);
      this.pending.set(id, {
        resolve: (value) => {
          clearTimeout(timer);
          resolve(value);
        },
        reject: (error) => {
          clearTimeout(timer);
          reject(error);
        },
      });
    });
    void writeChild(this.sessionId, JSON.stringify(payload)).catch((error) => {
      const message = error instanceof Error ? error : new Error(String(error));
      const request = this.pending.get(id);
      this.pending.delete(id);
      request?.reject(message);
    });
    return pending;
  }
  close(error) {
    if (this.closed) return;
    this.closed = true;
    const err = error ?? new Error(`${this.label} process exited`);
    for (const pending of this.pending.values()) pending.reject(err);
    this.pending.clear();
  }
  cancelRequest(id) {
    const pending = this.pending.get(id);
    this.pending.delete(id);
    pending?.reject(new Error(`${this.label} request cancelled`));
  }
}
