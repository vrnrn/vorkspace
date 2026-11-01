import { spawn } from "node:child_process";
import { Socket } from "node:net";

// fd 3 belongs only to the host. EOF means it exited, even after a hard crash.
// Unix stdio pipes are sockets: destroying one cancels its pending read.
// A filesystem read can otherwise keep a worker blocked during guard exit.
const parent = new Socket({ fd: 3, readable: true, writable: false });
parent.resume();
const [command, ...args] = process.argv.slice(2);
if (!command) throw new Error("Missing provider command");
const child = spawn(command, args, {
  cwd: process.cwd(),
  env: process.env,
  stdio: "pipe",
  detached: true,
});
process.stdin.pipe(child.stdin);
child.stdout.pipe(process.stdout);
child.stderr.pipe(process.stderr);
child.stdin.on("error", () => {});

let stopping = false;
let exitCode = 1;
let escalation;
let finished = false;
function finish() {
  if (finished) return;
  finished = true;
  clearTimeout(escalation);
  parent.destroy();
  // The watchdog/stdin pipes can remain open after the provider exits.
  // Ownership is finished; do not make the host kill an otherwise idle guard.
  process.stdout.write("", () => {
    process.stderr.write("", () => process.exit(exitCode));
  });
}
function stopTree() {
  if (stopping) return;
  stopping = true;
    // The provider leads its own process group, so the guard survives long
    // enough to escalate if one of its descendants ignores SIGTERM.
    try {
      process.kill(-child.pid, "SIGTERM");
    } catch {
      finish();
      return;
    }
    escalation = setTimeout(() => {
      try {
        process.kill(-child.pid, "SIGKILL");
      } catch {
        /* exited */
      }
      finish();
    }, 1_000);
  
}
parent.on("end", stopTree);
parent.on("error", stopTree);
process.on("SIGTERM", stopTree);
process.on("SIGINT", stopTree);
child.on("error", (error) => {
  console.error(error.message);
  stopTree();
});
child.on("close", (code) => {
  exitCode = code ?? 1;
  if (stopping) {
    try {
      process.kill(-child.pid, 0);
    } catch (error) {
      if (error.code === "ESRCH") finish();
    }
  }
  stopTree();
});
