import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import {
  appendFileSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const STABLE_TAG = /^mac-v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;

function parts(version) {
  assert.match(version, /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/);
  const result = version.split(".").map(Number);
  assert(
    result.every(Number.isSafeInteger),
    "Version components are too large",
  );
  return result;
}

function compare(left, right) {
  const a = parts(left);
  const b = parts(right);
  return a[0] - b[0] || a[1] - b[1] || a[2] - b[2];
}

export function planMacRelease(version, releases, env, tags = []) {
  parts(version);
  assert(compare(version, "1.0.0") >= 0, "Native releases require version 1.0.0 or newer");
  assert(Array.isArray(releases), "Expected the complete GitHub release list");
  for (const key of [
    "GITHUB_RUN_NUMBER",
    "GITHUB_RUN_ATTEMPT",
    "GITHUB_RUN_ID",
  ])
    assert.match(env[key] ?? "", /^[1-9]\d*$/, `${key} is required`);
  assert.match(env.GITHUB_SHA ?? "", /^[a-f0-9]{40}$/);
  assert.equal(env.GITHUB_REPOSITORY, "vrnrn/vorkspace");
  const workflow = `https://github.com/${env.GITHUB_REPOSITORY}/actions/runs/${env.GITHUB_RUN_ID}`;
  const build = `${env.GITHUB_RUN_NUMBER}.${env.GITHUB_RUN_ATTEMPT}`;
  if (env.GITHUB_REF_TYPE === "tag") {
    assert.equal(
      env.GITHUB_REF_NAME,
      `mac-v${version}`,
      "Tag must match the app version",
    );
    const existing = releases.find(
      (release) => release.tag_name === env.GITHUB_REF_NAME,
    );
    if (existing) {
      assert(existing.draft, "Published releases cannot be overwritten");
      assert.equal(
        existing.target_commitish,
        env.GITHUB_SHA,
        "An existing draft must belong to the same commit",
      );
    }
  } else {
    assert.equal(env.GITHUB_REF_TYPE, "branch", "Dispatch releases from main");
    assert.equal(env.GITHUB_REF_NAME, "main", "Dispatch releases from main");
    // A retry can finish its own draft. A new run never replaces another run's
    // artifacts, even if both happen to build the same source commit.
    const ownDrafts = releases.filter(
      (release) =>
        release.draft &&
        release.target_commitish === env.GITHUB_SHA &&
        (release.body ?? "").split("\n").includes(`Workflow: ${workflow}`),
    );
    assert(ownDrafts.length <= 1, "This run has multiple release drafts");
    if (ownDrafts.length) {
      assert.match(ownDrafts[0].tag_name, /^mac-v/);
      version = ownDrafts[0].tag_name.slice(5);
      parts(version);
      assert(
        !releases.some(
          (release) =>
            !release.draft && compareRelease(release.tag_name, version) >= 0,
        ),
        "A newer release was published; start a new workflow run",
      );
    } else {
      for (const tag of [
        ...releases.map((release) => release.tag_name),
        ...tags,
      ]) {
        if (compareRelease(tag, version) >= 0) {
          const [major, minor, patch] = parts(tag.slice(5));
          version = `${major}.${minor}.${patch + 1}`;
          parts(version);
        }
      }
    }
  }
  const notes = `Personal Apple Silicon Mac build from vrnrn/vorkspace. Uses ad hoc Apple signing and signed in-app updates.\n\nBuild: ${build}\nWorkflow: ${workflow}\n`;
  return { version, tag: `mac-v${version}`, build, workflow, notes };
}

function compareRelease(tag, version) {
  return STABLE_TAG.test(tag ?? "") ? compare(tag.slice(5), version) : -1;
}

export function assertDraftOwnership(plan, release, sha) {
  assert.equal(release.tag_name, plan.tag, "Draft tag changed");
  assert(release.draft, "Published releases cannot be overwritten");
  assert.equal(release.target_commitish, sha, "A draft must belong to the same commit");
  assert((release.body ?? "").split("\n").includes(`Workflow: ${plan.workflow}`), "A draft must belong to this workflow run");
}

export function generateMacReleaseNotes(plan, releases, sha, request) {
  const previous = releases
    .filter(
      (release) =>
        !release.draft &&
        !release.prerelease &&
        STABLE_TAG.test(release.tag_name) &&
        compareRelease(release.tag_name, plan.version) < 0,
    )
    .sort((a, b) => compareRelease(b.tag_name, a.tag_name.slice(5)))[0];
  const result = request({
    tag_name: plan.tag,
    target_commitish: sha,
    ...(previous ? { previous_tag_name: previous.tag_name } : {}),
  });
  assert.equal(
    typeof result.body,
    "string",
    "GitHub must return release notes",
  );
  assert(result.body.trim(), "GitHub returned empty release notes");
  return `${result.body.trim()}\n\n${plan.notes}`;
}

function githubList(endpoint, projection) {
  // Project allocation fields before buffering: asset metadata is irrelevant.
  // @json emits one complete record per line, including across API pages.
  const output = execFileSync(
    "gh",
    ["api", "--paginate", "--jq", `${projection} | @json`, endpoint],
    { encoding: "utf8", maxBuffer: 8 * 1024 * 1024 },
  );
  return output
    .split("\n")
    .filter(Boolean)
    .map((line) => JSON.parse(line));
}

export function stampMacRelease(root, plan) {
  assert(plan.notes?.trim(), "Release notes are required before stamping");
  const readJson = (path) => JSON.parse(readFileSync(join(root, path), "utf8"));
  if (readJson("package.json").version !== plan.version)
    execFileSync(
      process.execPath,
      [join(root, "scripts/release/bump-version.mjs"), plan.version],
      { stdio: "inherit" },
    );
  // This build artifact is bundled lazily; no changelog is kept in the repo.
  mkdirSync(join(root, "build"), { recursive: true });
  writeFileSync(join(root, "build/native-release.json"), `${JSON.stringify({version: plan.version, build: plan.build}, null, 2)}\n`);
  writeFileSync(
    join(root, "build/release-notes.md"),
    `## [${plan.version}]\n\n${plan.notes}`,
  );
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  const root = join(dirname(fileURLToPath(import.meta.url)), "../..");
  const version = JSON.parse(
    readFileSync(join(root, "package.json"), "utf8"),
  ).version;
  // Authentication/network failures abort preparation, rather than treating an
  // unavailable release list as an empty one and risking an overwrite.
  const releases = githubList(
    "repos/vrnrn/vorkspace/releases?per_page=100",
    ".[] | {tag_name, draft, prerelease, target_commitish, body}",
  );
  const tags = githubList(
    "repos/vrnrn/vorkspace/git/matching-refs/tags/mac-v",
    ".[] | .ref",
  ).map((ref) => ref.replace(/^refs\/tags\//, ""));
  const plan = planMacRelease(version, releases, process.env, tags);
  plan.notes = generateMacReleaseNotes(
    plan,
    releases,
    process.env.GITHUB_SHA,
    (fields) =>
      JSON.parse(
        execFileSync(
          "gh",
          [
            "api",
            "--method",
            "POST",
            "repos/vrnrn/vorkspace/releases/generate-notes",
            ...Object.entries(fields).flatMap(([key, value]) => [
              "-f",
              `${key}=${value}`,
            ]),
          ],
          { encoding: "utf8", maxBuffer: 1024 * 1024 },
        ),
      ),
  );
  stampMacRelease(root, plan);
  writeFileSync(
    join(process.env.RUNNER_TEMP, "macos-release-notes.md"),
    plan.notes,
  );
  const notesDelimiter = `VORKSPACE_RELEASE_NOTES_${randomUUID()}`;
  appendFileSync(
    process.env.GITHUB_ENV,
    `RELEASE_TAG=${plan.tag}\nRELEASE_VERSION=${plan.version}\nRELEASE_BUILD_NUMBER=${plan.build}\nVORKSPACE_BUILD_NUMBER=${plan.build}\nVORKSPACE_RELEASE_BUILD=1\nRELEASE_NOTES<<${notesDelimiter}\n${plan.notes}${notesDelimiter}\n`,
  );
  appendFileSync(
    process.env.GITHUB_STEP_SUMMARY,
    `Release: **${plan.version}**, build **${plan.build}**\n`,
  );
  console.log(`Prepared ${plan.tag}, build ${plan.build}`);
}
