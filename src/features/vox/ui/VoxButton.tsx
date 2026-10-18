import { useVox, type VoxControls } from "./VoxProvider";
import { useId, useRef, useState, useSyncExternalStore } from "react";
import { loadVoxEnabled, subscribeVoxEnabled } from "../model/availability";
import { voxFace } from "../model/expressions";
import { Popover } from "../../../shared/ui/Popover";
import { VoxFace } from "./VoxFace";

export function VoxButton() {
  const vox = useVox();
  const enabled = useSyncExternalStore(subscribeVoxEnabled, loadVoxEnabled);
  if (!enabled || !vox) return null;
  return <VoxTitleControl vox={vox} />;
}

function VoxTitleControl({ vox }: { vox: VoxControls }) {
  const button = useRef<HTMLButtonElement>(null);
  const id = useId();
  const [dismissed, setDismissed] = useState<string | null>(null);
  const snapshot = useSyncExternalStore(
    vox.display.subscribe,
    vox.display.getSnapshot,
  );
  const active = vox.state !== "idle";
  const reply = snapshot.lines[0];
  const message = vox.error
    ? { id: `error:${vox.error}`, text: vox.error, truncated: false }
    : vox.silent && active
      ? reply
      : undefined;
  const bubble = message && message.id !== dismissed;
  const face = voxFace({ ...vox, ...snapshot, micEnabled: true });
  const motion = !active
    ? "off"
    : vox.state === "connecting"
      ? "connecting"
      : snapshot.activity === "replying"
        ? "talking"
        : snapshot.activity === "thinking"
          ? "thinking"
          : "idle";
  const status = vox.error
    ? "connection lost"
    : vox.state === "connecting"
      ? "connecting…"
      : !active
        ? "off"
        : snapshot.activity;
  return (
    <div
      className="relative flex shrink-0 items-center"
      data-tauri-drag-region="false"
    >
      <button
        type="button"
        ref={button}
        aria-label="vox"
        aria-describedby={id}
        aria-pressed={active}
        title={
          vox.error
            ? "vox · connection lost · click to retry"
            : vox.state === "idle"
              ? "start vox"
              : "end vox"
        }
        onClick={() => {
          if (active) vox.stop();
          else {
            setDismissed(null);
            vox.start();
          }
        }}
        className={`grid h-7 min-w-14 place-items-center rounded-md px-2.5 font-mono text-[18px] font-medium tracking-[-0.03em] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-content/40 ${active ? "bg-selection-emphasis text-content" : "bg-content/5 text-content/75 hover:bg-selection-hover hover:text-content"}`}
      >
        <VoxFace id={face} motion={motion} />
        <span id={id} role="status" className="sr-only">
          {status}
        </span>
      </button>
      {bubble ? (
        <Popover
          anchor={button}
          side="bottom"
          align="end"
          gap={8}
          width={280}
          maxHeight={180}
          autoFocus={false}
          onDismiss={() => setDismissed(message.id)}
          role={vox.error ? "alert" : "status"}
          aria-label={vox.error ? "vox notice" : "vox reply"}
          aria-live="polite"
          tabIndex={0}
          className="select-text overflow-y-auto overscroll-contain whitespace-pre-wrap break-words px-3 py-2.5 text-[12px] leading-relaxed text-content lowercase focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-content/40"
        >
          {message.text}
          {message.truncated ? "…" : ""}
        </Popover>
      ) : null}
    </div>
  );
}
