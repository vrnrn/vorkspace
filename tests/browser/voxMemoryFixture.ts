/** The browser fixtures use the real memory UI/logic against isolated fake files. */
export function voxMemoryFixture(
  command: string,
  args?: Record<string, unknown>,
) {
  if (!command.startsWith("bot_")) return undefined;
  if (args?.bot !== "vox") throw new Error("Unexpected fixture memory owner");
  const key = "vox.fixture.memory";
  const files: Record<string, string> = JSON.parse(
    localStorage.getItem(key) ?? '{"MEMORY.md":"","SOUL.md":""}',
  );
  const path = args.path as string;
  const hash = (name: string) => `fixture:${files[name] ?? ""}`;
  if (command === "bot_read")
    return { text: files[path] ?? null, hash: hash(path) };
  if (command === "bot_load")
    return {
      id: "vox",
      dir: "/fixture/bots/vox",
      soul: "",
      soulHash: hash("SOUL.md"),
      memory: files["MEMORY.md"],
      memoryHash: hash("MEMORY.md"),
      memoryPath: "/fixture/bots/vox/MEMORY.md",
      topics: [],
    };
  if (command === "bot_save") {
    if (args.expectedHash !== hash(path)) throw "conflict";
    files[path] = args.text as string;
    localStorage.setItem(key, JSON.stringify(files));
    return hash(path);
  }
  throw new Error(`Unexpected fixture file operation: ${command}`);
}
