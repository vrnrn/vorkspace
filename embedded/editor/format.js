// Embed Prettier parsers and native editor formatting defaults.
import { formatWithCursor } from "prettier/standalone";
import * as estree from "prettier/plugins/estree";
import * as babel from "prettier/plugins/babel";
import * as typescript from "prettier/plugins/typescript";
import * as postcss from "prettier/plugins/postcss";
import * as html from "prettier/plugins/html";
import * as markdown from "prettier/plugins/markdown";
globalThis.vorkspaceFormat = async (path, source, cursorOffset, parser) => {
  try {
    return JSON.stringify(
      await formatWithCursor(source, {
        filepath: path,
        parser,
        cursorOffset,
        plugins: [estree, babel, typescript, postcss, html, markdown],
      }),
    );
  } catch {
    return "null";
  }
};
