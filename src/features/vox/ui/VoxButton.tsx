import { useRef } from "react";
import { Mic, Square } from "../../../shared/ui/icons";
import { Popover } from "../../../shared/ui/Popover";
import { useVox } from "./VoxProvider";

export function VoxButton() {
  const button = useRef<HTMLButtonElement>(null);
  const vox = useVox();
  if (!vox) return null;
  const { state, error } = vox;
  const label = state === "idle" ? "Start Vox" : "Stop Vox";
  return (
    <div
      className="relative flex shrink-0 items-center gap-1.5"
      data-tauri-drag-region="false"
    >
      {state !== "idle" ? (
        <span role="status" className="text-[11px] text-content/70">
          {state === "connecting" ? "Connecting…" : "Vox"}
        </span>
      ) : null}
      <button
        ref={button}
        type="button"
        aria-label={label}
        aria-pressed={state !== "idle"}
        title={`${label} · control chats and Monos by voice`}
        onClick={vox.toggle}
        className={`grid size-6.5 place-items-center rounded-md focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent ${state === "idle" ? "text-content/60 hover:bg-selection-hover hover:text-content" : "bg-selection-emphasis text-content"}`}
      >
        {state === "idle" ? (
          <Mic className="size-3.5" />
        ) : (
          <Square className="size-3 fill-current" />
        )}
      </button>
      {error ? (
        <Popover
          anchor={button}
          side="bottom"
          align="end"
          width={280}
          onDismiss={vox.dismiss}
          role="alert"
          className="p-3 text-[12px] text-content"
        >
          <p>{error}</p>
          <button
            type="button"
            onClick={vox.dismiss}
            className="mt-2 rounded px-1 text-content/70 hover:text-content focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent"
          >
            Dismiss
          </button>
        </Popover>
      ) : null}
    </div>
  );
}
