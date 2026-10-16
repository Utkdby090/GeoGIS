# GeoGIS

GeoGIS is the desktop GIS for GeoFlux: power-line inspection on a fast, native map. It opens
survey data, links each tower to its inspection photos, reads FLIR and DJI thermal images, and
produces inspection findings and reports, alongside a full set of GIS tools (layers, symbology,
geoprocessing, layouts, a command line and an AI assistant).

GeoGIS is built on [RustGIS](https://github.com/opengeos/RustGIS) (Apache-2.0) and keeps its
engine, crates and command system. It is developed separately: it has its own releases, update
check, documentation links and settings folder, and it does not contact RustGIS's servers.
Fixes from RustGIS can still be merged in when wanted.

## Build and run

Rust 1.90 or later.

```sh
cargo run --release -p rustgis                       # the desktop app (geogis), World Atlas sample
cargo run --release -p rustgis -- data.geojson       # open data (or a .rgis project)
cargo build --release -p geoflux-dirp                # optional: the DJI thermal helper
```

The desktop executable is `geogis` (`geogis.exe` on Windows). The crates keep RustGIS's package
names (`rustgis-*`) so upstream fixes merge cleanly.

DJI thermal photos need the helper above plus DJI's Thermal SDK libraries in
`target/release/dji-sdk` (or the folder named by `GEOFLUX_DJI_SDK`). DJI's licence does not allow
those libraries in this repository; they ship as a separate DJI pack.

## GeoFlux features

| Area | Where |
|---|---|
| Tower and inspection data model | `crates/geoflux-domain` |
| `geoflux.*` commands (import, photos, findings, thermal) | `crates/engine/src/cmd/geoflux.rs` |
| Inspection pane (tower form, photo slots) | `crates/ui-egui/src/geofluxpane.rs` |
| Thermal readings (FLIR in-app, DJI via helper) | `crates/geoflux-thermal`, `apps/geoflux-dirp` |

## Architecture

Layered crates, each depending only on the ones below it (checked by `cargo xtask layers`):

| Layer | Crate | Job |
|---|---|---|
| L0 | `rustgis-geom` | coordinates, extents, projections, geodesic measures |
| L1 | `rustgis-data` | features and attributes; vector and raster formats |
| L2 | `rustgis-map`, `rustgis-analysis`, `rustgis-tiles` | layers, symbology, basemaps; geoprocessing; web tiles |
| L3 | `rustgis-render`, `rustgis-plugins`, `rustgis-geolibre` | rendering and export; plugins; GeoLibre tools |
| L4 | `rustgis-engine` | session, undo, command registry |
| L5 | `rustgis-assistant` | the AI assistant |
| L6 | `rustgis-ui-egui` | the ribbon front end |
| apps | `rustgis` (`geogis`), `rustgis-cli`, `rustgis-web` | desktop, command line, browser |

`cargo xtask ci` runs formatting, clippy, the tests and the layering check. Contributor notes:
[AGENTS.md](AGENTS.md).

## Plugins

GeoGIS has no hosted plugin registry. Plugin files (`.rgplugin`) install from disk, and a
registry address can be entered in the Plugins window.

## License and credits

Licensed under the [Apache License 2.0](LICENSE); see [NOTICE](NOTICE). Based on
[RustGIS](https://github.com/opengeos/RustGIS) by the RustGIS contributors.
Sample data from [Natural Earth](https://www.naturalearthdata.com/) (public domain).
