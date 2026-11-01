import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const path = (name) => fileURLToPath(new URL(`../../vendor/${name}`, import.meta.url));
// A missing field here misaligns every shader input with no compile error.
const shared = ["Quad", "Shadow", "Underline", "MonochromeSprite", "PolychromeSprite", "PathSprite"];

function headerFields(source, name) {
  const body = source.match(new RegExp(`typedef struct ${name} \\{([\\s\\S]*?)\\} ${name};`));
  assert.ok(body, `${name} is missing from scene.h`);
  return body[1]
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .split(";")
    .map((field) => field.trim().split(/\s+/).pop())
    .filter(Boolean);
}

function rustFields(source, name) {
  const body = source.match(new RegExp(`struct ${name}(?:<[^>]*>)? \\{([\\s\\S]*?)\\n\\}`));
  assert.ok(body, `${name} is missing from the Rust scene`);
  return [...body[1].matchAll(/^\s*(?:pub(?:\([a-z]+\))? )?(\w+):/gm)].map((field) => field[1]);
}

test("scene.h keeps the Rust scene's fields in order", async () => {
  const header = await readFile(path("gpui-pre-apple/src/scene.h"), "utf8");
  const scene = (await readFile(path("gpui-pre/src/scene.rs"), "utf8"))
    + await readFile(path("gpui-pre-apple/src/metal_renderer.rs"), "utf8");
  for (const name of shared) {
    assert.deepEqual(headerFields(header, name), rustFields(scene, name), name);
  }
});
