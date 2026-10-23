# GeoFlux web app

The browser side of the GeoFlux account server: sign-in pages, the page that approves a GeoGIS
sign-in, synced surveys on a map, towers with photos, markup and PDF reports, the team and the
signed-in computers. It works offline for what the browser has already opened.

It is built into `geoflux-server`, so one program serves the app and its API.

## Run everything from one server

```sh
cd apps/geoflux-web
npm ci
npm run build          # makes dist/, which the server build picks up
cd ../..
cargo run -p geoflux-server
```

Open <http://localhost:8080>. Without a database the server keeps everything in memory, and it
writes the links it would email (confirm your address, reset your password, invitations) to its
log. With `GEOFLUX_DATABASE_URL`, set `GEOFLUX_MAIL_LOG=1` for the same.

A server built without `dist/` still runs; its pages say how to build the app.

## Work on the app

```sh
GEOFLUX_PUBLIC_URL=http://localhost:5173 cargo run -p geoflux-server   # the API on :8080
cd apps/geoflux-web && npm run dev                                     # the app on :5173
```

The development server sends `/api` to `:8080`, so the session cookie stays on one address.

## Tests

- `npm run check`: type check, and the API contract and page tests (vitest, no browser).
- `npm run e2e`: a browser smoke test (Playwright) against a running server with the app built in.
  Point `GEOFLUX_E2E_LOG` at the server's log so the test can follow the email link, and
  `PLAYWRIGHT_CHROMIUM` at a Chromium if Playwright's own isn't installed
  (`npx playwright install chromium`). CI runs it in the "Web app in a browser" job.
