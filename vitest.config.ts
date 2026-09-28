import react from "@vitejs/plugin-react";
import { configDefaults, defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  test: {
    // 보존한 감사 진단 사본은 정식 회귀의 실행·집계 대상이 아니다.
    exclude: [
      ...configDefaults.exclude,
      "logs/**",
      // This crypto regression uses node:test and runs separately with node --test.
      "scripts/verify-updater-signature.test.mjs",
    ],
    // jsdom/Fluent suites are CPU-heavy on Windows; concurrent files starve
    // unrelated DOM waits even though each test passes in isolation.
    fileParallelism: false,
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
  },
});
