import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { access, chmod, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { test } from "node:test";

test("macOS installer publishes a staged app and preserves the old app on failure", { skip: process.platform !== "darwin" }, async (t) => {
  for (const scenario of ["copy failure", "publish failure", "fresh install", "replacement"]) {
    await t.test(scenario, async (t) => {
      const root = await mkdtemp(join(tmpdir(), "vorkspace-install-test-"));
      t.after(() => rm(root, { recursive: true, force: true }));
      const source = join(root, "built app", "vorkspace.app");
      const applications = join(root, "Applications");
      const destination = join(applications, "vorkspace.app");
      await mkdir(source, { recursive: true });
      await writeFile(join(source, "version"), "fresh");
      await mkdir(applications);
      if (scenario !== "fresh install") {
        await mkdir(destination);
        await writeFile(join(destination, "version"), "old");
      }
      const bin = join(root, "bin");
      await mkdir(bin);
      if (scenario === "publish failure") {
        await writeFile(join(bin, "python3"), "#!/bin/sh\nexit 1\n");
        await chmod(join(bin, "python3"), 0o755);
      }
      const script = `
        const { installAppBundle } = await import(process.argv[1]);
        console.log(JSON.stringify({ installed: installAppBundle(process.argv[2], process.argv[3]) }));
      `;
      const result = spawnSync(process.execPath, ["--input-type=module", "-e", script,
        new URL("./build-native.mjs", import.meta.url).href,
        scenario === "copy failure" ? join(root, "missing.app") : source, destination,
      ], {
        env: {
          ...process.env, CI: "", VORKSPACE_NO_INSTALL: "",
          PATH: `${bin}${delimiter}${process.env.PATH}`,
        },
        encoding: "utf8",
      });
      assert.ifError(result.error);
      assert.equal(result.status, 0, result.stdout + result.stderr);
      const { installed } = JSON.parse(result.stdout.trim().split("\n").at(-1));
      assert.equal(installed, ["fresh install", "replacement"].includes(scenario), result.stdout + result.stderr);
      assert.equal(await readFile(join(destination, "version"), "utf8"), installed ? "fresh" : "old");
      await assert.rejects(access(join(destination, "vorkspace.app")));
      assert.deepEqual(await readdir(applications), ["vorkspace.app"]);
      assert.equal(result.stdout.includes(`Installed: ${destination}`), installed);
    });
  }
});
