import { useCallback, useEffect, useRef, useState } from "react";
import type { InboxItem } from "../model/githubTasks";
import { useInboxBots } from "../model/inboxBots";
import {
  ticketContextLink,
  type TicketChatTarget,
} from "../model/ticketContext";
import type { PickerBot } from "../../projects/ui/SearchableProjectPicker";
import { BotRailMascot } from "../../bots/ui/BotRailMascot";
import { ProjectMascot } from "../../projects/ui/ProjectMascot";
import { ProjectLogoIcon } from "../../projects/ui/ProjectLogoIcon";
import { sameProjectPath } from "../../projects/model/recents";
import { copyText } from "../../../platform/tauri/clipboard";
import { Check, ChevronDown, Copy, Search } from "../../../shared/ui/icons";
import { Popover } from "../../../shared/ui/Popover";

export type TicketProjectOption = {
  path: string;
  name: string;
  logoPath: string | null;
  mascotName: string | null;
  mascotColor: string;
};

export function TicketContextActions({
  item,
  cwd,
  projects,
  bots = [],
  busy = false,
  onStart,
}: {
  item: InboxItem;
  cwd: string;
  projects: readonly TicketProjectOption[];
  bots?: readonly PickerBot[];
  busy?: boolean;
  onStart?: (
    item: InboxItem,
    body?: string,
    botId?: string,
  ) => void | Promise<void>;
}) {
  const defaults = useInboxBots();
  const defaultBotId =
    item.provider === "jira" || item.provider === "gitlab"
      ? defaults[item.provider]
      : undefined;
  const defaultBot = bots.find((bot) => bot.id === defaultBotId);
  const [selection, setSelection] = useState<{
    url: string;
    target: TicketChatTarget;
  }>();
  const defaultTarget: TicketChatTarget = defaultBot
    ? { kind: "bot", id: defaultBot.id }
    : projects.length
      ? {
          kind: "project",
          path:
            projects.find((p) =>
              sameProjectPath(p.path, item.projectPath || cwd),
            )?.path || projects[0].path,
        }
      : bots.length
        ? { kind: "bot", id: bots[0].id }
        : { kind: "project", path: "" };
  const target = selection?.url === item.url ? selection.target : defaultTarget;
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const sendingRef = useRef(false);
  const [sending, setSending] = useState(false);
  const [copying, setCopying] = useState(false);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const anchor = useRef<HTMLButtonElement>(null);
  const focusSearch = useCallback((input: HTMLInputElement | null) => {
    // Popover measures its first render while hidden, then places it.
    if (input) requestAnimationFrame(() => {
      if (input.isConnected) input.focus({ preventScroll: true });
    });
  }, []);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  const project =
    target.kind === "project"
      ? projects.find((p) => sameProjectPath(p.path, target.path))
      : undefined;
  const bot =
    target.kind === "bot" ? bots.find((m) => m.id === target.id) : undefined;
  const valid = target.kind === "bot" ? !!bot : !!project;
  const select = (next: TicketChatTarget) => {
    setSelection({ url: item.url, target: next });
    setOpen(false);
    setQuery("");
    anchor.current?.focus();
  };
  const projectMark = (p: TicketProjectOption) =>
    p.logoPath ? (
      <ProjectLogoIcon path={p.logoPath} className="size-3.5 shrink-0" />
    ) : (
      <ProjectMascot
        project={p.name}
        name={p.mascotName ?? undefined}
        color={p.mascotColor}
        className="size-3.5 shrink-0"
      />
    );
  const botMark = (m: PickerBot) => (
    <BotRailMascot
      name={m.mascot}
      color={m.color}
      status={m.status}
      className="size-3.5 shrink-0"
    />
  );
  const matches = (name: string) =>
    name.toLowerCase().includes(query.trim().toLowerCase());
  const filteredProjects = projects.filter((p) => matches(p.name));
  const filteredBots = bots.filter((m) => matches(m.name));
  return (
    <>
      {onStart ? (
        <>
          <button
            type="button"
            disabled={sending || busy || !valid}
            className="inline-flex h-7 items-center gap-1.5 rounded-md bg-content px-2.5 text-[12px] font-medium text-background-base hover:bg-content/80 disabled:cursor-default disabled:opacity-40"
            onClick={async () => {
              if (sendingRef.current || !valid) return;
              sendingRef.current = true;
              setSending(true);
              setError(null);
              try {
                await onStart(
                  target.kind === "project"
                    ? { ...item, projectPath: target.path }
                    : item,
                  undefined,
                  target.kind === "bot" ? target.id : undefined,
                );
              } catch (err) {
                setError(err instanceof Error ? err.message : String(err));
              } finally {
                sendingRef.current = false;
                setSending(false);
              }
            }}
          >
            {sending ? "Opening…" : "Send to chat"}
          </button>
          <button
            ref={anchor}
            type="button"
            aria-label="Choose chat destination"
            aria-haspopup="dialog"
            aria-expanded={open}
            disabled={sending || (!projects.length && !bots.length)}
            onClick={() => setOpen(!open)}
            className="inline-flex h-7 max-w-48 items-center gap-1.5 rounded-md border border-content/10 bg-content/5 px-2 text-[12px] text-content/80 hover:bg-content/10 hover:text-content disabled:opacity-40"
          >
            {bot ? botMark(bot) : project ? projectMark(project) : null}
            <span className="min-w-0 truncate">
              {bot?.name ?? project?.name ?? "Choose destination"}
            </span>
            <ChevronDown className="size-3 shrink-0" />
          </button>
          {open ? (
            <Popover
              anchor={anchor}
              width={280}
              onDismiss={() => setOpen(false)}
              role="dialog"
              aria-label="Chat destination"
              onKeyDown={(event) => {
                if (event.key !== "ArrowDown" && event.key !== "ArrowUp")
                  return;
                event.preventDefault();
                const rows = Array.from(
                  event.currentTarget.querySelectorAll<HTMLButtonElement>(
                    'button[role="option"]',
                  ),
                );
                const index = rows.indexOf(
                  document.activeElement as HTMLButtonElement,
                );
                rows[
                  (index + (event.key === "ArrowDown" ? 1 : -1) + rows.length) %
                    rows.length
                ]?.focus();
              }}
            >
              <div className="flex items-center gap-2 border-b border-content/10 px-3 py-2">
                <Search className="size-3.5 text-content/60" />
                <input
                  ref={focusSearch}
                  aria-label="Find project or Bot"
                  placeholder="Find project or Bot"
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                  className="min-w-0 flex-1 bg-transparent text-[12px] text-content outline-none placeholder:text-content/60"
                />
              </div>
              <div
                role="listbox"
                aria-label="Projects and Bots"
                className="max-h-64 overflow-y-auto p-1"
              >
                {filteredProjects.length ? (
                  <div role="group" aria-label="Projects">
                    <p className="px-2 py-1.5 text-[11px] font-medium text-content/60">
                      Projects
                    </p>
                    {filteredProjects.map((p) => (
                      <button
                        key={p.path}
                        type="button"
                        role="option"
                        aria-selected={
                          target.kind === "project" &&
                          sameProjectPath(target.path, p.path)
                        }
                        className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[12px] text-content/80 hover:bg-content/8 focus-visible:bg-selection focus-visible:outline-none"
                        onClick={() =>
                          select({ kind: "project", path: p.path })
                        }
                      >
                        {projectMark(p)}
                        <span className="min-w-0 flex-1 truncate">
                          {p.name}
                        </span>
                      </button>
                    ))}
                  </div>
                ) : null}
                {filteredBots.length ? (
                  <div role="group" aria-label="Bots">
                    <p className="px-2 py-1.5 text-[11px] font-medium text-content/60">
                      Bots
                    </p>
                    {filteredBots.map((m) => (
                      <button
                        key={m.id}
                        type="button"
                        role="option"
                        aria-selected={
                          target.kind === "bot" && target.id === m.id
                        }
                        className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[12px] text-content/80 hover:bg-content/8 focus-visible:bg-selection focus-visible:outline-none"
                        onClick={() => select({ kind: "bot", id: m.id })}
                      >
                        {botMark(m)}
                        <span className="min-w-0 flex-1 truncate">
                          {m.name}
                        </span>
                      </button>
                    ))}
                  </div>
                ) : null}
                {!filteredProjects.length && !filteredBots.length ? (
                  <p className="px-2 py-3 text-[12px] text-content/60">
                    No matching destinations
                  </p>
                ) : null}
              </div>
            </Popover>
          ) : null}
        </>
      ) : null}
      <button
        type="button"
        disabled={copying}
        title="Copy a vorkspace link that loads the latest ticket description and comments"
        className="inline-flex h-7 items-center gap-1.5 rounded-md border border-content/15 px-2.5 text-[12px] font-medium text-content/80 hover:bg-content/5 disabled:opacity-40"
        onClick={async () => {
          setCopying(true);
          setError(null);
          try {
            await copyText(ticketContextLink(item));
            setCopied(true);
            clearTimeout(timer.current);
            timer.current = setTimeout(() => setCopied(false), 2000);
          } catch (err) {
            setError(err instanceof Error ? err.message : String(err));
          } finally {
            setCopying(false);
          }
        }}
      >
        {copied ? (
          <Check className="size-3.5" />
        ) : (
          <Copy className="size-3.5" />
        )}
        {copied ? "Copied" : "Copy context"}
      </button>
      {error ? (
        <p role="alert" className="w-full text-[12px] text-red-400/90">
          {error}
        </p>
      ) : null}
    </>
  );
}
