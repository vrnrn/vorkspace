import { execFileSync } from "node:child_process";
import {
  chmodSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createHash } from "node:crypto";
import * as fsPromises from "node:fs/promises";
import { afterEach, expect, it, vi } from "vitest";
import { HostStore } from "./store.js";
import { WorkspaceCommands } from "./workspace-commands.js";
import { indexHostFiles } from "./workspace.js";
const cleanups = [];
afterEach(() => {
  vi.restoreAllMocks();
  cleanups
    .splice(0)
    .reverse()
    .forEach((cleanup) => cleanup());
});
function fixture() {
  // Use the same native resolution as fs/promises.realpath in the host,
  // including expansion of Windows short directory names such as RUNNER~1.
  const home = realpathSync.native(
    mkdtempSync(join(tmpdir(), "vorkspace-selected-git-")),
  );
  const store = new HostStore(join(home, "host.db"));
  cleanups.push(() => {
    store.close();
    rmSync(home, { recursive: true, force: true });
  });
  const commands = new WorkspaceCommands(store, async (_id, action) =>
    action(),
  );
  const repo = (name = "a", initial = true) => {
    const cwd = join(home, name);
    mkdirSync(cwd, { recursive: true });
    const git = (...args) =>
      execFileSync("git", args, { cwd, encoding: "utf8" }).trim();
    git("init", "-q", "-b", "main");
    git("config", "user.name", "Test");
    git("config", "user.email", "test@example.com");
    git("config", "commit.gpgsign", "false");
    git("config", "core.autocrlf", "false");
    const write = (file, content) => writeFileSync(join(cwd, file), content);
    if (initial) {
      for (const file of ["chosen.txt", "other.txt", "deleted.txt"])
        write(file, "old\n");
      git("add", ".");
      git("commit", "-q", "-m", "Initial");
    }
    store.addProject(cwd, name);
    commands.invalidateRoots();
    return { cwd, git, write };
  };
  return { home, commands, repo };
}
const apiRoot = (path) =>
  path;
it("invalidates the cached project index after rename and delete", async () => {
  const { commands, repo } = fixture();
  const a = repo("a");
  writeFileSync(join(a.cwd, "transient.txt"), "untracked\n");
  expect(await indexHostFiles(a.cwd)).toContain("transient.txt");
  await commands.run("rename_path", {
    path: join(a.cwd, "transient.txt"),
    name: "renamed.txt",
  });
  expect(await indexHostFiles(a.cwd)).toContain("renamed.txt");
  expect(await indexHostFiles(a.cwd)).not.toContain("transient.txt");
  await commands.run("delete_path", { path: join(a.cwd, "renamed.txt") });
  expect(await indexHostFiles(a.cwd)).not.toContain("renamed.txt");
});
it("commits only selected working files and preserves other staging and repos", async () => {
  const { commands, repo } = fixture();
  const a = repo("a"),
    b = repo("b");
  a.write("other.txt", "unrelated staged\n");
  a.git("add", "other.txt");
  a.write("chosen.txt", "partial staging\n");
  a.git("add", "chosen.txt");
  a.write("chosen.txt", "selected work\n");
  b.write("chosen.txt", "second repo work\n");
  const bHead = b.git("rev-parse", "HEAD");
  await commands.run("git_commit", {
    cwd: a.cwd,
    message: "Selected",
    paths: ["chosen.txt"],
  });
  expect(a.git("show", "HEAD:chosen.txt")).toBe("selected work");
  expect(a.git("show", "HEAD:other.txt")).toBe("old");
  expect(a.git("show", ":other.txt")).toBe("unrelated staged");
  expect(a.git("diff", "--cached", "--name-only")).toBe("other.txt");
  expect(b.git("rev-parse", "HEAD")).toBe(bHead);
  expect(readFileSync(join(b.cwd, "chosen.txt"), "utf8")).toBe(
    "second repo work\n",
  );
  await commands.run("git_commit", {
    cwd: a.cwd,
    message: "Existing staged workflow",
  });
  expect(a.git("show", "HEAD:other.txt")).toBe("unrelated staged");
});
it("generates context for selected added and deleted files without reading excluded changes or altering the index", async () => {
  const { commands, repo } = fixture();
  const { cwd, git, write } = repo();
  write("other.txt", "EXCLUDED_SECRET\n");
  write("chosen.txt", "partial staging\n");
  git("add", ".");
  write("chosen.txt", "selected work\n");
  write("new.txt", "new selected work\n");
  rmSync(join(cwd, "deleted.txt"));
  git("add", "deleted.txt");
  const before = readFileSync(join(cwd, ".git/index"));
  const paths = ["chosen.txt", "new.txt", "deleted.txt"];
  const context = await commands.run("git_staged_context", {
    cwd,
    paths,
  });
  expect(context.patch).toContain("+selected work");
  expect(context.patch).toContain("+new selected work");
  expect(context.patch).toContain("deleted file mode");
  expect(context.patch).not.toContain("EXCLUDED_SECRET");
  expect(context.summary).not.toContain("other.txt");
  expect(readFileSync(join(cwd, ".git/index"))).toEqual(before);
  await commands.run("git_commit", {
    cwd,
    message: "Selected add and delete",
    paths,
  });
  expect(git("ls-tree", "--name-only", "HEAD").split("\n")).toEqual([
    "chosen.txt",
    "new.txt",
    "other.txt",
  ]);
  expect(git("show", ":other.txt")).toBe("EXCLUDED_SECRET");
});
it("supports the first commit while leaving other staged new files out", async () => {
  const { commands, repo } = fixture();
  const { cwd, git, write } = repo("new", false);
  write("chosen.txt", "chosen\n");
  write("other.txt", "other\n");
  git("add", "other.txt");
  const context = await commands.run("git_staged_context", {
    cwd,
    paths: ["chosen.txt"],
  });
  expect(context.patch).toContain("+chosen");
  await commands.run("git_commit", {
    cwd,
    message: "First",
    paths: ["chosen.txt"],
  });
  expect(git("ls-tree", "--name-only", "HEAD")).toBe("chosen.txt");
  expect(git("diff", "--cached", "--name-only")).toBe("other.txt");
});
it("rejects invalid and cross-repo selections before touching staging", async () => {
  const { commands, repo } = fixture();
  const { cwd, git, write } = repo();
  repo("a/nested");
  mkdirSync(join(cwd, "folder"));
  write("folder/child.txt", "new\n");
  git("add", "folder/child.txt");
  rmSync(join(cwd, "folder"), { recursive: true });
  write("chosen.txt", "unstaged\n");
  const index = readFileSync(join(cwd, ".git/index"));
  for (const invalid of [
    "",
    "../escape",
    ".git/config",
    "./chosen.txt",
    "nested/chosen.txt",
    "folder",
    "missing.txt",
  ]) {
    const args = { cwd, message: "Invalid", paths: ["chosen.txt", invalid] };
    await expect(commands.run("git_commit", args)).rejects.toThrow();
    await expect(commands.run("git_staged_context", args)).rejects.toThrow();
    expect(readFileSync(join(cwd, ".git/index"))).toEqual(index);
  }
  await expect(
    commands.run("git_commit", { cwd, message: "Empty", paths: [] }),
  ).rejects.toThrow();
  await expect(
    commands.run("git_commit", {
      cwd,
      message: "Amend",
      amend: true,
      paths: ["chosen.txt"],
    }),
  ).rejects.toThrow("cannot amend");
});
it(
  "treats wildcard, pathspec, newline and backslash filenames literally",
  async () => {
    const { commands, repo } = fixture();
    const { cwd, git, write } = repo();
    const paths = [
      "*",
      ":(glob)*",
      "-flag",
      "line\nbreak",
      "back\\slash",
      "hello é.txt",
    ];
    for (const path of paths) write(path, "literal\n");
    write("other.txt", "excluded\n");
    await commands.run("git_commit", { cwd, message: "Literal", paths });
    for (const path of paths)
      expect(git("show", `HEAD:${path}`)).toBe("literal");
    expect(git("show", "HEAD:other.txt")).toBe("old");
  },
);
it("locates each checkout and deleted parents while enforcing host boundaries", async () => {
  const { home, commands, repo } = fixture();
  const a = repo("a"),
    b = repo("b");
  repo("a/nested");
  const tree = join(home, "tree");
  a.git("worktree", "add", "-b", "other", tree);
  commands.invalidateRoots();
  const relatives = [
    "deleted/folder/file.txt",
    "chosen.txt",
    "chosen.txt",
    "nested/chosen.txt",
  ];
  const roots = [a.cwd, b.cwd, tree, a.cwd];
  expect(
    await commands.run("git_locate_files", {
      paths: relatives.map((path, i) => join(roots[i], path)),
    }),
  ).toEqual([
    { root: apiRoot(a.cwd), relative: relatives[0] },
    { root: apiRoot(b.cwd), relative: "chosen.txt" },
    { root: apiRoot(tree), relative: "chosen.txt" },
    { root: apiRoot(join(a.cwd, "nested")), relative: "chosen.txt" },
  ]);
  await expect(
    commands.run("git_locate_files", { paths: [join(home, "outside.txt")] }),
  ).rejects.toThrow();
  const outside = join(home, "non-git");
  mkdirSync(outside);
  const store = new HostStore(join(outside, "host.db"));
  cleanups.push(() => store.close());
  store.addProject(outside, "non-git");
  const nonGit = new WorkspaceCommands(store, async (_id, action) => action());
  expect(
    await nonGit.run("git_locate_files", {
      paths: [join(outside, "file.txt")],
    }),
  ).toEqual([null]);
});
it(
  "keeps file symlinks in their checkout and rejects traversal through outside directories",
  async () => {
    const { home, commands, repo } = fixture();
    const { cwd } = repo();
    writeFileSync(join(home, "secret.txt"), "OUTSIDE_SECRET\n");
    symlinkSync(join(home, "secret.txt"), join(cwd, "file-link"));
    symlinkSync(home, join(cwd, "dir-link"));
    expect(
      await commands.run("git_locate_files", {
        paths: [join(cwd, "file-link")],
      }),
    ).toEqual([{ root: apiRoot(cwd), relative: "file-link" }]);
    const context = await commands.run("git_staged_context", {
      cwd,
      paths: ["file-link"],
    });
    expect(context.patch).toContain("new file mode 120000");
    expect(context.patch).not.toContain("OUTSIDE_SECRET");
    await expect(
      commands.run("git_staged_context", {
        cwd,
        paths: ["dir-link/secret.txt"],
      }),
    ).rejects.toThrow();
    await expect(
      commands.run("git_commit", {
        cwd,
        message: "Escape",
        paths: ["dir-link/secret.txt"],
      }),
    ).rejects.toThrow();
  },
);
const digest = (content) => createHash("sha256").update(content).digest("hex");
it("reads exact editor bytes and rejects changed or deleted baselines without a watcher", async () => {
  const { commands, repo } = fixture();
  const { cwd, write } = repo();
  const path = join(cwd, "chosen.txt");
  const content = "\ufeffalpha\r\nbeta\r\n";
  write("chosen.txt", content);
  const baseline = await commands.run("read_editor_text_file", {
    path,
  });
  expect(baseline).toEqual({ path, content, hash: digest(content) });
  write("chosen.txt", "external\n");
  expect(
    await commands.run("write_editor_text_file", {
      path,
      content: "local\r\n",
      expectedHash: baseline.hash,
    }),
  ).toEqual({ status: "conflict", path, hash: digest("external\n") });
  expect(readFileSync(path, "utf8")).toBe("external\n");
  rmSync(path);
  expect(
    await commands.run("write_editor_text_file", {
      path,
      content: "local\r\n",
      expectedHash: baseline.hash,
    }),
  ).toEqual({ status: "conflict", path, hash: null });
  expect(readdirSync(cwd).some((name) => name.includes(".vorkspace-"))).toBe(
    false,
  );
  // Existing non-editor clients retain their original API, including creates.
  await commands.run("write_text_file", { path, content: "legacy\n" });
  expect(await commands.run("read_text_file", { path })).toBe("legacy\n");
});
it("advances successful editor baselines and preserves CRLF and file permissions", async () => {
  const { commands, repo } = fixture();
  const { cwd, write } = repo();
  const path = join(cwd, "chosen.txt");
  write("chosen.txt", "before\r\n");
  chmodSync(path, 0o751);
  const baseline = await commands.run("read_editor_text_file", {
    path,
  });
  const saved = await commands.run("write_editor_text_file", {
    path,
    content: "after\r\n",
    expectedHash: baseline.hash,
  });
  expect(saved).toEqual({ status: "saved", path, hash: digest("after\r\n") });
  expect(readFileSync(path, "utf8")).toBe("after\r\n");
  expect(statSync(path).mode & 0o777).toBe(0o751);
  expect(
    await commands.run("write_editor_text_file", {
      path,
      content: "again\r\n",
      expectedHash: saved.hash,
    }),
  ).toEqual({ status: "saved", path, hash: digest("again\r\n") });
  // An external edit after publication is retained by the next save attempt.
  write("chosen.txt", "after publication\n");
  expect(
    await commands.run("write_editor_text_file", {
      path,
      content: "next\n",
      expectedHash: digest("again\r\n"),
    }),
  ).toEqual({ status: "conflict", path, hash: digest("after publication\n") });
  expect(readFileSync(path, "utf8")).toBe("after publication\n");
});
it("rechecks external changes made during temporary preparation and cleans up", async () => {
  const { commands, repo } = fixture();
  const { cwd } = repo();
  const path = join(cwd, "chosen.txt");
  const baseline = await commands.run("read_editor_text_file", {
    path,
  });
  const handle = await fsPromises.open(path, "r");
  const prototype = Object.getPrototypeOf(handle);
  const realSync = prototype.sync;
  await handle.close();
  vi.spyOn(prototype, "sync").mockImplementation(async function () {
    writeFileSync(path, "external during preparation\n");
    await realSync.call(this);
  });
  expect(
    await commands.run("write_editor_text_file", {
      path,
      content: "local\n",
      expectedHash: baseline.hash,
    }),
  ).toEqual({
    status: "conflict",
    path,
    hash: digest("external during preparation\n"),
  });
  expect(readFileSync(path, "utf8")).toBe("external during preparation\n");
  expect(readdirSync(cwd).some((name) => name.includes(".vorkspace-"))).toBe(
    false,
  );
});
it(
  "preserves in-project symlinks and rejects editor access outside host projects",
  async () => {
    const { home, commands, repo } = fixture();
    const { cwd } = repo();
    const target = join(cwd, "chosen.txt");
    const path = join(cwd, "link.txt");
    symlinkSync(target, path);
    const baseline = await commands.run("read_editor_text_file", {
      path,
    });
    expect(
      await commands.run("write_editor_text_file", {
        path,
        content: "through link\r\n",
        expectedHash: baseline.hash,
      }),
    ).toEqual({ status: "saved", path, hash: digest("through link\r\n") });
    expect(lstatSync(path).isSymbolicLink()).toBe(true);
    expect(readFileSync(target, "utf8")).toBe("through link\r\n");
    const outside = join(home, "outside.txt");
    writeFileSync(outside, "outside\n");
    symlinkSync(outside, join(cwd, "outside-link"));
    await expect(
      commands.run("read_editor_text_file", {
        path: join(cwd, "outside-link"),
      }),
    ).rejects.toThrow("outside");
    await expect(
      commands.run("write_editor_text_file", {
        path: join(cwd, "outside-link"),
        content: "local\n",
        expectedHash: digest("outside\n"),
      }),
    ).rejects.toThrow("outside");
    expect(readFileSync(outside, "utf8")).toBe("outside\n");
  },
);
