import type { ReactNode } from "react";
import claude from "../../../assets/providers/claude.svg";
import codex from "../../../assets/providers/codex.svg";
import omp from "../../../assets/providers/omp.svg";
import pi from "../../../assets/providers/pi.svg";
import antigravity from "../../../assets/providers/antigravity.svg";
import type { HarnessId } from "../model/session";

export const HARNESS_ICONS: Record<HarnessId, string> = {
  claude,
  codex,
  pi,
  omp,
  antigravity,
};

/** White marks that must follow `currentColor` so they stay visible in light mode. */
export const MONOCHROME_HARNESSES = new Set<HarnessId>(["pi"]);

function BotIcon({
  className,
  children,
  viewBox = "-4 -4 37 37",
}: {
  className: string;
  children: ReactNode;
  viewBox?: string;
}) {
  return (
    <svg
      viewBox={viewBox}
      fill="currentColor"
      aria-hidden
      className={`block ${className}`}
    >
      {children}
    </svg>
  );
}

export function HarnessIcon({
  harness,
  className = "size-3.5",
}: {
  harness: HarnessId;
  className?: string;
}) {
  if (harness === "pi") {
    return (
      <BotIcon className={className}>
        <path
          fillRule="evenodd"
          d="M1 1H22V14.4997H14.9998V21.2499H8.0002V28H1V1ZM8.0002 7.75014V14.4997H14.9998V7.75014H8.0002Z"
        />
        <path d="M22 15H28V28H22V15Z" />
      </BotIcon>
    );
  }
  return (
    <img
      src={HARNESS_ICONS[harness]}
      alt=""
      draggable={false}
      className={`block object-contain ${className}`}
    />
  );
}
