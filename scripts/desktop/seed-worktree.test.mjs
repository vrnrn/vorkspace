import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { resolveNativeEnv } from "./dev-native.mjs";

const script = fileURLToPath(new URL("./seed-worktree.sh", import.meta.url));
const git = (cwd, ...args) => {
  const result = spawnSync("git", args, { cwd, encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
};
function checkouts(t) {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "vorkspace-seed-")));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const main = join(root, "main");
  const linked = join(root, "linked");
  mkdirSync(join(main, "src"), { recursive: true });
  writeFileSync(join(main, "Cargo.toml"), '[package]\nname = "probe"\nversion = "0.0.0"\nedition = "2024"\n');
  writeFileSync(join(main, "src/lib.rs"), "");
  git(main, "init", "-q");
  git(main, "add", ".");
  git(main, "-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "-m", "start");
  git(main, "worktree", "add", "-q", "--detach", linked);
  const seed = (cwd, env = {}) => {
    const base = { ...resolveNativeEnv(), ...env };
    if (!env.CARGO_TARGET_DIR) delete base.CARGO_TARGET_DIR;
    return spawnSync("bash", [script], { cwd, encoding: "utf8", env: base });
  };
  return { root, main, linked, seed };
}

test("a linked worktree clones the main build output once and never replaces its own", (t) => {
  const { main, linked, seed } = checkouts(t);
  assert.match(seed(linked).stderr, /build there first/);
  mkdirSync(join(main, "target"));
  writeFileSync(join(main, "target/built"), "artifact");
  assert.match(seed(main).stderr, /linked worktree/);
  assert.equal(seed(linked).status, 0);
  assert.ok(existsSync(join(linked, "target/built")));
  assert.match(seed(linked).stderr, /leaving it alone/);
});

test("seeding follows Cargo's configured target directory", (t) => {
  const { main, linked, seed } = checkouts(t);
  const env = { CARGO_TARGET_DIR: "custom-cache" };
  mkdirSync(join(main, "custom-cache"));
  writeFileSync(join(main, "custom-cache/built"), "artifact");
  mkdirSync(join(main, "target"));
  writeFileSync(join(main, "target/stale"), "unused");
  assert.equal(seed(linked, env).status, 0);
  assert.ok(existsSync(join(linked, "custom-cache/built")));
  assert.equal(existsSync(join(linked, "target")), false);
});

test("checkouts sharing one absolute target directory have nothing to clone", (t) => {
  const { root, linked, seed } = checkouts(t);
  const result = seed(linked, { CARGO_TARGET_DIR: join(root, "shared") });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /already share/);
});
