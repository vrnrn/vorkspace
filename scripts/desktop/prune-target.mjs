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

// `name-0123456789abcdef`, the shape Cargo gives hashed outputs.
const HASHED = /^(.+)-[0-9a-f]{16}$/;

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
function groups(dir, pick) {
  const byName = new Map();
  for (const entry of entries(dir)) {
    if (!pick(entry)) continue;
    const match = HASHED.exec(entry.name);
    if (!match) continue;
    const path = join(dir, entry.name);
    const list = byName.get(match[1]) ?? [];
    list.push({ path, time: modified(path) });
    byName.set(match[1], list);
  }
  for (const list of byName.values()) list.sort((a, b) => b.time - a.time);
  return byName;
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
  for (const profile of entries(target)) {
    if (!profile.isDirectory()) continue;
    const root = join(target, profile.name);
    // Linked executables carry no extension; libraries end in .rlib, .rmeta,
    // .dylib or .d and are shared by every build, so they stay.
    const deps = join(root, "deps");
    for (const list of groups(deps, (e) => e.isFile() && !e.name.includes(".")).values()) {
      for (const { path } of list.slice(keep)) {
        remove(path);
        rmSync(`${path}.d`, { force: true });
        rmSync(`${path}.dSYM`, { recursive: true, force: true });
      }
    }
    const incremental = join(root, "incremental");
    for (const list of groups(incremental, (e) => e.isDirectory()).values()) {
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
