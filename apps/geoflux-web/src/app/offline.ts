// Offline use (see public/sw.js): registering the service worker, knowing whether the browser is
// online, and forgetting what it saved when someone logs out.

import { useSyncExternalStore } from "react";

/** The caches that hold an account's data; the app's own files are kept. */
const ACCOUNT_CACHES = ["geoflux-api", "geoflux-photos"];

export function registerServiceWorker(): void {
  if (!("serviceWorker" in navigator)) return;
  window.addEventListener("load", () => {
    navigator.serviceWorker.register("/sw.js", { scope: "/" }).catch(() => {
      // Without it the app still works online; nothing to tell the person.
    });
  });
}

/** How long a log out waits for the service worker to confirm it has forgotten. */
const FORGET_WAIT_MS = 2_000;

/**
 * Deletes what the service worker saved for the signed-in account, and tells the worker so that
 * an answer still in flight from before isn't saved afterwards.
 */
export async function forgetAccountData(): Promise<void> {
  if (typeof caches === "undefined") return;
  await Promise.all([...ACCOUNT_CACHES.map((c) => caches.delete(c).catch(() => false)), tellWorker()]);
}

function tellWorker(): Promise<void> {
  const worker = typeof navigator !== "undefined" ? navigator.serviceWorker?.controller : null;
  if (!worker) return Promise.resolve();
  return new Promise((resolve) => {
    const channel = new MessageChannel();
    const timer = window.setTimeout(done, FORGET_WAIT_MS);
    function done() {
      window.clearTimeout(timer);
      channel.port1.close();
      resolve();
    }
    channel.port1.onmessage = done;
    worker.postMessage({ type: "forget" }, [channel.port2]);
  });
}

function subscribe(onChange: () => void): () => void {
  window.addEventListener("online", onChange);
  window.addEventListener("offline", onChange);
  return () => {
    window.removeEventListener("online", onChange);
    window.removeEventListener("offline", onChange);
  };
}

export function useOnline(): boolean {
  return useSyncExternalStore(subscribe, () => navigator.onLine, () => true);
}
