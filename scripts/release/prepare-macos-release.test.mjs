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
  assertDraftOwnership,
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
const plan = (releases = [], context = env, version = "1.0.8") =>
  planMacRelease(version, releases, context);

test("native release preparation retains the 1.x floor above older desktop versions", () => {
  assert.equal(planMacRelease("1.0.0", [release("0.13.13")], env).version, "1.0.0");
  assert.throws(() => planMacRelease("0.99.0", [], env), /1.0.0 or newer/);
});

test("release mutations revalidate draft ownership after a build", () => {
  const prepared = plan();
  const draft = release(prepared.version, { draft: true, body: prepared.notes });
  assert.doesNotThrow(() => assertDraftOwnership(prepared, draft, env.GITHUB_SHA));
  for (const changed of [{ draft: false }, { target_commitish: "another commit" }, { body: "another run" }, { tag_name: "mac-v1.0.99" }]) {
    assert.throws(() => assertDraftOwnership(prepared, { ...draft, ...changed }, env.GITHUB_SHA));
  }
});

test("the checked-in version is the minimum, and existing releases advance the patch", () => {
  assert.equal(plan().version, "1.0.8");
  assert.equal(plan([release("1.0.8")]).version, "1.0.9");
  assert.equal(
    plan([release("1.0.10"), release("1.0.8"), release("1.0.9")]).version,
    "1.0.11",
  );
  assert.equal(plan([release("1.0.8")], env, "1.1.0").version, "1.1.0");
  assert.equal(plan([release("1.1.9")]).version, "1.1.10");
  assert.equal(plan([release("1.0.99")]).version, "1.0.100");
});

test("drafts reserve versions, while unrelated and prerelease tags are ignored", () => {
  const result = plan([
    release("1.0.8"),
    release("1.0.9", { draft: true }),
    { tag_name: "host-v8.0.0" },
    { tag_name: "mac-v99.0.0-preview" },
  ]);
  assert.equal(result.version, "1.0.10");
  assert.equal(result.tag, "mac-v1.0.10");
  assert.equal(result.build, "42.1");
  assert.equal(
    planMacRelease("1.0.8", [release("1.0.8")], env, ["mac-v1.0.9"]).version,
    "1.0.10",
  );
});

test("a retry resumes only its own draft and advances the native attempt number", () => {
  const own = release("1.0.9", {
    draft: true,
    body: `Notes\nWorkflow: ${plan().workflow}\n`,
  });
  const retry = { ...env, GITHUB_RUN_ATTEMPT: "2" };
  assert.equal(plan([release("1.0.8"), own], retry).version, "1.0.9");
  assert.equal(plan([own], retry).build, "42.2");
  assert.equal(
    plan([own], { ...retry, GITHUB_RUN_ID: "1001" }).version,
    "1.0.10",
  );
  assert.equal(
    plan([own], { ...retry, GITHUB_SHA: "b".repeat(40) }).version,
    "1.0.10",
  );
  assert.throws(() => plan([own, own], retry), /multiple release drafts/);
  assert.throws(
    () => plan([own, release("1.0.10")], retry),
    /newer release was published/,
  );
  // An already published run is immutable, even when that run is retried.
  assert.equal(plan([{ ...own, draft: false }], retry).version, "1.0.10");
});

test("explicit tags retain their version and published-release protection", () => {
  const tag = {
    ...env,
    GITHUB_REF_TYPE: "tag",
    GITHUB_REF_NAME: "mac-v1.0.8",
  };
  assert.equal(plan([], tag).tag, "mac-v1.0.8");
  assert.equal(
    plan([release("1.0.8", { draft: true })], tag).version,
    "1.0.8",
  );
  assert.throws(() => plan([release("1.0.8")], tag), /cannot be overwritten/);
  assert.throws(
    () =>
      plan(
        [release("1.0.8", { draft: true, target_commitish: "other" })],
        tag,
      ),
    /same commit/,
  );
  assert.throws(
    () => plan([], { ...tag, GITHUB_REF_NAME: "mac-v1.0.9" }),
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
  assert.throws(() => plan([], env, "1.0.8+build.1"));
  assert.throws(() => plan([], env, "01.0.8"));
  assert.throws(() => plan([release("1.0.9007199254740991")]));
});

test("generates notes for the exact source since the newest earlier published stable release", () => {
  const result = plan([release("1.0.10")]);
  const notes = generateMacReleaseNotes(
    result,
    [
      release("1.0.9"),
      release("1.0.10"),
      release("1.0.12", { draft: true }),
      release("1.0.11"),
      { tag_name: "other", draft: false },
    ],
    env.GITHUB_SHA,
    (fields) => {
      assert.deepEqual(fields, {
        tag_name: "mac-v1.0.11",
        target_commitish: env.GITHUB_SHA,
        previous_tag_name: "mac-v1.0.10",
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

test("1.0.0 has milestone notes with shipped changes and separate future plans", () => {
  const milestone = plan([release("0.13.13")], env, "1.0.0");
  const notes = generateMacReleaseNotes(milestone, [], env.GITHUB_SHA, () => {
    assert.fail("The 1.0 milestone uses its curated release notes");
  });
  assert(notes.startsWith("# Vorkspace 1.0\n"));
  const shipped = notes.split("## Up next\n")[0];
  assert(shipped.includes("## In this release\n"));
  for (const text of ["Rust", "GPUI", "Vox", "Copilot", "draft", "Incremental", "Signed updates"])
    assert(shipped.includes(text), text);
  const future = notes.split("## Up next\n")[1].split("## Longer term\n")[0];
  for (const text of ["session management", "diff viewing", "Mermaid, SVG", "Bots and Vox", "performance"])
    assert(future.includes(text), text);
  assert(notes.includes("## Longer term\n\nModularization and plugins"));
  assert(notes.includes("future plans, not features included in 1.0.0"));
  assert(notes.endsWith(milestone.notes));
});

test("stamps all manifests and native build metadata without changing updater identity", () => {
  // Node resolves module URLs through macOS's /var -> /private/var symlink.
  const root = realpathSync(
    mkdtempSync(join(tmpdir(), "vorkspace-release-stamp-")),
  );
  const source = join(dirname(fileURLToPath(import.meta.url)), "../..");
  const paths = [
    "package.json",
    "package-lock.json",
    "Cargo.toml",
    "Cargo.lock",
    "scripts/release/bump-version.mjs",
    "scripts/release/prepare-macos-release.mjs",
  ];
  const json = (path) => JSON.parse(readFileSync(join(root, path), "utf8"));
  try {
    for (const path of paths) {
      mkdirSync(dirname(join(root, path)), { recursive: true });
      copyFileSync(join(source, path), join(root, path));
    }
    const initial = json("package.json").version;
    const target = initial === "9.9.9" ? "9.9.10" : "9.9.9";
    stampMacRelease(root, { ...plan(), version: target });
    assert.equal(
      readFileSync(join(root, "build/release-notes.md"), "utf8"),
      `## [${target}]\n\n${plan().notes}`,
    );
    assert.equal(json("package.json").version, target);
    assert.equal(json("package-lock.json").version, target);
    assert.equal(json("package-lock.json").packages[""].version, target);
    assert.match(
      readFileSync(join(root, "Cargo.toml"), "utf8"),
      new RegExp(`version = "${target.replaceAll(".", "\\.")}"`),
    );
    assert(
      readFileSync(join(root, "Cargo.lock"), "utf8").includes(
        `name = "vorkspace-gpui"\nversion = "${target}"`,
      ),
    );
    assert.deepEqual(json("build/native-release.json"), { version: target, build: "42.1" });
    // Stamping an explicit version or retry does not fail on unchanged versions.
    stampMacRelease(root, { ...plan(), version: target, build: "42.2" });
    assert.equal(
      json("build/native-release.json").build,
      "42.2",
    );
    // Exercise the actual CI entry point without network or repository writes.
    const bin = join(root, "bin");
    mkdirSync(bin);
    const gh = join(bin, "gh");
    writeFileSync(
      gh,
      '#!/usr/bin/env node\nif (process.argv.some(value => value.endsWith("/generate-notes"))) console.log(JSON.stringify({body: "## What’s Changed\\n\\n- Restore release notes.\\nVORKSPACE_RELEASE_NOTES\\nKEEP_INSIDE_NOTES=yes"})); else if (!process.argv.at(-1).includes("matching-refs")) JSON.parse(process.env.RELEASE_TEST_PAGES).flat().forEach(value => console.log(JSON.stringify(value)));\n',
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
    const script = join(root, "scripts/release/prepare-macos-release.mjs");
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
    const delimiter = variables.match(
      /^RELEASE_NOTES<<(VORKSPACE_RELEASE_NOTES_[a-f0-9-]+)$/m,
    )?.[1];
    assert(delimiter);
    assert(
      variables.includes(`RELEASE_NOTES<<${delimiter}\n${notes}${delimiter}\n`),
    );
    // A failed API read must not produce a conflicting version or new notes.
    const beforeFailure = readFileSync(
      join(root, "build/native-release.json"),
      "utf8",
    );
    writeFileSync(gh, "#!/usr/bin/env node\nprocess.exit(1);\n");
    assert.throws(() =>
      execFileSync(process.execPath, [script], { env: ciEnv, stdio: "pipe" }),
    );
    assert.equal(json("package.json").version, expected);
    assert.equal(
      readFileSync(join(root, "build/native-release.json"), "utf8"),
      beforeFailure,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
