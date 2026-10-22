import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// `npm run dev` serves the app on :5173 and sends /api to a geoflux-server on :8080, so the
// session cookie stays same-origin. Start that server with GEOFLUX_PUBLIC_URL=http://localhost:5173
// so its links (GeoGIS's /device page, the emails) open here.
export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    strictPort: true,
    proxy: { "/api": { target: "http://localhost:8080", changeOrigin: false } },
  },
  // MapLibre alone is about 1 MB; it loads only on the survey page.
  build: { outDir: "dist", sourcemap: false, assetsInlineLimit: 0, chunkSizeWarningLimit: 1100 },
  test: { environment: "jsdom", restoreMocks: true, include: ["src/**/*.test.{ts,tsx}"] },
});
