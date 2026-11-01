import { execFile, spawn } from "node:child_process";
import { EventEmitter } from "node:events";
import { promisify } from "node:util";
import { readFile, stat } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { isRemoteProvider } from "./runtime/models/connections/protocol.js";
import { providerLaunch, resolveProvider } from "./process.js";
const exec = promisify(execFile);
const ALLOWED_EXEC_ARGS = [
  ["--version"],
  ["--list-models"],
  ["models", "--verbose"],
  ["models", "--json"],
  ["models"],
  ["status", "--json"],
  ["agent", "list"],
];
function execArgsAllowed(_provider, args) {
  const matches = (allowed) =>
    allowed.length === args.length &&
    allowed.every((arg, index) => arg === args[index]);
  return ALLOWED_EXEC_ARGS.some(matches);
}
const HOST_OUTPUT_RECORD_BYTES = 8 * 1024 * 1024;
/** Frame raw UTF-8 bytes before decoding, including unterminated final tails. */
export class HostOutputFramer {
  maxBytes;
  parts = [];
  bytes = 0;
  constructor(maxBytes = HOST_OUTPUT_RECORD_BYTES) {
    this.maxBytes = maxBytes;
  }
  push(chunk, receive) {
    let start = 0;
    while (start < chunk.length) {
      const newline = chunk.indexOf(10, start);
      const end = newline < 0 ? chunk.length : newline;
      const part = chunk.subarray(start, end);
      if (part.length > this.maxBytes - this.bytes) {
        this.parts = [];
        this.bytes = 0;
        throw new Error(
          `Provider output record exceeds the ${this.maxBytes} byte limit; run stopped`,
        );
      }
      // Copy the unfinished piece so a tiny tail does not retain a large chunk.
      this.parts.push(Buffer.from(part));
      this.bytes += part.length;
      if (newline < 0) return;
      receive(this.take().replace(/\r$/, ""));
      start = newline + 1;
    }
  }
  end(receive) {
    if (this.bytes) receive(this.take());
  }
  take() {
    const line = Buffer.concat(this.parts, this.bytes).toString("utf8");
    this.parts = [];
    this.bytes = 0;
    return line;
  }
}
/** Native process implementation for the headless execution proof. */
export class HostChildBackend {
  binaries;
  outputRecordBytes;
  events = new EventEmitter();
  children = new Map();
  closing = false;
  constructor(binaries = {}, outputRecordBytes = HOST_OUTPUT_RECORD_BYTES) {
    this.binaries = binaries;
    this.outputRecordBytes = outputRecordBytes;
    this.events.setMaxListeners(0);
  }
  async resolve(provider) {
    if (this.binaries[provider]) return this.binaries[provider];
    return resolveProvider(provider);
  }
  async listen(event, handler) {
    this.events.on(event, handler);
    return () => {
      this.events.off(event, handler);
    };
  }
  emit(event, payload) {
    this.events.emit(event, { payload });
  }
  async invoke(command, args = {}) {
    const id = String(args.sessionId ?? "");
    if (command.startsWith("harness_resolve_")) {
      const provider = command.slice("harness_resolve_".length);
      if (!isRemoteProvider(provider))
        throw new Error(`Unsupported provider: ${provider}`);
      return {
        path: await this.resolve(provider),
        ...(provider === "antigravity"
          ? { args: process.platform === "linux" ? ["--uid="] : [] }
          : {}),
      };
    }
    switch (command) {
      case "harness_copilot_models": {
        const { stdout } = await exec(
          process.execPath,
          [
            fileURLToPath(new URL("./claude-copilot.mjs", import.meta.url)),
            "--list-models",
          ],
          { timeout: 15_000, maxBuffer: 1024 * 1024 },
        );
        return JSON.parse(stdout);
      }
      case "harness_exec": {
        const provider = args.binaryProvider;
        if (!isRemoteProvider(provider))
          throw new Error("Unsupported headless catalog command");
        const commandPath = await this.resolve(provider);
        if (
          args.command !== commandPath ||
          !Array.isArray(args.args) ||
          !args.args.every((arg) => typeof arg === "string") ||
          !execArgsAllowed(provider, args.args)
        )
          throw new Error("Unsupported headless catalog command");
        const launch = await providerLaunch(commandPath, args.args);
        const { stdout } = await exec(launch.command, launch.args, {
          cwd: typeof args.cwd === "string" ? args.cwd : undefined,
          timeout: 10_000,
          maxBuffer: 8 * 1024 * 1024,
        });
        return stdout;
      }
      case "harness_read_text_file": {
        const path = String(args.path ?? "");
        const info = await stat(path);
        if (!info.isFile() || info.size > 8 * 1024 * 1024)
          throw new Error("File is not a readable text file of at most 8 MiB");
        const bytes = await readFile(path);
        if (bytes.includes(0))
          throw new Error("Binary file is not readable as text");
        return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
      }
      case "harness_spawn":
        return await this.start(id, args);
      case "harness_write": {
        const child = this.children.get(id);
        if (!child || child.stdin.destroyed)
          throw new Error("Provider process is not running");
        await new Promise((resolve, reject) => {
          const timer = setTimeout(
            () => reject(new Error("Provider stdin write timed out")),
            15_000,
          );
          child.stdin.write(`${String(args.line)}\n`, (error) => {
            clearTimeout(timer);
            if (error) reject(error);
            else resolve();
          });
        });
        return undefined;
      }
      case "harness_kill":
        await this.kill(id);
        return undefined;
      case "harness_kill_all":
        await this.close();
        return undefined;
      default:
        throw new Error(`Unsupported headless process operation: ${command}`);
    }
  }
  async start(id, args) {
    if (this.closing) throw new Error("Host is stopping");
    await this.kill(id);
    if (this.closing) throw new Error("Host is stopping");
    const account = args.account;
    if (account?.id && account.id !== "default")
      throw new Error(
        "Named provider accounts are not supported by this host yet",
      );
    const commandArgs = args.args;
    let launch = await providerLaunch(String(args.command), commandArgs);
    if (
      args.binaryProvider === "claude" &&
      commandArgs.some(
        (arg, index) =>
          arg === "--model" && commandArgs[index + 1]?.startsWith("copilot/"),
      )
    ) {
      launch = {
        command: process.execPath,
        args: [
          fileURLToPath(new URL("./claude-copilot.mjs", import.meta.url)),
          launch.command,
          ...launch.args,
        ],
      };
    }
    if (this.closing) throw new Error("Host is stopping");
    const child = spawn(
      process.execPath,
      [
        fileURLToPath(new URL("./provider-guard.mjs", import.meta.url)),
        launch.command,
        ...launch.args,
      ],
      {
        cwd: String(args.cwd),
        stdio: ["pipe", "pipe", "pipe", "pipe"],
        detached: true,
        env: { ...process.env, VORKSPACE_HOST: "1" },
      },
    );
    this.children.set(id, child);
    child.stdin.on("error", () => {
      /* write callbacks report failures */
    });
    let outputError;
    const failOutput = (error) => {
      if (outputError) return;
      outputError = error instanceof Error ? error.message : String(error);
      if (this.children.get(id) === child) void this.kill(id);
    };
    this.lines(
      child,
      id,
      "stdout",
      failOutput,
      () => outputError !== undefined,
    );
    this.lines(
      child,
      id,
      "stderr",
      failOutput,
      () => outputError !== undefined,
    );
    child.on("close", (code) => {
      if (this.children.get(id) === child) this.children.delete(id);
      this.emit("harness-exit", {
        sessionId: id,
        code: outputError ? 1 : code,
        pid: child.pid,
        ...(outputError ? { error: outputError } : {}),
      });
    });
    await new Promise((resolve, reject) => {
      child.once("spawn", resolve);
      child.once("error", reject);
    });
    return child.pid;
  }
  lines(child, id, stream, fail, failed) {
    const framer = new HostOutputFramer(this.outputRecordBytes);
    const receive = (line) =>
      this.emit(`harness-${stream}`, { sessionId: id, line });
    child[stream].on("data", (data) => {
      if (failed()) return;
      try {
        framer.push(data, receive);
      } catch (error) {
        fail(error);
      }
    });
    child[stream].on("end", () => {
      if (!failed()) framer.end(receive);
    });
  }
  signal(child, signal) {
    try {
      if (child.pid) process.kill(-child.pid, signal);
    } catch {
      /* already exited */
    }
  }
  async kill(id) {
    const child = this.children.get(id);
    if (!child) return;
    this.children.delete(id);
    child.stdin.destroy();
    this.signal(child, "SIGTERM");
    await new Promise((resolve) => {
      const timer = setTimeout(() => {
        this.signal(child, "SIGKILL");
        resolve();
        // The guard needs a full second to escalate against the provider's
        // separate process group before we may kill the guard itself.
      }, 3_000);
      child.once("close", () => {
        clearTimeout(timer);
        resolve();
      });
    });
  }
  async close() {
    this.closing = true;
    await Promise.all([...this.children.keys()].map((id) => this.kill(id)));
  }
}
