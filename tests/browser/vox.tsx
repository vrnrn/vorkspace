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
import { TitleBar } from "../../src/app/shell/TitleBar";
import { applyThemePreference } from "../../src/features/settings/model/appearance";
import { setHarnessModels } from "../../src/features/sessions/model/models";
import "../../src/styles/index.css";

const metrics = {
  captures: 0,
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
        listeners.get("harness-stdout")?.({
          payload: {
            sessionId: child,
            line: JSON.stringify({
              id: request.id,
              result:
                request.method === "thread/start"
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
class MockPeer {
  connectionState = "new";
  localDescription = { sdp: "offer" };
  ontrack = null;
  onconnectionstatechange: (() => void) | null = null;
  constructor() {
    peer = this;
  }
  addTrack() {}
  createDataChannel() {
    return {
      readyState: "open",
      onopen: null,
      onclose: null,
      onerror: null,
      close() {},
    };
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
    getUserMedia: async () => {
      metrics.captures++;
      const track = {
        stop: () => {
          metrics.stopped++;
        },
        onended: null,
      };
      return { getTracks: () => [track], getAudioTracks: () => [track] };
    },
  },
});
Object.assign(window, {
  RTCPeerConnection: MockPeer,
  voxMetrics: metrics,
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
    invoke: async (command: string) =>
      command === "composer_draft_get" ? null : [],
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
  const [mono, setMono] = useState(false);
  composerId = mono ? "mono-helper" : owner;
  return (
    <main className="flex min-h-screen flex-col justify-center gap-6 bg-background-base p-10 text-content">
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
      <div className="flex gap-4 text-sm">
        <button onClick={() => setOwner("next")}>Switch chat</button>
        <button onClick={() => setEnabled(false)}>Hide chat</button>
        <button onClick={() => setRemote(true)}>Remote chat</button>
        <button onClick={() => setControlsKey((key) => key + 1)}>
          Move controls
        </button>
        <button onClick={() => setMono(true)}>Mono composer</button>
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
    execute={async ({ action, input, signal }) => {
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
