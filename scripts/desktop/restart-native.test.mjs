import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import { access, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { resolveNativeEnv } from "./build-native.mjs";

test("restart waits for process exit and preserves arguments without inheriting the runner hook", { timeout: 90_000 }, async (t) => {
  const root = await mkdtemp(join(tmpdir(), "vorkspace-restart-"));
  let parent;
  t.after(async () => {
    if (parent && parent.exitCode === null && parent.signalCode === null) {
      const exited = once(parent, "close");
      parent.kill();
      await exited;
    }
    await rm(root, { recursive: true, force: true });
  });

  // Compile the actual std-only restart helper with release-style assertions.
  // This never builds, installs, opens, or restarts the desktop application.
  const source = fileURLToPath(new URL("../../crates/vorkspace-macos/src/restart.rs", import.meta.url));
  const harness = join(root, "restart.rs");
  const executable = join(root, "restart");
  await writeFile(harness, `
    #[allow(dead_code)]
    #[path = ${JSON.stringify(source)}] mod restart;
    fn main() {
        let mut args = std::env::args_os().skip(1);
        let pid = args.next().unwrap().to_str().unwrap().parse().unwrap();
        let exe = std::path::PathBuf::from(args.next().unwrap());
        restart::wait_and_relaunch(pid, &exe, &args.collect::<Vec<_>>()).unwrap();
    }
  `);
  const compiled = spawnSync("rustc", ["--edition", "2024", "-C", "debug-assertions=no", harness, "-o", executable], {
    env: resolveNativeEnv(), encoding: "utf8", timeout: 60_000,
  });
  assert.ifError(compiled.error);
  assert.equal(compiled.status, 0, compiled.stdout + compiled.stderr);

  const state = join(root, "state");
  const receipt = join(root, "receipt.json");
  const old = join(root, "old.mjs");
  const next = join(root, "successor's program with spaces.mjs");
  await writeFile(old, `
    import { writeFileSync } from "node:fs";
    writeFileSync(process.argv[2], "old");
    process.stdout.write("ready");
    process.stdin.once("data", () => {
      writeFileSync(process.argv[2], "flushed");
      process.exit(0);
    });
  `);
  await writeFile(next, `
    import { readFileSync, writeFileSync } from "node:fs";
    writeFileSync(process.argv[2], JSON.stringify({
      args: process.argv.slice(4),
      state: readFileSync(process.argv[3], "utf8"),
      reload: process.env.VORKSPACE_DEV_RELOAD ?? null,
    }));
  `);
  parent = spawn(process.execPath, [old, state], { stdio: ["pipe", "pipe", "inherit"] });
  await once(parent.stdout, "data");
  const args = ["", "a project with spaces", "a'b", 'say "hello"', "trailing\\", "a\\\"b", "雪", "$(echo bad); & | %PATH%", "line\nbreak"];
  const scheduled = spawnSync(executable, [String(parent.pid), process.execPath, next, receipt, state, ...args], {
    env: { ...process.env, VORKSPACE_DEV_RELOAD: "1" }, encoding: "utf8", timeout: 10_000,
  });
  assert.ifError(scheduled.error);
  assert.equal(scheduled.status, 0, scheduled.stderr);

  // Keep the old process alive beyond the former 150ms delay. Its explicit stdin
  // gate, rather than a timer, controls the final state write and process exit.
  await delay(350);
  assert.equal(parent.exitCode, null);
  await assert.rejects(access(receipt));
  assert.equal(await readFile(state, "utf8"), "old");
  const exited = once(parent, "close");
  parent.stdin.end("finish\n");
  await exited;
  const deadline = Date.now() + 10_000;
  while (await access(receipt).then(() => false, () => true)) {
    assert.ok(Date.now() < deadline, "successor did not run after the old process exited");
    await delay(20);
  }
  assert.deepEqual(JSON.parse(await readFile(receipt, "utf8")), { args, state: "flushed", reload: null });
});
