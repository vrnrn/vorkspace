import { test } from "node:test";
import assert from "node:assert/strict";
import {
  chmodSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readlinkSync,
  readdirSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const script = resolve("scripts/migrate-monocode-to-vorkspace.sh");
const profile = "Library/Application Support/com.monocode.desktop";
const macTest = (name, fn) =>
  test(name, { skip: process.platform !== "darwin" }, fn);
function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), "vorkspace-migration-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const home = join(root, "home with spaces ' and $characters");
  const tools = join(root, "tools");
  mkdirSync(home);
  mkdirSync(tools);
  writeFileSync(
    join(tools, "pgrep"),
    '#!/bin/sh\nexit "${FIXTURE_PROCESS_STATUS:-1}"\n',
    { mode: 0o700 },
  );
  const put = (relative, value = "private fixture", mode = 0o600) => {
    const path = join(home, relative);
    mkdirSync(join(path, ".."), { recursive: true });
    writeFileSync(path, value, { mode });
    return path;
  };
  const backup = join(root, "backup");
  const run = (args = [], extra = {}) =>
    spawnSync("/bin/bash", [script, "--backup-dir", backup, ...args], {
      encoding: "utf8",
      env: { PATH: `${tools}:/usr/bin:/bin`, HOME: home, ...extra },
    });
  return { root, home, backup, tools, put, run };
}

macTest("dry-run leaves all data in place and creates no backup", (t) => {
  const f = fixture(t);
  f.put(`${profile}/monocode.db`, "session secret");
  const result = f.run();
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /Dry-run only/);
  assert.doesNotMatch(result.stdout, /session secret/);
  assert.equal(existsSync(f.backup), false);
  assert.equal(
    readFileSync(join(f.home, profile, "monocode.db"), "utf8"),
    "session secret",
  );
});

macTest(
  "backs up every app-owned location, metadata and symlinks without traversing providers",
  (t) => {
    const f = fixture(t);
    const files = [
      `${profile}/monocode.db`,
      `${profile}/monocode.db-wal`,
      `${profile}/monos/one/memory.md`,
      `${profile}/provider-accounts/account/auth.json`,
      `${profile}/attachments/file.bin`,
      `${profile}/settings.json`,
      "Library/Preferences/com.monocode.desktop.plist",
      "Library/Preferences/ByHost/com.monocode.desktop.ABC.plist",
      "Library/WebKit/com.monocode.desktop/WebsiteData/LocalStorage/settings",
      "Library/Caches/com.monocode.desktop/cache",
      "Library/HTTPStorages/com.monocode.desktop/data",
      "Library/HTTPStorages/com.monocode.desktop.binarycookies",
      "Library/Cookies/com.monocode.desktop.binarycookies",
      "Library/Saved Application State/com.monocode.desktop.savedState/windows.plist",
      "Library/LaunchAgents/com.monocode.host.plist",
      ".monocode-host/host.db",
      ".monocode-migrations/pending",
    ];
    for (const relative of files) f.put(relative, `secret:${relative}`);
    const external = f.put(".codex/auth.json", "do not copy provider secret");
    f.put(".claude/credentials", "do not copy provider secret");
    f.put(".aws/credentials", "do not copy provider secret");
    f.put("project/file", "do not copy workspace");
    symlinkSync(join(external, ".."), join(f.home, profile, "provider-link"));
    symlinkSync(join(external, ".."), join(f.home, ".monocode-symlink"));
    chmodSync(join(f.home, profile, "settings.json"), 0o400);
    const result = f.run(["--apply"]);
    assert.equal(result.status, 0, result.stderr);
    assert.doesNotMatch(result.stdout, /secret:/);
    assert.equal(statSync(f.backup).mode & 0o777, 0o700);
    for (const relative of files) {
      assert.equal(
        readFileSync(join(f.backup, relative), "utf8"),
        `secret:${relative}`,
      );
      assert.equal(existsSync(join(f.home, relative)), true);
    }
    assert.equal(
      statSync(join(f.backup, profile, "settings.json")).mode & 0o777,
      0o400,
    );
    assert.equal(
      lstatSync(join(f.backup, profile, "provider-link")).isSymbolicLink(),
      true,
    );
    assert.equal(
      readlinkSync(join(f.backup, profile, "provider-link")),
      join(external, ".."),
    );
    assert.equal(
      lstatSync(join(f.backup, ".monocode-symlink")).isSymbolicLink(),
      true,
    );
    for (const relative of [".codex", ".claude", ".aws", "project"]) {
      assert.equal(existsSync(join(f.backup, relative)), false);
    }
    assert.equal(
      existsSync(join(f.home, "Library/Application Support/vorkspace")),
      false,
    );
  },
);

macTest(
  "refuses live apps and an unavailable process check before writing",
  (t) => {
    const f = fixture(t);
    f.put(`${profile}/monocode.db`);
    for (const status of ["0", "2"]) {
      const result = f.run(["--apply"], { FIXTURE_PROCESS_STATUS: status });
      assert.notEqual(result.status, 0);
      assert.equal(existsSync(f.backup), false);
    }
  },
);

macTest(
  "recovery copies a whole profile into the normal consumer location without deleting source",
  (t) => {
    const f = fixture(t);
    const source = join(f.root, "legacy profile");
    mkdirSync(source);
    writeFileSync(join(source, "monocode.db"), "recovered sessions", {
      mode: 0o600,
    });
    const destination = join(f.home, profile);
    const args = ["--source", source, "--destination", destination];
    assert.equal(f.run(args).status, 0);
    assert.equal(existsSync(destination), false);
    const result = f.run([...args, "--apply"]);
    assert.equal(result.status, 0, result.stderr);
    for (const dir of [
      source,
      destination,
      join(f.backup, "recovery-source"),
    ]) {
      assert.equal(
        readFileSync(join(dir, "monocode.db"), "utf8"),
        "recovered sessions",
      );
    }
  },
);

macTest(
  "refuses populated destinations including hidden files, and existing backups",
  (t) => {
    const f = fixture(t);
    const source = join(f.root, "source");
    mkdirSync(source);
    const destination = join(f.root, "destination");
    mkdirSync(destination);
    writeFileSync(join(destination, ".existing"), "keep me");
    const result = f.run([
      "--apply",
      "--source",
      source,
      "--destination",
      destination,
    ]);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /populated/);
    assert.equal(existsSync(f.backup), false);
    assert.equal(
      readFileSync(join(destination, ".existing"), "utf8"),
      "keep me",
    );
    mkdirSync(f.backup);
    assert.notEqual(f.run(["--apply"]).status, 0);
    assert.deepEqual(readdirSync(f.backup), []);
  },
);

macTest(
  "refuses overlapping recovery/backup paths and follows no destination symlink",
  (t) => {
    const f = fixture(t);
    const source = join(f.root, "source");
    mkdirSync(source);
    const alias = join(f.root, "alias");
    symlinkSync(source, alias);
    for (const destination of [source, join(source, "nested"), alias]) {
      assert.notEqual(
        f.run(["--apply", "--source", source, "--destination", destination])
          .status,
        0,
      );
    }
    f.put(`${profile}/monocode.db`);
    assert.notEqual(
      f.run(["--apply", "--backup-dir", join(f.home, profile, "backup")])
        .status,
      0,
    );
    assert.equal(existsSync(f.backup), false);
  },
);

macTest("recovery atomically replaces an existing empty destination", (t) => {
  const f = fixture(t);
  const source = join(f.root, "source");
  const destination = join(f.root, "destination");
  mkdirSync(source);
  mkdirSync(destination);
  writeFileSync(join(source, "monocode.db"), "recovered sessions");
  const result = f.run([
    "--apply",
    "--source",
    source,
    "--destination",
    destination,
  ]);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(
    readFileSync(join(destination, "monocode.db"), "utf8"),
    "recovered sessions",
  );
  assert.equal(
    readdirSync(f.root).some((name) => name.startsWith(".vorkspace-recovery.")),
    false,
  );
});

macTest("a destination populated during backup is never overwritten", (t) => {
  const f = fixture(t);
  const source = join(f.root, "source");
  const destination = join(f.root, "destination");
  mkdirSync(source);
  writeFileSync(join(source, "monocode.db"), "old source sessions");
  writeFileSync(
    join(f.tools, "ditto"),
    `#!/bin/sh
set -e
/usr/bin/ditto "$@"
case "$2" in */recovery-source)
  mkdir -p "$FIXTURE_DESTINATION"
  printf '%s' 'new sessions created meanwhile' > "$FIXTURE_DESTINATION/monocode.db"
  ;;
esac
`,
    { mode: 0o700 },
  );
  const result = f.run(
    ["--apply", "--source", source, "--destination", destination],
    {
      FIXTURE_DESTINATION: destination,
    },
  );
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Recovery destination changed/);
  assert.equal(
    readFileSync(join(destination, "monocode.db"), "utf8"),
    "new sessions created meanwhile",
  );
  assert.equal(
    readFileSync(join(source, "monocode.db"), "utf8"),
    "old source sessions",
  );
  assert.equal(
    readFileSync(join(f.backup, "recovery-source", "monocode.db"), "utf8"),
    "old source sessions",
  );
  assert.equal(
    readdirSync(f.root).some((name) => name.startsWith(".vorkspace-recovery.")),
    false,
  );
});

macTest("dry-run reports monos -> bots migration without creating bots or backup", (t) => {
  const f = fixture(t);
  f.put(`${profile}/monos/one/SOUL.md`, "soul secret");
  f.put(`${profile}/monos/one/MEMORY.md`, "memory secret");
  const result = f.run();
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /Dry-run only/);
  assert.match(result.stdout, /monos -> bots/);
  assert.equal(existsSync(f.backup), false);
  assert.equal(existsSync(join(f.home, profile, "bots")), false);
  assert.equal(
    readFileSync(join(f.home, profile, "monos/one/SOUL.md"), "utf8"),
    "soul secret",
  );
});

macTest("apply copies monos to bots while keeping monos intact", (t) => {
  const f = fixture(t);
  f.put(`${profile}/monocode.db`, "session secret");
  f.put(`${profile}/monos/one/SOUL.md`, "soul content");
  f.put(`${profile}/monos/one/memory/topic.md`, "topic content");
  const result = f.run(["--apply"]);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /monos -> bots/);
  assert.equal(
    readFileSync(join(f.backup, profile, "monos/one/SOUL.md"), "utf8"),
    "soul content",
  );
  assert.equal(
    readFileSync(join(f.home, profile, "bots/one/SOUL.md"), "utf8"),
    "soul content",
  );
  assert.equal(
    readFileSync(join(f.home, profile, "bots/one/memory/topic.md"), "utf8"),
    "topic content",
  );
  assert.equal(
    readFileSync(join(f.home, profile, "monos/one/SOUL.md"), "utf8"),
    "soul content",
  );
  assert.equal(statSync(join(f.home, profile, "bots")).mode & 0o777, 0o700);
  assert.equal(
    readdirSync(join(f.home, profile)).some((name) =>
      name.startsWith(".vorkspace-bots."),
    ),
    false,
  );
});

macTest("refuses to clobber an existing bots directory", (t) => {
  const f = fixture(t);
  f.put(`${profile}/monocode.db`, "session secret");
  f.put(`${profile}/monos/one/SOUL.md`, "legacy soul");
  f.put(`${profile}/bots/existing/SOUL.md`, "existing bot soul");
  const dry = f.run();
  assert.equal(dry.status, 0, dry.stderr);
  assert.doesNotMatch(dry.stdout, /monos -> bots/);
  const result = f.run(["--apply"]);
  assert.equal(result.status, 0, result.stderr);
  assert.doesNotMatch(result.stdout, /monos -> bots/);
  assert.equal(
    readFileSync(join(f.home, profile, "bots/existing/SOUL.md"), "utf8"),
    "existing bot soul",
  );
  assert.equal(existsSync(join(f.home, profile, "bots/one")), false);
  assert.equal(
    readFileSync(join(f.home, profile, "monos/one/SOUL.md"), "utf8"),
    "legacy soul",
  );
});

macTest("recovery copies monos to bots inside staging before publication", (t) => {
  const f = fixture(t);
  const source = join(f.root, "legacy profile");
  mkdirSync(source);
  writeFileSync(join(source, "monocode.db"), "recovered db");
  mkdirSync(join(source, "monos/one"), { recursive: true });
  writeFileSync(join(source, "monos/one/SOUL.md"), "recovered soul");
  const destination = join(f.home, profile);
  const args = ["--source", source, "--destination", destination];

  const dry = f.run(args);
  assert.equal(dry.status, 0, dry.stderr);
  assert.match(dry.stdout, /monos -> bots/);
  assert.equal(existsSync(destination), false);

  const result = f.run([...args, "--apply"]);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /monos -> bots/);
  assert.equal(
    readFileSync(join(destination, "bots/one/SOUL.md"), "utf8"),
    "recovered soul",
  );
  assert.equal(
    readFileSync(join(destination, "monos/one/SOUL.md"), "utf8"),
    "recovered soul",
  );
  assert.equal(
    readFileSync(join(source, "monos/one/SOUL.md"), "utf8"),
    "recovered soul",
  );
  assert.equal(existsSync(join(source, "bots")), false);
  assert.equal(
    readFileSync(join(f.backup, "recovery-source", "monos/one/SOUL.md"), "utf8"),
    "recovered soul",
  );
  assert.equal(existsSync(join(f.backup, "recovery-source", "bots")), false);
});

macTest("recovery does not copy monos if source already contains bots", (t) => {
  const f = fixture(t);
  const source = join(f.root, "legacy profile");
  mkdirSync(source);
  writeFileSync(join(source, "monocode.db"), "recovered db");
  mkdirSync(join(source, "monos/one"), { recursive: true });
  writeFileSync(join(source, "monos/one/SOUL.md"), "old soul");
  mkdirSync(join(source, "bots/two"), { recursive: true });
  writeFileSync(join(source, "bots/two/SOUL.md"), "new soul");
  const destination = join(f.home, profile);
  const args = ["--source", source, "--destination", destination];

  const dry = f.run(args);
  assert.equal(dry.status, 0, dry.stderr);
  assert.doesNotMatch(dry.stdout, /monos -> bots/);

  const result = f.run([...args, "--apply"]);
  assert.equal(result.status, 0, result.stderr);
  assert.doesNotMatch(result.stdout, /monos -> bots/);
  assert.equal(
    readFileSync(join(destination, "bots/two/SOUL.md"), "utf8"),
    "new soul",
  );
  assert.equal(existsSync(join(destination, "bots/one")), false);
});

macTest("ignores symlink monos and does not create bots", (t) => {
  const f = fixture(t);
  f.put(`${profile}/monocode.db`, "db data");
  const external = join(f.root, "external-monos");
  mkdirSync(external);
  writeFileSync(join(external, "SOUL.md"), "external soul");
  symlinkSync(external, join(f.home, profile, "monos"));

  const dry = f.run();
  assert.equal(dry.status, 0, dry.stderr);
  assert.doesNotMatch(dry.stdout, /monos -> bots/);

  const result = f.run(["--apply"]);
  assert.equal(result.status, 0, result.stderr);
  assert.doesNotMatch(result.stdout, /monos -> bots/);
  assert.equal(existsSync(join(f.home, profile, "bots")), false);
});

macTest("a bots destination populated during staging is never overwritten", (t) => {
  const f = fixture(t);
  f.put(`${profile}/monocode.db`, "db data");
  f.put(`${profile}/monos/one/SOUL.md`, "original soul");
  const botsDir = join(f.home, profile, "bots");
  writeFileSync(
    join(f.tools, "ditto"),
    `#!/bin/sh
set -e
/usr/bin/ditto "$@"
case "$2" in */.vorkspace-bots.*)
  mkdir -p "$FIXTURE_BOTS"
  printf '%s' 'concurrent bot created meanwhile' > "$FIXTURE_BOTS/SOUL.md"
  ;;
esac
`,
    { mode: 0o700 },
  );
  const result = f.run(["--apply"], {
    FIXTURE_BOTS: botsDir,
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Bots destination changed/);
  assert.equal(
    readFileSync(join(botsDir, "SOUL.md"), "utf8"),
    "concurrent bot created meanwhile",
  );
  assert.equal(
    readFileSync(join(f.home, profile, "monos/one/SOUL.md"), "utf8"),
    "original soul",
  );
  assert.equal(
    readdirSync(join(f.home, profile)).some((name) =>
      name.startsWith(".vorkspace-bots."),
    ),
    false,
  );
});

macTest(
  "dry-run reports codex-monos -> codex-bots migration without creating codex-bots or backup",
  (t) => {
    const f = fixture(t);
    f.put(
      `${profile}/codex-monos/account-1/session.json`,
      "session secret",
    );
    f.put(
      `${profile}/codex-monos/account-1/history.jsonl`,
      "history secret",
    );
    const result = f.run();
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /Dry-run only/);
    assert.match(result.stdout, /codex-monos -> codex-bots/);
    assert.equal(existsSync(f.backup), false);
    assert.equal(existsSync(join(f.home, profile, "codex-bots")), false);
    assert.equal(
      readFileSync(
        join(f.home, profile, "codex-monos/account-1/session.json"),
        "utf8",
      ),
      "session secret",
    );
  },
);

macTest(
  "apply copies codex-monos to codex-bots while keeping codex-monos intact",
  (t) => {
    const f = fixture(t);
    f.put(`${profile}/monocode.db`, "session secret");
    f.put(
      `${profile}/codex-monos/account-1/session.json`,
      "session content",
    );
    f.put(
      `${profile}/codex-monos/account-1/logs/rollout.log`,
      "rollout content",
    );
    const result = f.run(["--apply"]);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /codex-monos -> codex-bots/);
    assert.equal(
      readFileSync(
        join(f.backup, profile, "codex-monos/account-1/session.json"),
        "utf8",
      ),
      "session content",
    );
    assert.equal(
      readFileSync(
        join(f.home, profile, "codex-bots/account-1/session.json"),
        "utf8",
      ),
      "session content",
    );
    assert.equal(
      readFileSync(
        join(f.home, profile, "codex-bots/account-1/logs/rollout.log"),
        "utf8",
      ),
      "rollout content",
    );
    assert.equal(
      readFileSync(
        join(f.home, profile, "codex-monos/account-1/session.json"),
        "utf8",
      ),
      "session content",
    );
    assert.equal(
      statSync(join(f.home, profile, "codex-bots")).mode & 0o777,
      0o700,
    );
    assert.equal(
      readdirSync(join(f.home, profile)).some((name) =>
        name.startsWith(".vorkspace-codex-bots."),
      ),
      false,
    );
  },
);

macTest("refuses to clobber an existing codex-bots directory", (t) => {
  const f = fixture(t);
  f.put(`${profile}/monocode.db`, "session secret");
  f.put(
    `${profile}/codex-monos/account-1/session.json`,
    "legacy session",
  );
  f.put(
    `${profile}/codex-bots/existing/session.json`,
    "existing session",
  );
  const dry = f.run();
  assert.equal(dry.status, 0, dry.stderr);
  assert.doesNotMatch(dry.stdout, /codex-monos -> codex-bots/);
  const result = f.run(["--apply"]);
  assert.equal(result.status, 0, result.stderr);
  assert.doesNotMatch(result.stdout, /codex-monos -> codex-bots/);
  assert.equal(
    readFileSync(
      join(f.home, profile, "codex-bots/existing/session.json"),
      "utf8",
    ),
    "existing session",
  );
  assert.equal(
    existsSync(join(f.home, profile, "codex-bots/account-1")),
    false,
  );
  assert.equal(
    readFileSync(
      join(f.home, profile, "codex-monos/account-1/session.json"),
      "utf8",
    ),
    "legacy session",
  );
});

macTest(
  "recovery copies codex-monos to codex-bots inside staging before publication",
  (t) => {
    const f = fixture(t);
    const source = join(f.root, "legacy profile");
    mkdirSync(source);
    writeFileSync(join(source, "monocode.db"), "recovered db");
    mkdirSync(join(source, "codex-monos/account-1"), { recursive: true });
    writeFileSync(
      join(source, "codex-monos/account-1/session.json"),
      "recovered codex session",
    );
    const destination = join(f.home, profile);
    const args = ["--source", source, "--destination", destination];

    const dry = f.run(args);
    assert.equal(dry.status, 0, dry.stderr);
    assert.match(dry.stdout, /codex-monos -> codex-bots/);
    assert.equal(existsSync(destination), false);

    const result = f.run([...args, "--apply"]);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /codex-monos -> codex-bots/);
    assert.equal(
      readFileSync(
        join(destination, "codex-bots/account-1/session.json"),
        "utf8",
      ),
      "recovered codex session",
    );
    assert.equal(
      readFileSync(
        join(destination, "codex-monos/account-1/session.json"),
        "utf8",
      ),
      "recovered codex session",
    );
    assert.equal(
      readFileSync(
        join(source, "codex-monos/account-1/session.json"),
        "utf8",
      ),
      "recovered codex session",
    );
    assert.equal(existsSync(join(source, "codex-bots")), false);
    assert.equal(
      readFileSync(
        join(
          f.backup,
          "recovery-source",
          "codex-monos/account-1/session.json",
        ),
        "utf8",
      ),
      "recovered codex session",
    );
    assert.equal(
      existsSync(join(f.backup, "recovery-source", "codex-bots")),
      false,
    );
  },
);

macTest(
  "recovery does not copy codex-monos if source already contains codex-bots",
  (t) => {
    const f = fixture(t);
    const source = join(f.root, "legacy profile");
    mkdirSync(source);
    writeFileSync(join(source, "monocode.db"), "recovered db");
    mkdirSync(join(source, "codex-monos/old"), { recursive: true });
    writeFileSync(
      join(source, "codex-monos/old/session.json"),
      "old codex session",
    );
    mkdirSync(join(source, "codex-bots/new"), { recursive: true });
    writeFileSync(
      join(source, "codex-bots/new/session.json"),
      "new codex session",
    );
    const destination = join(f.home, profile);
    const args = ["--source", source, "--destination", destination];

    const dry = f.run(args);
    assert.equal(dry.status, 0, dry.stderr);
    assert.doesNotMatch(dry.stdout, /codex-monos -> codex-bots/);

    const result = f.run([...args, "--apply"]);
    assert.equal(result.status, 0, result.stderr);
    assert.doesNotMatch(result.stdout, /codex-monos -> codex-bots/);
    assert.equal(
      readFileSync(
        join(destination, "codex-bots/new/session.json"),
        "utf8",
      ),
      "new codex session",
    );
    assert.equal(
      existsSync(join(destination, "codex-bots/old")),
      false,
    );
  },
);

macTest("ignores symlink codex-monos and does not create codex-bots", (t) => {
  const f = fixture(t);
  f.put(`${profile}/monocode.db`, "db data");
  const external = join(f.root, "external-codex-monos");
  mkdirSync(external);
  writeFileSync(join(external, "session.json"), "external session");
  symlinkSync(external, join(f.home, profile, "codex-monos"));

  const dry = f.run();
  assert.equal(dry.status, 0, dry.stderr);
  assert.doesNotMatch(dry.stdout, /codex-monos -> codex-bots/);

  const result = f.run(["--apply"]);
  assert.equal(result.status, 0, result.stderr);
  assert.doesNotMatch(result.stdout, /codex-monos -> codex-bots/);
  assert.equal(existsSync(join(f.home, profile, "codex-bots")), false);
});

macTest(
  "a codex-bots destination populated during staging is never overwritten",
  (t) => {
    const f = fixture(t);
    f.put(`${profile}/monocode.db`, "db data");
    f.put(
      `${profile}/codex-monos/one/session.json`,
      "original codex session",
    );
    const codexBotsDir = join(f.home, profile, "codex-bots");
    writeFileSync(
      join(f.tools, "ditto"),
      `#!/bin/sh
set -e
/usr/bin/ditto "$@"
case "$2" in */.vorkspace-codex-bots.*)
  mkdir -p "$FIXTURE_CODEX_BOTS"
  printf '%s' 'concurrent codex bot created meanwhile' > "$FIXTURE_CODEX_BOTS/session.json"
  ;;
esac
`,
      { mode: 0o700 },
    );
    const result = f.run(["--apply"], {
      FIXTURE_CODEX_BOTS: codexBotsDir,
    });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /Bots destination changed/);
    assert.equal(
      readFileSync(join(codexBotsDir, "session.json"), "utf8"),
      "concurrent codex bot created meanwhile",
    );
    assert.equal(
      readFileSync(
        join(f.home, profile, "codex-monos/one/session.json"),
        "utf8",
      ),
      "original codex session",
    );
    assert.equal(
      readdirSync(join(f.home, profile)).some((name) =>
        name.startsWith(".vorkspace-codex-bots."),
      ),
      false,
    );
  },
);

macTest(
  "copies both monos to bots and codex-monos to codex-bots in the same migration run",
  (t) => {
    const f = fixture(t);
    f.put(`${profile}/monocode.db`, "session db");
    f.put(`${profile}/monos/one/SOUL.md`, "soul content");
    f.put(
      `${profile}/codex-monos/account-1/session.json`,
      "codex session content",
    );
    const dry = f.run();
    assert.equal(dry.status, 0, dry.stderr);
    assert.match(dry.stdout, /monos -> bots/);
    assert.match(dry.stdout, /codex-monos -> codex-bots/);
    assert.equal(existsSync(f.backup), false);

    const result = f.run(["--apply"]);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /monos -> bots/);
    assert.match(result.stdout, /codex-monos -> codex-bots/);
    assert.equal(
      readFileSync(join(f.home, profile, "bots/one/SOUL.md"), "utf8"),
      "soul content",
    );
    assert.equal(
      readFileSync(join(f.home, profile, "monos/one/SOUL.md"), "utf8"),
      "soul content",
    );
    assert.equal(
      readFileSync(
        join(f.home, profile, "codex-bots/account-1/session.json"),
        "utf8",
      ),
      "codex session content",
    );
    assert.equal(
      readFileSync(
        join(f.home, profile, "codex-monos/account-1/session.json"),
        "utf8",
      ),
      "codex session content",
    );
  },
);

function execSqlite(dbPath, sql) {
  const result = spawnSync("/usr/bin/sqlite3", [dbPath], {
    input: sql,
    encoding: "utf8",
  });
  if (result.status !== 0) {
    throw new Error(`sqlite3 error (${dbPath}): ${result.stderr}`);
  }
  return result.stdout;
}

function getLocalstorageJson(dbPath, key) {
  const result = spawnSync(
    "/usr/bin/sqlite3",
    ["-tabs", dbPath, `SELECT hex(value) FROM ItemTable WHERE key = '${key}';`],
    { encoding: "utf8" },
  );
  if (result.status !== 0) throw new Error(result.stderr);
  const hex = result.stdout.trim();
  if (!hex) return null;
  return JSON.parse(Buffer.from(hex, "hex").toString("utf16le"));
}

function putLocalstorageJson(dbPath, key, val) {
  const hex = Buffer.from(JSON.stringify(val), "utf16le").toString("hex");
  execSqlite(
    dbPath,
    `INSERT INTO ItemTable (key, value) VALUES ('${key}', X'${hex}');`,
  );
}

function getSqliteHexJson(dbPath, query) {
  const result = spawnSync("/usr/bin/sqlite3", ["-tabs", dbPath, query], {
    encoding: "utf8",
  });
  if (result.status !== 0) throw new Error(result.stderr);
  const hex = result.stdout.trim();
  if (!hex) return null;
  return JSON.parse(Buffer.from(hex, "hex").toString("utf8"));
}

macTest(
  "migrates WebKit localStorage mono keys to bot keys with UTF-16LE encoding and channel transformations",
  (t) => {
    const f = fixture(t);
    const wkRelative =
      "Library/WebKit/com.monocode.desktop/WebsiteData/Default/LocalStorage/localstorage.sqlite3";
    const wkDb = join(f.home, wkRelative);
    mkdirSync(join(wkDb, ".."), { recursive: true });

    execSqlite(
      wkDb,
      "CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB NOT NULL ON CONFLICT FAIL);",
    );

    putLocalstorageJson(wkDb, "monocode:mono-roster", { m1: { name: "Agent 1" } });
    putLocalstorageJson(wkDb, "monocode:mono-handed", { left: "m1" });
    putLocalstorageJson(wkDb, "monocode:mono-channels", [
      { id: "c1", name: "General", monoIds: ["m1"] },
    ]);
    putLocalstorageJson(wkDb, "monocode:mono-channel-messages:c1", [
      {
        id: "msg1",
        from: { kind: "mono", monoId: "m1" },
        author: { kind: "mono", monoId: "m1" },
        reactions: [{ monoId: "m1" }],
      },
    ]);
    putLocalstorageJson(wkDb, "monocode.railMonosPinned", ["m1"]);
    putLocalstorageJson(wkDb, "monocode.monosEnabled", true);
    putLocalstorageJson(wkDb, "monocode.monoMenuBarIcon", "icon.png");
    putLocalstorageJson(wkDb, "monocode:open-mono", "m1");
    // Existing bot key should never be overwritten
    putLocalstorageJson(wkDb, "monocode:bot-handed", { left: "pre-existing-bot" });

    // Dry-run
    const dry = f.run();
    assert.equal(dry.status, 0, dry.stderr);
    assert.match(
      dry.stdout,
      /Legacy localStorage migration: monocode:mono-\* -> monocode:bot-\*/,
    );
    assert.equal(getLocalstorageJson(wkDb, "monocode:bot-roster"), null);
    assert.equal(existsSync(f.backup), false);

    // Apply
    const apply = f.run(["--apply"]);
    assert.equal(apply.status, 0, apply.stderr);
    assert.match(
      apply.stdout,
      /Legacy localStorage migration: monocode:mono-\* -> monocode:bot-\*/,
    );

    // Verify originals kept intact
    assert.deepEqual(getLocalstorageJson(wkDb, "monocode:mono-channels"), [
      { id: "c1", name: "General", monoIds: ["m1"] },
    ]);
    assert.deepEqual(
      getLocalstorageJson(wkDb, "monocode:mono-channel-messages:c1"),
      [
        {
          id: "msg1",
          from: { kind: "mono", monoId: "m1" },
          author: { kind: "mono", monoId: "m1" },
          reactions: [{ monoId: "m1" }],
        },
      ],
    );

    // Verify migrated keys with transformed values
    assert.deepEqual(getLocalstorageJson(wkDb, "monocode:bot-channels"), [
      { id: "c1", name: "General", botIds: ["m1"] },
    ]);
    assert.deepEqual(
      getLocalstorageJson(wkDb, "monocode:bot-channel-messages:c1"),
      [
        {
          id: "msg1",
          from: { kind: "bot", botId: "m1" },
          author: { kind: "bot", botId: "m1" },
          reactions: [{ botId: "m1" }],
        },
      ],
    );
    assert.deepEqual(getLocalstorageJson(wkDb, "monocode:bot-roster"), {
      m1: { name: "Agent 1" },
    });
    // Never overwrite existing bot key
    assert.deepEqual(getLocalstorageJson(wkDb, "monocode:bot-handed"), {
      left: "pre-existing-bot",
    });
    assert.deepEqual(getLocalstorageJson(wkDb, "monocode.railBotsPinned"), ["m1"]);
    assert.equal(getLocalstorageJson(wkDb, "monocode.botsEnabled"), true);
    assert.equal(getLocalstorageJson(wkDb, "monocode.botMenuBarIcon"), "icon.png");
    assert.equal(getLocalstorageJson(wkDb, "monocode:open-bot"), "m1");

    // Verify backup contains the original unmigrated state
    assert.equal(
      getLocalstorageJson(join(f.backup, wkRelative), "monocode:bot-roster"),
      null,
    );
    assert.deepEqual(
      getLocalstorageJson(join(f.backup, wkRelative), "monocode:mono-roster"),
      { m1: { name: "Agent 1" } },
    );

    // Idempotent: Subsequent dry-run does NOT report localStorage migration
    const dry2 = f.run(["--backup-dir", `${f.backup}-2`]);
    assert.equal(dry2.status, 0, dry2.stderr);
    assert.doesNotMatch(dry2.stdout, /Legacy localStorage migration/);
  },
);

macTest(
  "migrates monocode.db mono transcripts, turns, blocks and JSON fields to bot equivalents",
  (t) => {
    const f = fixture(t);
    const dbPath = join(f.home, profile, "monocode.db");
    mkdirSync(join(dbPath, ".."), { recursive: true });

    execSqlite(
      dbPath,
      `
      CREATE TABLE sessions (id TEXT PRIMARY KEY, blocks_json TEXT, queued_messages_json TEXT);
      CREATE TABLE mono_transcripts (session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE);
      CREATE TABLE mono_turns (session_id TEXT NOT NULL REFERENCES mono_transcripts(session_id) ON DELETE CASCADE, seq INTEGER NOT NULL, PRIMARY KEY (session_id, seq));
      CREATE TABLE mono_blocks (session_id TEXT NOT NULL REFERENCES mono_transcripts(session_id) ON DELETE CASCADE, seq INTEGER NOT NULL, block_id TEXT NOT NULL, block_json TEXT NOT NULL, search_text TEXT NOT NULL, is_user INTEGER NOT NULL, is_draft INTEGER NOT NULL, PRIMARY KEY (session_id, seq), UNIQUE (session_id, block_id));
      CREATE TABLE workspace_snapshot (id INTEGER PRIMARY KEY CHECK (id = 1), snapshot_json TEXT NOT NULL, updated_at INTEGER NOT NULL);

      CREATE TABLE bot_transcripts (session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE);
      CREATE TABLE bot_turns (session_id TEXT NOT NULL REFERENCES bot_transcripts(session_id) ON DELETE CASCADE, seq INTEGER NOT NULL, PRIMARY KEY (session_id, seq));
      CREATE TABLE bot_blocks (session_id TEXT NOT NULL REFERENCES bot_transcripts(session_id) ON DELETE CASCADE, seq INTEGER NOT NULL, block_id TEXT NOT NULL, block_json TEXT NOT NULL, search_text TEXT NOT NULL, is_user INTEGER NOT NULL, is_draft INTEGER NOT NULL, PRIMARY KEY (session_id, seq), UNIQUE (session_id, block_id));
      CREATE INDEX bot_blocks_user ON bot_blocks(session_id, is_user, is_draft);
      `,
    );

    const blockPayload = {
      id: "b1",
      role: "user",
      monoHabit: { id: "h1", name: "Morning report", at: 1000 },
      monoCard: { id: "card1", title: "Overview" },
      monoReaction: "👍",
      monoSessionCompletion: {
        sessionId: "sub-1",
        title: "Task 1",
        status: "completed",
      },
      monoPeerMessage: { fromMonoId: "p1", toMonoId: "p2", text: "hello" },
      monoSpawnedSessions: [{ id: "spawn-1" }],
      btwThreads: [
        {
          id: "t1",
          messages: [
            {
              id: "m1",
              blocks: [
                {
                  id: "b_nested",
                  monoSpawnedSessions: [{ id: "spawn-nested" }],
                },
              ],
            },
          ],
        },
      ],
    };

    const queuedPayload = [
      {
        id: "q1",
        monoPeerMessage: { fromMonoId: "p1", toMonoId: "p2", text: "queued peer" },
      },
    ];

    const snapPayload = {
      tabs: ["mono:helper-1", "project:/app", "mono:helper-2"],
    };

    execSqlite(
      dbPath,
      `
      INSERT INTO sessions VALUES (
        's_legacy',
        '${JSON.stringify([blockPayload]).replace(/'/g, "''")}',
        '${JSON.stringify(queuedPayload).replace(/'/g, "''")}'
      );
      INSERT INTO mono_transcripts VALUES ('s_legacy');
      INSERT INTO mono_turns VALUES ('s_legacy', 1);
      INSERT INTO mono_blocks VALUES (
        's_legacy',
        1,
        'b1',
        '${JSON.stringify(blockPayload).replace(/'/g, "''")}',
        'morning report search',
        1,
        0
      );
      INSERT INTO workspace_snapshot VALUES (
        1,
        '${JSON.stringify(snapPayload).replace(/'/g, "''")}',
        5000
      );

      INSERT INTO sessions VALUES ('s_pre_existing', '[]', '[]');
      INSERT INTO mono_transcripts VALUES ('s_pre_existing');
      INSERT INTO mono_turns VALUES ('s_pre_existing', 1);
      INSERT INTO mono_blocks VALUES ('s_pre_existing', 1, 'b_exist', '{"id":"b_exist","text":"old mono"}', 'old search', 1, 0);

      INSERT INTO bot_transcripts VALUES ('s_pre_existing');
      INSERT INTO bot_turns VALUES ('s_pre_existing', 1);
      INSERT INTO bot_blocks VALUES ('s_pre_existing', 1, 'b_exist', '{"id":"b_exist","text":"preserved bot"}', 'preserved search', 1, 0);
      `,
    );

    // Dry-run
    const dry = f.run();
    assert.equal(dry.status, 0, dry.stderr);
    assert.match(dry.stdout, /Legacy SQLite migration: mono_\* -> bot_\*/);
    assert.equal(
      execSqlite(
        dbPath,
        "SELECT COUNT(*) FROM bot_transcripts WHERE session_id = 's_legacy';",
      ).trim(),
      "0",
    );
    assert.equal(existsSync(f.backup), false);

    // Apply
    const apply = f.run(["--apply"]);
    assert.equal(apply.status, 0, apply.stderr);
    assert.match(apply.stdout, /Legacy SQLite migration: mono_\* -> bot_\*/);

    // Verify s_legacy in bot_transcripts, bot_turns, and bot_blocks
    assert.equal(
      execSqlite(
        dbPath,
        "SELECT COUNT(*) FROM bot_transcripts WHERE session_id = 's_legacy';",
      ).trim(),
      "1",
    );
    assert.equal(
      execSqlite(
        dbPath,
        "SELECT COUNT(*) FROM bot_turns WHERE session_id = 's_legacy' AND seq = 1;",
      ).trim(),
      "1",
    );

    const migratedBlock = getSqliteHexJson(
      dbPath,
      "SELECT hex(block_json) FROM bot_blocks WHERE session_id = 's_legacy' AND block_id = 'b1';",
    );
    assert.deepEqual(migratedBlock.botHabit, {
      id: "h1",
      name: "Morning report",
      at: 1000,
    });
    assert.deepEqual(migratedBlock.botCard, { id: "card1", title: "Overview" });
    assert.equal(migratedBlock.botReaction, "👍");
    assert.deepEqual(migratedBlock.botSessionCompletion, {
      sessionId: "sub-1",
      title: "Task 1",
      status: "completed",
    });
    assert.deepEqual(migratedBlock.botPeerMessage, {
      fromBotId: "p1",
      toBotId: "p2",
      text: "hello",
    });
    assert.deepEqual(migratedBlock.botSpawnedSessions, [{ id: "spawn-1" }]);
    assert.deepEqual(
      migratedBlock.btwThreads[0].messages[0].blocks[0].botSpawnedSessions,
      [{ id: "spawn-nested" }],
    );
    assert.equal(migratedBlock.monoHabit, undefined);
    assert.equal(migratedBlock.monoCard, undefined);
    assert.equal(migratedBlock.monoReaction, undefined);
    assert.equal(migratedBlock.monoSessionCompletion, undefined);
    assert.equal(migratedBlock.monoPeerMessage, undefined);
    assert.equal(migratedBlock.monoSpawnedSessions, undefined);

    // Verify pre-existing bot transcript session was NOT overwritten
    const preExistingBlock = getSqliteHexJson(
      dbPath,
      "SELECT hex(block_json) FROM bot_blocks WHERE session_id = 's_pre_existing' AND block_id = 'b_exist';",
    );
    assert.equal(preExistingBlock.text, "preserved bot");

    // Verify original mono tables remain intact
    assert.equal(
      execSqlite(dbPath, "SELECT COUNT(*) FROM mono_transcripts;").trim(),
      "2",
    );
    const originalMonoBlock = getSqliteHexJson(
      dbPath,
      "SELECT hex(block_json) FROM mono_blocks WHERE session_id = 's_legacy' AND block_id = 'b1';",
    );
    assert.equal(originalMonoBlock.monoHabit.id, "h1");

    // Verify sessions.blocks_json and queued_messages_json transformed
    const sessionBlocks = getSqliteHexJson(
      dbPath,
      "SELECT hex(blocks_json) FROM sessions WHERE id = 's_legacy';",
    );
    assert.equal(sessionBlocks[0].botHabit.id, "h1");
    assert.equal(sessionBlocks[0].monoHabit, undefined);

    const sessionQueued = getSqliteHexJson(
      dbPath,
      "SELECT hex(queued_messages_json) FROM sessions WHERE id = 's_legacy';",
    );
    assert.equal(sessionQueued[0].botPeerMessage.fromBotId, "p1");
    assert.equal(sessionQueued[0].botPeerMessage.fromMonoId, undefined);

    // Verify workspace_snapshot.snapshot_json transformed tab IDs
    const snapResult = getSqliteHexJson(
      dbPath,
      "SELECT hex(snapshot_json) FROM workspace_snapshot WHERE id = 1;",
    );
    assert.deepEqual(snapResult.tabs, [
      "bot:helper-1",
      "project:/app",
      "bot:helper-2",
    ]);

    // Idempotent: Subsequent dry-run does NOT report SQLite migration
    const dry2 = f.run(["--backup-dir", `${f.backup}-2`]);
    assert.equal(dry2.status, 0, dry2.stderr);
    assert.doesNotMatch(dry2.stdout, /Legacy SQLite migration/);
  },
);

macTest(
  "recovery mode migrates monocode.db inside staging leaving source untouched",
  (t) => {
    const f = fixture(t);
    const source = join(f.root, "legacy source profile");
    mkdirSync(source);
    const sourceDb = join(source, "monocode.db");
    const destination = join(f.home, profile);

    execSqlite(
      sourceDb,
      `
      CREATE TABLE sessions (id TEXT PRIMARY KEY, blocks_json TEXT);
      CREATE TABLE mono_transcripts (session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE);
      CREATE TABLE mono_turns (session_id TEXT NOT NULL REFERENCES mono_transcripts(session_id) ON DELETE CASCADE, seq INTEGER NOT NULL, PRIMARY KEY (session_id, seq));
      CREATE TABLE mono_blocks (session_id TEXT NOT NULL REFERENCES mono_transcripts(session_id) ON DELETE CASCADE, seq INTEGER NOT NULL, block_id TEXT NOT NULL, block_json TEXT NOT NULL, search_text TEXT NOT NULL, is_user INTEGER NOT NULL, is_draft INTEGER NOT NULL, PRIMARY KEY (session_id, seq), UNIQUE (session_id, block_id));

      INSERT INTO sessions VALUES ('s_rec', '[{"id":"b1","monoHabit":{"id":"h_rec","name":"Recovery","at":123}}]');
      INSERT INTO mono_transcripts VALUES ('s_rec');
      INSERT INTO mono_turns VALUES ('s_rec', 1);
      INSERT INTO mono_blocks VALUES ('s_rec', 1, 'b1', '{"id":"b1","monoHabit":{"id":"h_rec","name":"Recovery","at":123}}', 'rec search', 1, 0);
      `,
    );

    const args = ["--source", source, "--destination", destination];

    // Dry-run
    const dry = f.run(args);
    assert.equal(dry.status, 0, dry.stderr);
    assert.match(dry.stdout, /Legacy SQLite migration: mono_\* -> bot_\*/);
    assert.equal(existsSync(destination), false);

    // Apply
    const apply = f.run([...args, "--apply"]);
    assert.equal(apply.status, 0, apply.stderr);
    assert.match(apply.stdout, /Legacy SQLite migration: mono_\* -> bot_\*/);

    // Source DB was NEVER modified
    const sourceHasBotTranscripts = execSqlite(
      sourceDb,
      "SELECT 1 FROM sqlite_master WHERE type='table' AND name='bot_transcripts';",
    ).trim();
    assert.equal(sourceHasBotTranscripts, "");
    const sourceBlocksJson = getSqliteHexJson(
      sourceDb,
      "SELECT hex(blocks_json) FROM sessions WHERE id = 's_rec';",
    );
    assert.equal(sourceBlocksJson[0].monoHabit.id, "h_rec");
    assert.equal(sourceBlocksJson[0].botHabit, undefined);

    // Destination DB was migrated inside staging before publication
    const destDb = join(destination, "monocode.db");
    assert.equal(
      execSqlite(
        destDb,
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='bot_transcripts';",
      ).trim(),
      "1",
    );
    assert.equal(
      execSqlite(
        destDb,
        "SELECT session_id FROM bot_transcripts WHERE session_id = 's_rec';",
      ).trim(),
      "s_rec",
    );
    const destBlocksJson = getSqliteHexJson(
      destDb,
      "SELECT hex(blocks_json) FROM sessions WHERE id = 's_rec';",
    );
    assert.equal(destBlocksJson[0].botHabit.id, "h_rec");
    assert.equal(destBlocksJson[0].monoHabit, undefined);

    const destBlock = getSqliteHexJson(
      destDb,
      "SELECT hex(block_json) FROM bot_blocks WHERE session_id = 's_rec' AND block_id = 'b1';",
    );
    assert.equal(destBlock.botHabit.id, "h_rec");

    // Backup contains untouched original
    const backupDb = join(f.backup, "recovery-source", "monocode.db");
    const backupHasBotTranscripts = execSqlite(
      backupDb,
      "SELECT 1 FROM sqlite_master WHERE type='table' AND name='bot_transcripts';",
    ).trim();
    assert.equal(backupHasBotTranscripts, "");
  },
);


