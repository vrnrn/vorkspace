import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { access, chmod, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { test } from "node:test";

test("macOS installer reports replacement failures without nesting or replacing the old app", async (t) => {
  for (const failure of ["staging cleanup", "destination removal", "copy", "none"]) {
    await t.test(failure, async (t) => {
      const root = await mkdtemp(join(tmpdir(), "vorkspace-install-test-"));
      t.after(() => rm(root, { recursive: true, force: true }));
      const source = join(root, "built app", "vorkspace.app");
      const destination = join(root, "Applications", "vorkspace.app");
      const next = `${destination}.next`;
      for (const [path, version] of [[source, "fresh"], [destination, "old"], [next, "stale"]]) {
        await mkdir(path, { recursive: true });
        await writeFile(join(path, "version"), version);
      }
      const bin = join(root, "bin");
      await mkdir(bin);
      await writeFile(join(bin, "rm"), `#!/bin/sh
if [ "$2" = "$FAIL_RM_PATH" ]; then exit 1; fi
exec /bin/rm "$@"
`);
      await chmod(join(bin, "rm"), 0o755);
      const script = `
        Object.defineProperty(process, "platform", { value: "darwin" });
        const { installAppBundle } = await import(process.argv[1]);
        console.log(JSON.stringify({ installed: installAppBundle(process.argv[2], process.argv[3]) }));
      `;
      // Run the real installer against disposable directories. Only the selected
      // removal is rejected; cp/mv retain their actual filesystem semantics.
      const result = spawnSync(process.execPath, ["--input-type=module", "-e", script,
        new URL("./build-native.mjs", import.meta.url).href,
        failure === "copy" ? join(root, "missing.app") : source, destination,
      ], {
        env: {
          ...process.env, CI: "", VORKSPACE_NO_INSTALL: "",
          PATH: `${bin}${delimiter}${process.env.PATH}`,
          FAIL_RM_PATH: failure === "staging cleanup" ? next : failure === "destination removal" ? destination : "",
        },
        encoding: "utf8",
      });
      assert.ifError(result.error);
      assert.equal(result.status, 0, result.stdout + result.stderr);
      const { installed } = JSON.parse(result.stdout.trim().split("\n").at(-1));
      assert.equal(installed, failure === "none", result.stdout + result.stderr);
      assert.equal(await readFile(join(destination, "version"), "utf8"), installed ? "fresh" : "old");
      await assert.rejects(access(join(destination, "vorkspace.app.next")));
      assert.equal(result.stdout.includes(`Installed: ${destination}`), installed);
      if (failure === "staging cleanup") {
        assert.equal(await readFile(join(next, "version"), "utf8"), "stale");
      }
    });
  }
});
