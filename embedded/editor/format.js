// Prettier resolves embedded languages through the same lazy plugin catalog.
import { formatWithCursor } from "prettier/standalone";
import catalog from "native-format-plugins";

const plugins = catalog.map(({ name, parsers, printers, ...metadata }) => {
  let plugin;
  const load = () => (plugin ??= globalThis.vorkspaceLoadFormatPlugin(name));
  return {
    ...metadata,
    parsers: Object.fromEntries(
      parsers.map((parser) => [parser, () => load().parsers[parser]]),
    ),
    printers: Object.fromEntries(
      printers.map((printer) => [printer, () => load().printers[printer]]),
    ),
  };
});

globalThis.vorkspaceFormat = async (path, source, cursorOffset, parser) => {
  try {
    return JSON.stringify(
      await formatWithCursor(source, {
        filepath: path,
        parser,
        cursorOffset,
        plugins,
      }),
    );
  } catch {
    return "null";
  }
};
