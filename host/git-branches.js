import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { invalidateHostFileIndex } from "./workspace.js";
const exec = promisify(execFile);
const options = (cwd) => ({
  cwd,
  timeout: 10_000,
  maxBuffer: 1024 * 1024,
});
function currentBranch(cwd) {
  return exec("git", ["symbolic-ref", "--quiet", "HEAD"], options(cwd))
    .then(({ stdout }) => stdout.trim().replace(/^refs\/heads\//, ""))
    .catch(() => null);
}
export async function hostBranches(cwd, branch = currentBranch(cwd)) {
  const [{ stdout }, current] = await Promise.all([
    exec(
      "git",
      [
        "for-each-ref",
        "--format=%(refname)%00%(symref)",
        "refs/heads",
        "refs/remotes",
      ],
      { ...options(cwd), maxBuffer: 2 * 1024 * 1024 },
    ),
    Promise.resolve(branch).then((value) =>
      value === undefined ? currentBranch(cwd) : value,
    ),
  ]);
  const branches = [];
  const remotes = [];
  for (const line of stdout.split("\n")) {
    const [ref, symref] = line.split("\0");
    if (ref.startsWith("refs/heads/")) {
      branches.push(ref.slice("refs/heads/".length));
    } else if (ref.startsWith("refs/remotes/") && !symref) {
      const name = ref.slice("refs/remotes/".length);
      const slash = name.indexOf("/");
      if (slash > 0)
        remotes.push({ remote: name.slice(0, slash), name: name.slice(slash + 1) });
    }
  }
  return { current, branches, remotes };
}
export async function switchHostBranch(cwd, branch, remote) {
  if (
    typeof branch !== "string" ||
    branch.length > 255 ||
    !branch ||
    branch.startsWith("-")
  )
    throw new Error("Invalid branch");
  const state = await hostBranches(cwd);
  const remoteRef =
    typeof remote === "string" &&
    state.remotes.some(
      (entry) => entry.remote === remote && entry.name === branch,
    )
      ? `refs/remotes/${remote}/${branch}`
      : null;
  if (remote != null && !remoteRef)
    throw new Error("Choose an available remote branch");
  if (!state.branches.includes(branch) && !remoteRef)
    throw new Error("Choose an existing local branch");
  if (remoteRef && state.branches.includes(branch)) {
    const { stdout: upstream } = await exec(
      "git",
      ["for-each-ref", "--format=%(upstream)", `refs/heads/${branch}`],
      options(cwd),
    );
    if (upstream.trim() !== remoteRef)
      throw new Error(
        `Local branch ‘${branch}’ already exists and does not track ${remote}/${branch}. Select the local branch instead.`,
      );
  }
  if (state.current === branch) return state;
  const { stdout: changes } = await exec(
    "git",
    ["status", "--porcelain", "--untracked-files=all"],
    options(cwd),
  );
  if (changes)
    throw new Error(
      "Commit or stash changes on the host before switching branches",
    );
  if (remoteRef && !state.branches.includes(branch))
    await exec(
      "git",
      ["switch", "--track", "-c", branch, remoteRef],
      options(cwd),
    );
  else await exec("git", ["switch", "--no-guess", "--", branch], options(cwd));
  invalidateHostFileIndex(cwd);
  return hostBranches(cwd);
}
export async function createHostBranch(cwd, branch) {
  if (
    typeof branch !== "string" ||
    !branch ||
    branch.length > 255 ||
    branch.startsWith("-") ||
    branch.startsWith("@")
  )
    throw new Error("Enter a valid branch name");
  await exec("git", ["check-ref-format", "--branch", branch], options(cwd));
  const state = await hostBranches(cwd);
  if (state.branches.includes(branch)) throw new Error("Branch already exists");
  const { stdout: changes } = await exec(
    "git",
    ["status", "--porcelain", "--untracked-files=all"],
    options(cwd),
  );
  if (changes)
    throw new Error(
      "Commit or stash changes on the host before switching branches",
    );
  await exec("git", ["switch", "-c", branch], options(cwd));
  invalidateHostFileIndex(cwd);
  return hostBranches(cwd);
}
