import { defineConfig } from "vitest/config";
export default defineConfig({
  test: {
    environment: "node",
    include: ["host/**/*.test.js"],
    // These integration tests launch real Git and Node processes.
    // Competing suites can exceed the default 5s budget, leaving processes
    // alive when teardown tries to remove their directories.
    // Serial file execution avoids cross-suite contention on Git and child processes.
    fileParallelism: false,
    testTimeout: 30_000,
    hookTimeout: 30_000,
  },
});
