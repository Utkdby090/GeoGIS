# GeoFlux inspection

GeoGIS adds GeoFlux's inspection workflow to RustGIS. An **inspection schema** describes one
industry's assets: the form fields, image slots, how its survey CSV maps onto fields, and how a
finding's severity is rated. Schemas are JSON files, so a new tender's layers need a file, not
code. The power-line schema (`com.geo.telecom`) is built in.

## Commands

| Command | What it does |
|---|---|
| `geoflux.schemas` | Lists the built-in schemas, the project's own, and which layers follow which. |
| `geoflux.addSchema` | Adds a schema to the project from a `.json` file (`path`) or text (`json`). A project schema with a built-in id replaces the built-in one. Schemas travel inside the `.rgis` project. |
| `geoflux.importCsv` | Reads a survey CSV (`path`, or `text`) with a schema (`schema`, default `com.geo.telecom`) into a point layer. Field labels become aliases; extra columns are kept; rows without a position are kept without geometry; skipped rows are reported with a reason. Each asset is rated and the layer is styled by severity. |
| `geoflux.classify` | Rates every asset of the layer again from its schema's rule, after edits. Returns the count per level. |
| `geoflux.validate` | Checks every asset against the schema (required fields, numbers, dates, options, coordinates) and lists the problems. |
| `geoflux.setValues` | Saves an inspection form's values for one asset (`row`, `values`: field key → text) as one undoable edit. Every field is checked first; nothing changes if one is invalid. The asset is rated again. |
| `geoflux.sequence` | The layer's assets in natural order of their label (`9_0` before `10_0`); `flaggedOnly` keeps those rated above none. |
| `geoflux.goTo` | Selects an asset and centres the map on it. |
| `geoflux.linkPhotos` | Links a folder of photos (`folder`) to the layer's assets: one sub-folder per tower (`76_0/…`), or files named after the tower (`76_0_thermal.jpg`). Each photo goes in the image slot whose words its file name has; files that fit nowhere are listed with the reason. A single tower's own folder (`…/76_0`) links to that tower. Paths are kept relative to the folder, inside the project. |
| `geoflux.photos` | One asset's image slots and their photos. |
| `geoflux.addPhotos` | Adds photo files from anywhere (`paths`) to one slot (`slot`) of one asset (`row`). They show after the linked folder's photos and stay through a relink. |
| `geoflux.removePhoto` | Takes an added photo out of its slot; the file is kept. |
| `geoflux.thermal.measure` | Temperatures of a tower's thermal photo (`row`, `photo` index): min, max and mean over the whole photo or a `region` (`[x0, y0, x1, y1]` in pixels), the reference (a `reference` box, else the ambient temperature) and the rise. The form's emissivity, ambient temperature and humidity are used, and any the camera's library refused are listed. |
| `geoflux.thermal.record` | Writes the hottest pixel (Fault Temp) and its rise over the reference (Rise Temp) into the form as one undoable edit. |
| `geoflux.thermal.scan` | Measures every tower's thermal photo and records each, all as one undoable edit. Towers without a thermal photo are skipped; photos that can't be read are counted and listed. |

**Analyze › Inspection** opens the Inspection pane: the asset's form laid out from its schema,
previous and next (Page Up and Page Down) through the towers, a flagged-only filter, and the
photos by slot (click one for the full view). Press **+** in a slot to add photos to it from anywhere, and **×** to take an added one out.

**Analyze › Thermal** turns on the Thermal view under the photos: the tower's thermal photo as
temperatures (Iron, Rainbow, White Hot or Black Hot), a colour bar, the hottest pixel marked, and
the temperature under the mouse. **Spot** reads one pixel, **Box** measures a component (its
hottest pixel is the finding), and **Reference** boxes a healthy, similar component to compare
with instead of the ambient temperature. **Fixed range** keeps one scale while walking a line.
**Record Finding** writes Fault Temp and Rise Temp into the form, and **Scan Line** measures
every tower in the background (with Cancel) and records them all as one undoable edit.

```sh
rustgis-cli run --cmd 'geoflux.importCsv={"path":"line.csv"}' \
  --cmd 'geoflux.validate={}' --save line.rgis
```

## Survey CSVs

Files from Excel and survey tools are read as they come: UTF-8 with or without a byte-order mark,
UTF-16, or Windows "ANSI" (where `°` is one byte); separated by commas, semicolons or tabs.
Coordinates can be decimal or degrees/minutes/seconds (`27°30'44.3"N`), and a pair whose
hemisphere letters show the columns were swapped is put right.

## Writing a schema

See `crates/geoflux-domain/schemas/telecom.json`. Field types: `text`, `number`, `integer`,
`date` (`YYYY-MM-DD`), `dropdown` and `multiSelect` (with `options`), `boolean`, `photo`. The keys
`core.latitude` and `core.longitude` edit the asset's position.

`labelField` names the field that names an asset (default: the first field). Each image slot
lists `matches`, the words of a file name that put a photo in it (DJI's `_T`, `_W`, `_Z` suffixes
included); thermal slots are tried first, and one slot can be the `fallback`.

Severity is rated from `rise`, or from `measured − reference` when `rise` is blank, against
`thresholds` (low, medium, high, critical). An asset matching every `flaggedWhen` entry is never
rated below low, and is rated medium when it has no usable numbers.

## Thermal photos

FLIR radiometric JPEGs are read inside GeoGIS, in pure Rust (the raw sensor values and FLIR's
Planck and atmosphere constants, as ExifTool and Thermimage read them). Thermal photos show in
the desktop app; the web app can't read photo folders yet.

DJI R-JPEGs (H20T, H30T, M30T, M3T, M4T, XT S and others) need DJI's Thermal SDK, which GeoGIS
never loads itself: a small helper program, `geoflux-dirp`, loads it and reads photos sent over a
pipe. A photo that crashes DJI's library ends the helper, not GeoGIS; that photo fails and the
next one starts a fresh helper. Each photo has 20 seconds. The helper keeps GeoFlux's handling of
the SDK: settings are clamped to the range the camera reports, and when the camera refuses a
combination each setting is tried alone; the view says which were refused. The helper starts on
the first DJI photo and stops when the Inspection pane closes.

To read DJI photos on the desktop, put next to the GeoGIS program:

- `geoflux-dirp` (`geoflux-dirp.exe` on Windows), built with `cargo build --release -p geoflux-dirp`;
- a `dji-sdk` folder with DJI's library set for that system, from DJI's Thermal SDK
  (`utility/bin/windows/release_x64` or `utility/bin/linux/release_x64`): `libdirp` and its
  `libv_*` libraries.

For development, the `GEOFLUX_DIRP` and `GEOFLUX_DJI_SDK` environment variables name the helper and
the SDK folder instead. Without them, FLIR photos still work and DJI photos say the helper isn't
installed.
