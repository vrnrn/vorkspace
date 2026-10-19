import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

export function verifyMacRelease(tag, feed, release, version) {
  assert.match(version, /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/);
  assert.equal(
    tag,
    `mac-v${version}`,
    "Release tag must match the app version",
  );
  assert.equal(
    feed.version,
    version,
    "Updater feed must match the app version",
  );
  assert.equal(
    release.name,
    `vorkspace ${version}`,
    "Release title must match the app version without a platform suffix",
  );
  if (release.body !== undefined) {
    assert.equal(typeof release.body, "string", "Release body is required");
    assert.equal(
      feed.notes?.trimEnd(),
      release.body.trimEnd(),
      "Updater notes must match the GitHub release body",
    );
  }
  const allowed = new Set([
    "latest.json",
    `vorkspace_${version}_aarch64.dmg`,
    "vorkspace_aarch64.app.tar.gz",
    "vorkspace_aarch64.app.tar.gz.sig",
    "LICENSE",
    "migrate-monocode-to-vorkspace.sh",
    ...["arm64", "x64"].flatMap((arch) => [
      `vorkspace-host-darwin-${arch}.tar.gz`,
      `vorkspace-host-darwin-${arch}.tar.gz.sha256`,
    ]),
  ]);
  const names = new Set(release.assets.map(({ name }) => name));
  for (const name of names)
    assert(allowed.has(name), `Unexpected Mac release asset: ${name}`);
  assert(names.has("latest.json"), "Release must include the updater feed");
  assert(
    names.has(`vorkspace_${version}_aarch64.dmg`),
    "Apple Silicon DMG is missing",
  );
  for (const arch of ["arm64", "x64"]) {
    const name = `vorkspace-host-darwin-${arch}.tar.gz`;
    assert(names.has(name), `Mac remote host archive is missing: ${name}`);
    assert(
      names.has(`${name}.sha256`),
      `Mac host checksum is missing: ${name}`,
    );
  }
  assert(
    Object.keys(feed.platforms ?? {}).every((target) =>
      ["darwin-aarch64", "darwin-aarch64-app"].includes(target),
    ),
    "Feed must contain only Apple Silicon",
  );
  const entry = feed.platforms?.["darwin-aarch64"];
  assert(entry, "Updater feed is missing darwin-aarch64");
  if (feed.platforms["darwin-aarch64-app"]) {
    assert.deepEqual(feed.platforms["darwin-aarch64-app"], entry);
  }
  assert.equal(typeof entry.signature, "string");
  assert(
    entry.signature.trim(),
    "Updater signature is missing for darwin-aarch64",
  );
  const url = new URL(entry.url);
  assert.equal(url.origin, "https://github.com");
  assert.equal(url.search, "");
  assert.equal(url.hash, "");
  const prefix = `/vrnrn/vorkspace/releases/download/${tag}/`;
  assert(
    url.pathname.startsWith(prefix),
    "Updater must download this fork's tagged release",
  );
  const name = decodeURIComponent(url.pathname.slice(prefix.length));
  assert.match(name, /^[^/]+_aarch64\.app\.tar\.gz$/);
  assert(names.has(name), `Updater archive is missing: ${name}`);
  assert(
    names.has(`${name}.sig`),
    `Updater signature asset is missing: ${name}.sig`,
  );
}

export async function verifyBundledReleaseNotes(root, version, body) {
  assert.equal(typeof body, "string", "Release body is required");
  assert(body.trim(), "Release body must not be empty");
  const expected = `## [${version}]\n\n${body.trimEnd()}`;
  assert.equal(
    readFileSync(join(root, "build/release-notes.md"), "utf8").trimEnd(),
    expected,
  );
  const manifest = JSON.parse(
    readFileSync(join(root, "dist/.vite/manifest.json"), "utf8"),
  );
  const chunk = manifest["build/release-notes.md?raw"];
  assert(chunk?.file, "Release notes are missing from the frontend bundle");
  const bundled = await import(
    pathToFileURL(join(root, "dist", chunk.file)).href
  );
  assert.equal(
    bundled.default.trimEnd(),
    expected,
    "Bundled release notes must match the published body",
  );
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  const [tag, feedPath, assetsPath] = process.argv.slice(2);
  const readJson = (path) => JSON.parse(readFileSync(path, "utf8"));
  const release = readJson(assetsPath);
  const version = readJson("package.json").version;
  verifyMacRelease(tag, readJson(feedPath), release, version);
  await verifyBundledReleaseNotes(process.cwd(), version, release.body);
  console.log(
    "Verified Mac release title, assets, updater feed, download URLs, and bundled release notes.",
  );
}
