import { afterEach, beforeEach, expect, it, vi } from "vitest";

const host = vi.hoisted(() => ({
  lines: new Map<string, (line: string) => void>(),
  exits: new Map<string, () => void>(),
  requests: [] as Array<{
    method?: string;
    params?: Record<string, unknown>;
    id?: string | number;
    result?: { success?: boolean; contentItems?: { text: string }[] };
  }>,
  spawn: vi.fn(async (..._args: unknown[]) => {}),
  kill: vi.fn(async (_id: string) => {}),
  release: vi.fn(),
  closeOnStop: true,
  resumeError: undefined as string | undefined,
  threadGate: undefined as Promise<void> | undefined,
  history: { data: [] } as { data: unknown[] },
}));
vi.mock("./memory", () => ({
  voxMemoryContext: async () =>
    "<vox_memory>prefers concise answers</vox_memory>",
}));
vi.mock("../../../integrations/harness/core/child", () => ({
  acquireHarnessBridge: async () => host.release,
  resolveCodexBinary: async () => ({ path: "/fake/codex" }),
  spawnChild: host.spawn,
  killChild: host.kill,
  watchChild: (id: string, line: (line: string) => void, exit: () => void) => {
    host.lines.set(id, line);
    host.exits.set(id, exit);
  },
  writeChild: async (id: string, line: string) => {
    const request = JSON.parse(line);
    host.requests.push(request);
    if (request.id === undefined) return;
    if (request.method === "thread/start" && host.threadGate)
      await host.threadGate;
    if (request.method === "thread/resume" && host.resumeError) {
      host.lines.get(id)!(
        JSON.stringify({
          id: request.id,
          error: { code: -32000, message: host.resumeError },
        }),
      );
      return;
    }
    if (request.method === "thread/realtime/stop" && host.closeOnStop)
      host.lines.get(id)!(
        JSON.stringify({
          method: "thread/realtime/closed",
          params: { threadId: "vox-thread" },
        }),
      );
    if (request.method === "thread/realtime/start") {
      // Real Codex can notify before acknowledging the start request.
      host.lines.get(id)!(
        JSON.stringify({
          method: "thread/realtime/sdp",
          params: { threadId: "vox-thread", sdp: "answer" },
        }),
      );
    }
    const result = ["thread/start", "thread/resume"].includes(request.method)
      ? { thread: { id: "vox-thread" } }
      : request.method === "thread/turns/list"
        ? host.history
        : {};
    host.lines.get(id)!(JSON.stringify({ id: request.id, result }));
  },
}));

import { Vox } from "./vox";
import { VoxDisplay } from "./display";
import { loadVoxConversation, resetSavedVoxConversation } from "./conversation";

class Peer {
  static instances: Peer[] = [];
  connectionState = "new";
  ontrack: ((event: unknown) => void) | null = null;
  onconnectionstatechange: (() => void) | null = null;
  localDescription = { sdp: "offer" };
  addTrack = vi.fn();
  channel = {
    readyState: "open",
    onopen: null as (() => void) | null,
    onclose: null as (() => void) | null,
    onerror: null,
    onmessage: null as ((event: { data: unknown }) => void) | null,
    close: vi.fn(),
  };
  createDataChannel = vi.fn(() => this.channel);
  createOffer = vi.fn(async () => ({ type: "offer", sdp: "offer" }));
  setLocalDescription = vi.fn(async () => {});
  setRemoteDescription = vi.fn(async (_answer: unknown) => {});
  close = vi.fn();
  constructor() {
    Peer.instances.push(this);
  }
  connected() {
    this.connectionState = "connected";
    this.onconnectionstatechange?.();
  }
}
const track = () => ({
  stop: vi.fn(),
  enabled: true,
  onended: null as (() => void) | null,
});

function microphone() {
  const input = track();
  return { input, getTracks: () => [input], getAudioTracks: () => [input] };
}
let mic: ReturnType<typeof microphone>;
let getUserMedia: ReturnType<typeof vi.fn>;
let connected: ReturnType<typeof vi.fn>;
let error: ReturnType<typeof vi.fn>;
let calls: Vox[];
let execute: ReturnType<typeof vi.fn>;
function create(extra: Partial<ConstructorParameters<typeof Vox>[0]> = {}) {
  const call = new Vox({
    cwd: "/repo",
    onConnected: connected,
    onError: error,
    execute,
    ...extra,
  });
  calls.push(call);
  return call;
}
function notify(method: string, params: Record<string, unknown> = {}) {
  [...host.lines.values()].at(-1)!(
    JSON.stringify({ method, params: { threadId: "vox-thread", ...params } }),
  );
}
beforeEach(() => {
  vi.clearAllMocks();
  host.spawn.mockImplementation(async () => {});
  host.lines.clear();
  host.exits.clear();
  host.requests.length = 0;
  host.closeOnStop = true;
  host.resumeError = undefined;
  host.threadGate = undefined;
  host.history = { data: [] };
  const saved = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => saved.get(key) ?? null,
    setItem: (key: string, value: string) => saved.set(key, value),
  });
  Peer.instances = [];
  calls = [];
  mic = microphone();
  getUserMedia = vi.fn(async () => mic);
  connected = vi.fn();
  error = vi.fn();
  execute = vi.fn(async () => ({ submitted: true }));
  vi.stubGlobal("navigator", {
    mediaDevices: {
      getUserMedia,
      enumerateDevices: async () => [
        {
          kind: "audioinput",
          deviceId: "macbook",
          label: "MacBook Pro Microphone",
        },
      ],
    },
  });
  vi.stubGlobal("RTCPeerConnection", Peer);
  vi.stubGlobal(
    "Audio",
    class {
      autoplay = false;
      srcObject = null;
      play = vi.fn(async () => {});
      pause = vi.fn();
    },
  );
});
afterEach(async () => {
  await Promise.all(calls.map((call) => call.stop()));
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

it("does no idle work and negotiates direct audio with a durable private default-account conversation", async () => {
  const call = create();
  expect(getUserMedia).not.toHaveBeenCalled();
  expect(host.spawn).not.toHaveBeenCalled();
  await call.start();
  const peer = Peer.instances[0];
  expect(host.spawn.mock.calls[0].slice(1)).toEqual([
    "/fake/codex",
    ["app-server"],
    "/repo",
    { provider: "codex", id: "default" },
    "codex",
    "bot",
  ]);
  expect(
    host.requests.find((r) => r.method === "initialize")?.params,
  ).toMatchObject({
    capabilities: {
      optOutNotificationMethods: expect.arrayContaining([
        "thread/realtime/transcript/delta",
        "thread/realtime/transcript/done",
        "thread/realtime/outputAudio/delta",
      ]),
    },
  });
  expect(
    host.requests.find((r) => r.method === "thread/start")?.params,
  ).toMatchObject({
    sandbox: "read-only",
    approvalPolicy: "never",
  });
  expect(
    host.requests.find((r) => r.method === "thread/realtime/start")?.params,
  ).toMatchObject({
    version: "v3",
    model: "gpt-live-1-codex",
    outputModality: "audio",
    includeStartupContext: false,
    initialItems: [],
    flushTranscriptTailOnSessionEnd: true,
    transport: { type: "webrtc", sdp: "offer" },
  });
  expect(peer.addTrack).toHaveBeenCalledWith(mic.input, mic);
  const prompt = host.requests.find((r) => r.method === "thread/realtime/start")
    ?.params?.prompt;
  expect(prompt).toContain("You are vox");
  expect(prompt).toContain("Wait silently for the user's first spoken input");
  expect(prompt).toContain("Your backing agent can look up the active session");
  expect(host.requests.some((r) => r.method === "turn/start")).toBe(false);
  expect(execute).not.toHaveBeenCalled();
  expect(
    host.requests.find((r) => r.method === "thread/start")?.params,
  ).not.toHaveProperty("ephemeral");
  expect(prompt).toContain("prefers concise answers");
  expect(peer.setRemoteDescription).toHaveBeenCalledExactlyOnceWith({
    type: "answer",
    sdp: "answer",
  });
  expect(connected).not.toHaveBeenCalled();
  peer.connected();
  expect(connected).toHaveBeenCalledOnce();
  const count = host.requests.length;
  for (let i = 0; i < 100; i++)
    notify("thread/realtime/transcript/delta", { delta: "text" });
  expect(host.requests).toHaveLength(count);
  expect(connected).toHaveBeenCalledOnce();
  expect(error).not.toHaveBeenCalled();
  await call.stop();
  await call.stop();
  expect(mic.input.stop).toHaveBeenCalledOnce();
  expect(peer.close).toHaveBeenCalledOnce();
  expect(peer.channel.close).toHaveBeenCalledOnce();
  expect(host.kill).toHaveBeenCalledOnce();
  expect(host.release).toHaveBeenCalledOnce();
});

it("releases a late microphone grant without touching a replacement call", async () => {
  let grant!: (value: ReturnType<typeof microphone>) => void;
  const oldMic = microphone();
  getUserMedia.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        grant = resolve;
      }),
  );
  const old = create();
  const pending = old.start();
  await vi.waitFor(() => expect(getUserMedia).toHaveBeenCalledOnce());
  await old.stop();
  const next = create();
  await next.start();
  grant(oldMic);
  await pending;
  expect(oldMic.input.stop).toHaveBeenCalledOnce();
  expect(mic.input.stop).not.toHaveBeenCalled();
  expect(host.spawn).toHaveBeenCalledOnce();
  expect(host.kill).not.toHaveBeenCalled();
});

it("waits for admitted spawn before killing a stopped helper", async () => {
  let finishSpawn!: () => void;
  host.spawn.mockImplementationOnce(
    () =>
      new Promise<void>((resolve) => {
        finishSpawn = resolve;
      }),
  );
  const call = create();
  const starting = call.start();
  await vi.waitFor(() => expect(host.spawn).toHaveBeenCalledOnce());
  const stopping = call.stop();
  expect(mic.input.stop).toHaveBeenCalledOnce();
  expect(host.kill).not.toHaveBeenCalled();
  finishSpawn();
  await Promise.all([starting, stopping]);
  expect(host.kill).toHaveBeenCalledExactlyOnceWith(
    host.spawn.mock.calls[0][0],
  );
  expect(host.requests).toHaveLength(0);
});

it("rejects overlapping Vox calls without acquiring another microphone", async () => {
  await create().start();
  await create().start();
  expect(error).toHaveBeenCalledWith(expect.stringContaining("already open"));
  expect(getUserMedia).toHaveBeenCalledOnce();
});

it.each(["thread/realtime/error", "thread/realtime/closed"])(
  "releases audio and process on %s and permits recovery",
  async (method) => {
    const call = create();
    await call.start();
    notify(method);
    expect(mic.input.stop).toHaveBeenCalledOnce();
    expect(Peer.instances[0].close).toHaveBeenCalledOnce();
    expect(error).toHaveBeenCalledOnce();
    await call.stop();
    error.mockClear();
    await create().start();
    expect(error).not.toHaveBeenCalled();
    expect(host.spawn).toHaveBeenCalledTimes(2);
  },
);

function tool(
  id: string,
  callId: string,
  action = "sessions.send",
  input: Record<string, unknown> = {
    sessionId: "target",
    prompt: "Fix the bug",
  },
  threadId = "vox-thread",
  expression?: string,
) {
  [...host.lines.values()].at(-1)!(
    JSON.stringify({
      id,
      method: "item/tool/call",
      params: {
        threadId,
        callId,
        namespace: null,
        tool: "vorkspace",
        arguments: { action, input, expression },
      },
    }),
  );
}

it("uses native voice delegation with a bounded workspace tool, retaining target ownership", async () => {
  await create().start();
  expect(
    host.requests.find((r) => r.method === "thread/start")?.params,
  ).toMatchObject({
    model: "gpt-6.1-sol",
    dynamicTools: [expect.objectContaining({ name: "vorkspace" })],
  });
  notify("turn/started");
  tool("r1", "call-1");
  await vi.waitFor(() =>
    expect(host.requests.find((r) => r.id === "r1")?.result?.success).toBe(
      true,
    ),
  );
  expect(execute).toHaveBeenCalledWith(
    expect.objectContaining({
      action: "sessions.send",
      input: { sessionId: "target", prompt: "Fix the bug" },
    }),
  );
  expect(error).not.toHaveBeenCalled();
  expect(mic.input.stop).not.toHaveBeenCalled();
  tool("r2", "call-1");
  await vi.waitFor(() =>
    expect(host.requests.find((r) => r.id === "r2")?.result?.success).toBe(
      true,
    ),
  );
  expect(execute).toHaveBeenCalledOnce();
  tool("r3", "call-1", "sessions.send", {
    sessionId: "other",
    prompt: "different",
  });
  await vi.waitFor(() =>
    expect(host.requests.find((r) => r.id === "r3")?.result?.success).toBe(
      false,
    ),
  );
  expect(execute).toHaveBeenCalledOnce();
});

it("rejects other threads and unsupported actions before dispatch", async () => {
  await create().start();
  tool("r1", "call-1", "sessions.send", {}, "other-thread");
  tool("r2", "call-2", "sessions.delete");
  await vi.waitFor(() =>
    expect(
      host.requests.filter((r) => r.result?.success === false),
    ).toHaveLength(2),
  );
  expect(execute).not.toHaveBeenCalled();
});

it("does not start queued tools after stop, or publish an old completion into a replacement", async () => {
  const first = create();
  await first.start();
  tool("queued", "queued");
  await first.stop();
  expect(execute).not.toHaveBeenCalled();
  let finish!: (value: unknown) => void;
  execute.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const second = create();
  await second.start();
  tool("pending", "pending");
  await vi.waitFor(() => expect(execute).toHaveBeenCalledOnce());
  const request = execute.mock.calls[0][0];
  expect(request.isCurrent()).toBe(true);
  await second.stop();
  await create().start();
  expect(request.isCurrent()).toBe(false);
  finish({ submitted: true });
  await Promise.resolve();
  expect(host.requests.some((r) => r.id === "pending" && r.result)).toBe(false);
});

it("returns actionable failures without replaying a rejected call or ending voice", async () => {
  execute.mockRejectedValueOnce(new Error("Target session is busy"));
  await create().start();
  tool("r1", "call-1");
  await vi.waitFor(() =>
    expect(host.requests.find((r) => r.id === "r1")?.result?.success).toBe(
      false,
    ),
  );
  expect(
    host.requests.find((r) => r.id === "r1")?.result?.contentItems?.[0].text,
  ).toContain("busy");
  tool("r2", "call-1");
  await vi.waitFor(() =>
    expect(host.requests.find((r) => r.id === "r2")?.result?.success).toBe(
      false,
    ),
  );
  expect(execute).toHaveBeenCalledOnce();
  tool("r3", "call-3", "bots.list", {});
  await vi.waitFor(() =>
    expect(host.requests.find((r) => r.id === "r3")?.result?.success).toBe(
      true,
    ),
  );
  expect(error).not.toHaveBeenCalled();
});

it("bounds admitted tool receipts without evicting earlier mutation identities", async () => {
  await create().start();
  for (let i = 0; i < 129; i++) tool(`bound-${i}`, `bounded-call-${i}`);
  await vi.waitFor(() =>
    expect(
      host.requests.filter(
        (r) =>
          typeof r.id === "string" && r.id.startsWith("bound-") && r.result,
      ),
    ).toHaveLength(129),
  );
  expect(execute).toHaveBeenCalledTimes(128);
  expect(host.requests.find((r) => r.id === "bound-128")?.result?.success).toBe(
    false,
  );
  tool("replay", "bounded-call-0");
  await vi.waitFor(() =>
    expect(host.requests.find((r) => r.id === "replay")?.result?.success).toBe(
      true,
    ),
  );
  expect(execute).toHaveBeenCalledTimes(128);
});

it("bounds cached output and reuses the failure rather than recomputing it", async () => {
  execute.mockResolvedValueOnce({ text: "x".repeat(48_001) });
  await create().start();
  tool("large", "large-call", "sessions.read", { sessionId: "target" });
  await vi.waitFor(() =>
    expect(host.requests.find((r) => r.id === "large")?.result?.success).toBe(
      false,
    ),
  );
  tool("large-again", "large-call", "sessions.read", { sessionId: "target" });
  await vi.waitFor(() =>
    expect(
      host.requests.find((r) => r.id === "large-again")?.result?.success,
    ).toBe(false),
  );
  expect(execute).toHaveBeenCalledOnce();
});

it("ignores other threads and repeated SDP answers", async () => {
  await create().start();
  notify("thread/realtime/error", { threadId: "other" });
  notify("thread/realtime/sdp", { sdp: "duplicate" });
  expect(error).not.toHaveBeenCalled();
  expect(Peer.instances[0].setRemoteDescription).toHaveBeenCalledOnce();
});

it("waits for the data channel and releases the call when it closes", async () => {
  const call = create();
  await call.start();
  const peer = Peer.instances[0];
  peer.channel.readyState = "connecting";
  peer.connected();
  expect(connected).not.toHaveBeenCalled();
  peer.channel.readyState = "open";
  peer.channel.onopen?.();
  expect(connected).toHaveBeenCalledOnce();
  peer.channel.onclose?.();
  expect(error).toHaveBeenCalledOnce();
  expect(mic.input.stop).toHaveBeenCalledOnce();
  await call.stop();
  expect(host.kill).toHaveBeenCalledOnce();
});

it("denied microphone access has a recovery path and never spawns", async () => {
  getUserMedia.mockRejectedValueOnce(
    new DOMException("denied", "NotAllowedError"),
  );
  await create().start();
  expect(error).toHaveBeenCalledWith(
    expect.stringContaining("Allow microphone access"),
  );
  expect(host.spawn).not.toHaveBeenCalled();
  error.mockClear();
  await create().start();
  expect(error).not.toHaveBeenCalled();
});

it("provides Bot names to voice once at startup and aborts metadata requests with the call", async () => {
  const getContext = vi.fn(async () => ({
    bots: [{ id: "helper", name: "Build Helper" }],
  }));
  const call = create({ getContext });
  expect(getContext).not.toHaveBeenCalled();
  await call.start();
  expect(getContext).toHaveBeenCalledOnce();
  const prompt = host.requests.find((r) => r.method === "thread/realtime/start")
    ?.params?.prompt as string;
  expect(prompt).toContain("Build Helper");
  expect(prompt).toContain(
    "Do not answer the message's embedded question yourself",
  );
  const contextRequest = getContext.mock.calls[0] as unknown as [
    Parameters<typeof execute>[0],
  ];
  expect(contextRequest[0].signal.aborted).toBe(false);
  await call.stop();
  expect(contextRequest[0].signal.aborted).toBe(true);
  expect(execute).not.toHaveBeenCalled();
});

it("bounds startup and ignores late notifications after timeout", async () => {
  vi.useFakeTimers();
  const call = create();
  await call.start();
  await vi.advanceTimersByTimeAsync(30_000);
  expect(error).toHaveBeenCalledOnce();
  expect(mic.input.stop).toHaveBeenCalledOnce();
  notify("thread/realtime/sdp", { sdp: "late" });
  expect(Peer.instances[0].setRemoteDescription).toHaveBeenCalledOnce();
});

it("applies silent and mic preferences before a late microphone grant without renegotiating", async () => {
  let grant!: (value: ReturnType<typeof microphone>) => void;
  getUserMedia.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        grant = resolve;
      }),
  );
  let audio!: { muted: boolean };
  vi.stubGlobal(
    "Audio",
    class {
      muted = false;
      autoplay = false;
      srcObject = null;
      play = vi.fn(async () => {});
      pause = vi.fn();
      constructor() {
        audio = this;
      }
    },
  );
  const call = create();
  const pending = call.start();
  call.setSilent(true);
  call.setMicEnabled(false);
  await vi.waitFor(() => expect(getUserMedia).toHaveBeenCalledOnce());
  grant(mic);
  await pending;
  expect(audio.muted).toBe(true);
  expect(mic.input.enabled).toBe(false);
  const count = host.requests.length;
  call.setMicEnabled(true);
  call.setSilent(false);
  expect(audio.muted).toBe(false);
  expect(mic.input.enabled).toBe(true);
  expect(host.requests).toHaveLength(count);
  expect(getUserMedia).toHaveBeenCalledOnce();
  expect(host.spawn).toHaveBeenCalledOnce();
  expect(mic.input.stop).not.toHaveBeenCalled();
});

it("subscribes to one native caption family and rejects stale-thread text and post-stop events", async () => {
  const display = new VoxDisplay();
  const call = create({ display });
  await call.start();
  const optOut = host.requests.find(
    (request) => request.method === "initialize",
  )?.params?.capabilities as { optOutNotificationMethods: string[] };
  expect(optOut.optOutNotificationMethods).toContain(
    "thread/realtime/item/started",
  );
  expect(optOut.optOutNotificationMethods).toContain(
    "thread/realtime/item/transcript/delta",
  );
  expect(optOut.optOutNotificationMethods).toContain(
    "thread/realtime/transcript/delta",
  );
  expect(optOut.optOutNotificationMethods).not.toContain(
    "thread/realtime/transcript/done",
  );
  expect(optOut.optOutNotificationMethods).toContain(
    "thread/realtime/outputAudio/delta",
  );

  notify("thread/realtime/transcript/done", {
    role: "assistant",
    text: "OriginalCase.ts",
  });
  notify("thread/realtime/transcript/done", {
    threadId: "other",
    role: "assistant",
    text: "wrong thread",
  });
  expect(display.getSnapshot().lines[0].text).toBe("OriginalCase.ts");
  await call.stop();
  notify("thread/realtime/transcript/done", {
    role: "assistant",
    text: "stale result",
  });
  expect(display.getSnapshot().lines[0].text).toBe("OriginalCase.ts");
  display.dispose();
});

it("accepts model moods alongside an existing action without changing its payload or replaying the action", async () => {
  const display = new VoxDisplay();
  await create({ display }).start();
  tool(
    "face-1",
    "face-call",
    "composer.write",
    { text: "Review OriginalCase.ts" },
    "vox-thread",
    "curious",
  );
  await vi.waitFor(() =>
    expect(
      host.requests.find((request) => request.id === "face-1")?.result?.success,
    ).toBe(true),
  );
  expect(display.getSnapshot().mood).toBe("curious");
  expect(execute).toHaveBeenCalledWith(
    expect.objectContaining({ input: { text: "Review OriginalCase.ts" } }),
  );
  tool(
    "face-2",
    "face-call",
    "composer.write",
    { text: "Review OriginalCase.ts" },
    "vox-thread",
    "curious",
  );
  tool(
    "face-3",
    "bad-face",
    "composer.write",
    { text: "never insert" },
    "vox-thread",
    "error",
  );
  await vi.waitFor(() =>
    expect(
      host.requests.find((request) => request.id === "face-3")?.result?.success,
    ).toBe(false),
  );
  expect(execute).toHaveBeenCalledOnce();
  expect(display.getSnapshot().mood).toBe("curious");
  display.dispose();
});

it("resumes the same conversation after ending a call without creating a replacement", async () => {
  const first = create();
  await first.start();
  await first.stop();
  host.history = {
    data: [
      {
        items: [
          {
            type: "userMessage",
            content: [{ type: "text", text: "my project is orbit" }],
          },
        ],
      },
    ],
  };
  await create().start();
  expect(
    host.requests.filter((request) => request.method === "thread/start"),
  ).toHaveLength(1);
  expect(
    host.requests.find((request) => request.method === "thread/resume")?.params,
  ).toMatchObject({ threadId: "vox-thread", excludeTurns: true });
  expect(
    host.requests
      .filter((request) => request.method === "thread/turns/list")
      .map((request) => request.params),
  ).toEqual([{ threadId: "vox-thread", limit: 4, itemsView: "full" }]);
  expect(
    host.requests
      .filter((request) => request.method === "thread/realtime/start")
      .at(-1)?.params,
  ).toMatchObject({
    includeStartupContext: false,
    initialItems: [
      {
        role: "assistant",
        text: expect.stringContaining("my project is orbit"),
      },
    ],
  });
  expect(loadVoxConversation().threadId).toBe("vox-thread");
});

it("keeps saved history selected when resume fails instead of starting an empty thread", async () => {
  const first = create();
  await first.start();
  await first.stop();
  const saved = loadVoxConversation();
  host.resumeError = "missing native history";
  const retry = create();
  await retry.start();
  await retry.stop();
  expect(error).toHaveBeenCalledWith(
    expect.stringContaining("saved conversation could not be resumed"),
  );
  expect(loadVoxConversation()).toEqual(saved);
  expect(
    host.requests.filter((request) => request.method === "thread/start"),
  ).toHaveLength(1);
});

it("waits for native transcript persistence after the stop acknowledgement before killing the helper", async () => {
  host.closeOnStop = false;
  const call = create();
  await call.start();
  const stopping = call.stop();
  await vi.waitFor(() =>
    expect(
      host.requests.some(
        (request) => request.method === "thread/realtime/stop",
      ),
    ).toBe(true),
  );
  expect(mic.input.stop).toHaveBeenCalledOnce();
  expect(host.kill).not.toHaveBeenCalled();
  notify("thread/realtime/closed");
  await stopping;
  expect(host.kill).toHaveBeenCalledOnce();
});

it("reports uncertain saving but releases resources and allows a later call", async () => {
  const call = create();
  await call.start();
  host.closeOnStop = false;
  vi.useFakeTimers();
  const stopping = call.stop();
  const failure = expect(stopping).rejects.toThrow(
    "saving the latest speech could not be confirmed",
  );
  await vi.advanceTimersByTimeAsync(5_001);
  await failure;
  expect(host.kill).toHaveBeenCalledOnce();
  expect(host.release).toHaveBeenCalledOnce();
  calls = calls.filter((item) => item !== call);
  host.closeOnStop = true;
  await create().start();
  expect(
    host.requests.filter((request) => request.method === "thread/resume"),
  ).toHaveLength(1);
});

it("records a newly created identity even when End arrives before its acknowledgement", async () => {
  let finish!: () => void;
  host.threadGate = new Promise<void>((resolve) => {
    finish = resolve;
  });
  const call = create();
  const starting = call.start();
  await vi.waitFor(() =>
    expect(
      host.requests.some((request) => request.method === "thread/start"),
    ).toBe(true),
  );
  const stopping = call.stop();
  expect(host.kill).not.toHaveBeenCalled();
  finish();
  await Promise.all([starting, stopping]);
  expect(loadVoxConversation().threadId).toBe("vox-thread");
  expect(
    host.requests.some((request) => request.method === "thread/realtime/start"),
  ).toBe(false);
  await create().start();
  expect(
    host.requests.some((request) => request.method === "thread/resume"),
  ).toBe(true);
});

it("does not restore a retired conversation after reset races a creation acknowledgement", async () => {
  let finish!: () => void;
  host.threadGate = new Promise<void>((resolve) => {
    finish = resolve;
  });
  const call = create();
  const starting = call.start();
  await vi.waitFor(() =>
    expect(
      host.requests.some((request) => request.method === "thread/start"),
    ).toBe(true),
  );
  resetSavedVoxConversation();
  const reset = loadVoxConversation();
  finish();
  await starting;
  await call.stop();
  expect(loadVoxConversation()).toEqual(reset);
  expect(reset.threadId).toBeUndefined();
});

it("observes bounded playback control frames on the existing channel and ignores late frames after End", async () => {
  const display = new VoxDisplay();
  const call = create({ display });
  await call.start();
  const receive = Peer.instances[0].channel.onmessage!;
  receive({
    data: JSON.stringify({
      type: "output_transcript.added",
      start_ms: 0,
    }),
  });
  expect(display.getSnapshot().activity).toBe("replying");
  receive({ data: "invalid" });
  receive({ data: "x".repeat(9_000) });
  expect(display.getSnapshot().activity).toBe("replying");
  await call.stop();
  display.finish();
  receive({
    data: JSON.stringify({
      type: "output_transcript.added",
      start_ms: 1000,
    }),
  });
  expect(display.getSnapshot().activity).toBe("listening");
  expect(Peer.instances[0].channel.onmessage).toBeNull();
  display.dispose();
});
