import { expect, it } from "vitest";
import { extractToolPreview, MAX_PREVIEW_LINES } from "./runtime/protocol/preview.js";

it("retains the first change and preceding context while counting the entire patch", () => {
  const context = Array.from({ length: 30 }, (_, i) => ` line ${i + 1}`);
  const patch = [
    "diff --git a/report.txt b/report.txt",
    "--- a/report.txt",
    "+++ b/report.txt",
    "@@ -1,36 +1,36 @@",
    ...context,
    "-old line",
    "+new line",
    ...Array.from({ length: 8 }, (_, i) => ` more context ${i}`),
    "-last old line",
    "+last new line",
  ].join("\r\n");
  const preview = extractToolPreview({ kind: "edit", content: [{ type: "diff", patch }] }, {});
  expect(preview).toMatchObject({ path: "report.txt", additions: 2, deletions: 2 });
  expect(preview.lines).toHaveLength(MAX_PREVIEW_LINES);
  expect(preview.lines.slice(0, 3)).toEqual([
    { number: 30, kind: "context", text: "line 30" },
    { number: 31, kind: "del", text: "old line" },
    { number: 31, kind: "add", text: "new line" },
  ]);
});

it("keeps the first context lines when replacement text is unchanged", () => {
  const text = Array.from({ length: 30 }, (_, i) => `line ${i + 1}`).join("\n");
  const preview = extractToolPreview({ kind: "edit", rawInput: { oldText: text, newText: text } }, {});
  expect(preview).toMatchObject({ additions: 0, deletions: 0 });
  expect(preview.lines).toEqual(Array.from({ length: MAX_PREVIEW_LINES }, (_, i) => ({
    kind: "context",
    text: `line ${i + 1}`,
  })));
});
