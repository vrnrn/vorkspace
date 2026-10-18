// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from "vitest";
import {
  readComposer,
  registerComposerEditor,
  writeComposer,
} from "./composerControl";

const cleanups: Array<() => void> = [];
let sequence = 0;
function editor(id = `composer-control-${++sequence}`, text = "") {
  const field = document.createElement("textarea");
  field.value = text;
  document.body.append(field);
  const write = vi.fn((value: string) => {
    field.value = value;
  });
  const unregister = registerComposerEditor(id, { field, write });
  cleanups.push(() => {
    unregister();
    field.remove();
  });
  return { id, field, write, unregister };
}
afterEach(() => {
  cleanups.splice(0).forEach((clean) => clean());
});

it("fills an unsent composer while preserving existing input by default", async () => {
  const target = editor(undefined, "Keep this draft");
  const result = await writeComposer(target.id, { text: "New request" });
  expect(result).toMatchObject({ updated: true, sent: false });
  expect(target.field.value).toBe("Keep this draft\n\nNew request");
  expect(target.write).toHaveBeenCalledOnce();
  expect(await readComposer(target.id)).toMatchObject({
    text: target.field.value,
    truncated: false,
  });
});

it("replaces only the exact text read and rejects intervening manual edits", async () => {
  const target = editor(undefined, "A😀中");
  const read = await readComposer(target.id);
  target.field.value += " typed later";
  await expect(
    writeComposer(target.id, {
      text: "replacement",
      mode: "replace",
      expectedText: read.text,
    }),
  ).rejects.toThrow("user changed");
  expect(target.write).not.toHaveBeenCalled();
  await expect(
    writeComposer(target.id, { text: "replacement", mode: "replace" }),
  ).rejects.toThrow("Read the composer");
  const fresh = await readComposer(target.id);
  await writeComposer(target.id, {
    text: "replacement",
    mode: "replace",
    expectedText: fresh.text,
  });
  expect(target.field.value).toBe("replacement");
});

it("does not transfer a pending edit to a replacement composer owner", async () => {
  const old = editor();
  const pending = writeComposer(old.id, { text: "Old request" });
  old.unregister();
  const next = editor(old.id, "New owner");
  await expect(pending).rejects.toThrow("composer changed");
  expect(old.write).not.toHaveBeenCalled();
  expect(next.field.value).toBe("New owner");
  old.unregister();
  await writeComposer(next.id, { text: "Still owned by the replacement" });
  expect(next.field.value).toContain("Still owned by the replacement");
});

it("waits for a requested chat to mount and aborts abandoned waits", async () => {
  const id = `late-composer-${++sequence}`;
  const pending = writeComposer(id, { text: "Ready to review" });
  const target = editor(id);
  await pending;
  expect(target.field.value).toBe("Ready to review");
  const abort = new AbortController();
  const stopped = writeComposer(
    `missing-${++sequence}`,
    { text: "Never insert" },
    abort.signal,
  );
  abort.abort();
  await expect(stopped).rejects.toThrow("Request cancelled");
  const other = editor(`missing-${sequence}`);
  expect(other.write).not.toHaveBeenCalled();
});

it("honors disabled fields and rejects ambiguous live owners", async () => {
  const target = editor();
  target.field.disabled = true;
  const abort = new AbortController();
  const pending = writeComposer(target.id, { text: "No" }, abort.signal);
  abort.abort();
  await expect(pending).rejects.toThrow("Request cancelled");
  expect(target.write).not.toHaveBeenCalled();
  target.field.disabled = false;
  const duplicate = editor(target.id);
  await expect(writeComposer(target.id, { text: "Ambiguous" })).rejects.toThrow(
    "More than one composer",
  );
  duplicate.field.focus();
  await writeComposer(target.id, { text: "Focused owner" });
  expect(duplicate.field.value).toBe("Focused owner");
  expect(target.write).not.toHaveBeenCalled();
});
