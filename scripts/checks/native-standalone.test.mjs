import assert from "node:assert/strict";
import { readFile, readdir, access } from "node:fs/promises";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
const root = fileURLToPath(new URL("../../", import.meta.url));
async function files(path) {
  const result = [];
  for (const entry of await readdir(path, { withFileTypes: true })) {
    if (["node_modules", "target"].includes(entry.name)) continue;
    const name = join(path, entry.name);
    if (entry.isDirectory()) result.push(...await files(name)); else result.push(name);
  }
  return result;
}
test("native sources and host have complete local imports and no frontend dependencies", async () => {
  for (const dir of ["crates", "host", "embedded", "scripts"]) {
    for (const path of await files(join(root, dir))) {
      assert.ok(!/\.(?:tsx?|mts|cts)$/.test(path), `Source must be JavaScript or Rust: ${path}`);
      if (!/\.(rs|js|mjs)$/.test(path) || path.endsWith(".test.js")) continue;
      const source = await readFile(path, "utf8");
      // Only source-level declarations: build tools also contain quoted entry
      // points whose imports resolve against their own explicit working directory.
      const imports = path.endsWith(".rs") ? /include_(?:str|bytes)!\(\s*["']([^"']+)["']/g : /^\s*(?:import|export)\s+(?:[^;"']*\s+from\s+)?["']([^"']+)["']/gm;
      for (const match of source.matchAll(imports)) {
        if (!match[1].startsWith(".")) {
          assert.ok(!match[1].startsWith("@tauri-apps/") && match[1] !== "react", path);
          continue;
        }
        const target = resolve(dirname(path), match[1].replace(/\?raw$/, ""));
        if (!/\.(js|mjs|json|rs|sql|png|ps1|sh|md|svg|txt|LICENSE)$/.test(target)) continue;
        await access(target).catch(error => { throw new Error(`${path}: missing ${match[1]}`, { cause: error }); });
      }
    }
  }
});

// These user-visible removals also cover the embedded payload and renderer work.
test("desktop stays free of games and decorative title/composer effects", async () => {
  for (const name of [
    "embedded/arcade",
    "crates/vorkspace-gpui/assets/settings-arcade.js",
    "crates/vorkspace-gpui/src/arcade.rs",
    "crates/vorkspace-gpui/src/composer_runner.rs",
    "crates/vorkspace-gpui/src/particle_text.rs",
  ]) {
    await assert.rejects(access(join(root, name)), { code: "ENOENT" });
  }
  for (const name of [
    "crates/vorkspace-gpui/src/main.rs",
    "crates/vorkspace-gpui/src/conversation.rs",
    "crates/vorkspace-gpui/src/settings/model.rs",
    "crates/vorkspace-ui/src/preferences.rs",
    "vendor/gpui-pre-apple/src/metal_renderer.rs",
  ]) {
    const source = await readFile(join(root, name), "utf8");
    assert.doesNotMatch(source, /composer[_-](?:mascot|runner)|empty[_-]session[_-]games|particle_text|text_reveal_pipeline/);
  }
});
