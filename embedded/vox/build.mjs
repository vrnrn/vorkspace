import { build } from "esbuild";
import { fileURLToPath } from "node:url";
await build({
  absWorkingDir: fileURLToPath(new URL("../../", import.meta.url)),
  entryPoints: ["embedded/vox/index.js"],
  bundle: true,
  format: "iife",
  target: "safari15",
  minify: true,
  outfile: "crates/vorkspace-core/assets/vox-audio.js",
});
