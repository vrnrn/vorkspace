import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { appendFile, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { test } from "node:test";
import { parseDevArgs, startDev } from "./dev-native.mjs";

test("forwards app options and rejects the retired runner flags", () => {
  assert.deepEqual(parseDevArgs([]), { args: [], profile: "release" });
  assert.deepEqual(parseDevArgs(["--vorkspace-data", "--cwd", "a project"]), {
    args: ["--vorkspace-data", "--cwd", "a project"],
    profile: "release",
  });
  assert.deepEqual(parseDevArgs(["--native-dev", "--cwd", "a project"]), {
    args: ["--cwd", "a project"],
    profile: "native-dev",
  });
  assert.deepEqual(parseDevArgs(["--dev-data", "--cwd", "--watch", "--vorkspace-db", "--bundle"]), {
    args: ["--dev-data", "--cwd", "--watch", "--vorkspace-db", "--bundle"],
    profile: "release",
  });
  assert.deepEqual(parseDevArgs(["--", "--cwd", "a project"]), {
    args: ["--cwd", "a project"],
    profile: "release",
  });
  for (const arg of ["--watch", "--bundle"]) {
    assert.throws(() => parseDevArgs([arg]), /not supported.*rebuild and restart/);
  }
});

async function fixture(t, env = {}, options = {}) {
  const root = await realpath(await mkdtemp(join(tmpdir(), "vorkspace-dev-test-")));
  const messages = [];
  const children = [];
  const calls = [];
  const installs = [];
  let dev;
  // Register cleanup before setup so failed assertions never leave child processes.
  t.after(async () => {
    await dev?.close();
    for (const child of children) if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
    await rm(root, { recursive: true, force: true });
  });
  await mkdir(join(root, "crates/app/src"), { recursive: true });
  await writeFile(join(root, "crates/app/src/main.rs"), "initial");
  const helper = join(root, "fake-process.mjs");
  await writeFile(helper, `
    import { appendFileSync, existsSync } from "node:fs";
    import { join } from "node:path";
    const root = process.cwd();
    const event = (name) => appendFileSync(join(root, "events"), name + "\\n");
    if (process.argv[2] === "metadata") {
      console.log(JSON.stringify({ target_directory: join(root, "custom-target"), packages: [{name: "vorkspace-gpui", version: "0.15.0"}] }));
    } else if (process.argv[2] === "build") {
      event("build");
      await new Promise((resolve) => setTimeout(resolve, Number(process.env.TEST_BUILD_DELAY || 30)));
      if (existsSync(join(root, "fail-build"))) { event("failed"); process.exit(1); }
      event("built");
    } else if (process.argv[2] === "package") {
      event("package");
      if (existsSync(join(root, "fail-package"))) { event("package-failed"); process.exit(1); }
    } else {
      event("start:" + JSON.stringify(process.argv.slice(3)));
      process.stdin.once("data", () => { event("quit"); process.exit(0); });
      process.stdin.once("end", () => process.exit(0));
      process.stdin.resume();
    }
  `);
  dev = await startDev({
    root,
    args: ["--dev-data", "--cwd", "a project with spaces"],
    debounce: 40,
    installBundle: (source) => { installs.push(source); return false; },
    ...options,
    env: { ...process.env, ...env },
    log: (message) => messages.push(message),
    spawnProcess(command, args, options) {
      calls.push({ command, args, options });
      const task = command === "cargo" ? args[0] : command === "bash" ? "package" : "app";
      const child = spawn(process.execPath, [helper, task, ...(task === "app" ? args : [])], {
        ...options,
        stdio: command === "cargo" && args[0] === "metadata" ? ["ignore", "pipe", "pipe"] : ["pipe", "pipe", "pipe"],
      });
      children.push(child);
      return child;
    },
  });
  const events = async () => (await readFile(join(root, "events"), "utf8").catch(() => "")).trim().split("\n").filter(Boolean);
  const until = async (predicate) => {
    const deadline = Date.now() + 10_000;
    while (Date.now() < deadline) {
      if (await predicate(await events())) return;
      await delay(20);
    }
    assert.fail(`Timed out. Events: ${JSON.stringify(await events())}. Logs: ${messages.join("; ")}`);
  };
  const starts = (events) => events.filter((event) => event.startsWith("start:")).length;
  return { root, dev, calls, installs, messages, events, until, starts };
}

test("file saves never rebuild, including edits during a requested build", { timeout: 20_000 }, async (t) => {
  const f = await fixture(t, { TEST_BUILD_DELAY: "250" });
  await f.until((events) => f.starts(events) === 1);
  for (let edit = 0; edit < 3; edit++) {
    await appendFile(join(f.root, "crates/app/src/main.rs"), `edit ${edit}`);
    await delay(75);
  }
  await delay(600);
  assert.equal((await f.events()).filter((event) => event === "build").length, 1);

  f.dev.requestBuild();
  await f.until((events) => events.filter((event) => event === "build").length === 2);
  await appendFile(join(f.root, "crates/app/src/main.rs"), "edit during manual build");
  await f.until((events) => f.starts(events) === 2);
  await delay(600);
  const events = await f.events();
  assert.equal(events.filter((event) => event === "build").length, 2);
  assert.equal(events.filter((event) => event === "quit").length, 1);
});

test("manual restart keeps the app on a failed build and shuts it down after a successful retry", { timeout: 20_000 }, async (t) => {
  const f = await fixture(t);
  await f.until((events) => f.starts(events) === 1);
  assert.deepEqual(f.calls.find((call) => call.args[0] === "build").args, ["build", "--manifest-path", "Cargo.toml", "--locked", "-p", "vorkspace-gpui", "--bin", "vorkspace-gpui", "--release"]);
  const launch = f.calls.find((call) => !["cargo", "bash"].includes(call.command));
  assert.ok(launch.command.startsWith(join(f.root, "custom-target")));
  assert.equal(launch.options.env.VORKSPACE_DEV_RELOAD, "1");
  assert.deepEqual(launch.args, ["--dev-data", "--cwd", "a project with spaces"]);

  await writeFile(join(f.root, "fail-build"), "");
  f.dev.requestBuild();
  await f.until((events) => events.includes("failed"));
  assert.equal(f.starts(await f.events()), 1);
  assert.equal((await f.events()).includes("quit"), false);

  await rm(join(f.root, "fail-build"));
  f.dev.requestBuild();
  await f.until((events) => f.starts(events) === 2);
  const events = await f.events();
  assert.ok(events.lastIndexOf("built") < events.indexOf("quit"), events.join(", "));
  assert.ok(events.indexOf("quit") < events.findLastIndex((event) => event.startsWith("start:")));
  await f.dev.close();
  assert.equal((await f.events()).filter((event) => event === "quit").length, 2);
});

test("coalesces restart requests during a build without launching stale code or overlapping Cargo", { timeout: 20_000 }, async (t) => {
  const f = await fixture(t, { TEST_BUILD_DELAY: "900" });
  await f.until((events) => f.starts(events) === 1);
  f.dev.requestBuild();
  await f.until((events) => events.filter((event) => event === "build").length === 2);
  f.dev.requestBuild();
  await delay(100);
  f.dev.requestBuild();
  await f.until((events) => f.starts(events) === 2);
  const events = await f.events();
  assert.equal(events.filter((event) => event === "build").length, 3, events.join(", "));
  assert.equal(events.filter((event) => event === "quit").length, 1);
  assert.ok(events.indexOf("quit") > events.lastIndexOf("built"));
});

test("macOS bundles automatically and preserves the selected data mode for Finder", async (t) => {
  for (const [args, mode] of [
    [[], "vorkspace-data"],
    [["--dev-data"], "dev-data"],
    [["--vorkspace-data", "--dev-data", "--cwd", "--vorkspace-data"], "dev-data"],
  ]) {
    await t.test(`${mode}: ${JSON.stringify(args)}`, async (t) => {
      const f = await fixture(t, {}, { args });
      await f.until((events) => f.starts(events) === 1);
      const packaged = f.calls.find((call) => call.command === "bash");
      assert.deepEqual(packaged.args, ["scripts/desktop/bundle-native-macos.sh", "release", join(f.root, "custom-target"), "0.15.0", mode]);
      const app = f.calls.find((call) => !["cargo", "bash"].includes(call.command));
      assert.equal(app.command, join(f.root, "custom-target/release/vorkspace.app/Contents/MacOS/vorkspace-gpui"));
      assert.deepEqual(app.args, args);
      const events = await f.events();
      assert.ok(events.indexOf("built") < events.indexOf("package"));
      assert.ok(events.indexOf("package") < events.findIndex((event) => event.startsWith("start:")));
    });
  }
});

test("failed macOS packaging keeps the current app running", async (t) => {
  const f = await fixture(t);
  await f.until((events) => f.starts(events) === 1);
  await writeFile(join(f.root, "fail-package"), "");
  f.dev.requestBuild();
  await f.until((events) => events.includes("package-failed"));
  assert.equal(f.starts(await f.events()), 1);
  assert.equal((await f.events()).includes("quit"), false);
});

test("macOS launches the fresh bundle when installation fails and the installed bundle only on success", async (t) => {
  for (const installed of [false, true]) {
    await t.test(`installed: ${installed}`, async (t) => {
      const installs = [];
      const f = await fixture(t, { CI: "", VORKSPACE_NO_INSTALL: "" }, {
        installBundle: (source) => { installs.push(source); return installed; },
      });
      await f.until((events) => f.starts(events) === 1);
      const bundle = join(f.root, "custom-target/release/vorkspace.app");
      assert.deepEqual(installs, [bundle]);
      const app = f.calls.find((call) => !["cargo", "bash"].includes(call.command));
      assert.equal(app.command, join(installed ? "/Applications/vorkspace.app" : bundle, "Contents/MacOS/vorkspace-gpui"));
      f.dev.requestBuild();
      await f.until((events) => f.starts(events) === 2);
      assert.equal(installs.length, 2);
      assert.equal((await f.events()).filter((event) => event === "quit").length, 1);
    });
  }
});

test("CI, explicit no-install, and native-dev launch without attempting installation", async (t) => {
  for (const [env, profile] of [
    [{ CI: "1", VORKSPACE_NO_INSTALL: "" }, "release"],
    [{ CI: "", VORKSPACE_NO_INSTALL: "1" }, "release"],
    [{ CI: "", VORKSPACE_NO_INSTALL: "" }, "native-dev"],
  ]) {
    await t.test(`${profile}: ${JSON.stringify(env)}`, async (t) => {
      const f = await fixture(t, env, { profile });
      await f.until((events) => f.starts(events) === 1);
      assert.deepEqual(f.installs, []);
      const app = f.calls.find((call) => !["cargo", "bash"].includes(call.command));
      assert.equal(app.command, join(f.root, "custom-target", profile, "vorkspace.app/Contents/MacOS/vorkspace-gpui"));
    });
  }
});

test("the dev profile packages and launches Cargo's debug artifacts", async (t) => {
  assert.deepEqual(parseDevArgs(["--profile", "dev"]), { args: [], profile: "dev" });
  assert.deepEqual(parseDevArgs(["--profile=dev"]), { args: [], profile: "dev" });
  const f = await fixture(t, {}, { profile: "dev" });
  await f.until((events) => f.starts(events) === 1);
  const build = f.calls.find((call) => call.command === "cargo" && call.args[0] === "build");
  assert.deepEqual(build.args.slice(-2), ["--profile", "dev"]);
  const target = join(f.root, "custom-target");
  const executable = "vorkspace.app/Contents/MacOS/vorkspace-gpui";
  const launches = () => f.calls.filter((call) => !["cargo", "bash"].includes(call.command));
  assert.equal(launches()[0].command, join(target, "debug", executable));
  assert.deepEqual(f.calls.find((call) => call.command === "bash").args,
    ["scripts/desktop/bundle-native-macos.sh", "debug", target, "0.15.0", "dev-data"]);
  assert.deepEqual(f.installs, []);
  f.dev.requestBuild();
  await f.until((events) => f.starts(events) === 2);
  assert.equal(launches()[1].command, join(target, "debug", executable));
  const events = await f.events();
  assert.ok(events.lastIndexOf("built") < events.indexOf("quit"));
  assert.ok(events.indexOf("quit") < events.findLastIndex((event) => event.startsWith("start:")));
});

test("closing during compilation terminates Cargo and never launches an app", { timeout: 10_000 }, async (t) => {
  const f = await fixture(t, { TEST_BUILD_DELAY: "5000" });
  await f.until((events) => events.includes("build"));
  await f.dev.close();
  assert.equal(f.starts(await f.events()), 0);
  assert.ok(f.calls.every((call) => call.command === "cargo"));
});
