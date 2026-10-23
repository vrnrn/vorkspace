import { JsonRpcClient } from "./jsonRpc.js";
/** ACP JSON-RPC client — thin wrapper using numeric request IDs. */
export class AcpClient {
  handlers;
  rpc;
  constructor(sessionId, handlers) {
    this.handlers = handlers;
    const rpcHandlers = {
      onNotification: (method, params) =>
        this.handlers.onNotification?.(method, params),
      onRequest: (id, method, params) => {
        const numeric = typeof id === "number" ? id : Number(id);
        void this.handlers.onRequest?.(numeric, method, params);
      },
    };
    this.rpc = new JsonRpcClient(sessionId, rpcHandlers, {
      includeJsonrpc: true,
      label: "acp",
    });
  }
  pushLine(line) {
    this.rpc.pushLine(line);
  }
  close(error) {
    this.rpc.close(error);
  }
  rejectPending(error) {
    this.rpc.rejectPending(error);
  }
  request(method, params, timeoutMs = 0) {
    return this.rpc.request(method, params, timeoutMs);
  }
  notify(method, params) {
    return this.rpc.notify(method, params);
  }
  respond(id, result) {
    return this.rpc.respond(id, result);
  }
  respondError(id, error) {
    return this.rpc.respondError(id, error);
  }
}
