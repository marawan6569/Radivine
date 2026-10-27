import { defineConfig } from "@playwright/test";
const port = process.env.SITE_TEST_PORT || "4321";
export default defineConfig({
  testDir: "./tests",
  fullyParallel: true,
  use: { baseURL: `http://127.0.0.1:${port}`, browserName: "chromium" },
  webServer: {
    command: `npm run preview -- --host 127.0.0.1 --port ${port} --ignore-lock`,
    url: `http://127.0.0.1:${port}${process.env.SITE_BASE || "/Radivine/"}`,
    reuseExistingServer: false,
  },
});
