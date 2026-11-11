// Keep CodeMirror's rope, normalization and cursor engines without its view.
// Whole-word and replacement policy match SearchQuery's literal mode, checked
// against the installed SearchQuery when the assets are rebuilt.
import { CharCategory, EditorState, findClusterBreak } from "@codemirror/state";
import { RegExpCursor, SearchCursor } from "@codemirror/search";

function charBefore(text, at) {
  return text.slice(findClusterBreak(text, at, false), at);
}

function charAfter(text, at) {
  return text.slice(at, findClusterBreak(text, at));
}

function wordBoundary(text, from, to, category) {
  return (
    (category(charBefore(text, from)) !== CharCategory.Word ||
      category(charAfter(text, from)) !== CharCategory.Word) &&
    (category(charAfter(text, to)) !== CharCategory.Word ||
      category(charBefore(text, to)) !== CharCategory.Word)
  );
}

function replaceCaptures(replacement, match) {
  return replacement.replace(/\$([$&]|\d+)/g, (literal, capture) => {
    if (capture === "&") return match[0];
    if (capture === "$") return "$";
    for (let length = capture.length; length > 0; length--) {
      const index = Number(capture.slice(0, length));
      if (index > 0 && index < match.length)
        return match[index] + capture.slice(length);
    }
    return literal;
  });
}

globalThis.vorkspaceSearch = (
  source,
  query,
  replacement,
  caseSensitive,
  wholeWord,
  regexp,
) => {
  if (!query) return JSON.stringify({ matches: [], invalid: false });
  if (regexp) {
    try {
      new RegExp(query, "gmu");
    } catch {
      return JSON.stringify({ matches: [], invalid: true });
    }
  }
  const state = EditorState.create({ doc: source });
  let test;
  if (wholeWord) {
    const category = state.charCategorizer(state.selection.main.head);
    test = (from, to, text, offset) => {
      if (regexp)
        return (
          !text[0].length ||
          wordBoundary(
            text.input,
            text.index,
            text.index + text[0].length,
            category,
          )
        );
      if (offset > from || offset + text.length < to) {
        offset = Math.max(0, from - 2);
        text = state.doc.sliceString(
          offset,
          Math.min(state.doc.length, to + 2),
        );
      }
      return wordBoundary(text, from - offset, to - offset, category);
    };
  }
  const cursor = regexp
    ? new RegExpCursor(state.doc, query, { ignoreCase: !caseSensitive, test })
    : new SearchCursor(
        state.doc,
        query,
        0,
        state.doc.length,
        caseSensitive ? undefined : (text) => text.toLowerCase(),
        test,
      );
  const matches = [];
  while (!cursor.next().done) {
    const { from, to } = cursor.value;
    if (to > from)
      matches.push({
        from,
        to,
        insert: regexp
          ? replaceCaptures(replacement, cursor.value.match)
          : replacement,
      });
  }
  return JSON.stringify({ matches, invalid: false });
};
