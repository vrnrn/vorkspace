import { isHabitRun } from "../../monos/model/monoHabits";
import { findMono, isMonoSession } from "../../monos/model/mono";
import {
  listMonoPeerSummaries,
  resolveUniqueMonoPeer,
} from "../../monos/model/monoChannels";
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
  openMono: (id: string) => Promise<void>;
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
        monos: listMonoPeerSummaries().map(({ id, name, sessionId }) => ({
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
          ? ["sessionId", "mono", "text", "mode", "expectedText"]
          : ["sessionId", "mono"];
      if (Object.keys(input).some((key) => !fields.includes(key)))
        throw new Error("Unsupported composer field");
      if (
        input.sessionId !== undefined &&
        (typeof input.sessionId !== "string" || !input.sessionId.trim())
      )
        throw new Error("Provide a session ID");
      if (
        input.mono !== undefined &&
        (typeof input.mono !== "string" || !input.mono.trim())
      )
        throw new Error("Provide a Mono name");
      if (input.sessionId && input.mono)
        throw new Error("Choose a session or a Mono, not both");
      if (action === "workspace.open" && !input.sessionId && !input.mono)
        throw new Error("Choose a session or named Mono to open");
      let targetId = (input.sessionId as string | undefined) ?? selected?.id;
      if (typeof input.mono === "string") {
        const mono = resolveUniqueMonoPeer(input.mono);
        if (!mono) throw new Error("No Mono matches that name; use monos.list");
        assertVoxCurrent(request);
        await host.openMono(mono.id);
        targetId = findMono(mono.id)?.sessionId;
      } else {
        if (!targetId) throw new Error("Choose a chat first");
        const target = await host.getSession(targetId);
        if (
          !target ||
          target.orchestrationLeadId ||
          isHabitRun(target.id) ||
          (!isMonoSession(target.id) &&
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
