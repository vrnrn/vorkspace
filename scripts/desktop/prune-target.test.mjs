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
  }
  // Libraries are shared by every build and always stay.
  file(join(deps, "libserde-1111111111111111.rlib"), 99);
  file(join(deps, "libserde-2222222222222222.rlib"), 98);
  file(join(deps, "libserde-3333333333333333.rlib"), 97);
  for (const [hash, age] of [["0000000000000001", 5], ["0000000000000002", 6], ["0000000000000003", 7]]) {
    dir(join(incremental, `vorkspace_gpui-${hash}`), age);
  }
  dir(join(incremental, "vorkspace_ui-0000000000000009"), 50);

  const removed = pruneTarget(target);

  assert.ok(existsSync(join(deps, "vorkspace_gpui-aaaaaaaaaaaaaaaa")));
  assert.ok(existsSync(join(deps, "vorkspace_gpui-bbbbbbbbbbbbbbbb")));
  assert.ok(!existsSync(join(deps, "vorkspace_gpui-cccccccccccccccc")));
  assert.ok(!existsSync(join(deps, "vorkspace_gpui-cccccccccccccccc.d")), "its dep-info goes too");
  for (const hash of ["1111111111111111", "2222222222222222", "3333333333333333"]) {
    assert.ok(existsSync(join(deps, `libserde-${hash}.rlib`)));
  }
  assert.ok(existsSync(join(incremental, "vorkspace_gpui-0000000000000001")));
  assert.ok(existsSync(join(incremental, "vorkspace_gpui-0000000000000002")));
  assert.ok(!existsSync(join(incremental, "vorkspace_gpui-0000000000000003")));
  assert.ok(existsSync(join(incremental, "vorkspace_ui-0000000000000009")), "a lone cache stays");
  assert.equal(removed.length, 2);
});

test("a missing or empty target is left alone", () => {
  assert.deepEqual(pruneTarget(join(tmpdir(), "vorkspace-prune-missing")), []);
});
