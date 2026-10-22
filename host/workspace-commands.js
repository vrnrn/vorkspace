import {
  cp,
  lstat,
  mkdir,
  mkdtemp,
  open,
  readFile,
  realpath,
  rename,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import {
  basename,
  dirname,
  extname,
  isAbsolute,
  join,
  relative,
  resolve,
} from "node:path";
import { tmpdir } from "node:os";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { createHash, randomUUID } from "node:crypto";
import { hostWorktrees } from "./git-worktrees.js";
import {
  createHostBranch,
  hostBranches,
  switchHostBranch,
} from "./git-branches.js";
import {
  createHostPath,
  existingPath,
  hostFileDiff,
  hostGitAction,
  hostGitIndex,
  indexHostFiles,
  invalidateHostFileIndex,
  listHostFiles,
  searchHostContent,
  workspacePath,
} from "./workspace.js";
const exec = promisify(execFile);
/** Local file commands the host answers for a remote project, with the same
 * names, arguments and results as this app's Tauri commands, so the same UI
 * works on either machine. Paths are absolute host paths and must lie inside
 * a registered project or one of its worktrees. */
export const WORKSPACE_COMMANDS = [
  "list_dir",
  "list_project_files",
  "read_text_file",
  "read_editor_text_file",
  "read_binary_file",
  "read_file_preview",
  "write_text_file",
  "write_editor_text_file",
  "stat_files",
  "create_path",
  "rename_path",
  "delete_path",
  "copy_path",
  "move_path",
  "git_diff_index",
  "git_diff_files",
  "git_diff_stats",
  "git_file_diff",
  "git_stage_contents",
  "git_stage_file",
  "git_unstage_file",
  "git_discard_file",
  "git_discard_all",
  "git_stage_all",
  "git_unstage_all",
  "git_commit",
  "git_locate_files",
  "git_head_message",
  "git_push",
  "git_pull",
  "git_sync",
  "git_pr_status",
  "git_pr_create",
  "git_history",
  "git_commit_files",
  "git_commit_file_diff",
  "git_staged_context",
  "git_range_context",
  "git_branches",
  "git_checkout",
  "git_create_branch",
  "git_stash",
  "git_worktrees",
  "search_project",
];
// Remote RPC has bounded request and response bodies. Keep file operations
// within those bounds even after JSON escaping or base64 encoding.
const MAX_TEXT_FILE = 1024 * 1024;
const MAX_PREVIEW_FILE = 10 * 1024 * 1024;
const MAX_STAT_FILES = 64;
const ROOTS_TTL_MS = 5_000;
const slashed = (path) => path.replace(/\\/g, "/");
const joined = (parent, name) =>
  `${slashed(parent).replace(/\/+$/, "")}/${slashed(name).replace(/^\/+|\/+$/g, "")}`;
const alreadyExists = (name) =>
  `A file or folder ${name} already exists at this location. Please choose a different name.`;
export class WorkspaceCommands {
  store;
  withIdleProject;
  roots = new Map();
  rootsGeneration = 0;
  editorWrites = Promise.resolve();
  invalidateRoots() {
    this.rootsGeneration++;
    this.roots.clear();
  }
  constructor(store, withIdleProject) {
    this.store = store;
    this.withIdleProject = withIdleProject;
  }
  run(command, args) {
    if (!WORKSPACE_COMMANDS.includes(command))
      throw new Error("Unsupported workspace command");
    const input =
      args && typeof args === "object" && !Array.isArray(args) ? args : {};
    switch (command) {
      case "list_dir":
        return this.listDir(input.path);
      case "list_project_files":
        return this.listProjectFiles(input.cwd);
      case "read_text_file":
        return this.readText(input.path);
      case "read_editor_text_file":
        return this.readEditorText(input.path);
      case "read_binary_file":
        return this.readBinary(input.path);
      case "read_file_preview":
        return this.preview(input.path, input.maxLines, input.startLine);
      case "write_text_file":
        return this.writeText(input.path, input.content);
      case "write_editor_text_file": {
        const operation = this.editorWrites.then(() =>
          this.writeEditorText(input.path, input.content, input.expectedHash),
        );
        this.editorWrites = operation.catch(() => {});
        return operation;
      }
      case "stat_files":
        return this.statFiles(input.paths);
      case "create_path":
        return this.create(input.parent, input.name, input.isDir);
      case "rename_path":
        return this.rename(input.path, input.name);
      case "delete_path":
        return this.delete(input.path);
      case "copy_path":
        return this.copy(input.from, input.destParent);
      case "move_path":
        return this.move(input.from, input.destParent);
      case "git_diff_index":
      case "git_diff_files":
        return this.gitIndex(input.cwd);
      case "git_diff_stats":
        return this.gitIndex(input.cwd).then((index) => ({
          files: index.files.length,
          additions: index.additions,
          deletions: index.deletions,
        }));
      case "git_file_diff":
        return this.gitFileDiff(input.cwd, input.relative, input.staged);
      case "git_stage_contents":
        return this.gitAction(
          input.cwd,
          "stageContents",
          input.relative,
          undefined,
          input.contents,
        );
      case "git_stage_file":
        return this.gitAction(input.cwd, "stage", input.relative);
      case "git_unstage_file":
        return this.gitAction(input.cwd, "unstage", input.relative);
      case "git_discard_file":
        return this.gitAction(input.cwd, "discard", input.relative);
      case "git_discard_all":
        return this.gitAction(input.cwd, "discardAll");
      case "git_stage_all":
        return this.gitAction(input.cwd, "stageAll");
      case "git_unstage_all":
        return this.gitAction(input.cwd, "unstageAll");
      case "git_commit":
        return this.gitCommit(
          input.cwd,
          input.message,
          input.amend,
          input.paths,
        );
      case "git_locate_files":
        return this.gitLocateFiles(input.paths);
      case "git_head_message":
        return this.gitCommand(input.cwd, ["log", "-1", "--format=%B"]);
      case "git_push":
        return this.gitAction(input.cwd, "push");
      case "git_pull":
        return this.gitPull(input.cwd);
      case "git_sync":
        return this.gitSync(input.cwd);
      case "git_pr_status":
        return this.gitPrStatus(input.cwd);
      case "git_pr_create":
        return this.gitPrCreate(
          input.cwd,
          input.title,
          input.body,
          input.base,
          input.head,
        );
      case "git_history":
        return this.gitHistory(input.cwd, input.limit);
      case "git_commit_files":
        return this.gitCommitFiles(input.cwd, input.sha);
      case "git_commit_file_diff":
        return this.gitCommitFileDiff(input.cwd, input.sha, input.relative);
      case "git_staged_context":
        return this.gitStagedContext(input.cwd, input.paths);
      case "git_range_context":
        return this.gitRangeContext(input.cwd);
      case "git_branches":
        return this.gitBranches(input.cwd);
      case "git_checkout":
        return this.gitCheckout(input.cwd, input.name, input.remote);
      case "git_create_branch":
        return this.gitCreateBranch(input.cwd, input.name);
      case "git_stash":
        return this.gitStash(input.cwd, input.message);
      case "git_worktrees":
        return this.gitWorktrees(input.cwd);
      case "search_project":
        return this.searchProject(input.options);
    }
  }
  /** The project folders and worktrees files may be read and written in. */
  async allowedRoots() {
    const generation = this.rootsGeneration;
    const out = [];
    for (const project of this.store.projects()) {
      const cached = this.roots.get(project.cwd);
      if (cached && Date.now() - cached.at < ROOTS_TTL_MS) {
        out.push(...cached.roots);
        continue;
      }
      const roots = await hostWorktrees(project.cwd)
        .then((listed) => [
          project.cwd,
          ...listed.worktrees
            .filter((tree) => !tree.missing && tree.path !== project.cwd)
            .map((tree) => tree.path),
        ])
        .catch(() => [project.cwd]);
      if (generation === this.rootsGeneration)
        this.roots.set(project.cwd, { at: Date.now(), roots });
      out.push(...roots);
    }
    return out;
  }
  /** Finds the project root that contains `input`, which may not exist yet. */
  async locate(input) {
    if (
      typeof input !== "string" ||
      !isAbsolute(input) ||
      input.length > 4096 ||
      input.includes("\0")
    )
      throw new Error("Invalid workspace path");
    let actual = resolve(input);
    let missing = "";
    // Resolve symlinks on the nearest existing ancestor, then re-append the
    // part that does not exist yet (a file about to be created or written).
    for (;;) {
      const real = await realpath(actual).catch(() => undefined);
      if (real) {
        actual = missing ? resolve(real, missing) : real;
        break;
      }
      const parent = dirname(actual);
      if (parent === actual) break;
      missing = missing ? `${basename(actual)}/${missing}` : basename(actual);
      actual = parent;
    }
    for (const root of await this.allowedRoots()) {
      const rel = relative(root, actual);
      if (rel === "" || (!rel.startsWith("..") && !isAbsolute(rel)))
        return { root, relative: rel };
    }
    throw new Error("Path is outside this machine’s projects");
  }
  async existing(input, allowRoot = false) {
    const { root, relative: rel } = await this.locate(input);
    return { root, path: await existingPath(root, rel, allowRoot) };
  }
  async listDir(input) {
    const { root, relative: rel } = await this.locate(input);
    const entries = await listHostFiles(root, rel);
    return entries.map((entry) => ({
      ...entry,
      path: joined(input, entry.name),
    }));
  }
  async listProjectFiles(input) {
    const { path } = await this.existing(input, true);
    const cwd = input;
    return (await indexHostFiles(path)).map((file) => ({
      name: file.split("/").pop() || file,
      path: joined(cwd, file),
      relative: file,
    }));
  }
  async file(input, limit, tooLarge) {
    const { path } = await this.existing(input);
    const info = await stat(path);
    if (!info.isFile()) throw new Error("Not a file");
    if (info.size > limit)
      throw new Error(
        `File is too large to ${tooLarge} (maximum ${limit / 1024 / 1024} MB).`,
      );
    return readFile(path);
  }
  async readText(input) {
    const bytes = await this.file(input, MAX_TEXT_FILE, "edit");
    if (bytes.includes(0)) throw new Error("Binary files cannot be edited.");
    try {
      return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    } catch {
      throw new Error("File is not valid UTF-8.");
    }
  }
  async readEditorText(input) {
    // Hash bytes rather than decoded text: TextDecoder consumes a UTF-8 BOM.
    const bytes = await this.file(input, MAX_TEXT_FILE, "edit");
    if (bytes.includes(0)) throw new Error("Binary files cannot be edited.");
    let content;
    try {
      content = new TextDecoder("utf-8", {
        fatal: true,
        ignoreBOM: true,
      }).decode(bytes);
    } catch {
      throw new Error("File is not valid UTF-8.");
    }
    return {
      path: input,
      content,
      hash: createHash("sha256").update(bytes).digest("hex"),
    };
  }
  async writeEditorText(input, content, expectedHash) {
    if (typeof content !== "string" || typeof expectedHash !== "string")
      throw new Error("Invalid editor save");
    if (Buffer.byteLength(content, "utf8") > MAX_TEXT_FILE)
      throw new Error(
        `File is too large to save (maximum ${MAX_TEXT_FILE / 1024 / 1024} MB).`,
      );
    const { root, relative: rel } = await this.locate(input);
    const path = workspacePath(root, rel);
    const observe = async (target = path) => {
      // A deleted/replaced target conflicts; it does not become a create.
      const file = await open(target, "r").catch((error) => {
        if (error.code === "ENOENT" || error.code === "EISDIR") return null;
        throw error;
      });
      if (!file) return null;
      try {
        if (!(await file.stat()).isFile()) return null;
        const digest = createHash("sha256");
        for await (const bytes of file.createReadStream({ autoClose: false }))
          digest.update(bytes);
        return digest.digest("hex");
      } finally {
        await file.close();
      }
    };
    let hash = await observe();
    const conflict = () => ({
      status: "conflict",
      path: input,
      hash,
    });
    if (hash !== expectedHash) return conflict();
    const permissions = await stat(path).catch((error) => {
      if (error.code === "ENOENT") return null;
      throw error;
    });
    if (!permissions) {
      hash = null;
      return conflict();
    }
    const temporary = join(
      dirname(path),
      `.${basename(path)}.vorkspace-${randomUUID()}.tmp`,
    );
    const file = await open(temporary, "wx");
    try {
      await file.writeFile(content, "utf8");
      await file.sync();
      await file.chmod(permissions.mode);
      await file.close();
      hash = await observe();
      const latest = await this.locate(input);
      const latestPath = workspacePath(latest.root, latest.relative);
      if (latestPath !== path) {
        hash = await observe(latestPath);
        return conflict();
      }
      if (hash !== expectedHash) return conflict();
      // Our own editor writes are serialized, but external writers can still
      // race this last observation and rename. This is not filesystem CAS.
      await rename(temporary, path);
      invalidateHostFileIndex(root);
      const directory = await open(dirname(path), "r").catch(() => null);
      if (directory) {
        try {
          await directory.sync().catch(() => {});
        } finally {
          await directory.close();
        }
      }
      return {
        status: "saved",
        path: input,
        hash: createHash("sha256").update(content, "utf8").digest("hex"),
      };
    } finally {
      await file.close().catch(() => {});
      await rm(temporary, { force: true });
    }
  }
  /** Base64, since host responses are JSON; the client decodes it. */
  async readBinary(input) {
    return (await this.file(input, MAX_PREVIEW_FILE, "preview")).toString(
      "base64",
    );
  }
  async preview(input, maxLines, startLine) {
    const text = (await this.file(input, MAX_TEXT_FILE, "preview")).toString(
      "utf8",
    );
    if (text.includes("\0")) throw new Error("Binary file");
    const limit = Math.min(12, Math.max(1, Number(maxLines) || 6));
    const start = Math.max(1, Number(startLine) || 1);
    return text
      .split(/\r?\n/)
      .slice(start - 1, start - 1 + limit)
      .map((line) => (line.length > 200 ? `${line.slice(0, 199)}…` : line));
  }
  async writeText(input, content) {
    if (typeof content !== "string") throw new Error("Invalid file content");
    if (Buffer.byteLength(content, "utf8") > MAX_TEXT_FILE)
      throw new Error(
        `File is too large to save (maximum ${MAX_TEXT_FILE / 1024 / 1024} MB).`,
      );
    const { root, relative: rel } = await this.locate(input);
    const path = workspacePath(root, rel);
    if (
      await stat(path).then(
        (info) => info.isDirectory(),
        () => false,
      )
    )
      throw new Error("Cannot save text to a directory.");
    // Replace atomically, as the local command does.
    const temporary = `${path}.vorkspace-${process.pid}-${Date.now()}`;
    await writeFile(temporary, content, "utf8");
    await rename(temporary, path).catch(async (reason) => {
      await rm(temporary, { force: true });
      throw reason;
    });
    invalidateHostFileIndex(root);
  }
  async statFiles(input) {
    if (!Array.isArray(input) || input.length > MAX_STAT_FILES)
      throw new Error("Too many paths");
    return Promise.all(
      input.map(async (path) => {
        const mtimeMs = await this.existing(path)
          .then(({ path: actual }) => stat(actual))
          .then((info) => (info.isFile() ? Math.floor(info.mtimeMs) : null))
          .catch(() => null);
        return { path: String(path), mtimeMs };
      }),
    );
  }
  async create(parent, name, isDir) {
    const { root, relative: rel } = await this.locate(parent);
    try {
      await createHostPath(root, rel, name, isDir);
    } catch (reason) {
      if (reason.code === "EEXIST")
        throw new Error(alreadyExists(String(name)));
      throw reason;
    }
    return joined(parent, name);
  }
  async rename(input, name) {
    if (
      typeof name !== "string" ||
      !name.trim() ||
      /^[\\/]/.test(name) ||
      name.includes("\0")
    )
      throw new Error("A file or folder name must be provided.");
    const { root, path: from } = await this.existing(input);
    const target = joined(dirname(input), name);
    const { root: targetRoot, relative: rel } = await this.locate(target);
    if (targetRoot !== root)
      throw new Error("Cannot move a file between working copies.");
    const to = workspacePath(root, rel);
    if (to === from) return input;
    if (
      await lstat(to).then(
        () => true,
        () => false,
      )
    )
      throw new Error(alreadyExists(name));
    if (!relative(from, to).startsWith(".."))
      throw new Error("Cannot move a folder into itself.");
    await mkdir(dirname(to), { recursive: true });
    await rename(from, to);
    invalidateHostFileIndex(root);
    return target;
  }
  async delete(input) {
    const { root, path } = await this.existing(input);
    await rm(path, { recursive: true });
    invalidateHostFileIndex(root);
  }
  async destination(from, destParent) {
    const source = await this.existing(from);
    const parent = await this.existing(destParent, true);
    if (!(await stat(parent.path)).isDirectory())
      throw new Error(`${String(destParent)} is not a folder`);
    if (
      (await stat(source.path)).isDirectory() &&
      !relative(source.path, parent.path).startsWith("..")
    )
      throw new Error("Cannot paste a folder into itself.");
    return {
      source: source.path,
      parent: parent.path,
      sourceRoot: source.root,
      parentRoot: parent.root,
    };
  }
  async copy(from, destParent) {
    const { source, parent, sourceRoot, parentRoot } = await this.destination(
      from,
      destParent,
    );
    const name = basename(source);
    const ext = extname(name);
    const stem = ext ? name.slice(0, -ext.length) : name;
    for (let n = 0; ; n++) {
      const candidate =
        n === 0
          ? name
          : n === 1
            ? `${stem} copy${ext}`
            : `${stem} copy ${n}${ext}`;
      const to = resolve(parent, candidate);
      if (
        await lstat(to).then(
          () => true,
          () => false,
        )
      )
        continue;
      await cp(source, to, { recursive: true, errorOnExist: true });
      invalidateHostFileIndex(sourceRoot);
      invalidateHostFileIndex(parentRoot);
      return joined(destParent, candidate);
    }
  }
  async move(from, destParent) {
    const { source, parent, sourceRoot, parentRoot } = await this.destination(
      from,
      destParent,
    );
    const name = basename(source);
    const to = resolve(parent, name);
    if (to === source) return from;
    if (
      await lstat(to).then(
        () => true,
        () => false,
      )
    )
      throw new Error(alreadyExists(name));
    await rename(source, to);
    invalidateHostFileIndex(sourceRoot);
    invalidateHostFileIndex(parentRoot);
    return joined(destParent, name);
  }
  async gitRoot(input) {
    const { path } = await this.existing(input, true);
    if (!(await stat(path)).isDirectory())
      throw new Error("Not a working copy");
    return path;
  }
  async searchProject(input) {
    if (!input || typeof input !== "object" || Array.isArray(input))
      throw new Error("Invalid search");
    const options = input;
    return searchHostContent(await this.gitRoot(options.cwd), options);
  }
  async gitIndex(input) {
    return hostGitIndex(await this.gitRoot(input));
  }
  async gitFileDiff(cwd, relative, staged) {
    return hostFileDiff(await this.gitRoot(cwd), relative, staged === true);
  }
  async gitAction(cwd, action, relative, message, contents) {
    return hostGitAction(
      await this.gitRoot(cwd),
      action,
      relative,
      message,
      contents,
    );
  }
  async gitCommand(cwd, args, env) {
    const root = await this.gitRoot(cwd);
    return (
      await exec("git", ["-c", "core.pager=cat", ...args], {
        cwd: root,
        timeout: 30_000,
        maxBuffer: 4 * 1024 * 1024,
        encoding: "utf8",
        env: { ...process.env, GIT_TERMINAL_PROMPT: "0", ...env },
      })
    ).stdout;
  }
  async gitCommit(cwd, message, amend, paths) {
    if (
      typeof message !== "string" ||
      !message.trim() ||
      message.length > 100_000
    )
      throw new Error("Enter a commit message");
    if (paths != null) {
      if (amend === true)
        throw new Error("Selected-file commits cannot amend HEAD");
      const root = await this.gitRoot(cwd);
      const selected = await this.gitSelectedPaths(root, paths);
      const existing = [];
      for (const path of selected) {
        if (
          await lstat(join(root, path)).then(
            () => true,
            () => false,
          )
        )
          existing.push(path);
      }
      if (existing.length)
        await this.gitCommand(root, [
          "--literal-pathspecs",
          "add",
          "-A",
          "--",
          ...existing,
        ]);
      await this.gitCommand(root, [
        "--literal-pathspecs",
        "commit",
        "--only",
        "--cleanup=strip",
        "-m",
        message,
        "--",
        ...selected,
      ]);
    } else {
      await this.gitCommand(cwd, [
        "commit",
        ...(amend === true ? ["--amend"] : []),
        "-m",
        message,
      ]);
    }
  }
  async gitLocateFiles(paths) {
    if (!Array.isArray(paths)) throw new Error("Invalid file paths");
    const locations = [];
    for (const path of paths) locations.push(await this.gitLocateFile(path));
    return locations;
  }
  async gitLocateFile(path) {
    if (typeof path !== "string" || !isAbsolute(path) || path.includes("\0"))
      throw new Error("Invalid file path");
    // Resolve parents, keeping a symlink itself in the checkout that owns it.
    const parent = await this.locate(dirname(path));
    const actual = resolve(parent.root, parent.relative, basename(path));
    let directory = dirname(actual);
    while (
      !(await stat(directory).then(
        (entry) => entry.isDirectory(),
        () => false,
      ))
    ) {
      const next = dirname(directory);
      if (next === directory) throw new Error("File has no existing parent");
      directory = next;
    }
    let top;
    try {
      top = (
        await exec("git", ["-C", directory, "rev-parse", "--show-toplevel"], {
          timeout: 30_000,
          encoding: "utf8",
          env: { ...process.env, GIT_TERMINAL_PROMPT: "0" },
        })
      ).stdout.replace(/[\r\n]+$/, "");
    } catch (error) {
      if (String(error).includes("not a git repository")) return null;
      throw error;
    }
    // Discovery cannot widen the host's registered project boundaries.
    const root = await this.gitRoot(top);
    return {
      root: slashed(root),
      relative:
        process.platform === "win32"
          ? slashed(relative(root, actual))
          : relative(root, actual),
    };
  }
  async gitSelectedPaths(root, paths) {
    if (!Array.isArray(paths) || paths.length === 0)
      throw new Error("Select at least one file");
    const selected = [];
    for (const path of paths) {
      if (
        typeof path !== "string" ||
        isAbsolute(path) ||
        path.includes("\0") ||
        (process.platform === "win32" ? slashed(path) : path)
          .split("/")
          .some(
            (part) =>
              !part ||
              part === "." ||
              part === ".." ||
              part.toLowerCase() === ".git",
          )
      )
        throw new Error("Invalid selected file path");
      const target = workspacePath(root, path);
      const entry = await lstat(target).catch((error) => {
        if (error.code === "ENOENT") return null;
        throw error;
      });
      if (entry?.isDirectory())
        throw new Error("Select files rather than directories");
      const location = await this.gitLocateFile(target);
      if (!location || location.root !== slashed(root))
        throw new Error("Selected files must belong to this repository");
      // Even missing folders must not widen a literal selection to their children.
      const tracked = await this.gitCommand(root, [
        "--literal-pathspecs",
        "ls-files",
        "-z",
        "--",
        path,
      ]);
      if (tracked.split("\0").some((entry) => entry && entry !== path))
        throw new Error("Select files rather than directories");
      const type = await this.gitCommand(root, [
        "cat-file",
        "-t",
        `HEAD:${path}`,
      ]).catch(() => "");
      if (type.trim() === "tree")
        throw new Error("Select files rather than directories");
      if (!entry && !tracked && type.trim() !== "blob")
        throw new Error("Selected file does not exist in this repository");
      if (!selected.includes(path)) selected.push(path);
    }
    return selected;
  }
  async gitSync(cwd) {
    const root = await this.gitRoot(cwd);
    await this.gitCommand(root, ["pull", "--ff-only"]);
    invalidateHostFileIndex(root);
    await this.gitAction(root, "push");
  }
  async gitPull(cwd) {
    const root = await this.gitRoot(cwd);
    await this.gitCommand(root, ["pull", "--ff-only"]);
    invalidateHostFileIndex(root);
  }
  async ghCommand(cwd, args) {
    const root = await this.gitRoot(cwd);
    return (
      await exec("gh", args, {
        cwd: root,
        timeout: 30_000,
        maxBuffer: 1024 * 1024,
        encoding: "utf8",
        env: {
          ...process.env,
          GH_PROMPT_DISABLED: "1",
          GIT_TERMINAL_PROMPT: "0",
        },
      })
    ).stdout.trim();
  }
  async gitPrStatus(cwd) {
    const output = await this.ghCommand(cwd, [
      "pr",
      "view",
      "--json",
      "number,title,url,state",
    ]).catch(() => "");
    if (!output) return null;
    const pr = JSON.parse(output);
    return { ...pr, state: pr.state.toLowerCase() };
  }
  async gitPrCreate(cwd, title, body, base, head) {
    if (
      [title, body, base, head].some(
        (value) => typeof value !== "string" || value.length > 100_000,
      )
    )
      throw new Error("Invalid pull request");
    return this.ghCommand(cwd, [
      "pr",
      "create",
      "--title",
      title,
      "--body",
      body,
      "--base",
      base,
      "--head",
      head,
    ]);
  }
  gitSha(value) {
    if (typeof value !== "string" || !/^[0-9a-f]{4,40}$/i.test(value))
      throw new Error("Invalid commit");
    return value;
  }
  async gitCommitSha(cwd, input) {
    const sha = this.gitSha(input);
    const resolved = await this.gitCommand(cwd, [
      "rev-parse",
      "--verify",
      `${sha}^{commit}`,
    ]).catch(() => "");
    if (!/^[0-9a-f]{40,64}$/i.test(resolved.trim()))
      throw new Error("Unknown commit");
    return resolved.trim();
  }
  async gitHistory(cwd, limit) {
    const count = Number.isSafeInteger(limit)
      ? Math.min(500, Math.max(1, Number(limit)))
      : 200;
    const [head, upstream, index, remoteNames] = await Promise.all([
      this.gitCommand(cwd, ["rev-parse", "--verify", "HEAD"]).catch(() => ""),
      this.gitCommand(cwd, ["rev-parse", "--abbrev-ref", "@{upstream}"]).catch(
        () => "",
      ),
      this.gitIndex(cwd),
      this.gitCommand(cwd, ["remote"]).catch(() => ""),
    ]);
    const headSha = head.trim() || null;
    if (!headSha) return { head: null, commits: [] };
    const tips = ["HEAD"];
    if (upstream.trim()) tips.push("@{upstream}");
    if (index.defaultBranch && index.remote) {
      const defaultRef = `refs/remotes/origin/${index.defaultBranch}`;
      const exists = await this.gitCommand(cwd, [
        "rev-parse",
        "--verify",
        defaultRef,
      ]).then(
        () => true,
        () => false,
      );
      if (exists) tips.push(`origin/${index.defaultBranch}`);
    }
    const output = await this.gitCommand(cwd, [
      "log",
      "--topo-order",
      "--decorate=short",
      `--max-count=${count}`,
      "--format=%H%x00%h%x00%P%x00%an%x00%at%x00%D%x00%s%x1e",
      ...tips,
    ]).catch(() => "");
    const remotes = remoteNames
      .split("\n")
      .map((name) => name.trim())
      .filter(Boolean);
    const commits = output.split("\x1e").flatMap((record) => {
      const [sha, shortSha, parents, author, timestamp, decorations, subject] =
        record.trim().split("\0");
      if (!sha || !/^[0-9a-f]{40,64}$/i.test(sha)) return [];
      const refs = (decorations ?? "")
        .split(",")
        .map((raw) => raw.trim())
        .filter(Boolean)
        .flatMap((raw) => {
          if (raw === "HEAD" || raw.endsWith("/HEAD")) return [];
          if (raw.startsWith("HEAD -> "))
            return [{ name: raw.slice(8), kind: "local" }];
          if (raw.startsWith("tag: "))
            return [{ name: raw.slice(5), kind: "tag" }];
          return [
            {
              name: raw,
              kind: remotes.some(
                (remote) => raw === remote || raw.startsWith(`${remote}/`),
              )
                ? "remote"
                : "local",
            },
          ];
        });
      return [
        {
          sha,
          shortSha: shortSha || sha.slice(0, 7),
          parents: parents ? parents.split(" ") : [],
          author,
          timestamp: Number(timestamp),
          subject,
          refs,
          head: sha === headSha,
        },
      ];
    });
    return { head: headSha, commits };
  }
  async gitCommitFiles(cwd, shaInput) {
    const sha = await this.gitCommitSha(cwd, shaInput);
    const [names, stats] = await Promise.all([
      this.gitCommand(cwd, [
        "diff-tree",
        "--root",
        "--no-commit-id",
        "--no-renames",
        "--name-status",
        "-r",
        sha,
      ]),
      this.gitCommand(cwd, [
        "diff-tree",
        "--root",
        "--no-commit-id",
        "--no-renames",
        "--numstat",
        "-r",
        sha,
      ]),
    ]);
    const counts = new Map(
      stats
        .split("\n")
        .filter(Boolean)
        .map((line) => {
          const [added, removed, path] = line.split("\t");
          return [
            path,
            { additions: Number(added) || 0, deletions: Number(removed) || 0 },
          ];
        }),
    );
    return names
      .split("\n")
      .filter(Boolean)
      .map((line) => {
        const [code, relativePath] = line.split("\t");
        return {
          path: relativePath,
          relative: relativePath,
          status:
            code === "A" ? "added" : code === "D" ? "deleted" : "modified",
          ...(counts.get(relativePath) ?? { additions: 0, deletions: 0 }),
          staged: false,
          unstaged: false,
        };
      });
  }
  async gitCommitFileDiff(cwd, shaInput, input) {
    const sha = await this.gitCommitSha(cwd, shaInput);
    const root = await this.gitRoot(cwd);
    const path = relative(root, workspacePath(root, input)).replace(/\\/g, "/");
    const parent = await this.gitCommand(root, [
      "rev-parse",
      "--verify",
      `${sha}^`,
    ]).catch(() => "");
    const [original, current] = await Promise.all([
      parent
        ? this.gitBlob(root, `${parent.trim()}:${path}`)
        : Promise.resolve({ bytes: Buffer.alloc(0), tooLarge: false }),
      this.gitBlob(root, `${sha}:${path}`),
    ]);
    const binary = original.bytes.includes(0) || current.bytes.includes(0);
    const tooLarge =
      original.tooLarge ||
      current.tooLarge ||
      original.bytes.length > MAX_TEXT_FILE ||
      current.bytes.length > MAX_TEXT_FILE;
    const status =
      !original.bytes.length && current.bytes.length
        ? "added"
        : original.bytes.length && !current.bytes.length
          ? "deleted"
          : "modified";
    return {
      path,
      relative: path,
      status,
      original: binary || tooLarge ? "" : original.bytes.toString("utf8"),
      current: binary || tooLarge ? "" : current.bytes.toString("utf8"),
      binary,
      tooLarge,
    };
  }
  async gitBlob(root, spec) {
    try {
      const { stdout } = await exec("git", ["show", spec], {
        cwd: root,
        timeout: 30_000,
        maxBuffer: MAX_TEXT_FILE + 1024,
        encoding: "buffer",
        env: { ...process.env, GIT_TERMINAL_PROMPT: "0" },
      });
      return {
        bytes: Buffer.isBuffer(stdout) ? stdout : Buffer.from(stdout),
        tooLarge: false,
      };
    } catch (reason) {
      return {
        bytes: Buffer.alloc(0),
        tooLarge: String(reason).includes("maxBuffer"),
      };
    }
  }
  async gitStagedContext(cwd, paths) {
    if (paths != null) {
      const root = await this.gitRoot(cwd);
      const selected = await this.gitSelectedPaths(root, paths);
      const directory = await mkdtemp(
        join(tmpdir(), "vorkspace-git-selection-"),
      );
      const env = { GIT_INDEX_FILE: join(directory, "index") };
      const run = (args) =>
        this.gitCommand(root, ["--literal-pathspecs", ...args], env);
      try {
        const hasHead = await this.gitCommand(root, [
          "rev-parse",
          "--verify",
          "HEAD",
        ]).then(
          () => true,
          () => false,
        );
        await run(["read-tree", hasHead ? "HEAD" : "--empty"]);
        await run(["add", "-A", "--", ...selected]);
        const [index, summary, patch] = await Promise.all([
          this.gitIndex(root),
          run(["diff", "--cached", "--stat", "--no-renames"]),
          run([
            "diff",
            "--cached",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
          ]),
        ]);
        return { branch: index.branch, summary, patch };
      } finally {
        await rm(directory, { recursive: true, force: true });
      }
    }
    const [index, summary, patch] = await Promise.all([
      this.gitIndex(cwd),
      this.gitCommand(cwd, ["diff", "--cached", "--stat"]),
      this.gitCommand(cwd, ["diff", "--cached", "--no-ext-diff"]),
    ]);
    return { branch: index.branch, summary, patch };
  }
  async gitRangeContext(cwd) {
    const index = await this.gitIndex(cwd);
    if (!index.defaultBranch) throw new Error("Default branch is unknown");
    const base = `origin/${index.defaultBranch}`;
    const head = index.branch ?? "HEAD";
    const [commitSummary, diffSummary, diffPatch] = await Promise.all([
      this.gitCommand(cwd, ["log", "--oneline", `${base}..HEAD`]),
      this.gitCommand(cwd, ["diff", "--stat", `${base}...HEAD`]),
      this.gitCommand(cwd, ["diff", "--no-ext-diff", `${base}...HEAD`]),
    ]);
    return {
      base: index.defaultBranch,
      head,
      commitSummary,
      diffSummary,
      diffPatch,
    };
  }
  async gitBranches(cwd) {
    const state = await hostBranches(await this.gitRoot(cwd));
    return {
      current: state.current,
      detached: state.current === null,
      branches: [
        ...state.branches.map((name) => ({
          name,
          current: name === state.current,
          remote: null,
        })),
        ...state.remotes.map((entry) => ({
          name: entry.name,
          current: false,
          remote: entry.remote,
        })),
      ],
    };
  }
  async gitCheckout(cwd, name, remote) {
    const root = await this.gitRoot(cwd);
    const state = await this.withIdleGitProject(root, () =>
      switchHostBranch(root, name, remote),
    );
    invalidateHostFileIndex(root);
    return state.current ?? "HEAD";
  }
  async gitCreateBranch(cwd, name) {
    const root = await this.gitRoot(cwd);
    const state = await this.withIdleGitProject(root, () =>
      createHostBranch(root, name),
    );
    return state.current ?? "HEAD";
  }
  async withIdleGitProject(cwd, action) {
    const { root } = await this.locate(cwd);
    const project = this.store
      .projects()
      .find(
        (candidate) =>
          candidate.cwd === root ||
          this.roots.get(candidate.cwd)?.roots.includes(root),
      );
    if (!project) throw new Error("Project is unavailable");
    return this.withIdleProject(project.id, action);
  }
  async gitStash(cwd, message) {
    if (
      message != null &&
      (typeof message !== "string" || message.length > 1000)
    )
      throw new Error("Invalid stash message");
    const root = await this.gitRoot(cwd);
    await this.gitCommand(root, [
      "stash",
      "push",
      "-u",
      ...(message ? ["-m", message] : []),
    ]);
    invalidateHostFileIndex(root);
  }
  async gitWorktrees(cwd) {
    const listed = await hostWorktrees(await this.gitRoot(cwd));
    return {
      defaultRoot: listed.defaultRoot,
      worktrees: listed.worktrees.map((tree) => ({
        ...tree,
        locked: false,
        prunable: tree.missing,
        dirty: null,
        unpushed: null,
        sessionIds: [],
      })),
    };
  }
}
