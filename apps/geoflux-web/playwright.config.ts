// The browser smoke test (e2e/), run against a real geoflux-server with the app built in. CI
// starts that server; locally:
//   npm run build && cargo run -p geoflux-server 2> server.log
//   GEOFLUX_E2E_LOG=server.log npm run e2e
import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "e2e",
  timeout: 60_000,
  retries: 0,
  workers: 1,
  reporter: process.env["CI"] ? [["list"], ["html", { open: "never" }]] : "list",
  use: {
    baseURL: process.env["GEOFLUX_E2E_URL"] ?? "http://localhost:8080",
    trace: "retain-on-failure",
    viewport: { width: 1300, height: 800 },
    launchOptions: {
      // A browser already on the machine (PLAYWRIGHT_CHROMIUM), else Playwright's own; WebGL in
      // software so the map draws without a GPU.
      ...(process.env["PLAYWRIGHT_CHROMIUM"] ? { executablePath: process.env["PLAYWRIGHT_CHROMIUM"] } : {}),
      args: ["--use-gl=angle", "--use-angle=swiftshader", "--enable-unsafe-swiftshader"],
    },
  },
});
