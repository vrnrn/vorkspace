import { build } from "esbuild";
import { copyFile } from "node:fs/promises";

await build({
  entryPoints: ["host/cli.js"],
  outfile: "build/host/vorkspace-host.mjs",
  bundle: true,
  platform: "node",
  format: "esm",
  target: "node24",
  loader: { ".ps1": "text" },
  define: { "import.meta.hot": "undefined" },
  sourcemap: true,
});
await copyFile("host/provider-guard.mjs", "build/host/provider-guard.mjs");
await copyFile("host/claude-copilot.mjs", "build/host/claude-copilot.mjs");
