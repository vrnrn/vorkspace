import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import {
  generateMacReleaseNotes,
  planMacRelease,
  stampMacRelease,
} from "./prepare-macos-release.mjs";

const env = {
  GITHUB_REPOSITORY: "vrnrn/vorkspace",
  GITHUB_SHA: "a".repeat(40),
  GITHUB_REF_TYPE: "branch",
  GITHUB_REF_NAME: "main",
  GITHUB_RUN_NUMBER: "42",
  GITHUB_RUN_ATTEMPT: "1",
  GITHUB_RUN_ID: "1000",
};
const release = (version, extra = {}) => ({
  tag_name: `mac-v${version}`,
  draft: false,
  target_commitish: env.GITHUB_SHA,
  ...extra,
});
const plan = (releases = [], context = env, version = "0.13.8") =>
  planMacRelease(version, releases, context);

test("the checked-in version is the minimum, and existing releases advance the patch", () => {
  assert.equal(plan().version, "0.13.8");
  assert.equal(plan([release("0.13.8")]).version, "0.13.9");
  assert.equal(
    plan([release("0.13.10"), release("0.13.8"), release("0.13.9")]).version,
    "0.13.11",
  );
  assert.equal(plan([release("0.13.8")], env, "0.14.0").version, "0.14.0");
  assert.equal(plan([release("0.14.9")]).version, "0.14.10");
  assert.equal(plan([release("0.13.99")]).version, "0.13.100");
});

test("drafts reserve versions, while unrelated and prerelease tags are ignored", () => {
  const result = plan([
    release("0.13.8"),
    release("0.13.9", { draft: true }),
    { tag_name: "host-v8.0.0" },
    { tag_name: "mac-v99.0.0-preview" },
  ]);
  assert.equal(result.version, "0.13.10");
  assert.equal(result.tag, "mac-v0.13.10");
  assert.equal(result.build, "42.1");
  assert.equal(
    planMacRelease("0.13.8", [release("0.13.8")], env, ["mac-v0.13.9"]).version,
    "0.13.10",
  );
});

test("a retry resumes only its own draft and advances the native attempt number", () => {
  const own = release("0.13.9", {
    draft: true,
    body: `Notes\nWorkflow: ${plan().workflow}\n`,
  });
  const retry = { ...env, GITHUB_RUN_ATTEMPT: "2" };
  assert.equal(plan([release("0.13.8"), own], retry).version, "0.13.9");
  assert.equal(plan([own], retry).build, "42.2");
  assert.equal(
    plan([own], { ...retry, GITHUB_RUN_ID: "1001" }).version,
    "0.13.10",
  );
  assert.equal(
    plan([own], { ...retry, GITHUB_SHA: "b".repeat(40) }).version,
    "0.13.10",
  );
  assert.throws(() => plan([own, own], retry), /multiple release drafts/);
  assert.throws(
    () => plan([own, release("0.13.10")], retry),
    /newer release was published/,
  );
  // An already published run is immutable, even when that run is retried.
  assert.equal(plan([{ ...own, draft: false }], retry).version, "0.13.10");
});

test("explicit tags retain their version and published-release protection", () => {
  const tag = {
    ...env,
    GITHUB_REF_TYPE: "tag",
    GITHUB_REF_NAME: "mac-v0.13.8",
  };
  assert.equal(plan([], tag).tag, "mac-v0.13.8");
  assert.equal(
    plan([release("0.13.8", { draft: true })], tag).version,
    "0.13.8",
  );
  assert.throws(() => plan([release("0.13.8")], tag), /cannot be overwritten/);
  assert.throws(
    () =>
      plan(
        [release("0.13.8", { draft: true, target_commitish: "other" })],
        tag,
      ),
    /same commit/,
  );
  assert.throws(
    () => plan([], { ...tag, GITHUB_REF_NAME: "mac-v0.13.9" }),
    /Tag must match/,
  );
});

test("rejects invalid source refs, build counters, and versions before stamping", () => {
  assert.throws(() => plan([], { ...env, GITHUB_REF_NAME: "feature" }), /main/);
  assert.throws(
    () => plan([], { ...env, GITHUB_RUN_ATTEMPT: "0" }),
    /GITHUB_RUN_ATTEMPT/,
  );
  assert.throws(
    () => plan([], { ...env, GITHUB_RUN_NUMBER: "42\nBAD=1" }),
    /GITHUB_RUN_NUMBER/,
  );
  assert.throws(() => plan([], { ...env, GITHUB_REPOSITORY: "other/repo" }));
  assert.throws(() => plan([], env, "0.13.8+build.1"));
  assert.throws(() => plan([], env, "00.13.8"));
  assert.throws(() => plan([release("0.13.9007199254740991")]));
});

test("generates notes for the exact source since the newest earlier published stable release", () => {
  const result = plan([release("0.13.10")]);
  const notes = generateMacReleaseNotes(
    result,
    [
      release("0.13.9"),
      release("0.13.10"),
      release("0.13.12", { draft: true }),
      release("0.13.11"),
      { tag_name: "other", draft: false },
    ],
    env.GITHUB_SHA,
    (fields) => {
      assert.deepEqual(fields, {
        tag_name: "mac-v0.13.11",
        target_commitish: env.GITHUB_SHA,
        previous_tag_name: "mac-v0.13.10",
      });
      return { body: "## What's Changed\n\n- Restore release notes." };
    },
  );
  assert(notes.startsWith("## What's Changed\n\n- Restore release notes."));
  assert(notes.includes(`Workflow: ${result.workflow}\n`));
  for (const body of [null, "", " \n"]) {
    assert.throws(() =>
      generateMacReleaseNotes(result, [], env.GITHUB_SHA, () => ({ body })),
    );
  }
});

test("stamps all manifests and native build metadata without changing updater identity", () => {
  // Node resolves module URLs through macOS's /var -> /private/var symlink.
  const root = realpathSync(
    mkdtempSync(join(tmpdir(), "vorkspace-release-stamp-")),
  );
  const source = join(dirname(fileURLToPath(import.meta.url)), "..");
  const paths = [
    "package.json",
    "package-lock.json",
    "Cargo.toml",
    "Cargo.lock",
    "src-tauri/tauri.conf.json",
    "src-tauri/tauri.macos.conf.json",
    "scripts/bump-version.mjs",
    "scripts/prepare-macos-release.mjs",
  ];
  const json = (path) => JSON.parse(readFileSync(join(root, path), "utf8"));
  try {
    for (const path of paths) {
      mkdirSync(dirname(join(root, path)), { recursive: true });
      copyFileSync(join(source, path), join(root, path));
    }
    const initial = json("package.json").version;
    const target = initial === "9.9.9" ? "9.9.10" : "9.9.9";
    const original = json("src-tauri/tauri.macos.conf.json");
    original.bundle = { macOS: { minimumSystemVersion: "12.0" } };
    writeFileSync(
      join(root, "src-tauri/tauri.macos.conf.json"),
      JSON.stringify(original),
    );
    stampMacRelease(root, { ...plan(), version: target });
    assert.equal(
      readFileSync(join(root, "build/release-notes.md"), "utf8"),
      `## [${target}]\n\n${plan().notes}`,
    );
    assert.equal(json("package.json").version, target);
    assert.equal(json("package-lock.json").version, target);
    assert.equal(json("package-lock.json").packages[""].version, target);
    assert.equal(json("src-tauri/tauri.conf.json").version, target);
    assert.match(
      readFileSync(join(root, "Cargo.toml"), "utf8"),
      new RegExp(`version = "${target.replaceAll(".", "\\.")}"`),
    );
    assert(
      readFileSync(join(root, "Cargo.lock"), "utf8").includes(
        `name = "vorkspace"\nversion = "${target}"`,
      ),
    );
    const mac = json("src-tauri/tauri.macos.conf.json");
    assert.equal(mac.bundle.macOS.bundleVersion, "42.1");
    assert.equal(mac.bundle.macOS.minimumSystemVersion, "12.0");
    assert.deepEqual(mac.plugins, original.plugins);
    // Stamping an explicit version or retry does not fail on unchanged versions.
    stampMacRelease(root, { ...plan(), version: target, build: "42.2" });
    assert.equal(
      json("src-tauri/tauri.macos.conf.json").bundle.macOS.bundleVersion,
      "42.2",
    );
    // Exercise the actual CI entry point without network or repository writes.
    const bin = join(root, "bin");
    mkdirSync(bin);
    const gh = join(bin, "gh");
    writeFileSync(
      gh,
      '#!/usr/bin/env node\nif (process.argv.some(value => value.endsWith("/generate-notes"))) console.log(JSON.stringify({body: "## What’s Changed\\n\\n- Restore release notes."})); else if (!process.argv.at(-1).includes("matching-refs")) JSON.parse(process.env.RELEASE_TEST_PAGES).flat().forEach(value => console.log(JSON.stringify(value)));\n',
      { mode: 0o755 },
    );
    const ciEnv = {
      ...process.env,
      ...env,
      PATH: `${bin}:${process.env.PATH}`,
      GITHUB_ENV: join(root, "github-env"),
      GITHUB_STEP_SUMMARY: join(root, "summary"),
      RUNNER_TEMP: root,
      RELEASE_TEST_PAGES: JSON.stringify([[release(target)]]),
    };
    const script = join(root, "scripts/prepare-macos-release.mjs");
    execFileSync(process.execPath, [script], { env: ciEnv });
    const expected = `${target.slice(0, target.lastIndexOf(".") + 1)}${Number(target.split(".").at(-1)) + 1}`;
    assert.equal(json("package.json").version, expected);
    const variables = readFileSync(ciEnv.GITHUB_ENV, "utf8");
    assert(variables.includes(`RELEASE_TAG=mac-v${expected}\n`));
    assert(variables.includes("RELEASE_BUILD_NUMBER=42.1\n"));
    const notes = readFileSync(join(root, "macos-release-notes.md"), "utf8");
    assert(notes.includes("Restore release notes."));
    assert.equal(
      readFileSync(join(root, "build/release-notes.md"), "utf8"),
      `## [${expected}]\n\n${notes}`,
    );
    assert(notes.includes("Build: 42.1\n"));
    assert(notes.includes(`Workflow: ${plan().workflow}\n`));
    assert(
      variables.includes(
        `RELEASE_NOTES<<VORKSPACE_RELEASE_NOTES\n${notes}VORKSPACE_RELEASE_NOTES\n`,
      ),
    );
    // A failed API read must not produce a conflicting version or new notes.
    const beforeFailure = readFileSync(
      join(root, "src-tauri/tauri.macos.conf.json"),
      "utf8",
    );
    writeFileSync(gh, "#!/usr/bin/env node\nprocess.exit(1);\n");
    assert.throws(() =>
      execFileSync(process.execPath, [script], { env: ciEnv, stdio: "pipe" }),
    );
    assert.equal(json("package.json").version, expected);
    assert.equal(
      readFileSync(join(root, "src-tauri/tauri.macos.conf.json"), "utf8"),
      beforeFailure,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
