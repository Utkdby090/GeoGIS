# RustGIS — instructions for agents

RustGIS is a fast, free and open-source GIS in pure Rust for mapping, analyzing and sharing
geospatial data, on the desktop and in the browser. Its architecture: layered crates, everything
is a command, a thin egui front end, headless screenshots for UI work.

## Never crash
- No `unwrap()`, `expect()`, `panic!`, `unreachable!`, `todo!` in non-test code; no `unsafe`
  (`unsafe_code = "forbid"`). Crate roots carry the `#![deny(clippy::unwrap_used, …)]` block.
- Input from files and command parameters is hostile: `get()` over indexing, clamp sizes,
  bound recursion. `Session::run` and `analysis::run` catch escaped panics as a last resort.
- `crates/engine/src/tests.rs::hostile_params_never_panic` and the analysis equivalent run every
  command/tool with junk parameters. Keep them green; every crash fix gets a regression test.

## Architecture
| Layer | Crate | Job |
|---|---|---|
| L0 | `geom` | coordinates, extents, Web Mercator, geodesic measurement, scale |
| L1 | `data` | features, typed attributes, GeoJSON/CSV/Shapefile/KML/GPX I/O, GeoPackage/GeoParquet/FlatGeobuf/LiDAR (LAS/LAZ) providers (`source`; GeoPackages in the browser through the pure-Rust `sqlite` reader; GDAL formats and DuckDB queries through their programs), SQL where clauses, rasters (GeoTIFF/ASCII grid, Zarr stores, EPSG reprojection) |
| L2 | `map`, `analysis` | layers, symbology, labels, camera, basemaps, web tile layers; geoprocessing tools |
| L2 | `tiles` | web map tiles: XYZ/TMS/WMS rasters, Mapbox Vector Tiles, PMTiles and MBTiles archives, a MapLibre style subset; the tile cache and background fetching (see `docs/user-guide/maps.md` › Basemaps) |
| L3 | `render` | tiny-skia map renderer, label placement, PNG export, font discovery |
| L3 | `plugins` | plugins: sandboxed WebAssembly modules (`wasmi`) adding commands and tools; `.rgplugin` packages, the plugin registry (see `docs/plugins.md`) |
| L3 | `geolibre` | the GeoLibre/Whitebox tool suite as geoprocessing tools (engine feature `geolibre`): run by the optional `geolibre` program natively, by geolibre-rust's WASI runner in the browser |
| L4 | `engine` | `Session`, undo, command registry, sample project |
| L5 | `assistant` | the AI Assistant: a chat agent (sans I/O: `provider` builds and reads OpenAI/Anthropic/Gemini requests, `agent` loops, `tools` run commands; file, network and plugin commands wait for the user's approval) |
| L6 | `ui-egui` | ribbon, panes, map view, attribute table, dialogs (swappable) |
| apps | `rustgis`, `rustgis-cli`, `rustgis-web`, `rustgis-python` | desktop app, command line, browser (WebAssembly), the `rustgis` Python package (PyO3, built with maturin) |

- **Everything is a command**: a `CommandSpec` in `crates/engine/src/cmd/*.rs`. UI-only
  commands (`ui.*`) live in `crates/ui-egui/src/lib.rs`. The ribbon, shortcuts, command search
  and CLI dispatch by id. Commands never open dialogs.
- **Never break wasm**: everything below `apps/` must build for `wasm32-unknown-unknown`
  (CI runs `cargo build --target wasm32-unknown-unknown -p rustgis-web`; the Zstandard decoder in
  `rustgis-data` is C, so this needs clang). Use `web_time`, not
  `std::time`; file access goes through `Services` on the web. `rustgis-geolibre` builds for
  both: natively it runs the `geolibre` program; in the browser the web app runs geolibre-rust's
  WASI runner asynchronously (`external_start` → `Services::geolibre_run` → `gp.finish`).
- **Interface text is translatable**: wrap user-visible literals in `tl!("…")`, use
  `i18n::fmt("… {name} …", &[("name", …)])` instead of `format!` for messages, and pass labels to
  the ribbon widgets (they translate). Then add the new strings to every catalog in
  `crates/ui-egui/src/i18n/*.tsv` (format in `docs/i18n.md`). Command ids, data and the CLI are
  never translated.
- **Layering** is enforced by `cargo xtask layers`: nothing below L6 depends on egui/eframe/rfd.

## Assets
- No third-party vendor icons, images, styles, fonts, help text or code. Icons are drawn in
  code (`icons.rs`). Every asset file has a row in `ATTRIBUTION.md`.
- Screenshots and other large images are not committed here. The Natural Earth sample data is
  vendored in `crates/render/data` (GeoGIS does not download build assets from RustGIS).

## Quality gates
`cargo xtask ci` (fmt, clippy -D warnings, tests, layers) before every commit.
Pull requests run CI on Linux only; pushes to `main` add macOS and Windows. Label a pull request
`ci:macos`, `ci:windows` or `ci:full` to test there before merging (platform-specific code,
paths, file dialogs).

## Looking at the app
- `cargo run --release -p rustgis` (opens the World Atlas sample).
- Headless screenshots: `cargo run --release -p rustgis-ui-egui --example ui_shot -- script.jsonl`
  (see the example header). For UI work, look at the PNG.
- CLI: `rustgis-cli run --sample --cmd 'gp.run={"tool":"buffer","params":{"input":"Cities","distance":100}}' --cmd 'share.exportMap={"path":"out.png"}'`.

## GeoFlux rules (GeoGIS)
Everything above applies. These add what GeoGIS's inspection product needs: `crates/geoflux-*`,
`apps/geoflux-*`, the `geoflux.*` commands in `crates/engine/src/cmd/` and the GeoFlux panes in
`crates/ui-egui`. Definition of done: `docs/definition-of-done.md`. Decisions: `docs/decisions.md`.

### How work is done
- Pull requests target `dev`, never `main`; `dev` goes to `main` only when the owner asks.
- A new feature goes in three steps: (1) three options compared on memory, dependency weight,
  native packaging risk and development speed, no code; (2) a contracts PR with the types, traits
  and error types only; (3) the implementation. A bug fix starts at step 3.
- Read the code you change and its neighbours first; follow the patterns already there. Don't
  rewrite working subsystems, and don't mix unrelated changes into a PR.
- The PR says which checks actually ran (`cargo xtask ci`, `npm run check`, a screenshot) and
  what is known not to work yet. Never claim a check that didn't run.

### Where code goes
- `geoflux-domain`: inspection rules (schemas, rating, findings, photo kinds, tower matching). No
  file system, network, egui or server types; pure functions over plain data, unit-tested.
- `engine` `cmd/*.rs`: commands that read files, change the project (one undo step each) and call
  the domain. `ui-egui`: panes and dialogs only; no rules.
- `geoflux-cloud`: the HTTP contract (routes, request/response types, permissions, ports) shared
  by `apps/geoflux-server` and `geoflux-client`. `apps/geoflux-web/src/api` mirrors it with zod.
- Work on local files stays on the computer: nothing is uploaded just to look at it. The server
  holds only what is shared: accounts, synced surveys, photos, line health, web reports.
- Vendor SDKs run outside the process (DJI's in `geoflux-dirp`, over a pipe), so a bad file
  can't crash GeoGIS and the app stays `unsafe`-free.
- A new dependency is justified in the PR: what it does, licence, binary or bundle size, wasm and
  packaging impact. Prefer what the workspace already uses (`geo`, `proj4rs`, `sha2`, `ureq`).

### Interface text
- GeoFlux screens stay English-only for now (owner's decision, 2026-11-01). Still wrap every
  user-visible literal in `tl!` / `i18n::fmt` as above, so the catalogs can be filled in one go
  before release; just don't add catalog rows yet.

### Coordinates
- GeoFlux layers are WGS 84 longitude/latitude. KML is WGS 84 by its specification; photo GPS
  comes from EXIF in WGS 84. Distances between towers and photos are haversine metres, which is
  well within tolerance at tower scale.
- Input that may be in another system (a survey sheet in UTM, a DXF) is never assumed to be
  WGS 84: detect it, reproject through `crates/map/src/projection.rs`, or refuse with a message.
  Never drop such rows silently.
- Positions out of range, (0, 0) and missing GPS are reported, not guessed.

### Data safety
- Source files are never changed or moved. Photos are linked where they are or copied
  (`.part` then rename; a same-size copy is skipped; a different file with the same name is kept
  and reported). Projects are written with `rustgis_data::io::write_atomic`.
- Every edit is one undoable command; a command that fails changes nothing.
- Only confirmed findings fill forms, the PDF and the dashboard. A rule or model finding records
  its source and version, so a re-run doesn't repeat what was rejected.
- Destructive actions (closing unsaved work, deleting findings, overwriting outputs) ask first.

### Performance and threads
- Never block the UI thread with file reading, photo scanning, thermal decoding or network calls:
  run a worker thread with progress and Cancel (see the `.geox` import and photo scan in
  `geofluxpane.rs`). Server CPU work goes through `http::blocking`.
- Read only what is needed: photo scanning reads the head of each JPEG, not the image.
- Every limit is a named constant with a reason (`MAX_PICKED`, `FAR_METRES`, body limits); every
  loop over photos or towers is bounded by one.

### Server and web
- Every route checks a `Permission`; the organisation comes from the session, never from the
  request. Errors are `ApiError` codes; never return file paths, SQL or stack traces.
- Routes live under `/api/v1`; a breaking change needs a new version, not an edit.
- Every route has a body limit; uploads stream to a `.part` file and stop at the declared size.
- Configuration comes from `GEOFLUX_*` environment variables; secrets never reach the browser or
  the repository.
- Web: TypeScript `strict`, no `any`; every response is checked with zod; the browser's checks
  are for the user's convenience, the server's are the rule.

### Tests
- Domain: known coordinates and expected results, floats compared with a tolerance.
- Server: each flow runs on the in-memory backend and on PostgreSQL (`tests/flow.rs`).
- Web: page tests (`npm run check`) and the Playwright smoke test (`e2e/`).
- Every fixed bug gets a test that fails without the fix.
