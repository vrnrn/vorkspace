import { nativeCommandInvocation } from "../../protocol/nativeCommands.js";
import { asRecord } from "./piProtocol.js";
export function ompCommandsFromRpcData(data) {
  const commands = asRecord(data)?.commands;
  if (!Array.isArray(commands))
    throw new Error("OMP get_available_commands returned no commands array");
  const seen = new Set();
  return commands.flatMap((value) => {
    const row = asRecord(value);
    const name = row?.name;
    if (
      typeof name !== "string" ||
      !name ||
      /[\s/\\]/.test(name) ||
      seen.has(name)
    )
      return [];
    seen.add(name);
    const aliases = Array.isArray(row?.aliases)
      ? row.aliases.filter(
          (alias) =>
            typeof alias === "string" && !!alias && !/[\s/\\]/.test(alias),
        )
      : [];
    const hint = asRecord(row?.input)?.hint;
    const subcommands = Array.isArray(row?.subcommands)
      ? row.subcommands.flatMap((value) => {
          const sub = asRecord(value);
          if (typeof sub?.name !== "string" || !sub.name) return [];
          return [
            {
              name: sub.name,
              ...(typeof sub.description === "string"
                ? { description: sub.description }
                : {}),
              ...(typeof sub.usage === "string" ? { usage: sub.usage } : {}),
            },
          ];
        })
      : [];
    return [
      {
        name,
        invocation: nativeCommandInvocation("omp", name),
        source: "omp",
        description:
          typeof row?.description === "string" ? row.description : "",
        ...(typeof row?.source === "string" ? { origin: row.source } : {}),
        ...(aliases.length ? { aliases } : {}),
        ...(typeof hint === "string" ? { inputHint: hint } : {}),
        ...(subcommands.length ? { subcommands } : {}),
      },
    ];
  });
}
