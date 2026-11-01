#!/usr/bin/env node
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const version = process.argv[2];
if (!version || !/^\d+\.\d+\.\d+$/.test(version)) {
  console.error("usage: npm run set-version -- 0.1.1");
  process.exit(1);
}
const root = new URL("../../", import.meta.url);
const read = name => readFileSync(new URL(name, root), "utf8");
const packageJson = JSON.parse(read("package.json"));
const packageLock = JSON.parse(read("package-lock.json"));
packageJson.version = version;
packageLock.version = version;
packageLock.packages[""].version = version;
function replaceVersion(source, pattern, label) {
  if (!pattern.test(source)) throw new Error(`Missing version for ${label}`);
  return source.replace(pattern, (_match, before, after) => `${before}${version}${after}`);
}
const manifest = replaceVersion(read("Cargo.toml"), /^(version = ")[^"]+(")/m, "workspace");
// Every workspace crate inherits the version, so a stale lock entry fails --locked.
const lock = replaceVersion(read("Cargo.lock"), /(name = "vorkspace-[^"]+"\nversion = ")[^"]+(")/g, "workspace crates");
// Validate every manifest before changing any file; repeating a version is safe.
for (const [name, value] of [
  ["package.json", JSON.stringify(packageJson, null, 2) + "\n"],
  ["package-lock.json", JSON.stringify(packageLock, null, 2) + "\n"],
  ["Cargo.lock", lock],
  ["Cargo.toml", manifest],
]) writeFileSync(fileURLToPath(new URL(name, root)), value);
console.log(`version ${version}`);
