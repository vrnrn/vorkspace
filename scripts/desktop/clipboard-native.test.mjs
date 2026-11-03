import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const run = promisify(execFile);
test("native clipboard preview deadline, cancellation and cleanup", { skip: process.platform !== "darwin" }, async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "vorkspace-native-clipboard-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const source = await readFile(new URL("../../crates/vorkspace-helper/native/Clipboard.swift", import.meta.url), "utf8");
  const tests = await readFile(new URL("../../crates/vorkspace-helper/native/ClipboardTests.swift", import.meta.url), "utf8");
  const file = join(directory, "ClipboardTests.swift");
  const binary = join(directory, "ClipboardTests");
  await writeFile(file, `${source}\n${tests}`);
  await run("xcrun", ["swiftc", "-parse-as-library", "-module-cache-path", join(directory, "module-cache"), file, "-o", binary]);
  const { stdout } = await run(binary, []);
  assert.match(stdout, /Native clipboard preview lifecycle passed/);
});

test("native dictation custom words, volatile phrases and Unicode edits", { skip: process.platform !== "darwin" }, async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "vorkspace-native-dictation-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const binary = join(directory, "DictationTests");
  const sources = ["Dictation.swift", "DictationKey.swift", "DictationWords.swift", "DictationTarget.swift", "SpeechCapture.swift", "Microphones.swift", "DictationTests.swift"]
    .map(name => fileURLToPath(new URL(`../../crates/vorkspace-helper/native/${name}`, import.meta.url)));
  await run("xcrun", ["swiftc", "-warnings-as-errors", "-parse-as-library", "-module-cache-path", join(directory, "module-cache"),
    "-target", `${process.arch === "arm64" ? "arm64" : "x86_64"}-apple-macos26.0`, ...sources, "-o", binary]);
  const { stdout } = await run(binary, []);
  assert.match(stdout, /Native dictation words, phrase replacement and Unicode edit regressions passed/);
});
