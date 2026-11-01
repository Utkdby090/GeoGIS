// How a tower is rated on the maps: its worst finding, healthy when it was inspected and nothing
// is wrong, or not inspected. Kept apart from LineMap so pages can work it out without loading
// MapLibre.

import type { Severity, TowerFeature, TowerHealthRow } from "../api/schemas";

/** `none` is an inspected tower with nothing wrong; `unknown` is a tower nobody inspected yet. */
export type Rating = Severity | "unknown";

/** Worst first, the order of the map's legend. */
export const RATINGS: readonly Rating[] = ["critical", "high", "medium", "low", "none", "unknown"];

export const RATING_LABEL: Record<Rating, string> = {
  critical: "Critical",
  high: "High",
  medium: "Medium",
  low: "Low",
  none: "Healthy",
  unknown: "Not inspected",
};

/** The survey page's guess from the tower list alone: a tower with photos was inspected. */
export function featureRating(f: TowerFeature): Rating {
  if (f.properties.severity !== "none") return f.properties.severity;
  return f.properties.photos > 0 ? "none" : "unknown";
}

/** The dashboard's ratings, from the server's line health (which knows about findings too). */
export function healthRatings(rows: readonly TowerHealthRow[]): Map<string, Rating> {
  return new Map(rows.map((r) => [r.id, r.inspected ? r.severity : "unknown"]));
}

/** Black or white, whichever reads better on `hex` (`#rrggbb`). */
export function inkOn(hex: string): string {
  const m = /^#([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m?.[1]) return "#ffffff";
  const n = parseInt(m[1], 16);
  const lin = (c: number) => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  const l = 0.2126 * lin((n >> 16) & 255) + 0.7152 * lin((n >> 8) & 255) + 0.0722 * lin(n & 255);
  // Contrast against white is (1.05)/(l+0.05), against black (l+0.05)/0.05; pick the larger.
  return 1.05 / (l + 0.05) >= (l + 0.05) / 0.05 ? "#ffffff" : "#111111";
}
