import { defineConfig } from "vitest/config";
export default defineConfig({
  test: { environment: "node", include: ["embedded/vox/*.test.js"] },
});
