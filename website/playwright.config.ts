import { defineConfig } from "@playwright/test";
export default defineConfig({
  testDir: "./tests",
  fullyParallel: true,
  use: { baseURL: "http://127.0.0.1:4321", browserName: "chromium" },
  webServer: {
    command: "npm run preview -- --host 127.0.0.1 --ignore-lock",
    url: `http://127.0.0.1:4321${process.env.SITE_BASE || "/Radivine/"}`,
    reuseExistingServer: false,
  },
});
