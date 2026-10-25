import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { delimiter, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = fileURLToPath(new URL("../../", import.meta.url));

export function resolveNativeEnv(base = process.env) {
  const env = { ...base };
  const home = homedir();
  const cargoBin = join(home, ".cargo", "bin");
  if (existsSync(cargoBin) && (!env.PATH || !env.PATH.includes(cargoBin))) {
    env.PATH = `${cargoBin}${delimiter}${env.PATH || ""}`;
  }
  if (process.platform === "darwin") {
    const rustLib = join(home, ".rustup", "toolchains", "stable-aarch64-apple-darwin", "lib");
    if (existsSync(rustLib) && (!env.DYLD_FALLBACK_LIBRARY_PATH || !env.DYLD_FALLBACK_LIBRARY_PATH.includes(rustLib))) {
      env.DYLD_FALLBACK_LIBRARY_PATH = env.DYLD_FALLBACK_LIBRARY_PATH
        ? `${rustLib}${delimiter}${env.DYLD_FALLBACK_LIBRARY_PATH}`
        : rustLib;
    }
  }
  return env;
}

export function installAppBundle(sourceBundle, destBundle = "/Applications/vorkspace.app") {
  if (process.platform !== "darwin" || process.env.CI || process.env.VORKSPACE_NO_INSTALL === "1") {
    return false;
  }
  try {
    const destNext = `${destBundle}.next`;
    spawnSync("rm", ["-rf", destNext]);
    const cp = spawnSync("cp", ["-R", sourceBundle, destNext]);
    if (cp.status !== 0) return false;
    spawnSync("rm", ["-rf", destBundle]);
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

  run("cargo", ["build", "--manifest-path", "Cargo.toml", "--locked", "-p", app.name, "--release"]);
  const target = process.env.CARGO_BUILD_TARGET
    ? join(metadata.target_directory, process.env.CARGO_BUILD_TARGET)
    : metadata.target_directory;
  if (process.platform === "darwin") {
    run("bash", ["scripts/desktop/bundle-native-macos.sh", "release", target, app.version, "vorkspace-data"]);
    const sourceApp = join(target, "release", "vorkspace.app");
    installAppBundle(sourceApp);
  } else {
    const executable = process.platform === "win32" ? "vorkspace-gpui.exe" : "vorkspace-gpui";
    console.log(`Executable: ${join(target, "release", executable)}`);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  buildNative();
}
