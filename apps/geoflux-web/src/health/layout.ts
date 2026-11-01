// Where each panel sits on the dashboard. A person can drag and resize panels on a wide screen;
// their arrangement is kept in this browser only (a convenience, not shared), and a saved
// arrangement that doesn't fit this version of the page is ignored rather than trusted.

import type { Layout, LayoutItem } from "react-grid-layout";

export const PANELS = ["tiles", "map", "tower", "along", "severity", "defects", "parts", "attention", "peaks"] as const;
export type Panel = (typeof PANELS)[number];

export const COLS = { lg: 12, sm: 1 } as const;
/** Wider than this (in pixels of the grid itself) panels sit side by side and can be moved. */
export const BREAKPOINTS = { lg: 900, sm: 0 } as const;
export const ROW_HEIGHT = 40;

// v2 added the map and the tower; an older arrangement is simply not used.
const STORE_KEY = "geoflux.dashboard.layout.v2";

const WIDE: Layout = [
  { i: "tiles", x: 0, y: 0, w: 12, h: 3, minH: 3, minW: 6 },
  // The map and the 3D tower side by side, as on the dashboard sample; the tower runs down
  // beside the map and the chart along the line.
  { i: "map", x: 0, y: 3, w: 7, h: 10, minW: 4, minH: 6 },
  { i: "tower", x: 7, y: 3, w: 5, h: 18, minW: 4, minH: 10 },
  { i: "along", x: 0, y: 13, w: 7, h: 8, minW: 4, minH: 5 },
  { i: "severity", x: 0, y: 21, w: 4, h: 8, minW: 3, minH: 5 },
  { i: "defects", x: 4, y: 21, w: 4, h: 8, minW: 3, minH: 5 },
  { i: "parts", x: 8, y: 21, w: 4, h: 8, minW: 3, minH: 5 },
  { i: "attention", x: 0, y: 29, w: 8, h: 10, minW: 4, minH: 5 },
  { i: "peaks", x: 8, y: 29, w: 4, h: 10, minW: 3, minH: 4 },
];

const NARROW_HEIGHT: Partial<Record<string, number>> = { tiles: 5, map: 9, tower: 20 };

/** On a narrow screen panels stack in this order and stay put. */
const NARROW: Layout = (() => {
  let y = 0;
  return WIDE.map((p): LayoutItem => {
    const h = NARROW_HEIGHT[p.i] ?? p.h;
    const item = { i: p.i, x: 0, y, w: 1, h, static: true };
    y += h;
    return item;
  });
})();

export function defaultLayouts(): { lg: Layout; sm: Layout } {
  return { lg: WIDE, sm: NARROW };
}

/** The saved wide arrangement, when it names every panel once and fits the grid. */
export function loadWide(): Layout {
  let text: string | null = null;
  try {
    text = globalThis.localStorage?.getItem(STORE_KEY) ?? null;
  } catch {
    return WIDE;
  }
  if (!text) return WIDE;
  try {
    return checkWide(JSON.parse(text)) ?? WIDE;
  } catch {
    return WIDE;
  }
}

export function checkWide(value: unknown): Layout | null {
  if (!Array.isArray(value) || value.length !== PANELS.length) return null;
  const seen = new Set<string>();
  const out: LayoutItem[] = [];
  for (const v of value as unknown[]) {
    if (typeof v !== "object" || v === null) return null;
    const { i, x, y, w, h } = v as Record<string, unknown>;
    const base = WIDE.find((p) => p.i === i);
    if (!base || seen.has(base.i)) return null;
    const whole = (n: unknown, lo: number, hi: number): n is number => Number.isInteger(n) && (n as number) >= lo && (n as number) <= hi;
    if (!whole(x, 0, COLS.lg - 1) || !whole(w, base.minW ?? 1, COLS.lg) || x + w > COLS.lg) return null;
    if (!whole(y, 0, 500) || !whole(h, base.minH ?? 1, 60)) return null;
    seen.add(base.i);
    out.push({ ...base, x, y, w, h });
  }
  return out;
}

/** Keeps the wide arrangement for next time; only where, never what the panels showed. */
export function saveWide(layout: Layout): void {
  const plain = layout.filter((p) => (PANELS as readonly string[]).includes(p.i)).map(({ i, x, y, w, h }) => ({ i, x, y, w, h }));
  try {
    globalThis.localStorage?.setItem(STORE_KEY, JSON.stringify(plain));
  } catch {
    // Private window or storage full: the arrangement just isn't kept.
  }
}

export function forgetWide(): void {
  try {
    globalThis.localStorage?.removeItem(STORE_KEY);
  } catch {
    // Nothing to forget.
  }
}
