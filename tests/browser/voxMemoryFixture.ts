/** The browser fixtures use the real memory UI/logic against isolated fake files. */
export function voxMemoryFixture(
  command: string,
  args?: Record<string, unknown>,
) {
  if (!command.startsWith("mono_")) return undefined;
  if (args?.mono !== "vox") throw new Error("Unexpected fixture memory owner");
  const key = "vox.fixture.memory";
  const files: Record<string, string> = JSON.parse(
    localStorage.getItem(key) ?? '{"MEMORY.md":"","SOUL.md":""}',
  );
  const path = args.path as string;
  const hash = (name: string) => `fixture:${files[name] ?? ""}`;
  if (command === "mono_read")
    return { text: files[path] ?? null, hash: hash(path) };
  if (command === "mono_load")
    return {
      id: "vox",
      dir: "/fixture/monos/vox",
      soul: "",
      soulHash: hash("SOUL.md"),
      memory: files["MEMORY.md"],
      memoryHash: hash("MEMORY.md"),
      memoryPath: "/fixture/monos/vox/MEMORY.md",
      topics: [],
    };
  if (command === "mono_save") {
    if (args.expectedHash !== hash(path)) throw "conflict";
    files[path] = args.text as string;
    localStorage.setItem(key, JSON.stringify(files));
    return hash(path);
  }
  throw new Error(`Unexpected fixture file operation: ${command}`);
}
