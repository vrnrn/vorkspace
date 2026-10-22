import assert from "node:assert/strict";
import { access, readFile, readdir } from "node:fs/promises";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const root = fileURLToPath(new URL("../../", import.meta.url));
const docs = join(root, "docs");

async function markdownFiles(directory) {
  const result = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) result.push(...await markdownFiles(path));
    else if (entry.name.endsWith(".md")) result.push(path);
  }
  return result;
}

test("documentation has paired companions and resolvable local links", async () => {
  const files = await markdownFiles(docs);
  for (const path of files) {
    const name = relative(docs, path);
    const companion = name.startsWith(`eli5${sep}`)
      ? join(docs, name.slice("eli5/".length))
      : join(docs, "eli5", name);
    await access(companion).catch(error => {
      throw new Error(`Missing documentation counterpart for ${name}`, { cause: error });
    });
  }
  for (const path of [...files, join(root, "README.md"), join(root, "CONTRIBUTING.md")]) {
    const source = await readFile(path, "utf8");
    for (const [, link] of source.matchAll(/\]\(([^\s)]+)\)/g)) {
      if (/^[a-z][a-z\d+.-]*:/i.test(link) || link.startsWith("#")) continue;
      const target = resolve(dirname(path), decodeURIComponent(link.split("#")[0]));
      assert.ok(target.startsWith(root), `Local link leaves repository: ${path}: ${link}`);
      await access(target).catch(error => {
        throw new Error(`Broken documentation link in ${relative(root, path)}: ${link}`, { cause: error });
      });
    }
  }
});
