import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(new URL("./seed-worktree.sh", import.meta.url));
const git = (cwd, ...args) => {
  const result = spawnSync("git", args, { cwd, encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
};
const seed = (cwd) => spawnSync("bash", [script], { cwd, encoding: "utf8" });

test("a linked worktree clones the main build output once and never replaces its own", (t) => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "vorkspace-seed-")));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const main = join(root, "main");
  const linked = join(root, "linked");
  mkdirSync(main);
  git(main, "init", "-q");
  git(main, "-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "--allow-empty", "-m", "start");
  git(main, "worktree", "add", "-q", "--detach", linked);

  assert.match(seed(linked).stderr, /build there first/);
  mkdirSync(join(main, "target"));
  writeFileSync(join(main, "target/built"), "artifact");
  assert.match(seed(main).stderr, /linked worktree/);
  assert.equal(seed(linked).status, 0);
  assert.ok(existsSync(join(linked, "target/built")));
  assert.match(seed(linked).stderr, /leaving it alone/);
});
