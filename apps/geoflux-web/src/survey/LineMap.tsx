// The line on a map: each tower a triangle coloured by its rating (worst finding, healthy, or not
// inspected), joined in walking order, labelled with its number when zoomed in. Clicking a
// triangle selects the tower. MapLibre draws with WebGL, so it is loaded only on the pages that
// show a map (see their lazy imports) and never in the sign-in pages.
//
// Two basemaps: OpenFreeMap's streets (the survey page) and Esri's World Imagery (the dashboard).
// If the streets can't be reached (offline, blocked), the towers are still drawn on a plain
// background; if imagery tiles fail, they are drawn on the imagery style's dark background.

import "maplibre-gl/dist/maplibre-gl.css";

import type { FeatureCollection } from "geojson";
import {
  type GeoJSONSource,
  Map as MlMap,
  type MapLayerMouseEvent,
  NavigationControl,
  setWorkerUrl,
  type StyleSpecification,
  type SymbolLayerSpecification,
} from "maplibre-gl";
// MapLibre finds its worker next to its own file, which a bundler moves; point it at the copy
// the bundler serves. The worker file is self-contained.
import workerUrl from "maplibre-gl/dist/maplibre-gl-worker.mjs?url";
import { useEffect, useRef } from "react";

import type { TowerCollection } from "../api/schemas";
import { featureRating, type Rating, RATING_LABEL, RATINGS } from "./ratings";

export const BASEMAP_STYLE = "https://tiles.openfreemap.org/styles/liberty";
/** Esri World Imagery, keyless; its attribution must stay on the map. */
export const IMAGERY_TILES = "https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}";
const GLYPHS = "https://tiles.openfreemap.org/fonts/{fontstack}/{range}.pbf";
const BASEMAP_TIMEOUT_MS = 8_000;
const TOWERS = "towers";
const SPAN = "span";
/** Triangle icons are drawn at this many device pixels per CSS pixel, so they stay sharp. */
const ICON_RATIO = 2;
const ICON_SIZE = 18;

setWorkerUrl(workerUrl);

const PLAIN: StyleSpecification = {
  version: 8,
  sources: {},
  layers: [{ id: "background", type: "background", paint: { "background-color": "#e9edf1" } }],
};

const IMAGERY: StyleSpecification = {
  version: 8,
  glyphs: GLYPHS,
  sources: {
    imagery: {
      type: "raster",
      tiles: [IMAGERY_TILES],
      tileSize: 256,
      maxzoom: 19,
      attribution: "Powered by Esri · Esri, Maxar, Earthstar Geographics, and the GIS User Community",
    },
  },
  layers: [
    { id: "background", type: "background", paint: { "background-color": "#2b3328" } },
    { id: "imagery", type: "raster", source: "imagery" },
  ],
};

export type Basemap = "streets" | "imagery";

export interface LineMapProps {
  towers: TowerCollection;
  /** `[west, south, east, north]`, for the first view. */
  bounds: [number, number, number, number] | null;
  selected: string | null;
  onSelect: (towerId: string) => void;
  basemap?: Basemap;
  /** Each tower's rating by id; without it, a guess from the tower list (see featureRating). */
  ratings?: ReadonlyMap<string, Rating> | undefined;
  /** The legend's word for `none`, e.g. "No matching finding" while the dashboard is filtered. */
  healthyLabel?: string;
}

/** The colours of the ratings, from the page's CSS. Read when a style loads. */
function ratingColours(): Record<Rating, string> {
  const css = getComputedStyle(document.documentElement);
  const v = (name: string, fallback: string) => css.getPropertyValue(name).trim() || fallback;
  return {
    critical: v("--sev-critical", "#b3261e"),
    high: v("--sev-high", "#d9480f"),
    medium: v("--sev-medium", "#b8860b"),
    low: v("--sev-low", "#2563a8"),
    none: v("--sev-ok", "#2f7d4f"),
    unknown: v("--sev-unknown", "#8b95a1"),
  };
}

const RANK: Record<Rating, number> = { unknown: 0, none: 1, low: 2, medium: 3, high: 4, critical: 5 };

/** The towers that have a position, as GeoJSON MapLibre can index (ids moved into properties),
 * and the span through them in walking order. */
function drawable(towers: TowerCollection, ratings: ReadonlyMap<string, Rating> | undefined): [FeatureCollection, FeatureCollection] {
  const points: FeatureCollection["features"] = [];
  const line: [number, number][] = [];
  for (const f of towers.features) {
    if (!f.geometry) continue;
    const rating = ratings?.get(f.id) ?? featureRating(f);
    points.push({
      type: "Feature",
      geometry: { type: "Point", coordinates: f.geometry.coordinates },
      // Worse towers are drawn last, so on top of their neighbours.
      properties: { towerId: f.id, label: f.properties.label, rating, rank: RANK[rating] },
    });
    line.push(f.geometry.coordinates);
  }
  const span: FeatureCollection = {
    type: "FeatureCollection",
    features: line.length > 1 ? [{ type: "Feature", geometry: { type: "LineString", coordinates: line }, properties: {} }] : [],
  };
  return [{ type: "FeatureCollection", features: points }, span];
}

export default function LineMap({ towers, bounds, selected, onSelect, basemap = "streets", ratings, healthyLabel }: LineMapProps) {
  const container = useRef<HTMLDivElement>(null);
  const map = useRef<MlMap | null>(null);
  const ready = useRef(false);
  // The latest props, for the map's own event handlers.
  const latest = useRef({ towers, ratings, selected, onSelect });
  latest.current = { towers, ratings, selected, onSelect };

  // One map per page; removed (with its WebGL context and workers) when the page goes.
  useEffect(() => {
    if (!container.current) return;
    const m = new MlMap({
      container: container.current,
      style: basemap === "imagery" ? IMAGERY : BASEMAP_STYLE,
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
      const l = latest.current;
      addTowers(m, drawable(l.towers, l.ratings), l.selected, basemap === "imagery");
      ready.current = true;
    });
    const click = (e: MapLayerMouseEvent) => {
      const id = e.features?.[0]?.properties?.["towerId"];
      if (typeof id === "string") latest.current.onSelect(id);
    };
    m.on("click", "tower-marks", click);
    m.on("mouseenter", "tower-marks", () => (m.getCanvas().style.cursor = "pointer"));
    m.on("mouseleave", "tower-marks", () => (m.getCanvas().style.cursor = ""));
    return () => {
      window.clearTimeout(timer);
      ready.current = false;
      map.current = null;
      m.remove();
    };
    // The map is made once; bounds only set its first view, and the basemap is fixed per page.
  }, []);

  useEffect(() => {
    const m = map.current;
    if (!m || !ready.current) return;
    const [points, span] = drawable(towers, ratings);
    (m.getSource(TOWERS) as GeoJSONSource | undefined)?.setData(points);
    (m.getSource(SPAN) as GeoJSONSource | undefined)?.setData(span);
  }, [towers, ratings]);

  useEffect(() => {
    const m = map.current;
    if (!m || !ready.current) return;
    setSelected(m, selected);
    const f = selected ? towers.features.find((t) => t.id === selected) : undefined;
    if (f?.geometry && !m.getBounds().contains(f.geometry.coordinates)) {
      m.easeTo({ center: f.geometry.coordinates, duration: 400 });
    }
  }, [selected, towers]);

  const whole = () => {
    if (bounds) map.current?.fitBounds(bounds, { padding: 40, maxZoom: 15, duration: 400 });
  };

  return (
    <>
      <div ref={container} className="line-map" role="region" aria-label="Map of the line's towers" />
      {bounds ? (
        <button type="button" className="map-whole" onClick={whole}>
          Whole line
        </button>
      ) : null}
      <ul className="map-legend" aria-label="Legend">
        {RATINGS.map((r) => (
          <li key={r}>
            <span className={`tri tri-${r}`} aria-hidden="true" />
            {r === "none" && healthyLabel ? healthyLabel : RATING_LABEL[r]}
          </li>
        ))}
      </ul>
    </>
  );
}

const unselected = (id: string | null) => ["!=", ["get", "towerId"], id ?? ""] as ["!=", ["get", string], string];
const isSelected = (id: string | null) => ["==", ["get", "towerId"], id ?? ""] as ["==", ["get", string], string];

function setSelected(m: MlMap, id: string | null) {
  m.setFilter("tower-selected", isSelected(id));
  if (m.getLayer("tower-labels")) m.setFilter("tower-labels", unselected(id));
  if (m.getLayer("tower-label-selected")) m.setFilter("tower-label-selected", isSelected(id));
}

/** A triangle pointing up, `fill` with a dark edge, at ICON_RATIO device pixels per CSS pixel. */
function triangle(fill: string): ImageData | null {
  const size = ICON_SIZE * ICON_RATIO;
  const canvas = document.createElement("canvas");
  canvas.width = size;
  canvas.height = size;
  const g = canvas.getContext("2d");
  if (!g) return null;
  const pad = 2 * ICON_RATIO;
  g.beginPath();
  g.moveTo(size / 2, pad);
  g.lineTo(size - pad, size - pad);
  g.lineTo(pad, size - pad);
  g.closePath();
  g.fillStyle = fill;
  g.fill();
  g.lineJoin = "round";
  g.lineWidth = 1.5 * ICON_RATIO;
  g.strokeStyle = "rgba(0, 0, 0, 0.6)";
  g.stroke();
  return g.getImageData(0, 0, size, size);
}

function addTowers(m: MlMap, [points, span]: [FeatureCollection, FeatureCollection], selected: string | null, onImagery: boolean) {
  const colours = ratingColours();
  // A fallback style replaces the images with everything else, so they're added per style.
  for (const r of RATINGS) {
    const image = m.hasImage(`tri-${r}`) ? null : triangle(colours[r]);
    if (image) m.addImage(`tri-${r}`, image, { pixelRatio: ICON_RATIO });
  }
  m.addSource(SPAN, { type: "geojson", data: span });
  m.addSource(TOWERS, { type: "geojson", data: points });
  m.addLayer({
    id: "span",
    type: "line",
    source: SPAN,
    layout: { "line-join": "round", "line-cap": "round" },
    paint: { "line-color": onImagery ? "rgba(255, 255, 255, 0.7)" : "#5d6673", "line-width": 1.5 },
  });
  m.addLayer({
    id: "tower-selected",
    type: "circle",
    source: TOWERS,
    filter: isSelected(selected),
    paint: {
      "circle-radius": 15,
      "circle-color": "rgba(11, 107, 203, 0.3)",
      "circle-stroke-color": "#ffffff",
      "circle-stroke-width": 2.5,
    },
  });
  m.addLayer({
    id: "tower-marks",
    type: "symbol",
    source: TOWERS,
    layout: {
      "icon-image": ["concat", "tri-", ["get", "rating"]],
      "icon-size": ["interpolate", ["linear"], ["zoom"], 5, 0.65, 10, 0.85, 14, 1.15],
      "icon-allow-overlap": true,
      "icon-ignore-placement": true,
      "symbol-sort-key": ["get", "rank"],
    },
  });
  // Labels need fonts; the plain fallback has none.
  if (m.getStyle().glyphs) {
    const text: SymbolLayerSpecification["layout"] = {
      "text-field": ["get", "label"],
      "text-size": 12,
      "text-offset": [0, 1.1],
      "text-anchor": "top",
      "text-font": ["Noto Sans Regular"],
    };
    const paint = onImagery
      ? { "text-color": "#ffffff", "text-halo-color": "rgba(0, 0, 0, 0.75)", "text-halo-width": 1.5 }
      : { "text-color": "#1b1f24", "text-halo-color": "#ffffff", "text-halo-width": 1.2 };
    m.addLayer({ id: "tower-labels", type: "symbol", source: TOWERS, minzoom: 11, filter: unselected(selected), layout: text, paint });
    // The selected tower is always named, at any zoom.
    m.addLayer({
      id: "tower-label-selected",
      type: "symbol",
      source: TOWERS,
      filter: isSelected(selected),
      layout: { ...text, "text-allow-overlap": true, "text-size": 13 },
      paint,
    });
  }
}
