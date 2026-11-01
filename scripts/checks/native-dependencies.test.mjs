import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const buildScript = new URL("../../crates/vorkspace-gpui/build.rs", import.meta.url);
function resolveNativeEnv(base = process.env) {
  const env = { ...base };
  const home = homedir();
  const cargoBin = join(home, ".cargo", "bin");
  if (existsSync(cargoBin) && (!env.PATH || !env.PATH.includes(cargoBin))) {
    env.PATH = `${cargoBin}${delimiter}${env.PATH || ""}`;
  }
  return env;
}
const nativeEnv = resolveNativeEnv(process.env);
// The first Cargo build on a fresh Windows runner can exceed one minute.
const cargoTimeout = 60_000;

async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), "vorkspace-dependency-guard-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  await mkdir(join(root, "src"));
  await writeFile(join(root, "src/lib.rs"), "");
  await copyFile(buildScript, join(root, "build.rs"));
  await mkdir(join(root, "native"));
  await copyFile(new URL("../../crates/vorkspace-gpui/native/Clipboard.swift", import.meta.url), join(root, "native/Clipboard.swift"));
  // The copied build script also validates notes in a release CI environment.
  await mkdir(join(root, "build"));
  await writeFile(join(root, "build/release-notes.md"), "## [0.0.0]\n\nDependency guard fixture.\n");
  const manifest = `[package]
name = "dependency-guard-fixture"
version = "0.0.0"
edition = "2024"

[workspace]
`;
  await writeFile(join(root, "Cargo.toml"), manifest);
  const cargo = (...args) => {
    const result = spawnSync("cargo", [...args, "--offline"], {
      cwd: root,
      env: { ...nativeEnv, CARGO_TARGET_DIR: join(root, "target"), CARGO_TERM_COLOR: "never" },
      encoding: "utf8",
      timeout: cargoTimeout,
    });
    const output = (result.stdout ?? "") + (result.stderr ?? "");
    assert.equal(result.error, undefined, `${result.error?.message}\n${output}`);
    return { status: result.status, output };
  };
  const addDependency = async (optional) => {
    await mkdir(join(root, "capture/src"), { recursive: true });
    // A harmless local fixture; no code or license from zed-scap is copied.
    await writeFile(join(root, "capture/src/lib.rs"), "");
    await writeFile(join(root, "capture/Cargo.toml"), `[package]
name = "zed-scap"
version = "9.9.9"
edition = "2024"
`);
    await writeFile(join(root, "Cargo.toml"), `${manifest}
[dependencies]
capture = { package = "zed-scap", path = "capture", optional = ${optional} }
`);
  };
  return { root, manifest, cargo, addDependency };
}

test("native build rejects even an unused optional dependency and recovers after removal", async (t) => {
  const f = await fixture(t);
  let result = f.cargo("build");
  assert.equal(result.status, 0, result.output);

  await f.addDependency(true);
  result = f.cargo("build");
  assert.notEqual(result.status, 0, result.output);
  assert.match(result.output, /zed-scap is prohibited/);
  assert.match(await readFile(join(f.root, "Cargo.lock"), "utf8"), /name = "zed-scap"/);

  await writeFile(join(f.root, "Cargo.toml"), f.manifest);
  result = f.cargo("build");
  assert.equal(result.status, 0, result.output);
});

test("native build rejects a renamed active dependency at a different version", async (t) => {
  const f = await fixture(t);
  await f.addDependency(false);
  const result = f.cargo("build");
  assert.notEqual(result.status, 0, result.output);
  assert.match(result.output, /zed-scap is prohibited/);
});

test("GPUI refuses to enable screen capture before compiling its implementation", async (t) => {
  const f = await fixture(t);
  const result = spawnSync("rustc", [
    fileURLToPath(new URL("../../vendor/gpui-pre/build.rs", import.meta.url)),
    "--edition", "2024", "--cfg", 'feature="screen-capture"', "--out-dir", f.root,
  ], { env: nativeEnv, encoding: "utf8", timeout: 60_000 });
  assert.ifError(result.error);
  assert.notEqual(result.status, 0, result.stdout + result.stderr);
  assert.match(result.stderr, /Screen capture is disabled in Vorkspace/);
});
