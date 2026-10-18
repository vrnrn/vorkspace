import { VOX_FACES, type VoxFaceId } from "../model/expressions";
import "./vox.css";

/** Glyph frames stay in CSS: no animation clock, audio samples, or React ticks. */
export function VoxFace({
  id,
  motion,
}: {
  id: VoxFaceId;
  motion: "off" | "idle" | "thinking" | "talking" | "connecting";
}) {
  const frames: VoxFaceId[] =
    motion === "talking"
      ? [id, "focused", "happy", "delighted"]
      : motion === "thinking" || motion === "connecting"
        ? [id, "focused", "curious", "blink"]
        : motion === "idle"
          ? [id, "blink", "wink_right"]
          : ["neutral"];
  return (
    <span
      aria-hidden="true"
      data-vox-face={motion === "off" ? "neutral" : id}
      data-vox-motion={motion}
      className={`vox-face vox-face--${motion}`}
    >
      {frames.map((frame, index) => (
        <span key={index} className={`vox-face-frame vox-face-frame-${index}`}>
          {VOX_FACES[frame].face}
        </span>
      ))}
    </span>
  );
}
