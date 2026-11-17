// Browser tests of the UI on the mock backend, in WebKit (the engine of WKWebView and WebKitGTK).
// Run with `pnpm e2e`; set MDGJ_SHOTS_DIR to also save screenshots there.

import { defineConfig, devices } from "@playwright/test";

const url = "http://127.0.0.1:1420";

export default defineConfig({
  testDir: "e2e",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  reporter: process.env.CI ? "github" : "list",
  use: { baseURL: url, trace: "retain-on-failure" },
  projects: [{ name: "webkit", use: { ...devices["Desktop Safari"], viewport: { width: 1100, height: 720 } } }],
  webServer: { command: "pnpm dev", url, reuseExistingServer: !process.env.CI },
});
