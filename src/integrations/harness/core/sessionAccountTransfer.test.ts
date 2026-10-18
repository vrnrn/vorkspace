import { afterEach, describe, expect, it, vi } from "vitest";
import { transferProviderSession } from "./child";
import {
  registerHarness,
  resetHarnessIdlePark,
  sendHarnessTurn,
  stopHarnessSession,
  transferHarnessSessionAccount,
  type HarnessAdapter,
} from "./registry";

vi.mock("./child", () => ({
  transferProviderSession: vi.fn(),
  hasHeadlessChildBackend: () => false,
}));

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}
function setup(id: string) {
  const order: string[] = [];
  const adapter: HarnessAdapter = {
    id: "codex",
    live: true,
    sendTurn: vi.fn(async () => {
      order.push("send");
    }),
    steerTurn: vi.fn(async () => {}),
    cancelTurn: vi.fn(async () => {}),
    respondApproval: vi.fn(),
    stopSession: vi.fn(async () => {
      order.push("stop");
    }),
    forgetSession: vi.fn(async () => {
      order.push("forget");
    }),
    bindSession: vi.fn(() => {
      order.push("bind");
    }),
  };
  registerHarness(adapter);
  const input = {
    harness: "codex" as const,
    threadId: id,
    providerSessionId: "provider-thread",
    cwd: "/project",
    fromAccountId: "old",
    toAccountId: "new",
  };
  return { adapter, input, order };
}

describe("account transfer ownership", () => {
  afterEach(() => {
    resetHarnessIdlePark();
    vi.resetAllMocks();
  });

  it("stops, copies and binds before a queued send can run", async () => {
    const { input, order, adapter } = setup("transfer-queued-send");
    const copy = deferred();
    vi.mocked(transferProviderSession).mockImplementation(async () => {
      order.push("copy");
      await copy.promise;
    });
    const transfer = transferHarnessSessionAccount(input);
    await vi.waitFor(() => expect(order).toEqual(["stop", "copy"]));
    const sending = sendHarnessTurn({
      harness: "codex",
      sessionId: input.threadId,
      cwd: "/project",
      text: "Continue",
      model: "codex:gpt-5.4",
      runtimeMode: "supervised",
      onEvent: vi.fn(),
    });
    expect(adapter.sendTurn).not.toHaveBeenCalled();
    copy.resolve();
    await expect(transfer).resolves.toBe(true);
    await sending;
    expect(order).toEqual(["stop", "copy", "bind", "send"]);
    expect(adapter.bindSession).toHaveBeenCalledWith(
      input.threadId,
      "provider-thread",
      "/project",
      "new",
    );
  });

  it("does not bind when the session changes while copying", async () => {
    const { input, adapter } = setup("transfer-stale-owner");
    const copy = deferred();
    vi.mocked(transferProviderSession).mockReturnValue(copy.promise);
    let current = true;
    const transfer = transferHarnessSessionAccount({
      ...input,
      isCurrent: () => current,
    });
    await vi.waitFor(() => expect(transferProviderSession).toHaveBeenCalled());
    current = false;
    copy.resolve();
    await expect(transfer).resolves.toBe(false);
    expect(adapter.bindSession).not.toHaveBeenCalled();
  });

  it("honors an explicit stop during copying even if caller state has not rendered", async () => {
    const { input, adapter } = setup("transfer-stopped");
    const copy = deferred();
    vi.mocked(transferProviderSession).mockReturnValue(copy.promise);
    const transfer = transferHarnessSessionAccount(input);
    await vi.waitFor(() => expect(transferProviderSession).toHaveBeenCalled());
    await stopHarnessSession("codex", input.threadId);
    copy.resolve();
    await expect(transfer).resolves.toBe(false);
    expect(adapter.bindSession).not.toHaveBeenCalled();
  });

  it("releases the operation queue after copy failure so a retry can succeed", async () => {
    const { input, adapter } = setup("transfer-retry");
    vi.mocked(transferProviderSession).mockRejectedValueOnce(
      new Error("copy failed"),
    );
    await expect(transferHarnessSessionAccount(input)).rejects.toThrow(
      "copy failed",
    );
    expect(adapter.bindSession).not.toHaveBeenCalled();
    vi.mocked(transferProviderSession).mockResolvedValueOnce(undefined);
    await expect(transferHarnessSessionAccount(input)).resolves.toBe(true);
    expect(adapter.bindSession).toHaveBeenCalledTimes(1);
  });
  it("retains the original conversation when a queued send supersedes the copy", async () => {
    const { input, adapter } = setup("transfer-original-resume");
    let resumedAccount = "old";
    adapter.forgetSession = vi.fn(async () => {
      resumedAccount = "";
    });
    adapter.bindSession = vi.fn((_id, _provider, _cwd, account) => {
      resumedAccount = account ?? "";
    });
    const sentFrom: string[] = [];
    adapter.sendTurn = vi.fn(async () => {
      sentFrom.push(resumedAccount);
    });
    const copy = deferred();
    vi.mocked(transferProviderSession).mockReturnValue(copy.promise);
    let current = true;
    const transfer = transferHarnessSessionAccount({
      ...input,
      isCurrent: () => current,
    });
    await vi.waitFor(() => expect(transferProviderSession).toHaveBeenCalled());
    current = false;
    const sending = sendHarnessTurn({
      harness: "codex",
      sessionId: input.threadId,
      cwd: "/project",
      text: "Continue",
      model: "codex:gpt-5.4",
      runtimeMode: "supervised",
      onEvent: vi.fn(),
    });
    copy.resolve();
    await expect(transfer).resolves.toBe(false);
    await sending;
    expect(sentFrom).toEqual(["old"]);
    expect(adapter.forgetSession).not.toHaveBeenCalled();
    expect(adapter.bindSession).not.toHaveBeenCalled();
  });
});
