import {
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { ImagePlus, MoreHorizontal, Plus, Trash2 } from "../../shared/ui/icons";
import { Popover, type PopoverAnchor } from "../../shared/ui/Popover";
import { Shimmer } from "../../shared/ui/Shimmer";
import { useAnimatedReorder } from "../../shared/hooks/useAnimatedReorder";
import { MonoRailMascot } from "../../features/monos/ui/MonoRailMascot";
import { MonoIntroCard } from "../../features/monos/ui/MonoIntro";
import { RailAction } from "./RailAction";
import { saveMonosEnabled } from "../../features/settings/model/settings";
import { ProjectBackgroundDialog } from "../../features/projects/ui/ProjectBackgroundDialog";
import { monoBackgroundKey } from "../../features/monos/model/monoBackground";
import {
  ColorPicker,
  MascotPicker,
} from "../../features/monos/ui/monoPanelParts";
import {
  defaultMonoName,
  findMono,
  listMonos,
  MONO_STATUS_LABEL,
  monoLook,
  monoProjectsPhrase,
  monosSnapshot,
  monoWorksOn,
  reorderMonos,
  saveMonoMascot,
  saveMonoName,
  subscribeMonos,
  updateMono,
  type MonoState,
} from "../../features/monos/model/mono";
import {
  createMonoChannel,
  listMonoChannels,
  monoChannelsSnapshot,
  removeMonoChannel,
  subscribeMonoChannels,
} from "../../features/monos/model/monoChannels";

export type MonoRailProps = {
  /** The Mono open in the main area, if any. */
  activeId?: string;
  /** The multi-Mono channel open in the main area, if any. */
  activeChannelId?: string;
  /** Channels where a Mono is currently speaking or queued. */
  busyChannelIds?: ReadonlySet<string>;
  /** What each Mono is doing, by Mono id; missing reads as idle. */
  states: ReadonlyMap<string, MonoState>;
  /** Replies each Mono has that the user has not looked at yet, by Mono id. */
  unseenCounts?: ReadonlyMap<string, number>;
  onOpen: (monoId: string) => void;
  onOpenChannel?: (channelId: string) => void;
  /** Creates a Mono, given these projects to start with. */
  onCreate: (projects?: readonly string[]) => void;
  onDelete: (monoId: string) => void;
  onDeleteChannel?: (channelId: string) => void;
};

const IDLE: MonoState = { status: "idle" };

/** A Mono's unread count as its badge shows it. */
export function unreadBadge(count: number): string {
  return count > 99 ? "99+" : String(count);
}

export function unreadLabel(count: number): string {
  return count === 1 ? "1 new reply" : `${count} new replies`;
}

/**
 * The user's Monos, above the projects. Each opens like a project does, but
 * into its conversation; none belongs to a project, so they get a section of
 * their own and the shaded mascot rather than a project's flat one.
 */
export function MonoRailSection({
  activeId,
  activeChannelId,
  busyChannelIds,
  states,
  unseenCounts,
  onOpen,
  onOpenChannel,
  onCreate,
  onDelete,
  onDeleteChannel,
  project,
}: MonoRailProps & {
  /**
   * Lists only the Monos working on this project, as the inset sidebar does;
   * a Mono on several projects shows in each of them.
   */
  project?: string;
}) {
  const snapshot = useSyncExternalStore(subscribeMonos, monosSnapshot);
  const channelsSnap = useSyncExternalStore(
    subscribeMonoChannels,
    monoChannelsSnapshot,
  );
  const channels = useMemo(() => listMonoChannels(), [channelsSnap]);
  const monos = useMemo(
    () => listMonos().filter((mono) => !project || monoWorksOn(mono, project)),
    [snapshot, project],
  );
  const ids = monos.map((mono) => mono.id);
  const sortable = useAnimatedReorder(ids, reorderMonos, "y");
  const [menu, setMenu] = useState<{ id: string; anchor: PopoverAnchor }>();
  const [background, setBackground] = useState<string>();
  const [newChannelAnchor, setNewChannelAnchor] =
    useState<PopoverAnchor | null>(null);
  const [newChannelName, setNewChannelName] = useState("");
  const [newChannelTopic, setNewChannelTopic] = useState("");
  const [newChannelMembers, setNewChannelMembers] = useState<string[]>([]);
  const backgroundMono = background ? findMono(background) : undefined;

  // With no Mono yet, the card stands in for the rail's section. A project
  // with none of its own offers one under the label instead.
  if (monos.length === 0 && !project)
    return (
      <div className="mb-2 shrink-0" data-mono-rail>
        <MonoIntroCard
          onCreate={() => onCreate()}
          onHide={() => saveMonosEnabled(false)}
        />
      </div>
    );

  const openCreateChannel = (anchor: PopoverAnchor) => {
    setNewChannelName("");
    setNewChannelTopic("");
    setNewChannelMembers(monos.map((m) => m.id));
    setNewChannelAnchor(anchor);
  };

  const submitNewChannel = () => {
    const name = newChannelName.trim() || "general";
    const memberIds =
      newChannelMembers.length > 0
        ? newChannelMembers
        : monos.map((m) => m.id);
    const created = createMonoChannel({
      name,
      topic: newChannelTopic.trim() || undefined,
      memberIds,
    });
    setNewChannelAnchor(null);
    onOpenChannel?.(created.id);
  };

  return (
    <div
      className={`shrink-0 ${project ? "px-2 pb-3" : "mb-2"}`}
      data-mono-rail
      data-mono-project={project}
    >
      <div
        className={
          // The inset sidebar's section labels.
          project
            ? "flex h-7 items-center gap-1 px-2 text-[12px] font-medium text-content/50"
            : "flex items-center gap-1 px-3 pb-1.5 pt-1"
        }
      >
        <span
          className={`min-w-0 flex-1 truncate${
            project ? "" : " px-1 text-xs leading-5 text-content/50"
          }`}
        >
          Monos
        </span>
        {monos.length ? (
          <button
            type="button"
            title="New mono"
            aria-label="New mono"
            onClick={() => onCreate(project ? [project] : undefined)}
            className={`grid size-5 shrink-0 place-items-center rounded-md text-content/50 hover:bg-content/8 hover:text-content${
              project ? " -mr-0.5" : ""
            }`}
          >
            <Plus className="size-3.5" strokeWidth={1.75} />
          </button>
        ) : null}
      </div>
      <div className={`flex flex-col gap-px${project ? "" : " px-2"}`}>
        {monos.length === 0 ? (
          <RailAction
            label="New mono"
            icon={Plus}
            onClick={() => onCreate(project ? [project] : undefined)}
          />
        ) : null}
        {monos.map((mono) => {
          const look = monoLook(mono);
          const state = states.get(mono.id) ?? IDLE;
          const selected = mono.id === activeId && !activeChannelId;
          const unseen = selected ? 0 : (unseenCounts?.get(mono.id) ?? 0);
          const projects = look.projects.length
            ? monoProjectsPhrase(look.projects)
            : "No projects yet";
          const status =
            state.status === "idle"
              ? undefined
              : (state.activity ?? MONO_STATUS_LABEL[state.status]);
          return (
            <div
              key={mono.id}
              ref={(el) => sortable.setItemRef(mono.id, el)}
              data-selected={selected || undefined}
              data-mono-status={state.status}
              className={`reorder-item project-reorder-item group relative flex h-8 cursor-default touch-none items-stretch rounded-md px-2 ${
                selected ? "bg-selection-strong text-content" : "opacity-65"
              }`}
              onPointerDown={(event) => {
                if (event.button !== 0) return;
                if (
                  (event.target as HTMLElement | null)?.closest(
                    "[data-no-drag]",
                  )
                )
                  return;
                sortable.onItemPointerDown(mono.id, event);
              }}
              onClick={(event) => {
                if (
                  (event.target as HTMLElement | null)?.closest(
                    "[data-no-drag]",
                  )
                )
                  return;
                if (sortable.consumeClick()) return;
                onOpen(mono.id);
              }}
              onContextMenu={(event) => {
                event.preventDefault();
                setMenu({
                  id: mono.id,
                  anchor: { x: event.clientX, y: event.clientY },
                });
              }}
            >
              <button
                type="button"
                title={[look.name, projects, status].filter(Boolean).join("\n")}
                aria-label={[
                  look.name,
                  status ?? "idle",
                  unseen ? unreadLabel(unseen) : undefined,
                  projects,
                ]
                  .filter(Boolean)
                  .join(", ")}
                aria-current={selected ? "true" : undefined}
                className="flex min-w-0 flex-1 cursor-default items-center gap-2 text-left transition-[padding] duration-150 motion-reduce:transition-none group-hover:pr-6 group-has-[button:focus-visible]:pr-6"
              >
                <MonoRailMascot
                  name={look.mascot}
                  color={look.color}
                  status={state.status}
                />
                {state.status === "working" ? (
                  <Shimmer
                    as="span"
                    duration={1.4}
                    className="min-w-0 flex-1 truncate text-sm font-medium leading-tight"
                  >
                    {look.name}
                  </Shimmer>
                ) : (
                  <span className="min-w-0 flex-1 truncate text-sm font-medium leading-tight">
                    {look.name}
                  </span>
                )}
                {unseen ? (
                  <span
                    aria-hidden
                    data-mono-unread={unseen}
                    className="grid h-4 min-w-4 shrink-0 place-items-center rounded-full bg-accent px-1 text-[10px] font-medium tabular-nums leading-none text-white group-hover:hidden group-has-[button:focus-visible]:hidden"
                  >
                    {unreadBadge(unseen)}
                  </span>
                ) : null}
              </button>
              <button
                type="button"
                data-no-drag
                title="Mono options"
                aria-label={`${look.name} options`}
                aria-haspopup="menu"
                onPointerDown={(event) => event.stopPropagation()}
                onClick={(event) => {
                  event.stopPropagation();
                  setMenu({ id: mono.id, anchor: event.currentTarget });
                }}
                className="absolute right-1 top-1/2 hidden size-6 -translate-y-1/2 place-items-center rounded-md text-content/55 hover:bg-content/8 hover:text-content group-hover:grid group-has-[button:focus-visible]:grid"
              >
                <MoreHorizontal className="size-4" strokeWidth={1.75} />
              </button>
            </div>
          );
        })}
      </div>

      {/* Multi-Mono Channels Subsection (below Monos, above Projects) */}
      <div className="mt-2.5" data-mono-channels-rail>
        <div className="flex items-center gap-1 px-3 pb-1.5 pt-1">
          <span className="min-w-0 flex-1 truncate px-1 text-xs leading-5 text-content/50">
            Channels
          </span>
          <button
            type="button"
            title="New channel"
            aria-label="New channel"
            onClick={(event) => openCreateChannel(event.currentTarget)}
            className="grid size-5 shrink-0 place-items-center rounded-md text-content/50 hover:bg-content/8 hover:text-content"
          >
            <Plus className="size-3.5" strokeWidth={1.75} />
          </button>
        </div>
        <div className="flex flex-col gap-px px-2">
          {channels.length > 0 ? (
            channels.map((channel) => {
              const selected = channel.id === activeChannelId;
              const busy = busyChannelIds?.has(channel.id) ?? false;
              const memberLooks = channel.memberIds
                .map((id) => findMono(id))
                .filter((m): m is NonNullable<typeof m> => Boolean(m))
                .map(monoLook);
              const msgCount = channel.messages.length;
              return (
                <div
                  key={channel.id}
                  data-selected={selected || undefined}
                  data-channel-id={channel.id}
                  onClick={() => onOpenChannel?.(channel.id)}
                  className={`group relative flex h-8 cursor-default items-stretch rounded-md px-2 ${
                    selected
                      ? "bg-selection-strong text-content"
                      : "opacity-65 hover:bg-content/[0.04] hover:opacity-100"
                  }`}
                >
                  <button
                    type="button"
                    title={[
                      `#${channel.name}`,
                      channel.topic,
                      memberLooks.length
                        ? `Members: ${memberLooks.map((m) => m.name).join(", ")}`
                        : undefined,
                    ]
                      .filter(Boolean)
                      .join("\n")}
                    aria-current={selected ? "true" : undefined}
                    className="flex min-w-0 flex-1 cursor-default items-center gap-2 text-left transition-[padding] duration-150 motion-reduce:transition-none group-hover:pr-6"
                  >
                    <span
                      className={`grid size-4 shrink-0 place-items-center rounded text-[12px] font-bold leading-none ${
                        busy || selected
                          ? "bg-accent/15 text-accent"
                          : "text-content/45"
                      }`}
                    >
                      #
                    </span>
                    {busy ? (
                      <Shimmer
                        as="span"
                        duration={1.4}
                        className="min-w-0 flex-1 truncate text-sm font-medium leading-tight"
                      >
                        {channel.name}
                      </Shimmer>
                    ) : (
                      <span className="min-w-0 flex-1 truncate text-sm font-medium leading-tight">
                        {channel.name}
                      </span>
                    )}
                    <span className="flex shrink-0 items-center gap-1.5 group-hover:hidden">
                      <span className="flex items-center -space-x-1">
                        {memberLooks.slice(0, 3).map((look, idx) => (
                          <span
                            key={idx}
                            className="size-2 rounded-full ring-1 ring-background-base"
                            style={{ backgroundColor: look.color }}
                          />
                        ))}
                      </span>
                      {msgCount > 0 ? (
                        <span className="rounded-full bg-content/[0.07] px-1.5 py-0.5 text-[10px] font-medium tabular-nums leading-none text-content/55">
                          {msgCount > 99 ? "99+" : msgCount}
                        </span>
                      ) : null}
                    </span>
                  </button>
                  <button
                    type="button"
                    title={`Delete #${channel.name}`}
                    aria-label={`Delete #${channel.name}`}
                    onClick={(event) => {
                      event.stopPropagation();
                      if (onDeleteChannel) onDeleteChannel(channel.id);
                      else removeMonoChannel(channel.id);
                    }}
                    className="absolute right-1 top-1/2 hidden size-6 -translate-y-1/2 place-items-center rounded-md text-content/45 hover:bg-content/8 hover:text-red-400 group-hover:grid"
                  >
                    <Trash2 className="size-3.5" strokeWidth={1.75} />
                  </button>
                </div>
              );
            })
          ) : (
            <button
              type="button"
              onClick={(event) => openCreateChannel(event.currentTarget)}
              className="flex h-7 items-center gap-2 rounded-md px-2 text-left text-xs text-content/45 transition-colors hover:bg-content/[0.04] hover:text-content/75"
            >
              <span className="grid size-4 shrink-0 place-items-center text-[12px] font-semibold text-content/35">
                #
              </span>
              <span className="truncate">Add a channel...</span>
            </button>
          )}
        </div>
      </div>

      {newChannelAnchor ? (
        <Popover
          anchor={newChannelAnchor}
          side="right"
          align="start"
          gap={4}
          width={260}
          onDismiss={() => setNewChannelAnchor(null)}
          aria-label="New Mono channel"
          className="p-3"
        >
          <div className="mb-2 text-xs font-semibold text-content">
            New Mono Channel
          </div>
          <input
            type="text"
            autoFocus
            value={newChannelName}
            placeholder="channel-name (e.g. backend-sync)"
            onChange={(event) => setNewChannelName(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                submitNewChannel();
              }
            }}
            className="mb-2 w-full rounded-lg border border-content/10 bg-content/5 px-2.5 py-1.5 text-[12.5px] text-content outline-none ring-accent/40 placeholder:text-content/40 focus:ring-1"
          />
          <input
            type="text"
            value={newChannelTopic}
            placeholder="Optional topic or goal"
            onChange={(event) => setNewChannelTopic(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                submitNewChannel();
              }
            }}
            className="mb-2.5 w-full rounded-lg border border-content/10 bg-content/5 px-2.5 py-1.5 text-[12px] text-content outline-none ring-accent/40 placeholder:text-content/40 focus:ring-1"
          />
          <div className="mb-1.5 text-[11px] font-medium text-content/55">
            Members
          </div>
          <div className="mb-3 flex max-h-36 flex-col gap-1 overflow-y-auto">
            {monos.map((mono) => {
              const look = monoLook(mono);
              const checked = newChannelMembers.includes(mono.id);
              return (
                <label
                  key={mono.id}
                  className="flex cursor-pointer items-center gap-2 rounded-md px-1.5 py-1 text-[12.5px] text-content/85 hover:bg-content/6"
                >
                  <input
                    type="checkbox"
                    checked={checked}
                    onChange={() =>
                      setNewChannelMembers((prev) =>
                        prev.includes(mono.id)
                          ? prev.filter((id) => id !== mono.id)
                          : [...prev, mono.id],
                      )
                    }
                    className="rounded border-content/25"
                  />
                  <MonoRailMascot
                    name={look.mascot}
                    color={look.color}
                    status="idle"
                  />
                  <span className="truncate">{look.name}</span>
                </label>
              );
            })}
          </div>
          <div className="flex justify-end gap-1.5">
            <button
              type="button"
              onClick={() => setNewChannelAnchor(null)}
              className="rounded-lg px-2.5 py-1 text-xs text-content/60 hover:bg-content/8 hover:text-content"
            >
              Cancel
            </button>
            <button
              type="button"
              onClick={submitNewChannel}
              className="rounded-lg bg-accent px-3 py-1 text-xs font-medium text-white hover:opacity-90"
            >
              Create channel
            </button>
          </div>
        </Popover>
      ) : null}

      {menu ? (
        <MonoMenu
          key={menu.id}
          monoId={menu.id}
          anchor={menu.anchor}
          onBackground={() => {
            setMenu(undefined);
            setBackground(menu.id);
          }}
          onDelete={() => {
            setMenu(undefined);
            onDelete(menu.id);
          }}
          onClose={() => setMenu(undefined)}
        />
      ) : null}
      {backgroundMono ? (
        <ProjectBackgroundDialog
          project={monoBackgroundKey(backgroundMono.id)}
          name={monoLook(backgroundMono).name}
          locked
          onClose={() => setBackground(undefined)}
        />
      ) : null}
    </div>
  );
}

/**
 * Right-click menu for a Mono, like a project's: its name, mascot and color
 * up top, edited in place, then the actions.
 */
function MonoMenu({
  monoId,
  anchor,
  onBackground,
  onDelete,
  onClose,
}: {
  monoId: string;
  anchor: PopoverAnchor;
  onBackground: () => void;
  onDelete: () => void;
  onClose: () => void;
}) {
  useSyncExternalStore(subscribeMonos, monosSnapshot);
  const mono = findMono(monoId);
  const input = useRef<HTMLInputElement>(null);
  const [name, setName] = useState(mono?.name ?? "");

  useEffect(() => {
    input.current?.focus();
    input.current?.select();
  }, []);

  if (!mono) return null;
  const look = monoLook(mono);
  const commitName = () => {
    const next = name.trim().slice(0, 40);
    if (next !== (findMono(monoId)?.name ?? "")) saveMonoName(monoId, next);
  };

  return (
    <Popover
      anchor={anchor}
      side="right"
      align="start"
      gap={0}
      width={280}
      constrainHeight={false}
      onDismiss={() => {
        commitName();
        onClose();
      }}
      role="menu"
      aria-label="Mono options"
      onContextMenu={(event) => event.preventDefault()}
      className="p-2"
    >
      <input
        ref={input}
        value={name}
        placeholder={defaultMonoName(mono.mascot)}
        maxLength={40}
        aria-label="Mono name"
        onChange={(event) => setName(event.target.value)}
        onBlur={commitName}
        onKeyDown={(event) => {
          if (event.key !== "Enter") return;
          event.preventDefault();
          commitName();
          onClose();
        }}
        className="mb-2 w-full rounded-lg border border-content/10 bg-content/5 px-2.5 py-1.5 text-[13px] text-content outline-none ring-accent/40 placeholder:text-content/45 focus:ring-1"
      />
      <div className="mb-2 flex flex-col gap-2">
        <MascotPicker
          current={look.mascot}
          color={look.color}
          onPick={(mascot) => saveMonoMascot(monoId, mascot)}
        />
        <ColorPicker
          inline
          current={look.color}
          onPick={(color) => updateMono(monoId, (mono) => ({ ...mono, color }))}
        />
      </div>
      <div role="separator" className="my-1 h-px bg-content/10" />
      <button
        type="button"
        role="menuitem"
        onClick={() => {
          commitName();
          onBackground();
        }}
        className="flex w-full items-center gap-2.5 rounded-lg px-2.5 py-1.5 text-left text-[13px] text-content/85 hover:bg-content/8"
      >
        <ImagePlus className="size-3.5 shrink-0" strokeWidth={1.75} />
        Background image
      </button>
      <button
        type="button"
        role="menuitem"
        onClick={onDelete}
        className="flex w-full items-center gap-2.5 rounded-lg px-2.5 py-1.5 text-left text-[13px] text-red-400 hover:bg-content/8"
      >
        <Trash2 className="size-3.5 shrink-0" strokeWidth={1.75} />
        Delete mono…
      </button>
    </Popover>
  );
}
