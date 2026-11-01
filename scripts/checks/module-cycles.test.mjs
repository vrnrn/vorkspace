import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { basename, join, relative } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const crates = fileURLToPath(new URL("../../crates/", import.meta.url));
// Bot records carry their settings and turn settings carry the Bot. That cycle
// is in the data itself, so these two modules stay side by side.
const allowed = ["agent,bot_model"];

function sources(directory) {
  return readdirSync(directory).flatMap((name) => {
    const path = join(directory, name);
    return statSync(path).isDirectory() ? sources(path) : name.endsWith(".rs") ? [path] : [];
  });
}

function withoutTests(source) {
  let kept = "";
  let index = 0;
  for (const match of source.matchAll(/^#\[cfg\(test\)\]\n(?:#\[[^\n]*\]\n)*mod \w+ \{\n/gm)) {
    if (match.index < index) continue;
    kept += source.slice(index, match.index);
    let depth = 1;
    index = match.index + match[0].length;
    while (depth && index < source.length) {
      if (source[index] === "{") depth += 1;
      if (source[index] === "}") depth -= 1;
      index += 1;
    }
  }
  return (kept + source.slice(index)).replace(/\/\/[^\n]*/g, "");
}

// Each top-level name after `crate::`, reading whole `use` trees so a nested group
// doesn't end the one around it.
function crateModules(source) {
  const names = [];
  for (const match of source.matchAll(/crate::(\{|\w+)/g)) {
    if (match[1] !== "{") {
      names.push(match[1]);
      continue;
    }
    let depth = 1;
    let item = "";
    for (let index = match.index + match[0].length; depth; index += 1) {
      const character = source[index];
      if (character === "{") depth += 1;
      if (character === "}") depth -= 1;
      if (depth === 1 && character === ",") {
        names.push(item.trim().split(/\W/)[0]);
        item = "";
      } else if (depth) {
        item += character;
      }
    }
    names.push(item.trim().split(/\W/)[0]);
  }
  return names;
}

function moduleGraph(root) {
  const modules = new Set(readdirSync(root)
    .filter((name) => !["main.rs", "lib.rs"].includes(name))
    .map((name) => basename(name, ".rs")));
  const edges = new Map([...modules].map((name) => [name, new Set()]));
  for (const path of sources(root)) {
    if (/tests?\.rs$/.test(path)) continue;
    const owner = basename(relative(root, path).split("/")[0], ".rs");
    if (!modules.has(owner)) continue;
    for (const target of crateModules(withoutTests(readFileSync(path, "utf8")))) {
      if (target !== owner && modules.has(target)) edges.get(owner).add(target);
    }
  }
  return edges;
}

function cycles(edges) {
  const order = new Map();
  const low = new Map();
  const stack = [];
  const found = [];
  const visit = (node) => {
    order.set(node, order.size);
    low.set(node, order.get(node));
    stack.push(node);
    for (const next of edges.get(node)) {
      if (!order.has(next)) {
        visit(next);
        low.set(node, Math.min(low.get(node), low.get(next)));
      } else if (stack.includes(next)) {
        low.set(node, Math.min(low.get(node), order.get(next)));
      }
    }
    if (low.get(node) === order.get(node)) {
      const members = stack.splice(stack.indexOf(node));
      if (members.length > 1) found.push(members.sort().join(","));
    }
  };
  for (const node of edges.keys()) if (!order.has(node)) visit(node);
  return found;
}

test("top-level modules depend on each other in one direction", () => {
  for (const crate of readdirSync(crates)) {
    const found = cycles(moduleGraph(join(crates, crate, "src")));
    assert.deepEqual(found.filter((cycle) => !allowed.includes(cycle)), [], crate);
  }
});

test("imports after a nested group still count", () => {
  assert.deepEqual(
    crateModules("use crate::{a::{self, B}, c::D, e};"),
    ["a", "c", "e"],
  );
});
