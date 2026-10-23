import { build } from "esbuild";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

// A synthetic history workload, isolated from application data. Each sample
// starts a new Node process so transcript allocations cannot leak between runs.
const { values } = parseArgs({ options: {
  baseline: { type: "string" },
  sessions: { type: "string", default: "200" },
  chars: { type: "string", default: "262144" },
  runs: { type: "string", default: "7" },
} });
const counts = Object.fromEntries(["sessions", "chars", "runs"].map((key) => {
  const value = Number(values[key]);
  if (!Number.isSafeInteger(value) || value < 1) throw new Error(`Invalid --${key}`);
  return [key, value];
}));
const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const directory = mkdtempSync(join(tmpdir(), "vorkspace-startup-benchmark-"));
const variants = values.baseline ? ["baseline", "current"] : ["current"];
try {
  for (const variant of variants) {
    // Export the complete historical host, including its imports. Comparing only
    // engine/store against today's dependencies can silently measure mixed code.
    let sourceRoot = root;
    if (variant === "baseline") {
      sourceRoot = join(directory, "baseline-source");
      const revision = execFileSync("git", ["rev-parse", "--verify", `${values.baseline}^{commit}`],
        { cwd: root, encoding: "utf8" }).trim();
      const files = execFileSync("git", ["ls-tree", "-rz", "--name-only", revision, "--", "host/"],
        { cwd: root, encoding: "utf8" }).split("\0").filter(Boolean);
      if (!files.includes("host/engine.js") || !files.includes("host/store.js"))
        throw new Error("The baseline must contain a JavaScript remote host.");
      for (const path of files) {
        const target = join(sourceRoot, path);
        mkdirSync(dirname(target), { recursive: true });
        writeFileSync(target, execFileSync("git", ["show", `${revision}:${path}`],
          { cwd: root, maxBuffer: 16 * 1024 * 1024 }));
      }
    }
    await build({
      stdin: {
        contents: 'export { HostEngine } from "./host/engine.js"; export { HostStore } from "./host/store.js";',
        resolveDir: sourceRoot,
        loader: "js",
      },
      bundle: true,
      platform: "node",
      format: "esm",
      outfile: join(directory, `${variant}.mjs`),
      logLevel: "silent",
    });
  }
  writeFileSync(join(directory, "worker.mjs"), `
    const [variant, mode, db, count, chars] = process.argv.slice(2);
    const { HostStore, HostEngine } = await import('./' + variant + '.mjs');
    if (mode === 'seed') {
      const store = new HostStore(db);
      const project = store.addProject('/synthetic-project', 'Benchmark');
      const text = 'x'.repeat(Number(chars));
      store.transaction(() => {
        for (let i = 0; i < Number(count); i++) store.save({
          session: {
            id: 'session-' + i, cwd: project.cwd, harness: 'codex', model: 'codex:test',
            title: 'Synthetic history', runtimeMode: 'supervised', busy: false,
            providerSessionId: 'provider-' + i,
            blocks: [{ id: 'body', role: 'assistant', text }],
          }, projectId: project.id, revision: 1, status: 'idle', updatedAt: i,
        }, { type: 'fixture' });
      });
      store.close();
    } else {
      global.gc();
      const heapBefore = process.memoryUsage().heapUsed;
      let parsedChars = 0;
      const original = JSON.parse;
      JSON.parse = (source, ...args) => {
        parsedChars += source.length;
        return original(source, ...args);
      };
      const start = performance.now();
      const store = new HostStore(db);
      const engine = new HostEngine(store, {});
      const ms = performance.now() - start;
      JSON.parse = original;
      console.log(JSON.stringify({ ms, parsedChars,
        heapGrowthMiB: (process.memoryUsage().heapUsed - heapBefore) / 2 ** 20,
        peakRssMiB: process.resourceUsage().maxRSS / 1024,
      }));
      await engine.close();
      store.close();
    }
  `);
  const run = (args) => execFileSync(process.execPath,
    ["--expose-gc", "--disable-warning=ExperimentalWarning", join(directory, "worker.mjs"), ...args],
    { encoding: "utf8" });
  const db = join(directory, "fixture.db");
  run(["current", "seed", db, String(counts.sessions), String(counts.chars)]);
  const samples = Object.fromEntries(variants.map((variant) => [variant, []]));
  for (let i = 0; i < counts.runs; i++) {
    // Alternate order to reduce filesystem-cache and machine-load bias.
    for (const variant of i % 2 ? [...variants].reverse() : variants)
      samples[variant].push(JSON.parse(run([variant, "measure", db])));
  }
  const medians = Object.fromEntries(variants.map((variant) => [variant,
    Object.fromEntries(Object.keys(samples[variant][0]).map((key) => [key,
      samples[variant].map((sample) => sample[key]).sort((a, b) => a - b)[Math.floor(counts.runs / 2)],
    ])),
  ]));
  console.log(JSON.stringify({ node: process.version, ...counts, baseline: values.baseline,
    medians, samples }, null, 2));
} finally {
  rmSync(directory, { recursive: true, force: true });
}
