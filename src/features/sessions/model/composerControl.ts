import { composerDraftState, hydrateComposerDraft } from "./draftCache";

type Editor = {
  field: HTMLTextAreaElement;
  write: (text: string) => void;
};
const editors = new Map<string, Set<Editor>>();
const waiting = new Set<() => void>();

/** A mounted composer owns editing and persistence. Controls never manufacture
 * a second draft or write an unloaded payload back to storage. */
export function registerComposerEditor(sessionId: string, editor: Editor) {
  let owners = editors.get(sessionId);
  if (!owners) editors.set(sessionId, (owners = new Set()));
  owners.add(editor);
  for (const wake of waiting) wake();
  return () => {
    owners.delete(editor);
    if (!owners.size && editors.get(sessionId) === owners)
      editors.delete(sessionId);
  };
}

function current(sessionId: string): Editor | undefined {
  const owners = [...(editors.get(sessionId) ?? [])].filter(
    ({ field }) => field.isConnected && !field.disabled && !field.readOnly,
  );
  if (owners.length > 1) {
    const focused = owners.find(
      ({ field }) => field === document.activeElement,
    );
    if (focused) return focused;
    throw new Error(
      "More than one composer is open for this chat. Focus the one to edit.",
    );
  }
  return owners[0];
}

async function ready(sessionId: string, signal?: AbortSignal) {
  if (signal?.aborted)
    throw new Error("Request cancelled before the composer was edited");
  const editor =
    current(sessionId) ??
    (await new Promise<Editor>((resolve, reject) => {
      const clean = () => {
        clearTimeout(timer);
        waiting.delete(wake);
        signal?.removeEventListener("abort", abort);
      };
      const abort = () => {
        clean();
        reject(new Error("Request cancelled before the composer was edited"));
      };
      const wake = () => {
        try {
          const value = current(sessionId);
          if (value) {
            clean();
            resolve(value);
          }
        } catch (error) {
          clean();
          reject(error);
        }
      };
      const timer = setTimeout(() => {
        clean();
        reject(new Error("Open this chat's composer before editing it"));
      }, 5000);
      waiting.add(wake);
      signal?.addEventListener("abort", abort, { once: true });
    }));
  // Wait for restoration, then read live text again: typing and restored
  // attachments remain authoritative while a controller request is pending.
  await hydrateComposerDraft(sessionId);
  if (signal?.aborted || current(sessionId) !== editor)
    throw new Error(
      "The composer changed before editing. Try again in the intended chat.",
    );
  const issue = composerDraftState(sessionId)?.issue;
  if (issue && issue.kind !== "missing") throw new Error(issue.message);
  return editor;
}

export async function readComposer(sessionId: string, signal?: AbortSignal) {
  const { field } = await ready(sessionId, signal);
  return {
    sessionId,
    text: field.value.slice(0, 12_000),
    truncated: field.value.length > 12_000,
    length: field.value.length,
    selectionStart: field.selectionStart,
    selectionEnd: field.selectionEnd,
  };
}

export async function writeComposer(
  sessionId: string,
  input: { text: string; mode?: "append" | "replace"; expectedText?: string },
  signal?: AbortSignal,
) {
  if (
    typeof input.text !== "string" ||
    !input.text.trim() ||
    input.text.length > 60_000
  )
    throw new Error("Provide composer text up to 60,000 characters");
  const mode = input.mode ?? "append";
  if (mode !== "append" && mode !== "replace")
    throw new Error("Composer mode must be append or replace");
  if (mode === "replace" && typeof input.expectedText !== "string")
    throw new Error("Read the composer before replacing its text");
  const editor = await ready(sessionId, signal);
  const before = editor.field.value;
  if (input.expectedText !== undefined && input.expectedText !== before)
    throw new Error(
      "The user changed this draft. Read it again before replacing text.",
    );
  const text =
    mode === "replace" || !before
      ? input.text
      : `${before}${before.endsWith("\n\n") ? "" : before.endsWith("\n") ? "\n" : "\n\n"}${input.text}`;
  if (text.length > 100_000)
    throw new Error("The combined composer text is too long");
  editor.write(text);
  return { sessionId, updated: true, sent: false, length: text.length };
}
