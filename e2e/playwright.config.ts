import { defineConfig } from "@playwright/test";
import { BASE_URL } from "./helpers/server";

export default defineConfig({
  testDir: "./tests",
  timeout: 30_000,
  expect: { timeout: 10_000 },
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: [["list"], ["html", { outputFolder: "playwright-report", open: "never" }]],
  globalSetup: "./global-setup",
  globalTeardown: "./global-teardown",
  use: {
    baseURL: BASE_URL,
    storageState: "test-results/storage-state.json",
    trace: "on-first-retry",
    screenshot: "only-on-failure",
  },
});
