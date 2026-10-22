/**
 * Stands in for a turn that arrived with files but no words.
 *
 * Sent on the wire only. The transcript keeps the empty text and shows the
 * attachments on their own, so this never reaches the user's own message.
 */
export const ATTACHMENT_ONLY_PROMPT =
  "The user attached these files without saying anything. Use the conversation above to work out what they want done with them, then do that. If the conversation gives you nothing to go on, ask.";
/**
 * The turn's text, or a stand-in when files arrived without any.
 *
 * A turn carrying only attachments never says what to do with them, leaving a
 * model to guess or to ask what the files are for. The conversation so far is
 * the only clue the user left behind, so point the model at it instead.
 */
export function promptText(text, attachments = []) {
  const trimmed = text.trim();
  if (trimmed || !attachments.length) return trimmed;
  return ATTACHMENT_ONLY_PROMPT;
}
/** A copied folder. No harness can open one, so it travels as its path. */
const FOLDER_MIME = "inode/directory";
/** MIME types providers typically send as vision input. */
const VISION_MIME = new Set([
  "image/png",
  "image/jpeg",
  "image/jpg",
  "image/gif",
  "image/webp",
]);
export function promptBlocks(text, attachments = []) {
  const blocks = [];
  const body = promptText(text, attachments);
  if (body) blocks.push({ type: "text", text: body });
  for (const file of attachments) {
    blocks.push(contentBlockFor(file));
  }
  return blocks;
}
/** Require a deliverable source instead of silently dropping an attachment. */
export function attachmentPath(file) {
  if (!file.path?.trim()) {
    throw new Error(
      `Cannot attach ${JSON.stringify(file.name)}: no local file path is available. Attach the file again.`,
    );
  }
  return file.path;
}
/** Native harnesses without file blocks can ask their tools to read this path. */
export function attachmentPathText(file) {
  if (isAttachmentFolder(file)) {
    return `Attached folder (list or read the files inside from this path): ${JSON.stringify(attachmentPath(file))}`;
  }
  return `Attached file (read from disk): ${JSON.stringify(attachmentPath(file))}`;
}
export function isAttachmentFolder(file) {
  return file.mimeType === FOLDER_MIME;
}
function contentBlockFor(file) {
  // No harness can open a folder, so it travels as a path for the agent's own
  // tools rather than a resource link nothing can read.
  if (isAttachmentFolder(file)) {
    return { type: "text", text: attachmentPathText(file) };
  }
  if (file.data && isVisionImage(file.mimeType)) {
    return {
      type: "image",
      mimeType: normalizeImageMime(file.mimeType),
      data: file.data,
      ...(file.path ? { uri: fileUri(file.path) } : {}),
    };
  }
  return {
    type: "resource_link",
    uri: fileUri(attachmentPath(file)),
    name: file.name,
    mimeType: file.mimeType,
    size: file.size,
  };
}
export function isVisionImage(mimeType) {
  return VISION_MIME.has(mimeType.toLowerCase());
}
export function normalizeImageMime(mimeType) {
  const mime = mimeType.toLowerCase();
  if (mime === "image/jpg") return "image/jpeg";
  return mime;
}
function fileUri(path) {
  const normalized = path.replace(/\\/g, "/");
  const abs = normalized.startsWith("/") ? normalized : `/${normalized}`;
  return `file://${abs.split("/").map(encodeURIComponent).join("/")}`;
}
