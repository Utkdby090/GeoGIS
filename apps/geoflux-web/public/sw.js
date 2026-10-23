// GeoFlux's service worker: lets the web app open and show what it last loaded when there is no
// connection (read-only; nothing is changed offline). Online, everything comes from the server.
//
// - The app's own files: built files (/assets/, named by their content) are kept once fetched;
//   pages come from the network first, from the cache only when offline.
// - The API's reads (surveys, towers, the person): network first, the last answer when offline.
// - Photos never change once uploaded: cache first, at most MAX_PHOTOS kept.
// Logging out deletes the API and photo caches (see app/offline.ts), so the next person on the
// same browser can't read them. It also tells this worker, which then drops any answer still on
// its way from before the log out instead of saving it into the emptied cache.

const VERSION = "v1";
const APP = `geoflux-app-${VERSION}`;
const API = "geoflux-api";
const PHOTOS = "geoflux-photos";
const MAX_PHOTOS = 200;

/** Goes up at each log out; an answer fetched under an older one isn't kept. */
let generation = 0;

self.addEventListener("message", (event) => {
  if (event.data?.type !== "forget") return;
  generation += 1;
  event.waitUntil(Promise.all([caches.delete(API), caches.delete(PHOTOS)]).then(() => event.ports[0]?.postMessage(true)));
});

self.addEventListener("install", () => self.skipWaiting());

self.addEventListener("activate", (event) => {
  event.waitUntil(
    (async () => {
      for (const key of await caches.keys()) {
        if (key.startsWith("geoflux-app-") && key !== APP) await caches.delete(key);
      }
      await self.clients.claim();
    })(),
  );
});

self.addEventListener("fetch", (event) => {
  const req = event.request;
  if (req.method !== "GET") return;
  const url = new URL(req.url);
  if (url.origin !== self.location.origin) return;
  const path = url.pathname;
  if (path.startsWith("/api/")) {
    // Reports are downloads, made fresh each time; never kept.
    if (path.endsWith("/report.pdf") || path.endsWith("/report.zip")) return;
    if (/^\/api\/v1\/surveys\/[^/]+\/photos\//.test(path)) {
      event.respondWith(cacheFirst(req, PHOTOS, MAX_PHOTOS));
    } else {
      event.respondWith(networkFirst(req, API));
    }
  } else if (path.startsWith("/assets/")) {
    event.respondWith(cacheFirst(req, APP, 0));
  } else if (req.mode === "navigate") {
    // Every page of the app is index.html.
    event.respondWith(networkFirst(req, APP, "/index.html"));
  }
});

async function networkFirst(req, cacheName, key) {
  const started = generation;
  const cache = await caches.open(cacheName);
  try {
    const res = await fetch(req);
    // Keep only good answers: an error or a "log in first" must not replace the last real one.
    if (res.ok && started === generation) await cache.put(key ?? req, res.clone());
    return res;
  } catch (e) {
    const hit = await cache.match(key ?? req);
    if (hit) return hit;
    throw e;
  }
}

async function cacheFirst(req, cacheName, max) {
  const started = generation;
  const cache = await caches.open(cacheName);
  const hit = await cache.match(req);
  if (hit) return hit;
  const res = await fetch(req);
  if (res.ok && started === generation) {
    await cache.put(req, res.clone());
    if (max > 0) {
      const keys = await cache.keys();
      // Oldest first: drop the earliest kept until under the limit.
      for (const k of keys.slice(0, Math.max(0, keys.length - max))) await cache.delete(k);
    }
  }
  return res;
}
