# Architecture

RustGIS is a layered Cargo workspace. Lower layers never depend on the UI, which keeps the
engine scriptable and the front end swappable.

| Layer | Crate | Job |
| --- | --- | --- |
| L0 | `rustgis-geom` | Coordinates, extents, Web Mercator, geodesic distance/area, scale |
| L1 | `rustgis-data` | Features and typed attributes; GeoJSON, CSV, Shapefile; a read-only SQLite file reader (`sqlite`, for GeoPackages in the browser); data providers reading GeoPackage, GeoParquet and FlatGeobuf in place by their spatial index (GDAL formats and DuckDB queries converted by those programs when installed); SQL where clauses; rasters (GeoTIFF, ASCII grid) and their coordinate systems |
| L2 | `rustgis-map`, `rustgis-analysis` | Layers, symbology, labeling, camera, basemaps and web tile layers; geoprocessing tools |
| L2 | `rustgis-tiles` | Web map tiles: XYZ/TMS/WMS raster tiles, Mapbox Vector Tiles, PMTiles and MBTiles archives and a subset of MapLibre styles; a cache that fetches in the background (worker threads natively, the browser's `fetch` on the web) |
| L3 | `rustgis-render` | tiny-skia renderer, label placement, PNG export |
| L3 | `rustgis-geolibre` | The [GeoLibre tools](user-guide/analysis.md#geolibre-tools) as geoprocessing tools: run by the optional `geolibre` program natively, by geolibre-rust's WASI runner in the browser |
| L3 | `rustgis-plugins` | [Plugins](plugins.md): sandboxed WebAssembly modules that add commands and geoprocessing tools; `.rgplugin` packages and the plugin registry |
| L4 | `rustgis-engine` | Session, undo, command registry, sample project |
| L5 | `rustgis-assistant` | The [AI Assistant](user-guide/assistant.md): a chat agent that acts through commands; OpenAI-compatible, Anthropic and Gemini wire formats as plain request/response data, so the desktop (a thread), the browser (`fetch`) and the command line share one loop |
| L6 | `rustgis-ui-egui` | The ribbon front end |
| apps | `rustgis`, `rustgis-cli`, `rustgis-web` | Desktop app, command line, browser (WebAssembly) |

## Everything is a command

Each user-visible action is a `CommandSpec` with an id, label, ribbon location, shortcut,
parameter description, an `enabled` check and a `run` function. The ribbon, keyboard shortcuts,
command search and CLI all dispatch through `Session::run`, which also snapshots the project
for undo and restores it if a command fails.

Plugin commands (`plugin:<plugin>/<command>`) dispatch through `Session::run` too, as one undo
step. While they run, they reach the session only through permission-checked requests (see
[Plugins](plugins.md)).

## Never crash

Production code avoids `unwrap`, `expect` and panics, and treats file contents and command
parameters as hostile. Tests run every command and every geoprocessing tool with junk
parameters.

## Quality gates

```sh
cargo xtask ci        # fmt, clippy -D warnings, tests, layering check
```

## GeoFlux (GeoGIS)

GeoGIS adds GeoFlux's inspection product on top of these layers. The same layering rule holds:
nothing in a `geoflux-*` crate depends on egui.

| Layer | Crate or app | Job |
| --- | --- | --- |
| L1 | `geoflux-domain` | Inspection schemas, rating, findings, defect and tower-part taxonomies, photo kinds, tower matching, line KML, `.geox` mapping. No I/O. |
| L1 | `geoflux-thermal` | Thermal photo decoding (FLIR in Rust; DJI through the `geoflux-dirp` helper over a pipe) |
| L1 | `geoflux-licence` | Signed sign-in passes and the paid-feature gate |
| L1 | `geoflux-cloud` | The account server's HTTP contract: routes, request and response types, permissions, ports |
| L3 | `geoflux-report` | Inspection PDF reports |
| L3 | `geoflux-client` | HTTPS client for the account server (sign-in, Sync to Cloud) |
| L4 | `engine` `cmd/geoflux*.rs`, `photo_*.rs`, `findings.rs`, `cloud_sync.rs` … | The `geoflux.*` commands |
| L6 | `ui-egui` `geofluxpane.rs`, `findingsview.rs`, `cloudui.rs` … | Inspection pane, Findings window, Account and Sync |
| apps | `geoflux-dirp` | DJI thermal helper (the only process that loads DJI's SDK) |
| apps | `geoflux-server` | Account server (Axum; memory or PostgreSQL), which also serves the web app |
| apps | `geoflux-web` | Web app (React, MapLibre, ECharts, three.js), built into the server |

### Where work runs

| Work | Runs | Why |
| --- | --- | --- |
| Opening, scanning, sorting and reviewing a survey's files | Desktop | The photos are large and local; nothing is uploaded just to look at it |
| Thermal decoding | Desktop (DJI in its helper) | Vendor SDK isolated from the app |
| Inspection rules and rating | Shared (`geoflux-domain`) | Desktop, server and reports must agree |
| Accounts, teams, computers, sign-in passes | Server | One source of truth for who may do what |
| Synced surveys, photos, line health, web reports | Server | Shared with the team and clients |
| Viewing a synced survey and its dashboard | Browser | Reads the server's API; no rules of its own |

The browser's checks are for convenience; the server checks every permission itself.
