import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
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
  try {
    const destNext = `${destBundle}.next`;
    if (spawnSync("rm", ["-rf", destNext]).status !== 0) return false;
    const cp = spawnSync("cp", ["-R", sourceBundle, destNext]);
    if (cp.status !== 0) return false;
    // mv nests the source inside an existing destination directory. Only report
    // a replacement after removal succeeded, so the runner can fall back safely.
    if (spawnSync("rm", ["-rf", destBundle]).status !== 0) return false;
    const mv = spawnSync("mv", ["-f", destNext, destBundle]);
    if (mv.status === 0) {
      console.log(`Installed: ${destBundle}`);
      return true;
    }
  } catch (err) {
    console.warn(`Could not install to ${destBundle}: ${err.message}`);
  }
  return false;
}

const env = resolveNativeEnv(process.env);

function run(command, args, capture = false) {
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
  const metadata = JSON.parse(
    run("cargo", ["metadata", "--manifest-path", "Cargo.toml", "--locked", "--no-deps", "--format-version", "1"], true),
  );
  const app = metadata.packages.find((pkg) => pkg.name === "vorkspace-gpui");
  if (!app) throw new Error("The vorkspace-gpui package was not found.");

  run("cargo", ["build", "--manifest-path", "Cargo.toml", "--locked", "-p", app.name, "-p", "vorkspace-helper", "--release"]);
  const target = process.env.CARGO_BUILD_TARGET
    ? join(metadata.target_directory, process.env.CARGO_BUILD_TARGET)
    : metadata.target_directory;
  run("bash", ["scripts/desktop/bundle-native-macos.sh", "release", target, app.version, "vorkspace-data"]);
  installAppBundle(join(target, "release", "vorkspace.app"));
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  buildNative();
}
