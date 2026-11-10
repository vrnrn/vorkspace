// Search uses the same CodeMirror query and UTF-16 positions as the editor.
import { EditorState } from "@codemirror/state";
import { SearchQuery } from "@codemirror/search";

globalThis.vorkspaceSearch = (
  source,
  query,
  replacement,
  caseSensitive,
  wholeWord,
  regexp,
) => {
  const state = EditorState.create({ doc: source });
  const search = new SearchQuery({
    search: query,
    replace: replacement,
    caseSensitive,
    wholeWord,
    regexp,
    literal: true,
  });
  if (!search.valid) return JSON.stringify({ matches: [], invalid: !!query });
  const cursor = search.getCursor(state);
  const operation = search.create();
  const matches = [];
  while (!cursor.next().done) {
    const { from, to } = cursor.value;
    if (to > from)
      matches.push({
        from,
        to,
        insert: operation.getReplacement(cursor.value),
      });
  }
  return JSON.stringify({ matches, invalid: false });
};
