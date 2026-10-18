import {
  acquireHarnessBridge,
  killChild,
  resolveCodexBinary,
  spawnChild,
  watchChild,
} from "../../../integrations/harness/core/child";
import { JsonRpcClient } from "../../../integrations/harness/core/jsonRpc";
import type { JsonRpcId } from "../../../integrations/harness/core/jsonRpc";
import type { VoxDisplay } from "./display";
import { VOX_MOODS, type VoxMood } from "./expressions";
import { captureVoxMicrophone, VoxMicrophoneError } from "./microphone";
import {
  loadVoxConversation,
  saveVoxConversation,
  VoxConversationError,
} from "./conversation";
import { voxMemoryContext } from "./memory";
import { recentVoxContext } from "./context";
import {
  VOX_ACTIONS,
  VOX_AGENT_INSTRUCTIONS,
  VOX_INSTRUCTIONS,
  VOX_TOOL,
  voxCatalog,
  type VoxRequestHandler,
} from "./protocol";

type Options = {
  cwd: string;
  onConnected: () => void;
  onError: (message: string) => void;
  onStopped?: (saving: Promise<void>) => void;
  execute: VoxRequestHandler;
  getContext?: VoxRequestHandler;
  display?: VoxDisplay;
  silent?: boolean;
  micEnabled?: boolean;
};

// One call per renderer. A stopped generation keeps its own child identity so
// late permission/spawn completions cannot dispose a replacement call.
let active: Vox | undefined;
export async function stopActiveVox(): Promise<void> {
  await active?.stop();
}
const CONNECT_TIMEOUT_MS = 30_000;

/** Codex owns OAuth and voice delegation; the workspace owns app actions.
 * WebRTC owns audio. Only bounded native caption text enters the display store.
 */
export class Vox {
  private readonly id = `vorkspace-vox-${crypto.randomUUID()}`;
  private stopped = false;
  private started = false;
  private peer?: RTCPeerConnection;
  private channel?: RTCDataChannel;
  private microphone?: MediaStream;
  private audio?: HTMLAudioElement;
  private rpc?: JsonRpcClient;
  private spawn?: Promise<void>;
  private stopping?: Promise<void>;
  private timeout?: ReturnType<typeof setTimeout>;
  private threadId?: string;
  private resumed = false;
  private opening?: Promise<void>;
  private realtimeStarting?: Promise<unknown>;
  private resolveClosed!: () => void;
  private readonly closed = new Promise<void>((resolve) => {
    this.resolveClosed = resolve;
  });
  private acceptingAnswer = false;
  private connected = false;
  private silent: boolean;
  private micEnabled: boolean;
  private releaseBridge?: () => void;
  private readonly abort = new AbortController();
  private toolCalls = new Map<
    string,
    { signature: string; result: Promise<string> }
  >();

  constructor(private readonly options: Options) {
    this.silent = options.silent ?? false;
    this.micEnabled = options.micEnabled ?? true;
  }

  setSilent(silent: boolean) {
    this.silent = silent;
    if (this.audio) this.audio.muted = silent;
  }

  setMicEnabled(enabled: boolean) {
    this.micEnabled = enabled;
    for (const track of this.microphone?.getAudioTracks() ?? [])
      track.enabled = enabled;
  }

  async start(): Promise<void> {
    if (this.stopped || this.started) return;
    this.started = true;
    if (active) {
      this.options.onError("Vox is already open. Stop it first.");
      return;
    }
    active = this;
    this.timeout = setTimeout(
      () =>
        this.fail(
          "Vox could not connect. Check your Codex ChatGPT sign-in and try again.",
        ),
      CONNECT_TIMEOUT_MS,
    );
    try {
      await this.connect();
    } catch (error) {
      if (!this.stopped) this.fail(voxError(error));
    }
  }

  private async connect(): Promise<void> {
    if (
      !navigator.mediaDevices?.getUserMedia ||
      typeof RTCPeerConnection === "undefined"
    ) {
      throw new Error(
        "This webview does not support microphone capture and WebRTC.",
      );
    }
    const [microphone, binary, catalog, memory] = await Promise.all([
      captureVoxMicrophone(() => !this.stopped).then((stream) => {
        if (this.stopped) stream.getTracks().forEach((track) => track.stop());
        else {
          this.microphone = stream;
          for (const track of stream.getAudioTracks())
            track.enabled = this.micEnabled;
        }
        return stream;
      }),
      resolveCodexBinary(),
      this.options
        .getContext?.({
          sessionId: this.id,
          requestId: crypto.randomUUID(),
          action: "workspace.current",
          input: {},
          isCurrent: () => !this.stopped,
          signal: this.abort.signal,
        })
        .then(voxCatalog)
        .catch(() => "") ?? "",
      voxMemoryContext(),
    ]);
    if (this.stopped) return;

    const peer = new RTCPeerConnection();
    this.peer = peer;
    this.audio = new Audio();
    this.audio.autoplay = true;
    this.audio.muted = this.silent;
    peer.ontrack = (event) => {
      if (this.stopped || !this.audio) return;
      this.audio.srcObject = event.streams[0] ?? new MediaStream([event.track]);
      void this.audio.play().catch(() => {
        this.fail("Vox playback was blocked. Stop Vox and try again.");
      });
    };
    peer.onconnectionstatechange = () => {
      if (this.stopped) return;
      if (peer.connectionState === "connected") {
        this.publishConnected();
      } else if (
        peer.connectionState === "failed" ||
        peer.connectionState === "closed" ||
        peer.connectionState === "disconnected"
      ) {
        this.fail("Vox disconnected. Try starting Vox again.");
      }
    };
    for (const track of microphone.getAudioTracks()) {
      track.onended = () =>
        this.fail("The microphone disconnected. Check it and try again.");
      peer.addTrack(track, microphone);
    }
    // Negotiate the events channel; Codex's authenticated sideband consumes
    // control events. Media stays on the peer, never on the JSON-RPC pipe.
    this.channel = peer.createDataChannel("oai-events");
    this.channel.onopen = () => this.publishConnected();
    this.channel.onmessage = ({ data }) => {
      if (this.stopped || typeof data !== "string" || data.length > 64_000)
        return;
      try {
        const value: unknown = JSON.parse(data);
        if (value && typeof value === "object" && !Array.isArray(value))
          this.options.display?.realtime(value as Record<string, unknown>);
      } catch {
        // An unsupported control frame must never interrupt voice transport.
      }
    };
    this.channel.onclose = () =>
      this.fail("The Vox connection ended. Start Vox again to reconnect.");
    this.channel.onerror = () =>
      this.fail("The Vox connection failed. Try starting Vox again.");
    await peer.setLocalDescription(await peer.createOffer());
    if (this.stopped) return;
    const sdp = peer.localDescription?.sdp;
    if (!sdp)
      throw new Error("Could not prepare the voice connection. Try again.");

    const release = await acquireHarnessBridge();
    if (this.stopped) {
      release();
      return;
    }
    this.releaseBridge = release;

    const rpc: JsonRpcClient = new JsonRpcClient(
      this.id,
      {
        onNotification: (method, params) => this.notification(method, params),
        onRequest: (id, method, params) => {
          void this.request(rpc, id, method, params).catch(() => undefined);
        },
      },
      { includeJsonrpc: false, label: "vox" },
    );
    this.rpc = rpc;
    watchChild(
      this.id,
      (line) => rpc.pushLine(line),
      () => {
        this.rpc?.close(new Error("The Codex voice helper exited"));
        this.fail(
          "The Codex voice helper exited. Check your Codex sign-in and try again.",
        );
      },
    );
    this.spawn = spawnChild(
      this.id,
      binary.path,
      ["app-server"],
      this.options.cwd,
      { provider: "codex", id: "default" },
      "codex",
      "mono",
    );
    await this.spawn;
    if (this.stopped) return;
    await rpc.request(
      "initialize",
      {
        clientInfo: { name: "vorkspace", title: "vox", version: "0.1.0" },
        capabilities: {
          experimentalApi: true,
          // Suppress unused streams in app-server so they never enter stdout
          // batching, IPC, or renderer parsing. Only final native replies are
          // needed; partial speech and all audio stay off the display path.
          optOutNotificationMethods: [
            "thread/realtime/started",
            "thread/realtime/itemAdded",
            "thread/realtime/item/started",
            "thread/realtime/item/transcript/delta",
            "thread/realtime/item/completed",
            "thread/realtime/transcript/delta",
            ...(this.options.display
              ? []
              : ["thread/realtime/transcript/done"]),
            "thread/realtime/outputAudio/delta",
            "item/started",
            "item/completed",
            "item/agentMessage/delta",
            "item/reasoning/textDelta",
            "item/reasoning/summaryTextDelta",
            "item/commandExecution/outputDelta",
            "thread/tokenUsage/updated",
          ],
        },
      },
      CONNECT_TIMEOUT_MS,
    );
    if (this.stopped) return;
    await rpc.notify("initialized");
    this.opening = (async () => {
      const saved = loadVoxConversation();
      this.resumed = !!saved.threadId;
      const params = {
        cwd: this.options.cwd,
        model: "gpt-6.1-sol",
        config: { model_reasoning_effort: "low" },
        approvalPolicy: "never",
        sandbox: "read-only",
        developerInstructions: [VOX_AGENT_INSTRUCTIONS, memory].join("\n\n"),
      };
      // Resume failures never silently replace a long-lived conversation.
      const result = await rpc
        .request<{ thread: { id: string } }>(
          saved.threadId ? "thread/resume" : "thread/start",
          saved.threadId
            ? { ...params, threadId: saved.threadId, excludeTurns: true }
            : { ...params, dynamicTools: [VOX_TOOL] },
          CONNECT_TIMEOUT_MS,
        )
        .catch((error) => {
          if (saved.threadId)
            throw new VoxConversationError(
              "vox's saved conversation could not be resumed. try again or reset it in settings → vox.",
            );
          throw error;
        });
      const id = result.thread?.id?.trim();
      if (!id || (saved.threadId && id !== saved.threadId))
        throw new VoxConversationError(
          "vox's saved conversation could not be resumed. try again or reset it in settings → vox.",
        );
      // Complete this save even if End arrives while thread/start is pending.
      // No voice session starts until its durable identity is recorded.
      saveVoxConversation(saved, id);
      this.threadId = id;
    })();
    await this.opening;
    if (this.stopped) return;
    const initialItems = this.resumed
      ? recentVoxContext(
          await rpc.request(
            "thread/turns/list",
            { threadId: this.threadId, limit: 4, itemsView: "full" },
            CONNECT_TIMEOUT_MS,
          ),
        )
      : [];
    if (this.stopped) return;
    this.realtimeStarting = rpc.request(
      "thread/realtime/start",
      {
        threadId: this.threadId,
        outputModality: "audio",
        version: "v3",
        model: "gpt-live-1-codex",
        transport: { type: "webrtc", sdp },
        includeStartupContext: false,
        initialItems,
        flushTranscriptTailOnSessionEnd: true,
        prompt: [VOX_INSTRUCTIONS, catalog, memory]
          .filter(Boolean)
          .join("\n\n"),
      },
      CONNECT_TIMEOUT_MS,
    );
    await this.realtimeStarting;
  }

  private async request(
    rpc: JsonRpcClient,
    id: JsonRpcId,
    method: string,
    params: unknown,
  ): Promise<void> {
    if (this.stopped) return;
    if (method === "currentTime/read") {
      await rpc.respond(id, { currentTimeAt: Math.floor(Date.now() / 1000) });
      return;
    }
    if (method === "item/tool/call") {
      try {
        const value = params as {
          threadId?: string;
          tool?: string;
          namespace?: string | null;
          callId?: string;
          arguments?: unknown;
        };
        if (
          value?.threadId !== this.threadId ||
          value.tool !== "vorkspace" ||
          value.namespace ||
          typeof value.callId !== "string"
        )
          throw new Error("Unknown Vox tool call");
        const args = value.arguments as {
          action?: string;
          input?: Record<string, unknown>;
          expression?: VoxMood;
        };
        if (
          !args ||
          !VOX_ACTIONS.includes(args.action as (typeof VOX_ACTIONS)[number]) ||
          !args.input ||
          typeof args.input !== "object" ||
          Array.isArray(args.input)
        )
          throw new Error("Invalid workspace action");
        if (
          args.expression !== undefined &&
          !VOX_MOODS.includes(args.expression)
        )
          throw new Error("unknown vox expression");
        const signature = JSON.stringify(args);
        if (signature.length > 64_000)
          throw new Error("Voice request is too large");
        let call = this.toolCalls.get(value.callId);
        if (call && call.signature !== signature)
          throw new Error("Tool call ID was reused with different input");
        if (!call) {
          // Retain receipts for the call's lifetime; never evict a mutation and
          // then execute its replay again. A new voice call resets the bound.
          if (this.toolCalls.size >= 128)
            throw new Error("Start Vox again to continue");
          const result = Promise.resolve().then(async () => {
            if (this.stopped)
              throw new Error("Vox stopped before this request was sent");
            if (args.expression) this.options.display?.mood(args.expression);
            const output = await this.options.execute({
              sessionId: this.id,
              requestId: crypto.randomUUID(),
              action: args.action!,
              input: args.input!,
              isCurrent: () => !this.stopped,
              signal: this.abort.signal,
            });
            const text = JSON.stringify(output);
            if (!text || text.length > 48_000)
              throw new Error(
                "Result is too large. Request a narrower page of context.",
              );
            return text;
          });
          call = { signature, result };
          this.toolCalls.set(value.callId, call);
        }
        const result = await call.result;
        if (!this.stopped)
          await rpc.respond(id, {
            success: true,
            contentItems: [{ type: "inputText", text: result }],
          });
      } catch (error) {
        if (!this.stopped)
          await rpc.respond(id, {
            success: false,
            contentItems: [
              {
                type: "inputText",
                text:
                  error instanceof Error
                    ? error.message
                    : "Workspace action failed",
              },
            ],
          });
      }
      return;
    }
    // The controller delegates coding to existing sessions. Its own helper
    // never approves shell commands, file edits, or other external tools.
    if (method.endsWith("/requestApproval"))
      await rpc.respond(id, { decision: "decline" });
    else
      await rpc.respondError(id, {
        code: -32601,
        message: "Use the vorkspace tool to control sessions and Monos.",
      });
  }

  private publishConnected(): void {
    if (
      this.stopped ||
      this.connected ||
      this.peer?.connectionState !== "connected" ||
      this.channel?.readyState !== "open"
    )
      return;
    this.connected = true;
    clearTimeout(this.timeout);
    this.options.onConnected();
  }

  private notification(method: string, params: unknown): void {
    if (!params || typeof params !== "object") return;
    const value = params as Record<string, unknown>;
    if (value.threadId !== this.threadId) return;
    if (method === "thread/realtime/closed") this.resolveClosed();
    if (this.stopped) return;
    this.options.display?.notification(method, value);
    if (method === "thread/realtime/sdp" && typeof value.sdp === "string") {
      if (this.acceptingAnswer) return;
      this.acceptingAnswer = true;
      void this.peer
        ?.setRemoteDescription({ type: "answer", sdp: value.sdp })
        .catch(() => {
          this.fail(
            "Could not negotiate voice audio. Update Codex and try again.",
          );
        });
    } else if (method === "thread/realtime/error") {
      // Keep provider details out of UI: they can include URLs or credentials.
      this.fail(
        "ChatGPT voice is unavailable. Check your Codex ChatGPT sign-in, update Codex, and try again.",
      );
    } else if (method === "thread/realtime/closed") {
      this.fail("The Vox session ended. Start Vox again to reconnect.");
    }
  }

  private fail(message: string): void {
    if (this.stopped) return;
    void this.stop().catch(() => undefined);
    this.options.onError(message);
  }

  stop(): Promise<void> {
    if (this.stopping) return this.stopping;
    this.stopped = true;
    this.abort.abort();
    clearTimeout(this.timeout);
    for (const track of this.microphone?.getTracks() ?? []) {
      track.onended = null;
      track.stop();
    }
    this.microphone = undefined;
    if (this.channel) {
      this.channel.onopen = null;
      this.channel.onmessage = null;
      this.channel.onclose = null;
      this.channel.onerror = null;
      this.channel.close();
      this.channel = undefined;
    }
    if (this.peer) {
      this.peer.ontrack = null;
      this.peer.onconnectionstatechange = null;
      this.peer.close();
      this.peer = undefined;
    }
    if (this.audio) {
      this.audio.pause();
      (this.audio.srcObject as MediaStream | null)
        ?.getTracks()
        .forEach((track) => track.stop());
      this.audio.srcObject = null;
      this.audio = undefined;
    }
    this.stopping = (async () => {
      let saveError: VoxConversationError | undefined;
      // Media is already off. Keep RPC alive until native transcript-tail
      // persistence finishes; the stop acknowledgement alone is not a flush.
      await this.opening?.catch(() => undefined);
      if (
        this.threadId &&
        this.realtimeStarting &&
        this.rpc &&
        !this.rpc.isClosed
      ) {
        await this.realtimeStarting.catch(() => undefined);
        try {
          await this.rpc.request(
            "thread/realtime/stop",
            { threadId: this.threadId },
            5_000,
          );
          let timer: ReturnType<typeof setTimeout> | undefined;
          try {
            await Promise.race([
              this.closed,
              new Promise<never>((_, reject) => {
                timer = setTimeout(
                  () => reject(new Error("Vox transcript save timed out")),
                  5_000,
                );
              }),
            ]);
          } finally {
            clearTimeout(timer);
          }
        } catch {
          // Earlier native history remains intact; surface uncertainty rather
          // than claiming that the final speech was durably recorded.
          saveError = new VoxConversationError(
            "vox stopped, but saving the latest speech could not be confirmed. earlier conversation and memories are kept.",
          );
        }
      }
      this.rpc?.close();
      // harness_kill closes the server-side call too. Wait for spawn admission
      // before killing so Stop during process startup cannot leak a helper.
      if (this.spawn) {
        await this.spawn.catch(() => undefined);
        await killChild(this.id).catch(() => undefined);
      }
      this.releaseBridge?.();
      this.releaseBridge = undefined;
      if (active === this) active = undefined;
      if (saveError) throw saveError;
    })();
    this.options.onStopped?.(this.stopping);
    return this.stopping;
  }
}

if (import.meta.hot) {
  import.meta.hot.dispose(() => {
    void active?.stop().catch(() => undefined);
  });
}

function voxError(error: unknown): string {
  if (String(error).includes("Vox is already active in another window"))
    return "vox is already active in another window. end that call first.";
  if (
    error instanceof VoxMicrophoneError ||
    error instanceof VoxConversationError
  )
    return error.message;
  if (
    error instanceof DOMException &&
    ["NotAllowedError", "NotFoundError", "NotReadableError"].includes(
      error.name,
    )
  ) {
    return "Microphone unavailable. Allow microphone access in System Settings, check the input device, and try again.";
  }
  return "Could not start Vox. Check microphone access and your default Codex ChatGPT sign-in, update Codex, and try again.";
}
