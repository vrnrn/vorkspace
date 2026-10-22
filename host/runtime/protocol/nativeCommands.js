const RESERVED_COMMANDS = new Set(["plan", "compact", "add-to-folder"]);
export function nativeCommandInvocation(harness, name) {
  return RESERVED_COMMANDS.has(name) ? `${harness}:${name}` : name;
}
