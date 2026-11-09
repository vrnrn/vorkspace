import assert from "node:assert/strict";
import { existsSync, mkdirSync, mkdtempSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { pruneTarget } from "./prune-target.mjs";

function file(path, age) {
  writeFileSync(path, "");
  const at = new Date(Date.now() - age * 1000);
  utimesSync(path, at, at);
}

function dir(path, age) {
  mkdirSync(path, { recursive: true });
  const at = new Date(Date.now() - age * 1000);
  utimesSync(path, at, at);
}

test("keeps the newest outputs per name and every library", (t) => {
  const target = mkdtempSync(join(tmpdir(), "vorkspace-prune-"));
  t.after(() => rmSync(target, { recursive: true, force: true }));
  const deps = join(target, "native-dev", "deps");
  const incremental = join(target, "native-dev", "incremental");
  mkdirSync(deps, { recursive: true });
  mkdirSync(incremental, { recursive: true });
  // Three linked test binaries of one crate, newest first by age.
  for (const [hash, age] of [["aaaaaaaaaaaaaaaa", 10], ["bbbbbbbbbbbbbbbb", 20], ["cccccccccccccccc", 30]]) {
    file(join(deps, `vorkspace_gpui-${hash}`), age);
    file(join(deps, `vorkspace_gpui-${hash}.d`), age);
    file(join(deps, `vorkspace_gpui-${hash}.01rzebpm.rcgu.o`), age);
  }
  // Libraries are shared by every build and always stay.
  file(join(deps, "libserde-1111111111111111.rlib"), 99);
  file(join(deps, "libserde-2222222222222222.rlib"), 98);
  file(join(deps, "libserde-3333333333333333.rlib"), 97);
  // rustc incremental directories use 13-character base-36 suffixes.
  for (const [hash, age] of [["0lf6r14n965nd", 5], ["2x07w7tarx0df", 6], ["0h32cv21lt6eb", 7]]) {
    dir(join(incremental, `vorkspace_gpui-${hash}`), age);
  }
  dir(join(incremental, "vorkspace_ui-3li0crdfhhgsu"), 50);

  const removed = pruneTarget(target);

  assert.ok(existsSync(join(deps, "vorkspace_gpui-aaaaaaaaaaaaaaaa")));
  assert.ok(existsSync(join(deps, "vorkspace_gpui-bbbbbbbbbbbbbbbb")));
  assert.ok(!existsSync(join(deps, "vorkspace_gpui-cccccccccccccccc")));
  assert.ok(!existsSync(join(deps, "vorkspace_gpui-cccccccccccccccc.d")), "its dep-info goes too");
  assert.ok(!existsSync(join(deps, "vorkspace_gpui-cccccccccccccccc.01rzebpm.rcgu.o")), "its unpacked debug object goes too");
  for (const hash of ["1111111111111111", "2222222222222222", "3333333333333333"]) {
    assert.ok(existsSync(join(deps, `libserde-${hash}.rlib`)));
  }
  assert.ok(existsSync(join(incremental, "vorkspace_gpui-0lf6r14n965nd")));
  assert.ok(existsSync(join(incremental, "vorkspace_gpui-2x07w7tarx0df")));
  assert.ok(!existsSync(join(incremental, "vorkspace_gpui-0h32cv21lt6eb")));
  assert.ok(existsSync(join(incremental, "vorkspace_ui-3li0crdfhhgsu")), "a lone cache stays");
  assert.equal(removed.length, 2);
});

test("traverses target-triple, sub-target, and examples directories", (t) => {
  const target = mkdtempSync(join(tmpdir(), "vorkspace-prune-nested-"));
  t.after(() => rmSync(target, { recursive: true, force: true }));
  const tripleDeps = join(target, "aarch64-apple-darwin", "verify", "deps");
  const tripleExamples = join(target, "aarch64-apple-darwin", "verify", "examples");
  const tripleIncremental = join(target, "aarch64-apple-darwin", "native-dev", "incremental");
  const blockDeps = join(target, "block", "debug", "deps");
  mkdirSync(tripleDeps, { recursive: true });
  mkdirSync(tripleExamples, { recursive: true });
  mkdirSync(tripleIncremental, { recursive: true });
  mkdirSync(blockDeps, { recursive: true });

  for (const [hash, age] of [["1111111111111111", 10], ["2222222222222222", 20], ["3333333333333333", 30]]) {
    file(join(tripleDeps, `vorkspace_core-${hash}`), age);
    file(join(tripleExamples, `provider_smoke-${hash}`), age);
    file(join(blockDeps, `block-${hash}`), age);
  }
  for (const [hash, age] of [["09ze2xrwekt94", 10], ["0nbryzi8bk37j", 20], ["0v2ac2y2t6j8p", 30]]) {
    dir(join(tripleIncremental, `vorkspace_core-${hash}`), age);
  }

  const removed = pruneTarget(target);

  assert.equal(removed.length, 4);
  assert.ok(!existsSync(join(tripleDeps, "vorkspace_core-3333333333333333")));
  assert.ok(!existsSync(join(tripleExamples, "provider_smoke-3333333333333333")));
  assert.ok(!existsSync(join(blockDeps, "block-3333333333333333")));
  assert.ok(!existsSync(join(tripleIncremental, "vorkspace_core-0v2ac2y2t6j8p")));
});

test("a missing or empty target is left alone", () => {
  assert.deepEqual(pruneTarget(join(tmpdir(), "vorkspace-prune-missing")), []);
});
