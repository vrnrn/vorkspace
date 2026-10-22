import { expect, it, vi } from "vitest";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { readFileSync, existsSync } from "node:fs";
import { spawn } from "node:child_process";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { HostChildBackend, HostOutputFramer } from "./child-backend.js";
import { REMOTE_PROVIDERS } from "./runtime/models/connections/protocol.js";
it("resolves every provider and runs only allowed provider commands", async () => {
  const directory = mkdtempSync(join(tmpdir(), "vorkspace-catalog-test-"));
  const file = join(directory, "provider.cjs");
  writeFileSync(file, "console.log(JSON.stringify(process.argv.slice(2)))");
  const backend = new HostChildBackend(
    Object.fromEntries(REMOTE_PROVIDERS.map((provider) => [provider, file])),
  );
  try {
    for (const provider of REMOTE_PROVIDERS) {
      const resolved = await backend.invoke(`harness_resolve_${provider}`);
      expect(resolved.path).toBe(file);
      if (provider === "antigravity")
        expect(resolved.args).toEqual(
          process.platform === "linux" ? ["--uid="] : [],
        );
    }
    const output = await backend.invoke("harness_exec", {
      command: file,
      args: ["models", "--json"],
      binaryProvider: "pi",
      cwd: directory,
    });
    expect(JSON.parse(output)).toEqual(["models", "--json"]);
    const cleanupArgs = [
      "--no-auto-update",
      "sessions",
      "delete",
      "550e8400-e29b-41d4-a716-446655440000",
    ];
    await expect(
      backend.invoke("harness_exec", {
        command: file,
        args: ["-e", "console.log('unsafe')"],
        binaryProvider: "pi",
      }),
    ).rejects.toThrow("Unsupported headless catalog command");
    for (const [provider, args] of [
      ["fx", ["service", "start"]],
      ["opencode", ["service", "stop"]],
      ["opencode", ["service get", "password"]],
      ["opencode", ["service", "get", "password", "--json"]],
      ["opencode", ["models --json"]],
      ["cursor", cleanupArgs],
      ["grok", ["--no-auto-update", "sessions", "delete", "--all"]],
      ["grok", ["--no-auto-update", "sessions", "delete", "../sessions"]],
      ["grok", [...cleanupArgs, "--all"]],
    ]) {
      await expect(
        backend.invoke("harness_exec", {
          command: file,
          args,
          binaryProvider: provider,
        }),
      ).rejects.toThrow("Unsupported headless catalog command");
    }
    writeFileSync(join(directory, "note.txt"), "host-owned transcript");
    expect(
      await backend.invoke("harness_read_text_file", {
        path: join(directory, "note.txt"),
      }),
    ).toBe("host-owned transcript");
    const transcript = "x".repeat(1024 * 1024 + 1);
    writeFileSync(join(directory, "large-transcript.txt"), transcript);
    expect(
      await backend.invoke("harness_read_text_file", {
        path: join(directory, "large-transcript.txt"),
      }),
    ).toBe(transcript);
  } finally {
    await backend.close();
    rmSync(directory, { recursive: true, force: true });
  }
});
it("runs the resolved Claude version fallback in headless mode", async () => {
  const backend = new HostChildBackend({ claude: process.execPath });
  try {
    const version = await backend.invoke("harness_exec", {
      command: process.execPath,
      args: ["--version"],
      binaryProvider: "claude",
      cwd: process.cwd(),
    });
    expect(version.trim()).toBe(process.version);
    await expect(
      backend.invoke("harness_exec", {
        command: process.execPath,
        args: ["-e", "console.log('unsafe')"],
        binaryProvider: "claude",
      }),
    ).rejects.toThrow("Unsupported headless catalog command");
  } finally {
    await backend.close();
  }
});
it.each([false, true])(
  "stops a provider tree (ignores SIGTERM: %s)",
  async (stubborn) => {
    const directory = mkdtempSync(join(tmpdir(), "vorkspace-provider-tree-"));
    const file = join(directory, "provider.cjs");
    writeFileSync(
      file,
      `const { spawn } = require('node:child_process');
const child = spawn(process.execPath, ['-e', ${JSON.stringify(`${stubborn ? "process.on('SIGTERM', () => {});" : ""} console.log('ready'); setInterval(() => {}, 1000)`)}], { stdio: ['ignore', 'pipe', 'ignore'] });
child.stdout.once('data', () => console.log(JSON.stringify({ child: child.pid })));
setInterval(() => {}, 1000);
`,
    );
    const backend = new HostChildBackend();
    let descendant;
    const stopListening = await backend.listen(
      "harness-stdout",
      ({ payload }) => {
        descendant = JSON.parse(payload.line).child;
      },
    );
    try {
      await backend.invoke("harness_spawn", {
        sessionId: "tree",
        command: file,
        args: [],
        cwd: directory,
      });
      await vi.waitFor(() => expect(descendant).toBeTruthy());
      await backend.kill("tree");
      await vi.waitFor(
        () => expect(() => process.kill(descendant, 0)).toThrow(),
        { timeout: 5000 },
      );
    } finally {
      stopListening();
      await backend.close();
      if (descendant) {
        try {
          process.kill(descendant, "SIGKILL");
        } catch {
          /* gone */
        }
      }
      rmSync(directory, { recursive: true, force: true });
    }
  },
  15_000,
);
it("stops a provider tree when its host pipe closes unexpectedly", async () => {
  const directory = mkdtempSync(join(tmpdir(), "vorkspace-provider-crash-"));
  const treeFile = join(directory, "tree.json");
  const providerFile = join(directory, "provider.cjs");
  writeFileSync(
    providerFile,
    `const { spawn } = require('node:child_process');
const { writeFileSync } = require('node:fs');
const descendant = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], { stdio: 'ignore' });
writeFileSync(${JSON.stringify(treeFile)}, JSON.stringify({ provider: process.pid, descendant: descendant.pid }));
setInterval(() => {}, 1000);
`,
  );
  const guard = spawn(
    process.execPath,
    [resolve("build/host/provider-guard.mjs"), process.execPath, providerFile],
    {
      cwd: directory,
      stdio: ["pipe", "ignore", "ignore", "pipe"],
      detached: process.platform !== "win32",
      windowsHide: true,
    },
  );
  let guardClosed = false;
  guard.once("close", () => {
    guardClosed = true;
  });
  let tree;
  try {
    await vi.waitFor(() => expect(existsSync(treeFile)).toBe(true));
    tree = JSON.parse(readFileSync(treeFile, "utf8"));
    guard.stdio[3]?.destroy();
    await vi.waitFor(
      () => {
        expect(() => process.kill(tree.provider, 0)).toThrow();
        expect(() => process.kill(tree.descendant, 0)).toThrow();
      },
      { timeout: 5_000 },
    );
    // The guard's cwd keeps this directory locked on Windows until it exits.
    await vi.waitFor(() => expect(guardClosed).toBe(true), { timeout: 5_000 });
  } finally {
    guard.stdio[3]?.destroy();
    guard.kill("SIGKILL");
    for (const pid of [tree?.provider, tree?.descendant]) {
      if (pid)
        try {
          process.kill(pid, "SIGKILL");
        } catch {
          /* gone */
        }
    }
    await vi.waitFor(() => expect(guardClosed).toBe(true), { timeout: 5_000 });
    rmSync(directory, {
      recursive: true,
      force: true,
      maxRetries: 10,
      retryDelay: 100,
    });
  }
}, 10_000);
it("frames split UTF-8 and final tails by bytes with injectable tiny budgets", () => {
  const framer = new HostOutputFramer(8);
  const lines = [];
  const utf8 = Buffer.from("éé\r\ntail");
  for (const byte of utf8)
    framer.push(Buffer.from([byte]), (line) => lines.push(line));
  framer.end((line) => lines.push(line));
  expect(lines).toEqual(["éé", "tail"]);
  for (const input of ["ééé", "123456", "123456\n"]) {
    const oversized = new HostOutputFramer(5);
    const receive = vi.fn();
    expect(() => oversized.push(Buffer.from(input), receive)).toThrow(
      "5 byte limit",
    );
    expect(receive).not.toHaveBeenCalled();
  }
});
it("delivers ordered protocol records and its final tail before exit", async () => {
  const directory = mkdtempSync(join(tmpdir(), "vorkspace-output-tail-"));
  const file = join(directory, "provider.cjs");
  writeFileSync(
    file,
    `process.stdout.write('approval\\ncompletion\\nfinal tail')`,
  );
  const backend = new HostChildBackend();
  const events = [];
  await backend.listen("harness-stdout", ({ payload }) =>
    events.push(payload.line),
  );
  await backend.listen("harness-exit", ({ payload }) =>
    events.push(`exit:${payload.code}`),
  );
  try {
    await backend.invoke("harness_spawn", {
      sessionId: "tail",
      command: file,
      args: [],
      cwd: directory,
    });
    await vi.waitFor(() =>
      expect(events).toEqual([
        "approval",
        "completion",
        "final tail",
        "exit:0",
      ]),
    );
  } finally {
    await backend.close();
    rmSync(directory, { recursive: true, force: true });
  }
});
it("stops an oversized unterminated record with an explicit failed exit", async () => {
  const directory = mkdtempSync(join(tmpdir(), "vorkspace-output-overflow-"));
  const file = join(directory, "provider.cjs");
  writeFileSync(
    file,
    `process.stdout.write('é'.repeat(20)); setInterval(() => {}, 1000)`,
  );
  const backend = new HostChildBackend({}, 32);
  const receive = vi.fn();
  const exited = vi.fn();
  await backend.listen("harness-stdout", receive);
  await backend.listen("harness-exit", exited);
  try {
    await backend.invoke("harness_spawn", {
      sessionId: "overflow",
      command: file,
      args: [],
      cwd: directory,
    });
    await vi.waitFor(() => expect(exited).toHaveBeenCalledOnce());
    expect(exited.mock.calls[0][0].payload).toMatchObject({
      code: 1,
      error: expect.stringContaining("32 byte limit"),
    });
    expect(receive).not.toHaveBeenCalled();
    await backend.kill("overflow");
  } finally {
    await backend.close();
    rmSync(directory, { recursive: true, force: true });
  }
});
