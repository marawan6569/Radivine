import { defineConfig } from "@playwright/test";
const port = process.env.SITE_TEST_PORT || "4321";
export default defineConfig({
  testDir: "./tests",
  testMatch: "**/*.spec.ts",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: 0,
  reporter: [["list"], ["html", { open: "never" }]],
  use: { baseURL: `http://127.0.0.1:${port}`, trace: "retain-on-failure" },
  projects: (process.env.SITE_BROWSERS || "chromium").split(",").map((name) => {
    if (!["chromium", "firefox", "webkit"].includes(name))
      throw new Error(`Unknown browser: ${name}`);
    return {
      name,
      use: { browserName: name as "chromium" | "firefox" | "webkit" },
    };
  }),
  webServer: {
    command: `node scripts/serve-static.mjs`,
    env: { SITE_TEST_PORT: port },
    url: `http://127.0.0.1:${port}${process.env.SITE_BASE || "/Radivine/"}`,
    reuseExistingServer: false,
  },
});
