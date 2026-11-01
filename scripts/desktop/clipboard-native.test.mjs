import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import { test } from "node:test";

const run = promisify(execFile);
test("native clipboard preview deadline, cancellation and cleanup", { skip: process.platform !== "darwin" }, async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "vorkspace-native-clipboard-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const source = await readFile(new URL("../../crates/vorkspace-clipboard/native/Clipboard.swift", import.meta.url), "utf8");
  const tests = await readFile(new URL("../../crates/vorkspace-clipboard/native/ClipboardTests.swift", import.meta.url), "utf8");
  const file = join(directory, "ClipboardTests.swift");
  const binary = join(directory, "ClipboardTests");
  await writeFile(file, `${source}\n${tests}`);
  await run("xcrun", ["swiftc", "-parse-as-library", file, "-o", binary]);
  const { stdout } = await run(binary, []);
  assert.match(stdout, /Native clipboard preview lifecycle passed/);
});
