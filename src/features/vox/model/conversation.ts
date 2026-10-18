/** Vox's identity is independent of a call, selected chat, and project. */
export const VOX_MEMORY_ID = "vox";
export const VOX_CONVERSATION_KEY = "vorkspace.vox.conversation.v1";
type Conversation = { revision: string; threadId?: string };

export class VoxConversationError extends Error {}

export function loadVoxConversation(): Conversation {
  try {
    const raw = localStorage.getItem(VOX_CONVERSATION_KEY);
    if (raw === null) return { revision: "initial" };
    const saved = JSON.parse(raw);
    if (
      typeof saved?.revision !== "string" ||
      (saved.threadId !== undefined &&
        (typeof saved.threadId !== "string" ||
          !/^[a-zA-Z0-9_-]{1,128}$/.test(saved.threadId)))
    )
      throw new Error("invalid conversation identity");
    return { revision: saved.revision, threadId: saved.threadId };
  } catch {
    throw new VoxConversationError(
      "vox's saved conversation could not be read. try again or reset it in settings → vox.",
    );
  }
}

export function saveVoxConversation(expected: Conversation, threadId: string) {
  const current = loadVoxConversation();
  if (
    current.revision !== expected.revision ||
    current.threadId !== expected.threadId
  )
    throw new VoxConversationError(
      "vox's conversation changed. start vox again.",
    );
  try {
    localStorage.setItem(
      VOX_CONVERSATION_KEY,
      JSON.stringify({ ...current, threadId }),
    );
  } catch {
    throw new VoxConversationError(
      "vox's conversation could not be saved. check available storage and try again.",
    );
  }
}

/** Reset the conversation only. MEMORY.md and its topic files are untouched. */
export function resetSavedVoxConversation() {
  localStorage.setItem(
    VOX_CONVERSATION_KEY,
    JSON.stringify({ revision: crypto.randomUUID() }),
  );
}
