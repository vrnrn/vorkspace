import { createHash } from "node:crypto";
import {
  mkdir,
  readFile,
  writeFile,
  chmod,
  copyFile,
  rm,
} from "node:fs/promises";
import { execFileSync } from "node:child_process";
import { resolve, join } from "node:path";

// Official Node release checksums, pinned so release builds are reproducible.
// https://nodejs.org/dist/v24.21.0/SHASUMS256.txt
const nodeVersion = "24.21.0";
const runtimes = {
  "darwin-arm64":
    "bed7eea5325e1108f32ce5228ddd6a5f0f08a499ee42aa7442aea583702f6057",
  "darwin-x64":
    "1462cb3b3046b815cf8ea436d3da450ec1a9f11dac7e5a46b0ada5305d7e8097",
  "linux-arm64":
    "724282c3b43aec998aa9527380465b45d229e021b58035f5f4f63095eabfe5d5",
  "linux-x64":
    "6e1db87ef58b8819e5d5402eff1536491b18edd8eb7bee5ef7897876e88dc5ff",
};
const args = process.argv.slice(2);
const target = args.includes("--target")
  ? args[args.indexOf("--target") + 1]
  : `${process.platform}-${process.arch}`;
const targets = args.includes("--all") ? Object.keys(runtimes) : [target];
const output = resolve("build/host-packages");
const cache = resolve("build/host-runtime-cache");
await mkdir(output, { recursive: true });
await mkdir(cache, { recursive: true });
const { version } = JSON.parse(await readFile("package.json", "utf8"));
for (const target of targets) {
  if (!(target in runtimes))
    throw new Error(`Unsupported host target: ${target}`);
  const extension = "tar.gz";
  const stem = `node-v${nodeVersion}-${target}`;
  const archive = join(cache, `${stem}.${extension}`);
  let bytes;
  try {
    bytes = await readFile(archive);
  } catch {
    /* first download */
  }
  if (
    !bytes ||
    createHash("sha256").update(bytes).digest("hex") !== runtimes[target]
  ) {
    const response = await fetch(
      `https://nodejs.org/dist/v${nodeVersion}/${stem}.${extension}`,
      { signal: AbortSignal.timeout(120_000) },
    );
    if (!response.ok)
      throw new Error(`Runtime download failed: HTTP ${response.status}`);
    bytes = Buffer.from(await response.arrayBuffer());
    if (createHash("sha256").update(bytes).digest("hex") !== runtimes[target])
      throw new Error("Node runtime checksum mismatch");
    await writeFile(archive, bytes);
  }
  const folder = join(output, target);
  await rm(folder, { recursive: true, force: true });
  await mkdir(folder, { recursive: true });
  execFileSync("tar", [
    "-xzf",
    archive,
    "-C",
    folder,
    "--strip-components=1",
    `${stem}/bin/node`,
    `${stem}/LICENSE`,
  ]);
  await copyFile("build/host/vorkspace-host.mjs", join(folder, "host.mjs"));
  await copyFile("host/provider-guard.mjs", join(folder, "provider-guard.mjs"));
  await copyFile("host/claude-copilot.mjs", join(folder, "claude-copilot.mjs"));
  await copyFile("LICENSE", join(folder, "VORKSPACE-LICENSE"));
  await writeFile(
    join(folder, "version.json"),
    JSON.stringify({ version, nodeVersion, target }),
  );
  await writeFile(
    join(folder, "vorkspace-host"),
    '#!/bin/sh\nset -eu\nDIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)\nexec "$DIR/bin/node" "$DIR/host.mjs" "$@"\n',
  );
  await chmod(join(folder, "vorkspace-host"), 0o755);
  if (target === `${process.platform}-${process.arch}`) {
    const executable = join(folder, "bin/node");
    const actual = execFileSync(
      executable,
      [join(folder, "host.mjs"), "--version"],
      {
        encoding: "utf8",
      },
    ).trim();
    if (actual !== version)
      throw new Error("Packaged host failed its executable smoke test");
  }
  const filename = `vorkspace-host-${target}.${extension}`;
  await rm(join(output, filename), { force: true });
  execFileSync("tar", ["-czf", join(output, filename), "-C", folder, "."]);
  const hash = createHash("sha256")
    .update(await readFile(join(output, filename)))
    .digest("hex");
  await writeFile(join(output, `${filename}.sha256`), `${hash}  ${filename}\n`);
  console.log(`${filename} (${version}, Node ${nodeVersion})`);
}
