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
