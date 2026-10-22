export const HOST_PROTOCOL_VERSION = 1;
/** The host understands exact-byte editor baselines and checked save receipts. */
export const WORKSPACE_EDITOR_SAVE_CAPABILITY =
  "workspace.editorSavePrecondition";
import { HARNESSES } from "../sessions/harnessIds.js";
export const REMOTE_PROVIDERS = HARNESSES;
/** Throws when the delta does not apply to `known`; request a snapshot then. */
export function applySessionSync(known, sync) {
  if (sync.kind === "snapshot") return sync.value;
  if (
    !known ||
    known.revision !== (sync.kind === "delta" ? sync.base : sync.revision)
  )
    throw new Error("Session sync base does not match");
  if (sync.kind === "unchanged") return known;
  const blocks = new Map(
    known.session.blocks.map((block) => [block.id, block]),
  );
  for (const block of sync.blocks) blocks.set(block.id, block);
  return {
    ...sync.value,
    session: {
      ...sync.value.session,
      blocks: sync.blockIds.map((id) => {
        const block = blocks.get(id);
        if (!block) throw new Error("Session sync is missing a block");
        return block;
      }),
    },
  };
}
export function isRemoteProvider(value) {
  return (
    typeof value === "string" &&
    REMOTE_PROVIDERS.some((provider) => provider === value)
  );
}
export function requireHostDescriptor(value) {
  if (
    value?.protocolVersion !== HOST_PROTOCOL_VERSION ||
    typeof value.environmentId !== "string" ||
    !value.environmentId ||
    !Array.isArray(value.providers) ||
    !value.providers.every(isRemoteProvider)
  ) {
    throw new Error("This machine is running an incompatible vorkspace Host");
  }
  return value;
}
