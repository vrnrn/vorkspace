// Embed the arcade state engines for GPUI's bounded interpreter.
import { build } from "esbuild";
import { fileURLToPath } from "node:url";
await build({
  absWorkingDir: fileURLToPath(new URL("../../", import.meta.url)),
  entryPoints: ["embedded/arcade/index.js"],
  bundle: true,
  format: "iife",
  globalName: "NativeArcade",
  platform: "neutral",
  target: "es2020",
  minify: true,
  outfile: "crates/vorkspace-gpui/assets/settings-arcade.js",
});
