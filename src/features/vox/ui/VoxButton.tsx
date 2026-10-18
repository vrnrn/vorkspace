import { useId, useRef, useState } from "react";
import { Popover } from "../../../shared/ui/Popover";
import { useVox } from "./VoxProvider";
import { VoxPanel } from "./VoxPanel";

export function VoxButton() {
  const button = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const id = useId();
  const vox = useVox();
  if (!vox) return null;
  function close(focus = false) {
    setOpen(false);
    if (focus) button.current?.focus({ preventScroll: true });
  }
  return (
    <div
      className="relative flex shrink-0 items-center"
      data-tauri-drag-region="false"
    >
      <button
        ref={button}
        type="button"
        aria-label="vox"
        aria-expanded={open}
        aria-controls={open ? id : undefined}
        aria-pressed={vox.state !== "idle"}
        title={
          vox.error
            ? "vox · connection lost"
            : vox.state === "idle"
              ? "open vox"
              : "vox · microphone session active"
        }
        onClick={() => {
          if (open) close();
          else {
            setOpen(true);
            if (vox.state === "idle" && !vox.error) vox.start();
          }
        }}
        className={`h-7 rounded-md px-3 font-mono text-[15px] font-medium tracking-[-0.03em] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-content/40 ${vox.state === "idle" && !vox.error ? "bg-content/5 text-content/75 hover:bg-selection-hover hover:text-content" : "bg-selection-emphasis text-content"}`}
      >
        vox
      </button>
      {open ? (
        <Popover
          anchor={button}
          side="bottom"
          align="end"
          width={340}
          maxHeight={420}
          role="dialog"
          aria-label="vox"
          id={id}
          tabIndex={-1}
          autoFocus
          onDismiss={(reason) => close(reason === "escape")}
        >
          <VoxPanel vox={vox} onClose={() => close(true)} />
        </Popover>
      ) : null}
    </div>
  );
}
