import { homeDir } from "../../platform/fs.js";
import { AcpClient } from "../../protocol/acp.js";
import {
  killChild,
  resolveAntigravityBinary,
  spawnChild,
  unwatchChild,
  watchChild,
} from "../../protocol/child.js";
import {
  antigravitySpawnCwd,
  modelsFromSessionNew,
} from "./antigravityProtocol.js";
const PROBE_ID = "vorkspace-antigravity-probe";
const REQUEST_TIMEOUT_MS = 12000;
export async function discoverAntigravityModels(workingDirectory) {
  const { path, args } = await resolveAntigravityBinary();
  const cwd = workingDirectory ?? (await homeDir());
  const probeId = `${PROBE_ID}-${crypto.randomUUID()}`;
  const acp = new AcpClient(probeId, {
    onRequest: (id, method) => {
      const response =
        method === "session/request_permission"
          ? acp.respond(id, { outcome: { outcome: "cancelled" } })
          : acp.respondError(id, {
              code: -32601,
              message: `Method not found: ${method}`,
            });
      void response.catch(() => undefined);
    },
  });
  watchChild(
    probeId,
    (line) => acp.pushLine(line),
    () => acp.close(new Error("Antigravity probe exited")),
  );
  try {
    await spawnChild(
      probeId,
      path,
      args,
      antigravitySpawnCwd(path, cwd),
      undefined,
      "antigravity",
    );
    await acp.request(
      "initialize",
      {
        protocolVersion: 1,
        clientCapabilities: {
          fs: { readTextFile: false, writeTextFile: false },
          terminal: false,
          // Without this claim a compliant agent may omit configOptions from
          // session/new, leaving the catalog with nothing to enumerate.
          session: { configOptions: { boolean: {} } },
        },
        clientInfo: { name: "vorkspace", version: "0.1.0" },
      },
      REQUEST_TIMEOUT_MS,
    );
    // Authentication stays in the provider's Terminal UI; never start OAuth here.
    const created = await acp.request(
      "session/new",
      { cwd, mcpServers: [] },
      REQUEST_TIMEOUT_MS,
    );
    return modelsFromSessionNew(created);
  } finally {
    acp.close();
    unwatchChild(probeId);
    await killChild(probeId).catch(() => undefined);
  }
}
