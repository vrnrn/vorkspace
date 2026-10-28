import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, expect, it } from "vitest";
import {
  createHostBranch,
  hostBranches,
  switchHostBranch,
} from "./git-branches.js";
const dirs = [];
afterEach(() => {
  for (const dir of dirs.splice(0))
    rmSync(dir, { recursive: true, force: true });
});
it("lists and switches only clean existing local branches", async () => {
  const cwd = mkdtempSync(join(tmpdir(), "vorkspace-host-branches-"));
  dirs.push(cwd);
  const git = (...args) => execFileSync("git", args, { cwd });
  git("init");
  git("checkout", "-b", "main");
  writeFileSync(join(cwd, "file.txt"), "initial");
  git("add", "file.txt");
  git(
    "-c",
    "user.name=Test",
    "-c",
    "user.email=test@example.com",
    "commit",
    "-m",
    "initial",
  );
  git("branch", "feature");
  expect(await hostBranches(cwd)).toEqual({
    current: "main",
    branches: ["feature", "main"],
    remotes: [],
  });
  expect(await switchHostBranch(cwd, "feature")).toMatchObject({
    current: "feature",
  });
  await expect(switchHostBranch(cwd, "missing")).rejects.toThrow(
    "existing local branch",
  );
  writeFileSync(join(cwd, "file.txt"), "changed");
  await expect(switchHostBranch(cwd, "main")).rejects.toThrow(
    "Commit or stash",
  );
  expect((await hostBranches(cwd)).current).toBe("feature");
  await expect(createHostBranch(cwd, "new-feature")).rejects.toThrow(
    "Commit or stash",
  );
  writeFileSync(join(cwd, "file.txt"), "initial");
  expect(await createHostBranch(cwd, "new-feature")).toMatchObject({
    current: "new-feature",
  });
  git("remote", "add", "origin", cwd);
  git("update-ref", "refs/remotes/origin/review", "HEAD");
  expect((await hostBranches(cwd)).remotes).toContainEqual({
    remote: "origin",
    name: "review",
  });
  expect(await switchHostBranch(cwd, "review", "origin")).toMatchObject({
    current: "review",
  });
});

it("only reuses a local branch for its selected upstream and respects occupied worktrees", async () => {
  const root = mkdtempSync(join(tmpdir(), "vorkspace-host-tracking-"));
  dirs.push(root);
  const cwd = join(root, "project");
  mkdirSync(cwd);
  const git = (...args) => execFileSync("git", args, { cwd });
  git("init", "-b", "main");
  writeFileSync(join(cwd, "file"), "original");
  git("add", "file");
  git(
    "-c", "user.name=Test", "-c", "user.email=test@example.com",
    "-c", "commit.gpgsign=false", "commit", "-m", "initial",
  );
  for (const remote of ["origin", "other"]) {
    git("remote", "add", remote, "/unused");
    git("update-ref", `refs/remotes/${remote}/main`, "HEAD");
  }
  git("branch", "--set-upstream-to=origin/main", "main");
  git("switch", "-c", "feature");
  await expect(switchHostBranch(cwd, "main", "other")).rejects.toThrow(
    "does not track",
  );
  writeFileSync(join(cwd, "file"), "unsaved");
  await expect(switchHostBranch(cwd, "main", "origin")).rejects.toThrow("Commit or stash");
  expect(await switchHostBranch(cwd, "feature")).toMatchObject({ current: "feature" });
  writeFileSync(join(cwd, "file"), "original");
  expect(await switchHostBranch(cwd, "main", "origin")).toMatchObject({
    current: "main",
  });
  git("switch", "feature");
  const occupied = join(root, "occupied");
  git("worktree", "add", occupied, "main");
  await expect(switchHostBranch(cwd, "main", "origin")).rejects.toThrow("already");
  expect((await hostBranches(cwd)).current).toBe("feature");
  expect(git("-C", occupied, "branch", "--show-current").toString().trim()).toBe(
    "main",
  );
});
