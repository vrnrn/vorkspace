import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { homedir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = fileURLToPath(new URL("../../", import.meta.url));

export function resolveNativeEnv(base = process.env) {
  const env = { ...base };
  const home = homedir();
  const cargoBin = join(env.CARGO_HOME || join(home, ".cargo"), "bin");
  if (existsSync(cargoBin)) {
    // Rustup's shims must precede any old concrete toolchain on PATH, so Cargo
    // and rustc both honor this checkout's rust-toolchain.toml.
    env.PATH = [cargoBin, ...(env.PATH || "").split(delimiter).filter((entry) => entry !== cargoBin)].join(delimiter);
  }
  // Some installations put concrete compiler symlinks in .cargo/bin. Resolve
  // the active toolchain through rustup so those cannot bypass the project pin.
  const compiler = spawnSync("rustup", ["which", "cargo"], { cwd: root, env, encoding: "utf8" });
  if (compiler.error || compiler.status !== 0) {
    throw new Error(`Could not resolve the project Rust toolchain: ${compiler.error?.message || compiler.stderr}`);
  }
  env.PATH = `${dirname(compiler.stdout.trim())}${delimiter}${env.PATH}`;
  return env;
}

export function installAppBundle(sourceBundle, destBundle = "/Applications/vorkspace.app") {
  if (process.platform !== "darwin" || process.env.CI || process.env.VORKSPACE_NO_INSTALL === "1") {
    return false;
  }
  let staging;
  try {
    // Stage beside the destination so publishing is one filesystem operation.
    staging = mkdtempSync(join(dirname(destBundle), ".vorkspace-install-"));
    const next = join(staging, "vorkspace.app");
    const cp = spawnSync("cp", ["-R", sourceBundle, next]);
    if (cp.status !== 0) return false;
    const publisher = fileURLToPath(new URL("./publish-native-bundle.py", import.meta.url));
    const publish = spawnSync("python3", [publisher, next, destBundle], { encoding: "utf8" });
    if (publish.status === 0) {
      console.log(`Installed: ${destBundle}`);
      return true;
    }
    const error = publish.error?.message ?? publish.stderr?.trim();
    if (error) console.warn(`Could not install to ${destBundle}: ${error}`);
  } catch (err) {
    console.warn(`Could not install to ${destBundle}: ${err.message}`);
  } finally {
    if (staging) {
      try { rmSync(staging, { recursive: true, force: true }); }
      catch (err) { console.warn(`Could not clean ${staging}: ${err.message}`); }
    }
  }
  return false;
}

function run(env, command, args, capture = false) {
  const result = spawnSync(command, args, {
    cwd: root,
    env,
    encoding: "utf8",
    stdio: capture ? ["ignore", "pipe", "inherit"] : "inherit",
  });
  if (result.error) {
    console.error(result.error.message);
    process.exit(1);
  }
  if (result.status !== 0) process.exit(result.status ?? 1);
  return result.stdout;
}

export function buildNative() {
  const env = resolveNativeEnv(process.env);
  const metadata = JSON.parse(
    run(env, "cargo", ["metadata", "--manifest-path", "Cargo.toml", "--locked", "--no-deps", "--format-version", "1"], true),
  );
  const app = metadata.packages.find((pkg) => pkg.name === "vorkspace-gpui");
  if (!app) throw new Error("The vorkspace-gpui package was not found.");

  run(env, "cargo", ["build", "--manifest-path", "Cargo.toml", "--locked", "-p", app.name, "-p", "vorkspace-helper", "--release"]);
  const target = process.env.CARGO_BUILD_TARGET
    ? join(metadata.target_directory, process.env.CARGO_BUILD_TARGET)
    : metadata.target_directory;
  run(env, "bash", ["scripts/desktop/bundle-native-macos.sh", "release", target, app.version, "vorkspace-data"]);
  installAppBundle(join(target, "release", "vorkspace.app"));
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  buildNative();
}
