import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const workflows = fileURLToPath(new URL("../../.github/workflows/", import.meta.url));

test("workflows that run the checks install the Rust test runner first", async () => {
  for (const name of await readdir(workflows)) {
    const source = await readFile(join(workflows, name), "utf8");
    const check = source.indexOf("npm run check");
    if (check === -1) continue;
    const install = source.indexOf("taiki-e/install-action@nextest");
    assert.ok(install !== -1 && install < check, `${name} runs nextest without installing it`);
  }
});
