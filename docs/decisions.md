# GeoGIS decisions

The decisions that shape GeoGIS, newest last. Each says what was chosen, what was not, and why.
Add a row when a choice would surprise someone reading the code later.

| Date | Decision | Not chosen | Why |
| --- | --- | --- | --- |
| 2026-10-15 | Fork RustGIS and port GeoFlux into it phase by phase | Waiting for RustGIS 1.0; RustGIS as a sidecar CLI | Plugins can't add panels or forms; a fork gives one desktop app now |
| 2026-10-16 | DJI thermal SDK only in the `geoflux-dirp` helper, over a pipe; FLIR decoded in Rust | Loading DJI's DLLs in the app | A bad photo or SDK crash can't take GeoGIS down; the app keeps `unsafe` forbidden |
| 2026-10-16 | GeoGIS is a standalone repository; internal `rustgis-*` crate names stay | Renaming every crate | RustGIS fixes can still be merged; Apache-2.0 NOTICE and LICENSE stay |
| 2026-10-16 | PDF reports as real vector PDFs (`pdf-writer`), Helvetica (Latin) by default | Images of pages; bundling Noto Sans now | Searchable, small files; Hindi fonts when needed |
| 2026-10-19 | Subscription model: GeoGIS free, GeoFlux account plans unlock paid features; signed passes work offline 30 days | Per-machine licence keys | One account across desktop and web; air-gapped sites get pass files |
| 2026-10-19 | Feature PRs target `dev`; `dev` goes to `main` when the owner asks | PRs straight to `main` | The owner reviews `dev` as a whole |
| 2026-10-20 | Account server in Rust (Axum) reusing GeoGIS crates; built-in logins; PostgreSQL | A separate stack for the server | One set of rules and contract types for desktop and server |
| 2026-10-20 | The web app is built into the one server | A CDN-hosted app; rules and PDFs in the browser | One deploy on free hosting; the server makes the same PDFs as GeoGIS |
| 2026-10-30 | Photos are linked to their nearest tower in place; copying to tower folders is a separate command | Copying by default; span-aware matching first | Originals untouched; fast; copies when the team needs folders |
| 2026-10-30 | Findings are first-class records (photo hash, region, defect, part, source and version, confidence, status); form fields and the PDF derive from confirmed findings | Findings as form fields; an AI pipeline first | A later AI model becomes one more Inspector, not a new data model |
| 2026-10-31 | Line health dashboard built in the web app (ECharts, react-grid-layout) | Superset, Kibana or Grafana | Same login and team as Sync; no second system to host |
| 2026-10-31 | One static 3D tower model with named attach points for every tower | A model per tower type now; photogrammetry | Works for every line today; a real model drops in later |
| 2026-11-01 | Engineering rules for GeoFlux code in `AGENTS.md`; done list in `docs/definition-of-done.md` | One long prompt | Rules live with the code, under review like code |
