import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { parseArgs } from "node:util";

// Each sample is a new process with temporary data, so one run's caches cannot
// flatter the next. Medians are printed; --save and --compare use a JSON path
// outside the repository, because measurements do not belong in it.
const { values } = parseArgs({ options: {
  runs: { type: "string", default: "7" },
  save: { type: "string" },
  compare: { type: "string" },
} });
const runs = Number(values.runs);
if (!Number.isSafeInteger(runs) || runs < 1) throw new Error("Invalid --runs");

const profile = ["test", "--locked", "-p", "vorkspace-gpui", "--profile", "perf"];
const filter = ["shell::perf_tests::native_perf_sample", "--", "--ignored", "--exact"];
execFileSync("cargo", [...profile, "--no-run"], { stdio: "inherit" });
const directory = mkdtempSync(join(tmpdir(), "vorkspace-perf-"));
const samples = {};
try {
  for (let run = 0; run < runs; run++) {
    const output = join(directory, "sample.json");
    execFileSync("cargo", [...profile, ...filter], {
      stdio: "ignore",
      env: { ...process.env, VORKSPACE_PERF_OUTPUT: output },
    });
    for (const [name, value] of Object.entries(JSON.parse(readFileSync(output, "utf8"))))
      (samples[name] ??= []).push(value);
  }
} finally {
  rmSync(directory, { recursive: true, force: true });
}
const medians = Object.fromEntries(Object.entries(samples).map(([name, values]) => {
  const sorted = [...values].sort((a, b) => a - b);
  return [name, { median: sorted[sorted.length >> 1], min: sorted[0], max: sorted.at(-1) }];
}));
const baseline = values.compare ? JSON.parse(readFileSync(values.compare, "utf8")) : {};
for (const [name, { median, min, max }] of Object.entries(medians)) {
  const before = baseline[name]?.median;
  const change = before ? `  ${(((median - before) / before) * 100).toFixed(1).padStart(6)}%` : "";
  console.log(`${name.padEnd(24)} ${median.toFixed(2).padStart(10)} ms  [${min.toFixed(2)}–${max.toFixed(2)}]${change}`);
}
if (values.save) writeFileSync(values.save, `${JSON.stringify(medians, null, 2)}\n`);
