#!/usr/bin/env node
// Keeps a Cargo target directory from growing without bound. Cargo never
// deletes superseded output: each code change links a new test binary beside
// the old ones, and each crate's incremental cache gains a directory per build
// configuration. Per profile this keeps the newest few of each and removes the
// rest. Everything removed is rebuildable; the next build simply recreates it.
import { spawnSync } from "node:child_process";
import { readdirSync, rmSync, statSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

// Two of each name survive: a crate's test harness and its plain build, or the
// current build and the one before it.
export const KEEP = 2;

// `name-0123456789abcdef`, the shape Cargo gives hashed outputs in `deps/` and `examples/`.
const HASHED = /^(.+)-[0-9a-f]{16}$/;

// `name-0h32cv21lt6eb`, the base-36 crate-id suffix rustc gives `incremental/` directories.
const INCREMENTAL = /^(.+)-[0-9a-z]{13}$/;

function entries(dir) {
  try {
    return readdirSync(dir, { withFileTypes: true });
  } catch {
    return [];
  }
}

function modified(path) {
  try {
    return statSync(path).mtimeMs;
  } catch {
    return 0;
  }
}

/** Newest-first groups of hashed entries in `dir`, keyed by their name. */
function groups(dir, pattern, pick, all = entries(dir)) {
  const byName = new Map();
  for (const entry of all) {
    if (!pick(entry)) continue;
    const match = pattern.exec(entry.name);
    if (!match) continue;
    const path = join(dir, entry.name);
    const list = byName.get(match[1]) ?? [];
    list.push({ name: entry.name, path, time: modified(path) });
    byName.set(match[1], list);
  }
  for (const list of byName.values()) {
    list.sort((a, b) => b.time - a.time || a.name.localeCompare(b.name));
  }
  return byName;
}

function isProfileRoot(dir) {
  return entries(dir).some(
    (entry) =>
      entry.isDirectory() &&
      (entry.name === "deps" || entry.name === "incremental" || entry.name === "examples"),
  );
}

/**
 * Finds profile directories under `dir`, whether built directly (`<target>/<profile>`),
 * for a target triple (`<target>/<triple>/<profile>`), or in a nested sub-target
 * (`<target>/block/<profile>`).
 */
function profileRoots(dir, depth = 3) {
  if (depth <= 0) return [];
  const roots = [];
  for (const entry of entries(dir)) {
    if (!entry.isDirectory()) continue;
    const path = join(dir, entry.name);
    if (isProfileRoot(path)) {
      roots.push(path);
    } else {
      roots.push(...profileRoots(path, depth - 1));
    }
  }
  return roots;
}

function pruneBinaries(dir, keep, remove) {
  const all = entries(dir);
  for (const list of groups(dir, HASHED, (e) => e.isFile() && !e.name.includes("."), all).values()) {
    for (const { name, path } of list.slice(keep)) {
      remove(path);
      const prefix = `${name}.`;
      for (const companion of all) {
        if (companion.name.startsWith(prefix)) {
          rmSync(join(dir, companion.name), { recursive: true, force: true });
        }
      }
    }
  }
}

/**
 * Removes superseded test and binary outputs and incremental caches under each
 * profile of `target`. Returns the removed paths.
 */
export function pruneTarget(target, keep = KEEP) {
  const removed = [];
  const remove = (path) => {
    rmSync(path, { recursive: true, force: true });
    removed.push(path);
  };
  for (const root of profileRoots(target)) {
    // Linked executables carry no extension; libraries end in .rlib, .rmeta,
    // .dylib or .d and are shared by every build, so they stay.
    pruneBinaries(join(root, "deps"), keep, remove);
    pruneBinaries(join(root, "examples"), keep, remove);
    const incremental = join(root, "incremental");
    for (const list of groups(incremental, INCREMENTAL, (e) => e.isDirectory()).values()) {
      for (const { path } of list.slice(keep)) remove(path);
    }
  }
  return removed;
}

/** The checkout's target directory, honouring CARGO_TARGET_DIR and config. */
export function targetDirectory(cwd = process.cwd()) {
  const result = spawnSync(
    "cargo",
    ["metadata", "--no-deps", "--offline", "--format-version", "1"],
    { cwd, encoding: "utf8" },
  );
  if (result.status !== 0) throw new Error(result.stderr || "cargo metadata failed");
  return JSON.parse(result.stdout).target_directory;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const target = process.argv[2] ?? targetDirectory();
  const removed = pruneTarget(target);
  console.log(`Pruned ${removed.length} superseded build outputs in ${target}.`);
}
