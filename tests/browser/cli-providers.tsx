import { createRoot } from "react-dom/client";
import { voxMemoryFixture } from "./voxMemoryFixture";
import "../../src/styles/index.css";

const providers = ["claude", "codex", "pi", "omp", "antigravity"] as const;
const voxSettings =
  new URLSearchParams(location.search).get("section") === "vox";
if (voxSettings)
  Object.defineProperty(navigator, "mediaDevices", {
    configurable: true,
    value: {
      enumerateDevices: async () => [
        {
          kind: "audioinput",
          deviceId: "default",
          label: "Default - AirPods Pro",
        },
        {
          kind: "audioinput",
          deviceId: "communications",
          label: "AirPods Pro",
        },
        {
          kind: "audioinput",
          deviceId: "macbook",
          label: "MacBook Pro Microphone",
        },
        { kind: "audioinput", deviceId: "airpods", label: "AirPods Pro" },
      ],
      getUserMedia: async () => {
        throw new Error("settings must not capture audio");
      },
    },
  });
Object.assign(window, {
  __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} },
  __TAURI_INTERNALS__: {
    metadata: {
      currentWindow: { label: "main" },
      currentWebview: { label: "main" },
    },
    transformCallback: () => 1,
    unregisterCallback: () => {},
    invoke: async (command: string, args?: Record<string, unknown>) => {
      if (command.startsWith("mono_")) return voxMemoryFixture(command, args);
      if (command === "plugin:dialog|message")
        return (window as unknown as { voxConfirm?: boolean }).voxConfirm ===
          false
          ? "Cancel"
          : ((args?.buttons as { OkCancelCustom?: string[] })
              ?.OkCancelCustom?.[0] ?? "Yes");
      if (command.startsWith("harness_resolve_"))
        return { path: `/dummy/${command.slice("harness_resolve_".length)}` };
      if (command === "harness_exec")
        return `${args?.binaryProvider === "codex" ? "codex-cli" : args?.binaryProvider} 1.2.3`;
      if (command === "fetch_claude_usage")
        return { status: "unavailable", error: "Sign in to connect" };
      if (command === "provider_account_identity") return null;
      return undefined;
    },
  },
});
const { applyThemePreference } =
  await import("../../src/features/settings/model/appearance");
applyThemePreference(
  new URLSearchParams(location.search).get("theme") === "light"
    ? "light"
    : "dark",
);
const { registerBuiltinHarnesses } =
  await import("../../src/integrations/harness/core/register");
registerBuiltinHarnesses();
const { setHarnessModels } =
  await import("../../src/features/sessions/model/models");
for (const harness of providers)
  setHarnessModels(
    harness,
    harness === "codex"
      ? [
          { id: "codex:gpt-6.1-sol", harness, name: "GPT-6.1 Sol" },
          { id: "codex:gpt-5.6-luna", harness, name: "GPT-5.6 Luna" },
          { id: "codex:gpt-6-astra", harness, name: "GPT-6 Astra" },
        ]
      : [{ id: `${harness}:demo`, harness, name: "Demo model" }],
  );
const { setHarnessAvailability, markHarnessAvailabilityProbed } =
  await import("../../src/integrations/harness/core/availabilityState");
setHarnessAvailability({
  claude: true,
  codex: true,
  pi: true,
  omp: true,
  antigravity: true,
});
markHarnessAvailabilityProbed();
const { useState } = await import("react");
const { ModelPicker } =
  await import("../../src/features/sessions/ui/ModelPicker");
function PickerPreview() {
  const [model, setModel] = useState("codex:gpt-5.6-luna");
  return (
    <div className="flex h-screen items-center justify-center bg-background-base text-content">
      <ModelPicker
        harness="codex"
        model={model}
        values={{}}
        hideSettings
        project={
          new URLSearchParams(location.search).get("project") ?? undefined
        }
        side="bottom"
        onChange={(_, model) => setModel(model)}
        onSettingsChange={() => {}}
      />
    </div>
  );
}
const { SettingsView } =
  await import("../../src/features/settings/ui/SettingsView");
createRoot(document.getElementById("root")!).render(
  new URLSearchParams(location.search).get("view") === "picker" ? (
    <PickerPreview />
  ) : (
    <div className="h-screen">
      <SettingsView
        section={
          voxSettings
            ? "vox"
            : new URLSearchParams(location.search).get("section") === "general"
              ? "general"
              : "providers"
        }
        cwd="/repo"
        sessions={[]}
        onClose={() => {}}
        onSelectSection={() => {}}
        onOpenSession={() => {}}
        onArchiveSession={() => {}}
        onDeleteSession={() => {}}
        onOpenWhatsNew={() => {}}
      />
    </div>
  ),
);
