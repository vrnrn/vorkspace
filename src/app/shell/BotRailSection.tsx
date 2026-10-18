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
import { BotRailMascot } from "../../features/bots/ui/BotRailMascot";
import { BotIntroCard } from "../../features/bots/ui/BotIntro";
import { RailAction } from "./RailAction";
import { saveBotsEnabled } from "../../features/settings/model/settings";
import { ProjectBackgroundDialog } from "../../features/projects/ui/ProjectBackgroundDialog";
import { botBackgroundKey } from "../../features/bots/model/botBackground";
import {
  ColorPicker,
  MascotPicker,
} from "../../features/bots/ui/botPanelParts";
import {
  defaultBotName,
  findBot,
  listBots,
  BOT_STATUS_LABEL,
  botLook,
  botProjectsPhrase,
  botsSnapshot,
  botWorksOn,
  reorderBots,
  saveBotMascot,
  saveBotName,
  subscribeBots,
  updateBot,
  type BotState,
} from "../../features/bots/model/bot";
import {
  createBotChannel,
  listBotChannels,
  botChannelsSnapshot,
  removeBotChannel,
  subscribeBotChannels,
} from "../../features/bots/model/botChannels";

export type BotRailProps = {
  /** The Bot open in the main area, if any. */
  activeId?: string;
  /** The multi-Bot channel open in the main area, if any. */
  activeChannelId?: string;
  /** Channels where a Bot is currently speaking or queued. */
  busyChannelIds?: ReadonlySet<string>;
  /** What each Bot is doing, by Bot id; missing reads as idle. */
  states: ReadonlyMap<string, BotState>;
  /** Replies each Bot has that the user has not looked at yet, by Bot id. */
  unseenCounts?: ReadonlyMap<string, number>;
  onOpen: (botId: string) => void;
  onOpenChannel?: (channelId: string) => void;
  /** Creates a Bot, given these projects to start with. */
  onCreate: (projects?: readonly string[]) => void;
  onDelete: (botId: string) => void;
  onDeleteChannel?: (channelId: string) => void;
};

const IDLE: BotState = { status: "idle" };

/** A Bot's unread count as its badge shows it. */
export function unreadBadge(count: number): string {
  return count > 99 ? "99+" : String(count);
}

export function unreadLabel(count: number): string {
  return count === 1 ? "1 new reply" : `${count} new replies`;
}

/**
 * The user's Bots, above the projects. Each opens like a project does, but
 * into its conversation; none belongs to a project, so they get a section of
 * their own and the shaded mascot rather than a project's flat one.
 */
export function BotRailSection({
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
}: BotRailProps & {
  /**
   * Lists only the Bots working on this project, as the inset sidebar does;
   * a Bot on several projects shows in each of them.
   */
  project?: string;
}) {
  const snapshot = useSyncExternalStore(subscribeBots, botsSnapshot);
  const channelsSnap = useSyncExternalStore(
    subscribeBotChannels,
    botChannelsSnapshot,
  );
  const channels = useMemo(() => listBotChannels(), [channelsSnap]);
  const bots = useMemo(
    () => listBots().filter((bot) => !project || botWorksOn(bot, project)),
    [snapshot, project],
  );
  const ids = bots.map((bot) => bot.id);
  const sortable = useAnimatedReorder(ids, reorderBots, "y");
  const [menu, setMenu] = useState<{ id: string; anchor: PopoverAnchor }>();
  const [background, setBackground] = useState<string>();
  const [newChannelAnchor, setNewChannelAnchor] =
    useState<PopoverAnchor | null>(null);
  const [newChannelName, setNewChannelName] = useState("");
  const [newChannelTopic, setNewChannelTopic] = useState("");
  const [newChannelMembers, setNewChannelMembers] = useState<string[]>([]);
  const backgroundBot = background ? findBot(background) : undefined;

  // With no Bot yet, the card stands in for the rail's section. A project
  // with none of its own offers one under the label instead.
  if (bots.length === 0 && !project)
    return (
      <div className="mb-2 shrink-0" data-bot-rail>
        <BotIntroCard
          onCreate={() => onCreate()}
          onHide={() => saveBotsEnabled(false)}
        />
      </div>
    );

  const openCreateChannel = (anchor: PopoverAnchor) => {
    setNewChannelName("");
    setNewChannelTopic("");
    setNewChannelMembers(bots.map((m) => m.id));
    setNewChannelAnchor(anchor);
  };

  const submitNewChannel = () => {
    const name = newChannelName.trim() || "general";
    const memberIds =
      newChannelMembers.length > 0
        ? newChannelMembers
        : bots.map((m) => m.id);
    const created = createBotChannel({
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
      data-bot-rail
      data-bot-project={project}
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
          Bots
        </span>
        {bots.length ? (
          <button
            type="button"
            title="New bot"
            aria-label="New bot"
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
        {bots.length === 0 ? (
          <RailAction
            label="New bot"
            icon={Plus}
            onClick={() => onCreate(project ? [project] : undefined)}
          />
        ) : null}
        {bots.map((bot) => {
          const look = botLook(bot);
          const state = states.get(bot.id) ?? IDLE;
          const selected = bot.id === activeId && !activeChannelId;
          const unseen = selected ? 0 : (unseenCounts?.get(bot.id) ?? 0);
          const projects = look.projects.length
            ? botProjectsPhrase(look.projects)
            : "No projects yet";
          const status =
            state.status === "idle"
              ? undefined
              : (state.activity ?? BOT_STATUS_LABEL[state.status]);
          return (
            <div
              key={bot.id}
              ref={(el) => sortable.setItemRef(bot.id, el)}
              data-selected={selected || undefined}
              data-bot-status={state.status}
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
                sortable.onItemPointerDown(bot.id, event);
              }}
              onClick={(event) => {
                if (
                  (event.target as HTMLElement | null)?.closest(
                    "[data-no-drag]",
                  )
                )
                  return;
                if (sortable.consumeClick()) return;
                onOpen(bot.id);
              }}
              onContextMenu={(event) => {
                event.preventDefault();
                setMenu({
                  id: bot.id,
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
                <BotRailMascot
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
                    data-bot-unread={unseen}
                    className="grid h-4 min-w-4 shrink-0 place-items-center rounded-full bg-accent px-1 text-[10px] font-medium tabular-nums leading-none text-white group-hover:hidden group-has-[button:focus-visible]:hidden"
                  >
                    {unreadBadge(unseen)}
                  </span>
                ) : null}
              </button>
              <button
                type="button"
                data-no-drag
                title="Bot options"
                aria-label={`${look.name} options`}
                aria-haspopup="menu"
                onPointerDown={(event) => event.stopPropagation()}
                onClick={(event) => {
                  event.stopPropagation();
                  setMenu({ id: bot.id, anchor: event.currentTarget });
                }}
                className="absolute right-1 top-1/2 hidden size-6 -translate-y-1/2 place-items-center rounded-md text-content/55 hover:bg-content/8 hover:text-content group-hover:grid group-has-[button:focus-visible]:grid"
              >
                <MoreHorizontal className="size-4" strokeWidth={1.75} />
              </button>
            </div>
          );
        })}
      </div>

      {/* Multi-Bot Channels Subsection (below Bots, above Projects) */}
      <div className="mt-2.5" data-bot-channels-rail>
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
                .map((id) => findBot(id))
                .filter((m): m is NonNullable<typeof m> => Boolean(m))
                .map(botLook);
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
                      else removeBotChannel(channel.id);
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
          aria-label="New Bot channel"
          className="p-3"
        >
          <div className="mb-2 text-xs font-semibold text-content">
            New Bot Channel
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
            {bots.map((bot) => {
              const look = botLook(bot);
              const checked = newChannelMembers.includes(bot.id);
              return (
                <label
                  key={bot.id}
                  className="flex cursor-pointer items-center gap-2 rounded-md px-1.5 py-1 text-[12.5px] text-content/85 hover:bg-content/6"
                >
                  <input
                    type="checkbox"
                    checked={checked}
                    onChange={() =>
                      setNewChannelMembers((prev) =>
                        prev.includes(bot.id)
                          ? prev.filter((id) => id !== bot.id)
                          : [...prev, bot.id],
                      )
                    }
                    className="rounded border-content/25"
                  />
                  <BotRailMascot
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
        <BotMenu
          key={menu.id}
          botId={menu.id}
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
      {backgroundBot ? (
        <ProjectBackgroundDialog
          project={botBackgroundKey(backgroundBot.id)}
          name={botLook(backgroundBot).name}
          locked
          onClose={() => setBackground(undefined)}
        />
      ) : null}
    </div>
  );
}

/**
 * Right-click menu for a Bot, like a project's: its name, mascot and color
 * up top, edited in place, then the actions.
 */
function BotMenu({
  botId,
  anchor,
  onBackground,
  onDelete,
  onClose,
}: {
  botId: string;
  anchor: PopoverAnchor;
  onBackground: () => void;
  onDelete: () => void;
  onClose: () => void;
}) {
  useSyncExternalStore(subscribeBots, botsSnapshot);
  const bot = findBot(botId);
  const input = useRef<HTMLInputElement>(null);
  const [name, setName] = useState(bot?.name ?? "");

  useEffect(() => {
    input.current?.focus();
    input.current?.select();
  }, []);

  if (!bot) return null;
  const look = botLook(bot);
  const commitName = () => {
    const next = name.trim().slice(0, 40);
    if (next !== (findBot(botId)?.name ?? "")) saveBotName(botId, next);
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
      aria-label="Bot options"
      onContextMenu={(event) => event.preventDefault()}
      className="p-2"
    >
      <input
        ref={input}
        value={name}
        placeholder={defaultBotName(bot.mascot)}
        maxLength={40}
        aria-label="Bot name"
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
          onPick={(mascot) => saveBotMascot(botId, mascot)}
        />
        <ColorPicker
          inline
          current={look.color}
          onPick={(color) => updateBot(botId, (bot) => ({ ...bot, color }))}
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
        Delete bot…
      </button>
    </Popover>
  );
}
