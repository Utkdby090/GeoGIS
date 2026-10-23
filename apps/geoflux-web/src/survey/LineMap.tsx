// The line on a map: each tower a dot coloured by its worst finding, labelled with its number.
// Clicking a dot selects the tower. MapLibre draws with WebGL, so it is loaded only on the survey
// page (see SurveyPage's lazy import) and never in the sign-in pages.
//
// The basemap is OpenFreeMap's. If it can't be reached (offline, blocked), the towers are still
// drawn on a plain background.

import "maplibre-gl/dist/maplibre-gl.css";

import type { FeatureCollection } from "geojson";
import { type GeoJSONSource, Map as MlMap, type MapLayerMouseEvent, NavigationControl, setWorkerUrl } from "maplibre-gl";
// MapLibre finds its worker next to its own file, which a bundler moves; point it at the copy
// the bundler serves. The worker file is self-contained.
import workerUrl from "maplibre-gl/dist/maplibre-gl-worker.mjs?url";
import { useEffect, useRef } from "react";

import type { TowerCollection } from "../api/schemas";

export const BASEMAP_STYLE = "https://tiles.openfreemap.org/styles/liberty";
const BASEMAP_TIMEOUT_MS = 8_000;
const SOURCE = "towers";

setWorkerUrl(workerUrl);

const PLAIN: Exclude<Parameters<MlMap["setStyle"]>[0], string | null> = {
  version: 8,
  sources: {},
  layers: [{ id: "background", type: "background", paint: { "background-color": "#e9edf1" } }],
};

export interface LineMapProps {
  towers: TowerCollection;
  /** `[west, south, east, north]`, for the first view. */
  bounds: [number, number, number, number] | null;
  selected: string | null;
  onSelect: (towerId: string) => void;
}

function severityColor(name: string, fallback: string): string {
  const v = getComputedStyle(document.documentElement).getPropertyValue(`--sev-${name}`).trim();
  return v || fallback;
}

/** The towers that have a position, as GeoJSON MapLibre can index (ids moved into properties). */
function drawable(towers: TowerCollection): FeatureCollection {
  return {
    type: "FeatureCollection",
    features: towers.features.flatMap((f) =>
      f.geometry
        ? [
            {
              type: "Feature" as const,
              geometry: { type: "Point" as const, coordinates: f.geometry.coordinates },
              properties: { towerId: f.id, label: f.properties.label, severity: f.properties.severity },
            },
          ]
        : [],
    ),
  };
}

export default function LineMap({ towers, bounds, selected, onSelect }: LineMapProps) {
  const container = useRef<HTMLDivElement>(null);
  const map = useRef<MlMap | null>(null);
  const ready = useRef(false);
  // The latest props, for the map's own event handlers.
  const latest = useRef({ towers, selected, onSelect });
  latest.current = { towers, selected, onSelect };

  // One map per page; removed (with its WebGL context and workers) when the page goes.
  useEffect(() => {
    if (!container.current) return;
    const m = new MlMap({
      container: container.current,
      style: BASEMAP_STYLE,
      // A line's towers, or India when none has a position.
      ...(bounds ? { bounds, fitBoundsOptions: { padding: 40, maxZoom: 15 } } : { center: [78.9, 22.6] as [number, number], zoom: 3.5 }),
      attributionControl: { compact: true },
    });
    map.current = m;
    m.addControl(new NavigationControl({ showCompass: false }), "top-right");
    let usedPlain = false;
    const plain = () => {
      if (usedPlain || ready.current) return;
      usedPlain = true;
      m.setStyle(PLAIN);
    };
    const timer = window.setTimeout(plain, BASEMAP_TIMEOUT_MS);
    m.on("error", () => plain());
    m.on("style.load", () => {
      window.clearTimeout(timer);
      addTowers(m, latest.current.towers, latest.current.selected);
      ready.current = true;
    });
    const click = (e: MapLayerMouseEvent) => {
      const id = e.features?.[0]?.properties?.["towerId"];
      if (typeof id === "string") latest.current.onSelect(id);
    };
    m.on("click", "tower-dots", click);
    m.on("mouseenter", "tower-dots", () => (m.getCanvas().style.cursor = "pointer"));
    m.on("mouseleave", "tower-dots", () => (m.getCanvas().style.cursor = ""));
    return () => {
      window.clearTimeout(timer);
      ready.current = false;
      map.current = null;
      m.remove();
    };
    // The map is made once; bounds only set its first view.
  }, []);

  useEffect(() => {
    const m = map.current;
    if (!m || !ready.current) return;
    (m.getSource(SOURCE) as GeoJSONSource | undefined)?.setData(drawable(towers));
  }, [towers]);

  useEffect(() => {
    const m = map.current;
    if (!m || !ready.current) return;
    m.setFilter("tower-selected", ["==", ["get", "towerId"], selected ?? ""]);
    const f = selected ? towers.features.find((t) => t.id === selected) : undefined;
    if (f?.geometry && !m.getBounds().contains(f.geometry.coordinates)) {
      m.easeTo({ center: f.geometry.coordinates, duration: 400 });
    }
  }, [selected, towers]);

  return <div ref={container} className="line-map" role="region" aria-label="Map of the line's towers" />;
}

function addTowers(m: MlMap, towers: TowerCollection, selected: string | null) {
  m.addSource(SOURCE, { type: "geojson", data: drawable(towers) });
  const sev = (n: string, f: string) => severityColor(n, f);
  m.addLayer({
    id: "tower-selected",
    type: "circle",
    source: SOURCE,
    filter: ["==", ["get", "towerId"], selected ?? ""],
    paint: { "circle-radius": 12, "circle-color": "#0b6bcb", "circle-opacity": 0.35 },
  });
  m.addLayer({
    id: "tower-dots",
    type: "circle",
    source: SOURCE,
    paint: {
      "circle-radius": ["interpolate", ["linear"], ["zoom"], 5, 3, 12, 6, 16, 8],
      "circle-color": [
        "match",
        ["get", "severity"],
        "critical",
        sev("critical", "#b3261e"),
        "high",
        sev("high", "#d9480f"),
        "medium",
        sev("medium", "#b8860b"),
        "low",
        sev("low", "#2f7d32"),
        "#6b7785",
      ],
      "circle-stroke-color": "#ffffff",
      "circle-stroke-width": 1.5,
    },
  });
  // Labels need the basemap's fonts; the plain fallback has none.
  if (m.getStyle().glyphs) {
    m.addLayer({
      id: "tower-labels",
      type: "symbol",
      source: SOURCE,
      minzoom: 11,
      layout: {
        "text-field": ["get", "label"],
        "text-size": 12,
        "text-offset": [0, 1.1],
        "text-anchor": "top",
        "text-font": ["Noto Sans Regular"],
      },
      paint: { "text-color": "#1b1f24", "text-halo-color": "#ffffff", "text-halo-width": 1.2 },
    });
  }
}
