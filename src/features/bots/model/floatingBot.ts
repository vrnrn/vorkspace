import type { ApprovalDecision } from "../../../integrations/harness";
import type { Attachment, Session } from "../../sessions/model/session";
import type { UserQuestionReply } from "../../sessions/model/userQuestion";
import { displayAttachments } from "../../sessions/model/attachments";
import { listBots, botLook, botState, type BotStatus } from "./bot";
import { pixelLayers } from "../../projects/model/pixelMascots";

export const FLOATING_BOT_CHANGED = "bot_chat_changed";
export const FLOATING_BOT_REQUEST = "bot_chat_request";
export type FloatingBotEntry = {
  id: string;
  name: string;
  mascot: string;
  color: string;
  sessionId: string | null;
  status: BotStatus;
};
export type FloatingBotView = {
  bots: FloatingBotEntry[];
  botId: string | null;
  session: Session | null;
  error: string | null;
};
export type FloatingBotAction =
  | { kind: "open" }
  | { kind: "submit"; text: string; attachments: Attachment[] }
  | { kind: "stop" }
  | { kind: "create" }
  | { kind: "approval"; requestId: number; decision: ApprovalDecision }
  | { kind: "question"; requestId: number; reply: UserQuestionReply }
  | { kind: "questionInteraction"; requestId: number }
  | { kind: "reveal" }
  | { kind: "openFile"; path: string }
  | { kind: "openArtifact"; id: string }
  | { kind: "resume" };
export type FloatingBotRequest = {
  id: number;
  botId: string;
  action: FloatingBotAction;
};

export function floatingBotRoster(
  enabled: boolean,
  sessions: readonly Session[] = [],
): FloatingBotEntry[] {
  return enabled
    ? listBots().map((bot) => {
        const session = sessions.find((s) => s.id === bot.sessionId);
        return {
          id: bot.id,
          ...botLook(bot),
          sessionId: bot.sessionId ?? null,
          status: session ? botState(session).status : "idle",
        };
      })
    : [];
}

/** Draw the same portraits in AppKit without a second set of mascot assets. */
export function floatingBotMenuMascots(bots: FloatingBotEntry[]) {
  return bots.map((bot) => {
    const { body, open } = pixelLayers(bot.mascot, bot.color);
    return { botId: bot.id, rects: [...body, ...open] };
  });
}

/** Blob URLs belong to their webview; send attachment bytes/paths instead. */
export function floatingBotSession(session: Session): Session {
  return {
    ...session,
    blocks: session.blocks.map((block) =>
      block.attachments?.length
        ? { ...block, attachments: floatingBotAttachments(block.attachments) }
        : block,
    ),
    queuedMessages: session.queuedMessages?.map((message) => ({
      ...message,
      attachments: floatingBotAttachments(message.attachments),
    })),
  };
}

export function floatingBotAttachments(
  attachments: Attachment[],
): Attachment[] {
  return displayAttachments(attachments).map(
    ({ previewUrl: _preview, ...file }) => file,
  );
}

export type FloatingBotHost = {
  open(botId: string): Promise<Session | undefined>;
  submit(
    sessionId: string,
    text: string,
    attachments: Attachment[],
  ): boolean | void;
  stop(sessionId: string): void;
  approval(
    sessionId: string,
    requestId: number,
    decision: ApprovalDecision,
  ): void;
  question(
    sessionId: string,
    requestId: number,
    reply: UserQuestionReply,
  ): void;
  questionInteraction(sessionId: string, requestId: number): void;
  reveal(botId: string): Promise<void>;
  openFile(path: string): void | Promise<void>;
  openArtifact?(botId: string, id: string): void | Promise<void>;
  resume(sessionId: string): void;
  /** Add a Bot and show it in place of the chat that asked. */
  create?(fromBotId: string): Promise<void>;
};

/** Preparation may await disk; recheck the receipt before mutating a session. */
export async function deliverFloatingBotRequest(
  request: FloatingBotRequest,
  host: FloatingBotHost,
  accept: () => Promise<boolean>,
): Promise<Session | undefined> {
  const session = await host.open(request.botId);
  if (!session) throw new Error("That Bot is no longer available.");
  if (!(await accept())) return undefined;
  const action = request.action;
  switch (action.kind) {
    case "open":
      break;
    case "submit":
      if (host.submit(session.id, action.text, action.attachments) === false)
        throw new Error(
          "The message could not be sent. Try again in a moment.",
        );
      break;
    case "stop":
      host.stop(session.id);
      break;
    case "approval":
      host.approval(session.id, action.requestId, action.decision);
      break;
    case "question":
      host.question(session.id, action.requestId, action.reply);
      break;
    case "questionInteraction":
      host.questionInteraction(session.id, action.requestId);
      break;
    case "reveal":
      await host.reveal(request.botId);
      break;
    case "openFile":
      await host.openFile(action.path);
      break;
    case "openArtifact":
      if (!host.openArtifact) throw new Error("Artifacts are unavailable.");
      await host.openArtifact(request.botId, action.id);
      break;
    case "resume":
      host.resume(session.id);
      break;
    case "create":
      if (!host.create) throw new Error("New Bots are unavailable here.");
      await host.create(request.botId);
      break;
    default:
      throw new Error("Unknown chat action.");
  }
  return session;
}
