import { isHabitRun } from "../../bots/model/botHabits";
import { findBot, isBotSession } from "../../bots/model/bot";
import {
  listBotPeerSummaries,
  resolveUniqueBotPeer,
} from "../../bots/model/botChannels";
import {
  isRemoteProjectPath,
  looksLikeProject,
  sameProjectPath,
} from "../../projects/model/recents";
import {
  readComposer,
  writeComposer,
} from "../../sessions/model/composerControl";
import type { Session } from "../../sessions/model/session";
import { handleVoxMemory } from "./memory";
import {
  VOX_ACTIONS,
  type VoxRequest,
  type VoxRequestHandler,
} from "./protocol";

/** Explicit in-process capability. Native CLI input cannot construct this. */
export type VoxAppContext = {
  request: VoxRequest;
  projects: string[];
  source: Session;
};

type Host = {
  snapshot: () => { selected?: Session; projects: string[]; cwd: string };
  getSession: (id: string) => Promise<Session | null | undefined>;
  openBot: (id: string) => Promise<void>;
  openSession: (id: string) => Promise<void>;
  dispatch: (request: VoxRequest, context: VoxAppContext) => Promise<unknown>;
};

export function assertVoxCurrent(request: VoxRequest) {
  if (!request.isCurrent() || request.signal?.aborted)
    throw new Error("Vox stopped before this request was sent");
}

/** Owns Vox's target selection and scope. Existing app actions still own sends
 * and runs; mounted composers still own edits and durable drafts. */
export function createVoxController(host: Host): VoxRequestHandler {
  return async (request) => {
    assertVoxCurrent(request);
    if (!VOX_ACTIONS.includes(request.action as (typeof VOX_ACTIONS)[number]))
      throw new Error("Unsupported Vox action");
    const { selected, projects: paths, cwd } = host.snapshot();
    const projects = [...new Set(paths)].filter(
      (path) => looksLikeProject(path) && !isRemoteProjectPath(path),
    );
    const { action, input } = request;
    if (action.startsWith("memory."))
      return handleVoxMemory(request, () => assertVoxCurrent(request));
    if (action === "workspace.current")
      return {
        session: selected
          ? {
              id: selected.id,
              title: selected.title,
              cwd: selected.cwd,
              harness: selected.harness,
              model: selected.model,
              busy: !!selected.busy,
            }
          : null,
        projects,
        bots: listBotPeerSummaries().map(({ id, name, sessionId }) => ({
          id,
          name,
          sessionId,
        })),
      };
    if (
      ["workspace.open", "composer.read", "composer.write"].includes(action)
    ) {
      const fields =
        action === "composer.write"
          ? ["sessionId", "bot", "text", "mode", "expectedText"]
          : ["sessionId", "bot"];
      if (Object.keys(input).some((key) => !fields.includes(key)))
        throw new Error("Unsupported composer field");
      if (
        input.sessionId !== undefined &&
        (typeof input.sessionId !== "string" || !input.sessionId.trim())
      )
        throw new Error("Provide a session ID");
      const botQuery = input.bot as string | undefined;
      if (
        botQuery !== undefined &&
        (typeof botQuery !== "string" || !botQuery.trim())
      )
        throw new Error("Provide a Bot name");
      if (input.sessionId && botQuery)
        throw new Error("Choose a session or a Bot, not both");
      if (action === "workspace.open" && !input.sessionId && !botQuery)
        throw new Error("Choose a session or named Bot to open");
      let targetId = (input.sessionId as string | undefined) ?? selected?.id;
      if (typeof botQuery === "string") {
        const bot = resolveUniqueBotPeer(botQuery);
        if (!bot) throw new Error("No Bot matches that name; use bots.list");
        assertVoxCurrent(request);
        await host.openBot(bot.id);
        targetId = findBot(bot.id)?.sessionId;
      } else {
        if (!targetId) throw new Error("Choose a chat first");
        const target = await host.getSession(targetId);
        if (
          !target ||
          target.orchestrationLeadId ||
          isHabitRun(target.id) ||
          (!isBotSession(target.id) &&
            !projects.some((path) => sameProjectPath(path, target.cwd)))
        )
          throw new Error("That local chat is unavailable");
        assertVoxCurrent(request);
        await host.openSession(targetId);
      }
      assertVoxCurrent(request);
      if (!targetId) throw new Error("That chat could not be opened");
      if (action === "workspace.open")
        return { sessionId: targetId, opened: true };
      if (action === "composer.read")
        return readComposer(targetId, request.signal);
      return writeComposer(
        targetId,
        input as {
          text: string;
          mode?: "append" | "replace";
          expectedText?: string;
        },
        request.signal,
      );
    }
    return host.dispatch(request, {
      request,
      projects,
      source: {
        id: request.sessionId,
        harness: "codex",
        model: "codex:gpt-6.1-sol",
        modelSettings: {},
        runtimeMode: selected?.runtimeMode ?? "supervised",
        title: "vox",
        blocks: [],
        ephemeral: true,
        cwd:
          projects.find((path) =>
            sameProjectPath(path, selected?.cwd ?? cwd),
          ) ??
          projects[0] ??
          "~",
      },
    });
  };
}
