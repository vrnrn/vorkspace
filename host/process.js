import { createReadStream } from "node:fs";
import { access, readFile, realpath, stat } from "node:fs/promises";
import { constants } from "node:fs";
import { homedir } from "node:os";
import { delimiter, dirname, join } from "node:path";
const binaryNames = {
  codex: ["codex"],
  claude: ["claude"],
  pi: ["pi-coding-agent", "pi"],
  omp: ["omp"],
  antigravity: ["agy_acp_server.par"],
};
const providerDirectories = (provider) => {
  const home = homedir();
  const extra = {
    claude: [
      join(home, ".claude", "local"),
      join(home, ".local", "share", "claude"),
    ],
    antigravity: [join(home, ".local", "share", "agy-acp")],
  };
  return [
    ...new Set([
      ...(process.env.PATH ?? "").split(delimiter),
      join(home, ".local", "bin"),
      join(home, ".npm-global", "bin"),
      join(home, ".cargo", "bin"),
      join(home, "n", "bin"),
      join(home, ".bun", "bin"),
      ...(extra[provider] ?? []),
      "/opt/homebrew/bin",
      "/usr/local/bin",
      "/usr/bin",
      "/snap/bin",
    ]),
  ];
};
/** Verify that an ambiguous launcher name belongs to requested provider. */
async function matchesProvider(candidate, provider, name) {
  if (provider === "pi" && name === "pi") return isPiLaunchCandidate(candidate);
  return true;
}
/** Scan file content for any marker, stopping after optional byte limit. */
async function fileContains(
  path,
  markers,
  maxBytes = Number.POSITIVE_INFINITY,
) {
  let read = 0;
  let carry = "";
  for await (const chunk of createReadStream(path, {
    highWaterMark: 64 * 1024,
  })) {
    const bytes = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
    const length = Math.min(bytes.length, maxBytes - read);
    const text = (
      carry + bytes.subarray(0, length).toString("latin1")
    ).toLowerCase();
    if (markers.some((marker) => text.includes(marker))) return true;
    carry = text.slice(-64);
    read += length;
    if (read >= maxBytes) break;
  }
  return false;
}
/** Marker strings that identify a Pi coding agent install. */
const PI_MARKERS = [
  "pi-coding-agent",
  "@earendil-works/pi",
  "@mariozechner/pi-coding-agent",
  "pi_coding_agent",
];
const PI_PACKAGE_NAMES = new Set([
  "@earendil-works/pi-coding-agent",
  "@mariozechner/pi-coding-agent",
]);
/**
 * npm's bin launcher for pi is a thin `#!/usr/bin/env node` stub whose only
 * job is to `createRequire(...)("./cli-runtime.js")` — the marker strings live
 * megabytes deeper in `dist/bundle/chunks/*.js`, past any reasonable head
 * scan. Identify the enclosing package instead: resolve symlinks (npm bins
 * symlink into `lib/node_modules/<pkg>/...`), then walk up to the nearest
 * `package.json` and check its exact `name` against supported Pi packages.
 */
async function isPiLaunchCandidate(candidate) {
  if (await fileContains(candidate, PI_MARKERS, 64 * 1024)) return true;
  let real;
  try {
    real = await realpath(candidate);
  } catch {
    return false;
  }
  let dir = dirname(real);
  // A scoped package's package.json sits two directories up from a nested
  // bin/script; a few hops bound the walk without leaving the package.
  for (let hop = 0; hop < 6; hop += 1) {
    const manifest = join(dir, "package.json");
    try {
      const pkg = JSON.parse(await readFile(manifest, "utf8"));
      const name = pkg.name?.toLowerCase();
      return name !== undefined && PI_PACKAGE_NAMES.has(name);
    } catch {
      /* not a package root; keep walking */
    }
    const parent = dirname(dir);
    if (parent === dir) return false;
    dir = parent;
  }
  return false;
}
/** Resolve executable launcher for remote provider without running it. */
export async function resolveProvider(provider) {
  for (const directory of providerDirectories(provider)) {
    if (!directory) continue;
    for (const name of binaryNames[provider]) {
      const candidate = join(directory.replace(/^"|"$/g, ""), name);
      try {
        await access(candidate, constants.X_OK);
        if (!(await stat(candidate)).isFile()) continue;
        await providerLaunch(candidate, []);
        if (!(await matchesProvider(candidate, provider, name))) continue;
        return candidate;
      } catch {
        /* try the next installed launcher */
      }
    }
  }
  throw new Error(
    `${provider} is not installed on this host or is missing from its PATH. Install its native CLI or standard npm package.`,
  );
}
/** Script launchers run with the bundled Node, preserving argv without a shell. */
export async function providerLaunch(command, args) {
  if (/\.(cjs|mjs|js)$/i.test(command))
    return { command: process.execPath, args: [command, ...args] };
  return { command, args };
}
