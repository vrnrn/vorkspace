import { expect, it } from "vitest";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { acquireHostOwner } from "./owner.js";
it("rejects simultaneous owners and recovers a stale modern PID lock", async () => {
  const directory = mkdtempSync(join(tmpdir(), "vorkspace-owner-"));
  let release;
  try {
    release = await acquireHostOwner(directory);
    await expect(acquireHostOwner(directory)).rejects.toThrow("already owns");
    const retired = release;
    retired();
    release = undefined;
    writeFileSync(
      join(directory, "owner.lock"),
      JSON.stringify({ version: 2, pid: process.pid }),
    );
    release = await acquireHostOwner(directory);
    retired();
    expect(JSON.parse(readFileSync(join(directory, "owner.lock"), "utf8")))
      .toEqual({ version: 2, pid: process.pid });
  } finally {
    release?.();
    rmSync(directory, { recursive: true, force: true });
  }
});
