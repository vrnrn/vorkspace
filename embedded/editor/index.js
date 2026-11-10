// The editor's parsers, completions and folding run without a DOM
// in a bounded QuickJS worker. All offsets at this boundary are UTF-16.
import { EditorState, Text } from "@codemirror/state";
import { ensureSyntaxTree, foldable, syntaxTree } from "@codemirror/language";
import { CompletionContext, completeFromList } from "@codemirror/autocomplete";
import { highlightTree } from "@lezer/highlight";
import {
  languageForPath,
  syntaxTagHighlighter,
} from "./editorLanguage.js";
import { Chunk } from "@codemirror/merge";
import {
  isLintable,
  syntaxDiagnostics,
} from "./editorLint.js";
import {
  stageChunkText,
  revertChunkText,
} from "./editorGit.js";
import { emmetExtension, tryExpandEmmet } from "./editorEditing.js";
async function stateFor(path, source, pos = 0, fullParse = false) {
  const language = (await languageForPath(path)) ?? [];
  const state = EditorState.create({
    doc: source,
    selection: { anchor: pos },
    extensions: [language, emmetExtension(path)],
  });
  return parsedState(state, fullParse);
}
function parsedState(state, fullParse) {
  // Analysis has no EditorView/idle worker to finish a partial viewport tree.
  // Finish it here, on the native background worker, whose interpreter enforces
  // the overall time and memory limits. Refresh the state's tree afterwards.
  ensureSyntaxTree(state, state.doc.length, fullParse ? Infinity : 100);
  return state.update({}).state;
}
function originalState(state, source, original) {
  // Reuse the parsed current document for the diff baseline. Rebuilding both
  // trees from scratch doubles large-file work even for a one-line change.
  let from = 0,
    to = source.length,
    end = original.length;
  while (
    from < to &&
    from < end &&
    source.charCodeAt(from) === original.charCodeAt(from)
  )
    from++;
  while (
    to > from &&
    end > from &&
    source.charCodeAt(to - 1) === original.charCodeAt(end - 1)
  ) {
    to--;
    end--;
  }
  return parsedState(
    state.update({ changes: { from, to, insert: original.slice(from, end) } })
      .state,
    true,
  );
}
globalThis.vorkspaceAnalyze = async (path, source, original) => {
  const state = await stateFor(path, source, 0, true);
  const tokensFor = (state) => {
    const tokens = [];
    highlightTree(syntaxTree(state), syntaxTagHighlighter, (from, to, kind) =>
      tokens.push({ from, to, kind }),
    );
    return tokens;
  };
  const changes = [];
  if (original != null && original !== source) {
    const baseline = Text.of(original.split("\n"));
    const chunks = Chunk.build(baseline, state.doc, {
      scanLimit: 5_000,
      timeout: 100,
    });
    const originalTokens = chunks.length
      ? tokensFor(originalState(state, source, original))
      : [];
    const lineAt = (offset) =>
      baseline.lineAt(Math.min(offset, baseline.length)).number;
    let first = 0;
    for (const chunk of chunks) {
      const { fromA, toA, fromB, toB, endB } = chunk;
      const old = original.slice(fromA, toA);
      const oldLine = lineAt(fromA);
      const oldEnd = fromA + old.length - Number(old.endsWith("\n"));
      // Both streams are in document order. Keep a boundary-spanning token
      // available for the next chunk, without revisiting preceding tokens.
      while (originalTokens[first]?.to <= fromA) first++;
      const tokens = [];
      for (let i = first; originalTokens[i]?.from < toA; i++) {
        const { from, to, kind } = originalTokens[i];
        tokens.push({
          from: Math.max(from, fromA) - fromA,
          to: Math.min(to, toA) - fromA,
          kind,
        });
      }
      changes.push({
        kind: fromA !== toA ? (fromB !== toB ? "mod" : "del") : "add",
        from: Math.min(fromB, source.length),
        to: Math.min(endB, source.length),
        original: old,
        old_line: oldLine,
        tokens,
        deleted: lineAt(oldEnd) - oldLine + 1,
      });
    }
  }
  const folds = [];
  for (let n = 1; n <= state.doc.lines; n++) {
    const line = state.doc.line(n);
    const range = foldable(state, line.from, line.to);
    if (range && state.doc.lineAt(range.to).number > n) folds.push(range);
  }
  const tokens = tokensFor(state);
  return JSON.stringify({
    diagnostics: isLintable(path) ? syntaxDiagnostics(state) : [],
    changes,
    folds,
    tokens,
  });
};
globalThis.vorkspaceComplete = async (path, source, pos, explicit) => {
  const state = await stateFor(path, source, pos);
  const context = new CompletionContext(state, pos, explicit);
  const results = await Promise.all(
    state
      .languageDataAt("autocomplete", pos)
      .map((data) =>
        (Array.isArray(data) ? completeFromList(data) : data)(context),
      ),
  );
  const options = [];
  for (const result of results) {
    if (!result) continue;
    const prefix = source.slice(result.from, pos).toLowerCase();
    for (const option of result.options) {
      if (!option.label.toLowerCase().startsWith(prefix)) continue;
      let insert =
        typeof option.apply === "string" ? option.apply : option.label;
      let cursor = insert.length;
      // CodeMirror snippet applications only need state and dispatch to expand.
      if (typeof option.apply === "function") {
        try {
          option.apply(
            {
              state,
              dispatch: (spec) => {
                const tr = state.update(spec);
                insert = tr.newDoc.sliceString(
                  result.from,
                  tr.newDoc.length - (source.length - (result.to ?? pos)),
                );
                cursor = tr.newSelection.main.head - result.from;
              },
            },
            option,
            result.from,
            result.to ?? pos,
          );
        } catch {
          /* ordinary labels remain usable for view-dependent sources */
        }
      }
      options.push({
        label: option.label,
        detail: option.detail ?? "",
        kind: option.type ?? "",
        from: result.from,
        to: result.to ?? pos,
        insert,
        cursor,
      });
    }
  }
  return JSON.stringify(
    options.sort((a, b) => a.label.localeCompare(b.label)).slice(0, 100),
  );
};
globalThis.vorkspaceEmmet = async (path, source, pos) => {
  const state = await stateFor(path, source, pos);
  let result = null;
  tryExpandEmmet({
    state,
    dispatch: (spec) => {
      const tr = state.update(spec);
      result = {
        formatted: tr.newDoc.toString(),
        cursorOffset: tr.newSelection.main.head,
      };
    },
  });
  return JSON.stringify(result);
};
globalThis.vorkspaceChunkAction = (original, current, pos, start, end, stage) =>
  JSON.stringify(
    (stage ? stageChunkText : revertChunkText)(
      original,
      current,
      pos,
      start === end ? null : { from: start, to: end },
    ),
  );
