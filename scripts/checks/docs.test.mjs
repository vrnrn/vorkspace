import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../", import.meta.url));
const scripts = JSON.parse(readFileSync(join(root, "package.json"), "utf8")).scripts;
const crates = new Set(readdirSync(join(root, "crates")));
const markdown = (directory) => readdirSync(join(root, directory))
  .filter((name) => name.endsWith(".md"))
  .map((name) => join(directory, name));
const documents = [
  ...markdown("."),
  ...markdown("docs"),
  ...markdown("docs/eli5"),
];

test("documents name npm scripts and crates that exist", () => {
  for (const document of documents) {
    const text = readFileSync(join(root, document), "utf8");
    for (const [, script] of text.matchAll(/npm run ([\w:-]+)/g)) {
      assert.ok(script in scripts, `${document}: npm run ${script}`);
    }
    for (const [, crate] of text.matchAll(/ -p (vorkspace-[\w-]+)/g)) {
      assert.ok(crates.has(crate), `${document}: -p ${crate}`);
    }
  }
});

test("every guide has a plain-English companion", () => {
  for (const document of markdown("docs")) {
    const name = document.slice("docs/".length);
    assert.ok(existsSync(join(root, "docs/eli5", name)), `docs/eli5/${name}`);
  }
  for (const document of markdown("docs/eli5")) {
    const name = document.slice("docs/eli5/".length);
    assert.ok(existsSync(join(root, "docs", name)), `docs/${name}`);
  }
});
