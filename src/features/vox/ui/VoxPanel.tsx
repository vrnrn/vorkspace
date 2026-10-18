import {
  useEffect,
  useLayoutEffect,
  useRef,
  useSyncExternalStore,
} from "react";
import { Mic, MicOff, Square, VolumeX, X } from "../../../shared/ui/icons";
import { voxFace, VOX_FACES, type VoxFaceId } from "../model/expressions";
import type { VoxControls } from "./VoxProvider";

function Face({ id }: { id: VoxFaceId }) {
  const field = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    if (
      id === "error" ||
      window.matchMedia("(prefers-reduced-motion: reduce)").matches
    )
      return;
    const animation = field.current?.animate?.(
      [{ transform: "scaleY(.86)" }, { transform: "none" }],
      { duration: 150, easing: "cubic-bezier(.16,1,.3,1)" },
    );
    return () => animation?.cancel();
  }, [id]);
  return (
    <span
      ref={field}
      role="img"
      aria-label={VOX_FACES[id].emotion}
      data-vox-face={id}
      className="inline-block w-[3ch] select-none text-center font-mono text-[76px] leading-none tracking-[-0.04em] text-content/85"
      style={{ fontVariantLigatures: "none" }}
    >
      {VOX_FACES[id].face}
    </span>
  );
}

export function VoxPanel({
  vox,
  onClose,
}: {
  vox: VoxControls;
  onClose: () => void;
}) {
  const snapshot = useSyncExternalStore(
    vox.display.subscribe,
    vox.display.getSnapshot,
  );
  const tape = useRef<HTMLDivElement>(null);
  const follow = useRef(true);
  useLayoutEffect(() => {
    if (follow.current && tape.current)
      tape.current.scrollTop = tape.current.scrollHeight;
  }, [snapshot.lines]);
  const id = voxFace({ ...vox, ...snapshot });
  const label = vox.error
    ? "connection lost"
    : vox.state === "idle"
      ? "ready when you are"
      : vox.state === "connecting"
        ? "connecting…"
        : !vox.micEnabled
          ? "mic off"
          : snapshot.activity;
  const lastFinal = snapshot.lines.reduce<
    (typeof snapshot.lines)[number] | undefined
  >((last, line) => (line.done ? line : last), undefined);
  const control =
    "inline-flex h-8 items-center justify-center gap-1.5 rounded-md px-2 text-[12px] text-content/75 hover:bg-selection-hover hover:text-content focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-content/40 disabled:opacity-40";
  return (
    <div className="flex aspect-square min-h-0 flex-col gap-3 p-4 text-[12px] text-content">
      <div className="flex h-5 shrink-0 items-center justify-between">
        <span className="font-medium">vox</span>
        <button
          type="button"
          aria-label="close vox panel"
          onClick={onClose}
          className="grid size-6 place-items-center rounded text-content/60 hover:bg-selection-hover hover:text-content focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-content/40"
        >
          <X className="size-3.5" />
        </button>
      </div>
      <div className="grid min-h-[98px] shrink-0 content-center justify-items-center gap-2">
        <Face id={id} />
        <span role="status" className="text-content/65">
          {label}
          {vox.silent && vox.state !== "idle" ? " · silent replies" : ""}
        </span>
      </div>
      <div
        ref={tape}
        aria-label="vox replies"
        onScroll={() => {
          const el = tape.current;
          if (el)
            follow.current =
              el.scrollHeight - el.scrollTop - el.clientHeight < 24;
        }}
        className="min-h-0 flex-1 space-y-3 overflow-y-auto overscroll-contain pr-1 leading-relaxed lowercase select-text"
      >
        {vox.error ? (
          <p role="alert">{vox.error}</p>
        ) : snapshot.lines.some((line) => line.text) ? (
          snapshot.lines
            .filter((line) => line.text)
            .map((line) => (
              <div
                key={line.id}
                className="grid grid-cols-[26px_minmax(0,1fr)] items-baseline gap-2"
              >
                <span className="text-content/60">vox</span>
                <p className="whitespace-pre-wrap break-words text-content">
                  {line.text}
                  {line.truncated ? "…" : ""}
                </p>
              </div>
            ))
        ) : (
          <p className="text-content/60">vox’s replies appear here.</p>
        )}
      </div>
      <span className="sr-only" aria-live="polite">
        {lastFinal ? `vox: ${lastFinal.text.toLowerCase()}` : ""}
      </span>
      <div className="flex shrink-0 items-center justify-between gap-1 border-t border-content/10 pt-2">
        <button
          type="button"
          onClick={vox.state === "idle" ? vox.start : vox.toggleMic}
          aria-pressed={vox.state !== "idle" && vox.micEnabled}
          className={`${control} ${vox.state !== "idle" && vox.micEnabled ? "bg-selection-emphasis text-content" : ""}`}
        >
          {vox.micEnabled ? (
            <Mic className="size-3.5" />
          ) : (
            <MicOff className="size-3.5" />
          )}
          {vox.state === "idle"
            ? "start"
            : vox.micEnabled
              ? "mic on"
              : "mic off"}
        </button>
        <button
          type="button"
          onClick={vox.toggleSilent}
          aria-pressed={vox.silent}
          title="mute vox’s speaker; leave your microphone unchanged"
          className={`${control} ${vox.silent ? "bg-selection-emphasis text-content" : ""}`}
        >
          <VolumeX className="size-3.5" />
          silent replies
        </button>
        <button
          type="button"
          onClick={vox.stop}
          disabled={vox.state === "idle"}
          className={control}
        >
          <Square className="size-3" />
          end
        </button>
      </div>
    </div>
  );
}
