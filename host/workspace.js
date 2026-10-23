import { execFile, spawn } from "node:child_process";
import { realpathSync } from "node:fs";
import { promisify } from "node:util";
import {
  lstat,
  mkdir,
  open,
  readFile,
  readdir,
  realpath,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import { isAbsolute, relative, resolve, sep } from "node:path";
const exec = promisify(execFile);
const MAX_FILE = 1024 * 1024;
export function workspacePath(root, input, allowRoot = false) {
  if (typeof input !== "string" || input.length > 4096 || input.includes("\0"))
    throw new Error("Invalid workspace path");
  const path = resolve(root, input);
  const rel = relative(root, path);
  if (
    (!allowRoot && !rel) ||
    rel === ".." ||
    rel.startsWith(`..${sep}`) ||
    isAbsolute(rel) ||
    rel.split(sep).some((part) => part.toLowerCase() === ".git")
  )
    throw new Error("Path is outside the workspace");
  return path;
}
export async function existingPath(root, input, allowRoot = false) {
  const path = workspacePath(root, input, allowRoot);
  const actual = await realpath(path);
  const rel = relative(root, actual);
  if (
    (!allowRoot && !rel) ||
    rel === ".." ||
    rel.startsWith(`..${sep}`) ||
    isAbsolute(rel) ||
    rel.split(sep).some((part) => part.toLowerCase() === ".git")
  )
    throw new Error("Path is outside the workspace");
  return actual;
}
async function git(root, args, maxBuffer = 4 * 1024 * 1024) {
  return (
    await exec("git", ["-c", "core.pager=cat", ...args], {
      cwd: root,
      timeout: 10_000,
      maxBuffer,
      encoding: "utf8",
      env: { ...process.env, LC_ALL: "C" },
    })
  ).stdout;
}
function gitWithInput(root, args, input) {
  return new Promise((resolve, reject) => {
    const child = spawn("git", ["-c", "core.pager=cat", ...args], {
      cwd: root,
      stdio: ["pipe", "pipe", "pipe"],
    });
    const stdout = [];
    const stderr = [];
    const timer = setTimeout(() => child.kill(), 10_000);
    child.stdout.on("data", (chunk) => stdout.push(chunk));
    child.stderr.on("data", (chunk) => stderr.push(chunk));
    child.stdin.on("error", reject);
    child.on("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.on("close", (code) => {
      clearTimeout(timer);
      if (code === 0) resolve(Buffer.concat(stdout).toString("utf8"));
      else
        reject(
          new Error(
            Buffer.concat(stderr).toString("utf8") || `git exited ${code}`,
          ),
        );
    });
    child.stdin.end(input);
  });
}
export async function listHostFiles(root, input) {
  const dir = await existingPath(root, input, true);
  if (!(await stat(dir)).isDirectory())
    throw new Error("Path is not a directory");
  const entries = await readdir(dir, { withFileTypes: true });
  if (entries.length > 5000)
    throw new Error("Directory has too many entries to display");
  const visible = entries.filter((entry) => entry.name !== ".DS_Store");
  const paths = visible.map((entry) =>
    relative(root, resolve(dir, entry.name)).split(sep).join("/"),
  );
  let ignored = new Set();
  if (paths.length) {
    try {
      const output = await git(root, ["check-ignore", "-z", "--", ...paths]);
      ignored = new Set(output.split("\0").filter(Boolean));
    } catch {
      /* no ignored entries or a non-Git folder */
    }
  }
  const out = await Promise.all(
    visible.map(async (entry, index) => {
      const path = paths[index];
      const isDir =
        entry.isDirectory() ||
        (entry.isSymbolicLink() &&
          (await existingPath(root, path)
            .then((actual) => stat(actual))
            .then((value) => value.isDirectory())
            .catch(() => false)));
      return {
        name: entry.name,
        path,
        isDir,
        ignored: entry.name === ".git" || ignored.has(path),
      };
    }),
  );
  return out.sort(
    (a, b) =>
      Number(b.isDir) - Number(a.isDir) ||
      a.name.localeCompare(b.name, undefined, {
        numeric: true,
        sensitivity: "base",
      }),
  );
}
/** Upper bound on paths sent for Go to File, as for a local project. */
const MAX_INDEXED_FILES = 20_000;
const HOST_FILE_INDEX_TTL_MS = 1_000;
const HOST_FILE_INDEX_MAX_ROOTS = 24;
const HOST_FILE_INDEX_MAX_BYTES = 8 * 1024 * 1024;
/** A short-lived, bounded index shared by the picker and fuzzy file search. */
export class HostFileIndexCache {
  load;
  ttlMs;
  maxRoots;
  maxBytes;
  entries = new Map();
  flights = new Map();
  bytes = 0;
  constructor(
    load,
    ttlMs = HOST_FILE_INDEX_TTL_MS,
    maxRoots = HOST_FILE_INDEX_MAX_ROOTS,
    maxBytes = HOST_FILE_INDEX_MAX_BYTES,
  ) {
    this.load = load;
    this.ttlMs = ttlMs;
    this.maxRoots = maxRoots;
    this.maxBytes = maxBytes;
  }
  get(root) {
    const cached = this.entries.get(root);
    if (cached && cached.expiresAt > Date.now()) {
      this.entries.delete(root);
      this.entries.set(root, cached);
      return Promise.resolve(cached.paths);
    }
    if (cached) this.remove(root);
    const existing = this.flights.get(root);
    if (existing) return existing.promise;
    const canTrack = this.flights.size < this.maxRoots;
    const flight = { promise: Promise.resolve([]) };
    flight.promise = this.load(root).then(
      (paths) => {
        if (canTrack && this.flights.get(root) === flight) {
          this.flights.delete(root);
          const weight = paths.reduce(
            (sum, path) => sum + path.length * 2 + 48,
            0,
          );
          if (weight <= this.maxBytes) {
            this.entries.set(root, {
              paths,
              weight,
              expiresAt: Date.now() + this.ttlMs,
            });
            this.bytes += weight;
            this.trim();
          }
        }
        return paths;
      },
      (error) => {
        if (this.flights.get(root) === flight) this.flights.delete(root);
        throw error;
      },
    );
    if (canTrack) this.flights.set(root, flight);
    return flight.promise;
  }
  invalidate(root) {
    this.remove(root);
    // An old request may still finish for its callers, but it cannot publish
    // into the cache after invalidation because its flight identity is gone.
    this.flights.delete(root);
  }
  remove(root) {
    const entry = this.entries.get(root);
    if (!entry) return;
    this.entries.delete(root);
    this.bytes -= entry.weight;
  }
  trim() {
    while (this.entries.size > this.maxRoots || this.bytes > this.maxBytes) {
      const oldest = this.entries.keys().next().value;
      if (oldest === undefined) return;
      this.remove(oldest);
    }
  }
}
/** Every file in the worktree, honoring .gitignore when it is a repository. */
async function hostFilePaths(root) {
  let paths;
  let repository = false;
  try {
    repository =
      (await git(root, ["rev-parse", "--is-inside-work-tree"])).trim() ===
      "true";
  } catch (error) {
    if (!String(error.stderr).includes("not a git repository")) throw error;
  }
  if (repository) {
    paths = (
      await git(
        root,
        ["ls-files", "-z", "--cached", "--others", "--exclude-standard"],
        8 * 1024 * 1024,
      )
    )
      .split("\0")
      .filter(Boolean);
  } else {
    paths = [];
    const pending = [""];
    while (pending.length && paths.length < 5000) {
      const dir = pending.pop();
      const entries = await readdir(resolve(root, dir), {
        withFileTypes: true,
      }).catch(() => []);
      for (const entry of entries) {
        if (entry.name === ".git" || entry.name === "node_modules") continue;
        const path = dir ? `${dir}/${entry.name}` : entry.name;
        if (entry.isDirectory()) pending.push(path);
        else if (entry.isFile()) paths.push(path);
      }
    }
  }
  return paths;
}
const hostFileIndex = new HostFileIndexCache(hostFilePaths);
async function canonicalFileIndexRoot(root) {
  return realpath(root);
}
/** Invalidate both filename search and Go to File indexes for one checkout. */
export function invalidateHostFileIndex(root) {
  let canonical;
  try {
    canonical = realpathSync.native(root);
  } catch {
    canonical = resolve(root);
  }
  hostFileIndex.invalidate(canonical);
}
async function indexedHostFilePaths(root) {
  const canonical = await canonicalFileIndexRoot(root);
  return hostFileIndex.get(canonical);
}
/** Relative paths of the worktree's files, for fuzzy Go to File on the client. */
export async function indexHostFiles(root) {
  return (await indexedHostFilePaths(root)).slice(0, MAX_INDEXED_FILES);
}
/** Bounded filename search for the remote Explorer. */
export async function searchHostFiles(root, input) {
  if (typeof input !== "string" || input.length > 200)
    throw new Error("Invalid search");
  const query = input.trim().toLocaleLowerCase();
  if (!query) return [];
  return (await indexedHostFilePaths(root))
    .filter((path) => path.toLocaleLowerCase().includes(query))
    .slice(0, 200)
    .map((path) => ({
      name: path.split("/").pop() || path,
      path,
      isDir: false,
      ignored: false,
    }));
}
function searchOptions(input) {
  if (!input || typeof input !== "object" || Array.isArray(input))
    throw new Error("Invalid search");
  const value = input;
  if (
    typeof value.query !== "string" ||
    value.query.length > 200 ||
    value.query.includes("\0") ||
    [value.include, value.exclude].some(
      (item) =>
        item !== undefined &&
        (typeof item !== "string" || item.length > 1000 || item.includes("\0")),
    )
  )
    throw new Error("Invalid search");
  for (const key of ["caseSensitive", "wholeWord", "regex"])
    if (value[key] !== undefined && typeof value[key] !== "boolean")
      throw new Error("Invalid search");
  const tokens = (item) =>
    (typeof item === "string" ? item : "")
      .split(",")
      .map((part) => part.trim())
      .filter(Boolean);
  const query = value.query.trim();
  return {
    query,
    columnPattern: new RegExp(
      query.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"),
      value.caseSensitive === true ? "g" : "gi",
    ),
    caseSensitive: value.caseSensitive === true,
    wholeWord: value.wholeWord === true,
    regex: value.regex === true,
    include: tokens(value.include),
    exclude: tokens(value.exclude),
  };
}
function searchColumn(line, options) {
  if (options.regex) return 1;
  for (const match of line.matchAll(options.columnPattern)) {
    if (options.wholeWord) {
      const before = line[match.index - 1];
      const after = line[match.index + match[0].length];
      if (
        (before && /[\p{L}\p{N}_]/u.test(before)) ||
        (after && /[\p{L}\p{N}_]/u.test(after))
      )
        continue;
    }
    return match.index + 1;
  }
  return 0;
}
/** Search file contents with the same controls and result shape as local search. */
export async function searchHostContent(root, input) {
  const options = searchOptions(input);
  if (!options.query) return { matches: [], truncated: false };
  const args = ["grep", "-z", "-n"];
  if (!options.caseSensitive) args.push("-i");
  if (options.wholeWord) args.push("-w");
  args.push(options.regex ? "-E" : "-F", "-e", options.query, "--");
  args.push(
    ...options.include,
    ...options.exclude.map((glob) => `:(exclude)${glob}`),
  );
  try {
    const output = await git(root, args, 8 * 1024 * 1024);
    const matches = [];
    let offset = 0;
    while (offset < output.length && matches.length < 501) {
      const pathEnd = output.indexOf("\0", offset);
      const lineEnd = output.indexOf("\0", pathEnd + 1);
      const previewEnd = output.indexOf("\n", lineEnd + 1);
      if (pathEnd < 0 || lineEnd < 0) break;
      const relative = output.slice(offset, pathEnd).replace(/\\/g, "/");
      const preview = output.slice(
        lineEnd + 1,
        previewEnd < 0 ? undefined : previewEnd,
      );
      offset = previewEnd < 0 ? output.length : previewEnd + 1;
      if (
        !relative ||
        !Number.isFinite(Number(output.slice(pathEnd + 1, lineEnd)))
      )
        continue;
      const path = workspacePath(root, relative);
      matches.push({
        path: path.split(sep).join("/"),
        relative,
        line: Number(output.slice(pathEnd + 1, lineEnd)),
        column: searchColumn(preview, options) || 1,
        preview,
      });
    }
    return { matches: matches.slice(0, 500), truncated: matches.length > 500 };
  } catch (reason) {
    const code = reason.code;
    if (code === 1 || code === "1") return { matches: [], truncated: false };
    if (
      !/not a git repository|not in a git directory|maxBuffer/i.test(
        String(reason),
      )
    )
      throw reason;
  }
  // Ordinary folders use a bounded text scan, like the local fallback.
  if (options.regex) return { matches: [], truncated: false };
  const matches = [];
  const pending = [""];
  let files = 0;
  let truncated = false;
  while (pending.length && matches.length <= 500 && files < 5000) {
    const parent = pending.pop();
    const entries = await readdir(resolve(root, parent), {
      withFileTypes: true,
    }).catch(() => []);
    for (const entry of entries) {
      if (entry.name === ".git" || entry.name === "node_modules") continue;
      const relative = parent ? `${parent}/${entry.name}` : entry.name;
      if (entry.isDirectory()) {
        pending.push(relative);
        continue;
      }
      if (!entry.isFile()) continue;
      files++;
      if (files > 5000) {
        truncated = true;
        break;
      }
      if (!matchesFilters(relative, options)) continue;
      let content;
      let path;
      try {
        path = await existingPath(root, relative);
        if ((await stat(path)).size > 512 * 1024) continue;
        content = await readHostFile(root, relative);
      } catch {
        continue;
      }
      for (const [index, line] of content.split(/\r?\n/).entries()) {
        const column = searchColumn(line, options);
        if (!column) continue;
        matches.push({
          path: path.split(sep).join("/"),
          relative,
          line: index + 1,
          column,
          preview: line,
        });
        if (matches.length > 500) {
          truncated = true;
          break;
        }
      }
      if (matches.length > 500) break;
    }
  }
  if (files >= 5000 && pending.length) truncated = true;
  return { matches: matches.slice(0, 500), truncated };
}
function matchesFilters(path, options) {
  const matches = (glob) => {
    const normalized = glob.replace(/^\.\//, "");
    if (!/[?*]/.test(normalized))
      return path === normalized || path.startsWith(`${normalized}/`);
    const pattern = normalized
      .replace(/[.+^${}()|[\]\\]/g, "\\$&")
      .replace(/\*\*/g, "\u0000")
      .replace(/\*/g, "[^/]*")
      .replace(/\?/g, "[^/]")
      .replace(/\u0000/g, ".*");
    const regex = new RegExp(`^${pattern}$`);
    return (
      regex.test(path) ||
      (!normalized.includes("/") && regex.test(path.split("/").pop()))
    );
  };
  return (
    (!options.include.length || options.include.some(matches)) &&
    !options.exclude.some(matches)
  );
}
export async function readHostFile(root, input) {
  const path = await existingPath(root, input);
  const info = await stat(path);
  if (!info.isFile()) throw new Error("Path is not a file");
  if (info.size > MAX_FILE) throw new Error("File is too large to preview");
  const bytes = await readFile(path);
  if (bytes.includes(0)) throw new Error("Binary file cannot be previewed");
  return bytes.toString("utf8");
}
export async function writeHostFile(root, input, expected, content) {
  if (
    typeof expected !== "string" ||
    typeof content !== "string" ||
    Buffer.byteLength(content, "utf8") > MAX_FILE ||
    content.includes("\0")
  )
    throw new Error("Invalid file content");
  const path = await existingPath(root, input);
  const current = await readHostFile(root, input);
  if (current !== expected)
    throw new Error("File changed on the host; reload before saving");
  await writeFile(path, content, "utf8");
  invalidateHostFileIndex(root);
}
/** Create a new file or folder under an existing workspace directory. */
export async function createHostPath(root, parent, name, isDir) {
  if (
    typeof name !== "string" ||
    typeof isDir !== "boolean" ||
    !name ||
    name.length > 4096 ||
    name.includes("\0") ||
    /^[\\/]/.test(name)
  )
    throw new Error("Invalid file name");
  const parts = name.replace(/[\\/]+$/, "").split(/[\\/]/);
  if (
    !parts.length ||
    parts.some(
      (part) =>
        !part ||
        part === "." ||
        part === ".." ||
        part.toLowerCase() === ".git" ||
        part.length > 255 ||
        /^\s+$/.test(part),
    )
  )
    throw new Error("Invalid file name");
  let directory = await existingPath(root, parent, true);
  if (!(await stat(directory)).isDirectory())
    throw new Error("Path is not a directory");
  for (const part of parts.slice(0, -1)) {
    const candidate = workspacePath(
      root,
      relative(root, resolve(directory, part)),
    );
    await mkdir(candidate).catch((reason) => {
      if (reason.code !== "EEXIST") throw reason;
    });
    directory = await existingPath(root, relative(root, candidate));
    if (!(await stat(directory)).isDirectory())
      throw new Error("Path is not a directory");
  }
  const path = workspacePath(
    root,
    relative(root, resolve(directory, parts.at(-1))),
  );
  if (isDir) await mkdir(path);
  else {
    const file = await open(path, "wx");
    await file.close();
  }
  invalidateHostFileIndex(root);
  return relative(root, path).split(sep).join("/");
}
function statusName(code) {
  if (code === "??") return "untracked";
  if (code.includes("D")) return "deleted";
  if (code.includes("A") || code.includes("?")) return "added";
  return "modified";
}
export async function hostGitIndex(root) {
  // An ordinary folder has an empty Changes view, just as it does locally.
  const repository = await git(root, [
    "rev-parse",
    "--is-inside-work-tree",
  ]).catch((error) => {
    if (String(error).includes("not a git repository")) return "";
    throw error;
  });
  if (repository.trim() !== "true") {
    return {
      branch: null,
      head: null,
      files: [],
      additions: 0,
      deletions: 0,
      remote: null,
      upstream: null,
      defaultBranch: null,
      ahead: 0,
      behind: 0,
      aheadOfDefault: 0,
      headPushed: false,
    };
  }
  const [
    branchText,
    headText,
    statusText,
    numstat,
    remoteText,
    upstreamText,
    countsText,
    defaultText,
  ] = await Promise.all([
    git(root, ["symbolic-ref", "--quiet", "--short", "HEAD"]).catch(() => ""),
    git(root, ["rev-parse", "--verify", "HEAD"]).catch(() => ""),
    git(root, ["status", "--porcelain=v1", "-z", "--untracked-files=all"]),
    git(root, [
      "diff",
      "--no-ext-diff",
      "--no-textconv",
      "--numstat",
      "HEAD",
      "--",
    ]).catch(() => ""),
    git(root, ["remote", "get-url", "origin"]).catch(() => ""),
    git(root, [
      "rev-parse",
      "--abbrev-ref",
      "--symbolic-full-name",
      "@{upstream}",
    ]).catch(() => ""),
    git(root, [
      "rev-list",
      "--left-right",
      "--count",
      "@{upstream}...HEAD",
    ]).catch(() => ""),
    git(root, [
      "symbolic-ref",
      "--quiet",
      "--short",
      "refs/remotes/origin/HEAD",
    ]).catch(() => ""),
  ]);
  let [behind, ahead] = countsText.trim().split(/\s+/).map(Number);
  let defaultBranch = defaultText.trim().replace(/^origin\//, "");
  if (!defaultBranch && remoteText.trim()) {
    for (const name of ["main", "master"]) {
      const exists = await git(root, [
        "rev-parse",
        "--verify",
        `refs/remotes/origin/${name}`,
      ]).then(
        () => true,
        () => false,
      );
      if (exists) {
        defaultBranch = name;
        break;
      }
    }
  }
  if (!upstreamText.trim() && defaultBranch) {
    const fallback = await git(root, [
      "rev-list",
      "--left-right",
      "--count",
      `origin/${defaultBranch}...HEAD`,
    ]).catch(() => "");
    [behind, ahead] = fallback.trim().split(/\s+/).map(Number);
  }
  const headPushed = Boolean(
    (
      await git(root, [
        "for-each-ref",
        "--count=1",
        "--contains",
        "HEAD",
        "refs/remotes",
      ]).catch(() => "")
    ).trim(),
  );
  const aheadOfDefault = defaultBranch
    ? Number(
        (
          await git(root, [
            "rev-list",
            "--count",
            `origin/${defaultBranch}..HEAD`,
          ]).catch(() => "0")
        ).trim(),
      ) || 0
    : 0;
  const counts = new Map();
  for (const line of numstat.split("\n")) {
    const match = /^(\d+|-)\t(\d+|-)\t(.+)$/.exec(line);
    if (match)
      counts.set(match[3], {
        additions: Number(match[1]) || 0,
        deletions: Number(match[2]) || 0,
      });
  }
  const parts = statusText.split("\0");
  const files = [];
  for (let index = 0; index < parts.length; index++) {
    const item = parts[index];
    if (!item || item.length < 4) continue;
    const code = item.slice(0, 2);
    const path = item.slice(3);
    if (code.includes("R") || code.includes("C")) index++;
    const count = counts.get(path) ?? { additions: 0, deletions: 0 };
    if (code === "??") {
      try {
        const info = await lstat(resolve(root, path));
        if (info.size <= MAX_FILE && info.isFile()) {
          const content = await readFile(resolve(root, path), "utf8");
          count.additions = content
            ? content.split("\n").length - Number(content.endsWith("\n"))
            : 0;
        }
      } catch {
        /* a file may change during refresh */
      }
    }
    files.push({
      path,
      relative: path,
      status: statusName(code),
      additions: count.additions,
      deletions: count.deletions,
      staged: code[0] !== " " && code !== "??",
      unstaged: code[1] !== " " || code === "??",
    });
  }
  files.sort((a, b) => a.relative.localeCompare(b.relative));
  return {
    branch: branchText.trim() || null,
    head: headText.trim() || null,
    files,
    additions: files.reduce((sum, file) => sum + file.additions, 0),
    deletions: files.reduce((sum, file) => sum + file.deletions, 0),
    remote: remoteText.trim() ? "origin" : null,
    upstream: upstreamText.trim() || null,
    defaultBranch: defaultBranch || null,
    ahead: ahead || 0,
    behind: behind || 0,
    aheadOfDefault,
    headPushed,
  };
}
async function gitBlob(root, spec) {
  try {
    const text = await git(root, ["show", spec], MAX_FILE + 1024);
    return { text, binary: text.includes("\0"), tooLarge: false };
  } catch (error) {
    return {
      text: "",
      binary: false,
      tooLarge: String(error).includes("maxBuffer"),
    };
  }
}
export async function hostFileDiff(root, input, staged) {
  const path = workspacePath(root, input);
  const relativePath = relative(root, path).split(sep).join("/");
  const index = await hostGitIndex(root);
  const file = index.files.find((entry) => entry.relative === relativePath);
  if (!file) throw new Error("File has no uncommitted changes");
  const original = await gitBlob(
    root,
    `${staged ? "HEAD" : ""}:${relativePath}`,
  );
  const current = staged
    ? await gitBlob(root, `:${relativePath}`)
    : await readHostFile(root, relativePath).then(
        (text) => ({ text, binary: false, tooLarge: false }),
        (error) => {
          if (file.status === "deleted")
            return { text: "", binary: false, tooLarge: false };
          if (String(error).includes("too large"))
            return { text: "", binary: false, tooLarge: true };
          if (String(error).includes("Binary file"))
            return { text: "", binary: true, tooLarge: false };
          throw error;
        },
      );
  return {
    path: relativePath,
    relative: relativePath,
    status: file.status,
    original: original.text,
    current: current.text,
    binary: original.binary || current.binary,
    tooLarge: original.tooLarge || current.tooLarge,
  };
}
async function performHostGitAction(root, action, input, message, contents) {
  const discard = async (file) => {
    const path = relative(root, workspacePath(root, file.relative));
    if (file.status === "untracked") await rm(resolve(root, path));
    else await git(root, ["restore", "--worktree", "--", path]);
  };
  switch (action) {
    case "stageContents": {
      const path = relative(root, workspacePath(root, input))
        .split(sep)
        .join("/");
      if (
        typeof contents !== "string" ||
        Buffer.byteLength(contents) > MAX_FILE
      )
        throw new Error("File too large or invalid");
      const hash = (
        await gitWithInput(
          root,
          ["hash-object", "-w", "--path", path, "--stdin"],
          contents,
        )
      ).trim();
      const staged = await git(root, ["ls-files", "--stage", "--", path]);
      const mode = staged.match(/^([0-7]{6})\s/)?.[1] ?? "100644";
      await git(root, [
        "update-index",
        "--add",
        "--cacheinfo",
        mode,
        hash,
        path,
      ]);
      return;
    }
    case "stage":
    case "unstage": {
      const path = relative(root, workspacePath(root, input));
      await git(
        root,
        action === "stage"
          ? ["--literal-pathspecs", "add", "--", path]
          : ["--literal-pathspecs", "restore", "--staged", "--", path],
      );
      return;
    }
    case "stageAll":
      await git(root, ["add", "-A", "--", "."]);
      return;
    case "unstageAll":
      await git(root, ["reset", "-q", "--", "."]);
      return;
    case "discard": {
      const path = relative(root, workspacePath(root, input))
        .split(sep)
        .join("/");
      const file = (await hostGitIndex(root)).files.find(
        (entry) => entry.relative === path && entry.unstaged,
      );
      if (!file) throw new Error("File has no changes to discard");
      await discard(file);
      return;
    }
    case "discardAll":
      for (const file of (await hostGitIndex(root)).files.filter(
        (entry) => entry.unstaged,
      ))
        await discard(file);
      return;
    case "commit": {
      if (
        typeof message !== "string" ||
        !message.trim() ||
        message.length > 100_000
      )
        throw new Error("Enter a commit message");
      await git(root, ["commit", "-m", message], 1024 * 1024);
      return;
    }
    case "push":
      await git(root, ["push", "-u", "origin", "HEAD"]);
      return;
    case "createPr": {
      const output = await exec("gh", ["pr", "create", "--fill"], {
        cwd: root,
        timeout: 30_000,
        maxBuffer: 1024 * 1024,
        env: {
          ...process.env,
          GH_PROMPT_DISABLED: "1",
          GIT_TERMINAL_PROMPT: "0",
        },
      });
      return output.stdout.trim();
    }
    default:
      throw new Error("Unsupported Git action");
  }
}
export async function hostGitAction(root, action, input, message, contents) {
  const result = await performHostGitAction(
    root,
    action,
    input,
    message,
    contents,
  );
  if (action === "discard" || action === "discardAll")
    invalidateHostFileIndex(root);
  return result;
}
