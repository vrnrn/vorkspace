import { useState } from "react";
import { createRoot } from "react-dom/client";
import { configureChildBackend } from "../../src/integrations/harness/core/child";
import { Composer } from "../../src/features/sessions/ui/Composer";
import { MonoComposer } from "../../src/features/monos/ui/MonoComposer";
import {
  readComposer,
  writeComposer,
} from "../../src/features/sessions/model/composerControl";
import { VoxProvider } from "../../src/features/vox/ui/VoxProvider";
import { VoxButton } from "../../src/features/vox/ui/VoxButton";
import { saveVoxEnabled } from "../../src/features/vox/model/availability";
import { voxMemoryFixture } from "./voxMemoryFixture";
import { handleVoxMemory } from "../../src/features/vox/model/memory";
import { SettingsView } from "../../src/features/settings/ui/SettingsView";
import { TitleBar } from "../../src/app/shell/TitleBar";
import { applyThemePreference } from "../../src/features/settings/model/appearance";
import { setHarnessModels } from "../../src/features/sessions/model/models";
import "../../src/styles/index.css";

const metrics = {
  captures: 0,
  captureInputs: [] as unknown[],
  stopped: 0,
  closed: 0,
  spawned: 0,
  killed: 0,
  writes: [] as string[],
  actions: [] as Array<{ action: string; input: Record<string, unknown> }>,
  responses: [] as Array<{ id: string; result: unknown }>,
  submitted: 0,
  contextReads: 0,
};
// Voice interaction fixtures explicitly opt in; production defaults to off.
saveVoxEnabled(true);
const listeners = new Map<string, (event: { payload: unknown }) => void>();
let child = "";
function notify(method: string, params: Record<string, unknown>) {
  listeners.get("harness-stdout")?.({
    payload: {
      sessionId: child,
      line: JSON.stringify({
        method,
        params: { threadId: "preview-thread", ...params },
      }),
    },
  });
}
configureChildBackend({
  listen: async (event, handler) => {
    listeners.set(event, handler as (event: { payload: unknown }) => void);
    return () => {
      listeners.delete(event);
    };
  },
  invoke: async <T,>(command: string, args?: Record<string, unknown>) => {
    if (command === "harness_resolve_codex")
      return { path: "/fixture/codex" } as T;
    if (command === "harness_spawn") {
      metrics.spawned++;
      child = args!.sessionId as string;
      return 123 as T;
    }
    if (command === "harness_kill") {
      metrics.killed++;
      return undefined as T;
    }
    if (command === "harness_write") {
      const request = JSON.parse(args!.line as string);
      metrics.writes.push(request.method);
      if (request.result) metrics.responses.push(request);
      if (request.id !== undefined) {
        if (request.method === "thread/realtime/start")
          notify("thread/realtime/sdp", { sdp: "answer" });
        if (request.method === "thread/realtime/stop")
          notify("thread/realtime/closed", {});
        listeners.get("harness-stdout")?.({
          payload: {
            sessionId: child,
            line: JSON.stringify({
              id: request.id,
              result: ["thread/start", "thread/resume"].includes(request.method)
                ? { thread: { id: "preview-thread" } }
                : {},
            }),
          },
        });
      }
    }
    return undefined as T;
  },
});

let peer: MockPeer;
let inputTrack: { enabled: boolean } | undefined;
let outputAudio: HTMLAudioElement | undefined;
const NativeAudio = window.Audio;
window.Audio = function () {
  outputAudio = new NativeAudio();
  return outputAudio;
} as typeof Audio;
class MockPeer {
  connectionState = "new";
  localDescription = { sdp: "offer" };
  ontrack = null;
  onconnectionstatechange: (() => void) | null = null;
  constructor() {
    peer = this;
  }
  addTrack() {}
  channel = {
    readyState: "open",
    onopen: null,
    onclose: null,
    onerror: null,
    onmessage: null as ((event: { data: string }) => void) | null,
    close() {},
  };
  createDataChannel() {
    return this.channel;
  }
  async createOffer() {
    return { type: "offer", sdp: "offer" };
  }
  async setLocalDescription() {}
  async setRemoteDescription() {}
  close() {
    metrics.closed++;
  }
}
Object.defineProperty(navigator, "mediaDevices", {
  value: {
    enumerateDevices: async () => [
      {
        kind: "audioinput",
        deviceId: "macbook",
        label: "MacBook Pro Microphone",
      },
      { kind: "audioinput", deviceId: "airpods", label: "AirPods Pro" },
    ],
    getUserMedia: async (constraints: MediaStreamConstraints) => {
      metrics.captures++;
      metrics.captureInputs.push(constraints.audio);
      const track = {
        enabled: true,
        stop: () => {
          metrics.stopped++;
        },
        onended: null,
      };
      inputTrack = track;
      return { getTracks: () => [track], getAudioTracks: () => [track] };
    },
  },
});
Object.assign(window, {
  RTCPeerConnection: MockPeer,
  voxMetrics: metrics,
  voxNotify: notify,
  voxRealtime: (value: Record<string, unknown>) =>
    peer.channel.onmessage?.({ data: JSON.stringify(value) }),
  voxNativeState: () => ({
    muted: outputAudio?.muted,
    micEnabled: inputTrack?.enabled,
  }),
  connectVox: () => {
    peer.connectionState = "connected";
    peer.onconnectionstatechange?.();
  },
  failVox: () => notify("thread/realtime/error", {}),
  voxAction: (id: string, action: string, input: Record<string, unknown>) =>
    listeners.get("harness-stdout")?.({
      payload: {
        sessionId: child,
        line: JSON.stringify({
          id,
          method: "item/tool/call",
          params: {
            threadId: "preview-thread",
            callId: id,
            tool: "vorkspace",
            namespace: null,
            arguments: { action, input },
          },
        }),
      },
    }),
  __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} },
  __TAURI_INTERNALS__: {
    metadata: {
      currentWindow: { label: "main" },
      currentWebview: { label: "main" },
    },
    transformCallback: () => 0,
    invoke: async (command: string, args?: Record<string, unknown>) =>
      command.startsWith("mono_")
        ? voxMemoryFixture(command, args)
        : command === "plugin:dialog|message"
          ? ((args?.buttons as { OkCancelCustom?: string[] })
              ?.OkCancelCustom?.[0] ?? "Yes")
          : command === "composer_draft_get"
            ? null
            : [],
  },
});
applyThemePreference(
  new URLSearchParams(location.search).get("theme") === "light"
    ? "light"
    : "dark",
);
setHarnessModels("codex", [
  { id: "codex:gpt-6.1-sol", harness: "codex", name: "GPT-6.1 Sol" },
]);
const noop = () => {};
let composerId = "first";
function Fixture() {
  const [owner, setOwner] = useState("first");
  const [enabled, setEnabled] = useState(true);
  const [remote, setRemote] = useState(false);
  const [controlsKey, setControlsKey] = useState(0);
  const [showControls, setShowControls] = useState(true);
  const [mono, setMono] = useState(false);
  const [settings, setSettings] = useState(false);
  composerId = mono ? "mono-helper" : owner;
  if (settings)
    return (
      <div className="h-screen">
        <SettingsView
          titlebarControl={<VoxButton />}
          section="vox"
          cwd="/repo"
          sessions={[]}
          onClose={() => setSettings(false)}
          onSelectSection={noop}
          onOpenSession={noop}
          onArchiveSession={noop}
          onDeleteSession={noop}
          onOpenWhatsNew={noop}
        />
      </div>
    );
  return (
    <main className="flex min-h-screen flex-col gap-6 bg-background-base text-content">
      {showControls ? (
        <TitleBar
          key={controlsKey}
          tabs={[]}
          activeId=""
          cwd="/repo"
          onToggleSidebar={noop}
          onSelect={noop}
          onClose={noop}
          onCloseMany={noop}
          onReorder={noop}
          voxControl={<VoxButton />}
        />
      ) : (
        <div className="h-10 border-b border-stroke px-5 py-2 text-sm">
          Settings
        </div>
      )}
      <div className="flex flex-wrap gap-4 pl-10 pr-[254px] text-sm">
        <button
          onClick={() => {
            if (!peer) return;
            peer.connectionState = "connected";
            peer.onconnectionstatechange?.();
            notify("thread/realtime/transcript/delta", {
              role: "user",
              delta: "Put review the tests in Helper’s composer",
            });
            notify("thread/realtime/transcript/done", {
              role: "user",
              text: "Put review the tests in Helper’s composer.",
            });
            notify("thread/realtime/transcript/delta", {
              role: "assistant",
              delta: "draft ready in helper’s composer.",
            });
            notify("thread/realtime/transcript/done", {
              role: "assistant",
              text: "draft ready in helper’s composer. not sent.",
            });
          }}
        >
          Preview vox conversation
        </button>
        <button onClick={() => setOwner("next")}>Switch chat</button>
        <button onClick={() => setEnabled(false)}>Hide chat</button>
        <button onClick={() => setRemote(true)}>Remote chat</button>
        <button onClick={() => setControlsKey((key) => key + 1)}>
          Move controls
        </button>
        <button onClick={() => setShowControls((value) => !value)}>
          Toggle controls
        </button>
        <button onClick={() => setMono(true)}>Mono composer</button>
        <button onClick={() => setSettings(true)}>Vox settings</button>
      </div>
      {mono ? (
        <MonoComposer
          sessionId="mono-helper"
          name="Helper"
          persistDraft={false}
          enabled={enabled}
          onSubmit={() => {
            metrics.submitted++;
          }}
        />
      ) : (
        <Composer
          enabled={enabled}
          focused
          harness="codex"
          model="codex:gpt-6.1-sol"
          runtimeMode="supervised"
          cwd="/repo"
          executionCwd="/repo"
          sessionId={owner}
          persistDraft={false}
          remoteSession={remote}
          hideTopBar
          hideProjectPicker
          hideBranchPicker
          onFocus={noop}
          onCwdChange={noop}
          onModelChange={noop}
          onRuntimeModeChange={noop}
          onSubmit={() => {
            metrics.submitted++;
          }}
        />
      )}
    </main>
  );
}
createRoot(document.getElementById("root")!).render(
  <VoxProvider
    cwd="/repo"
    execute={async (request) => {
      const { action, input, signal } = request;
      if (action.startsWith("memory."))
        return handleVoxMemory(request, () => {
          if (!request.isCurrent()) throw new Error("stopped");
        });
      if (action === "workspace.current") {
        metrics.contextReads++;
        return { monos: [{ id: "helper", name: "Helper" }] };
      }
      metrics.actions.push({ action, input });
      const id =
        typeof input.sessionId === "string" ? input.sessionId : composerId;
      if (action === "composer.read") return readComposer(id, signal);
      if (action === "composer.write")
        return writeComposer(
          id,
          input as {
            text: string;
            mode?: "append" | "replace";
            expectedText?: string;
          },
          signal,
        );
      return { submitted: true, sessionId: input.sessionId };
    }}
  >
    <Fixture />
  </VoxProvider>,
);
