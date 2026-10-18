import { CHAT_BACKGROUND_OPACITY_DEFAULT } from "../../settings/model/appearance";
import { clearProjectChatBackground } from "../../projects/model/chatBackground";
import { migrateBotStorage } from "./botStorageMigration";
import {
  clearProjectChatBackgroundSetting,
  loadProjectChatBackgroundSettings,
  type ProjectChatBackgroundSettings,
} from "../../projects/model/projectChatBackground";

/**
 * A Bot's background is stored like a project's, under a key no project
 * folder has, so the project background dialog can edit it as is.
 */
export function botBackgroundKey(botId: string): string {
  migrateBotStorage();
  return `bot:${botId}`;
}

/**
 * How a Bot's chat shows its image: always the Haze, dimmed the same on an
 * empty chat as in a running one. Only the image is the user's choice.
 */
export function botChatBackground(
  botId: string,
): ProjectChatBackgroundSettings | null {
  const stored = loadProjectChatBackgroundSettings(botBackgroundKey(botId));
  if (!stored) return null;
  return {
    path: stored.path,
    emptyOpacity: CHAT_BACKGROUND_OPACITY_DEFAULT,
    sessionOpacity: CHAT_BACKGROUND_OPACITY_DEFAULT,
    scope: "all",
    effect: "gradient-blur",
  };
}

/** Forgets the image along with its Bot. */
export function removeBotBackground(botId: string): void {
  const key = botBackgroundKey(botId);
  if (!loadProjectChatBackgroundSettings(key)) return;
  clearProjectChatBackgroundSetting(key);
  void clearProjectChatBackground(key).catch(() => {});
}
