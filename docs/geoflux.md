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
| `geoflux.linkPhotos` | Links a folder of photos (`folder`) to the layer's assets: one sub-folder per tower (`76_0/…`), or files named after the tower (`76_0_thermal.jpg`). Each photo goes in the image slot whose words its file name has; files that fit nowhere are listed with the reason. Paths are kept relative to the folder, inside the project. |
| `geoflux.photos` | One asset's image slots and their photos. |

**Analyze › Inspection** opens the Inspection pane: the asset's form laid out from its schema,
previous and next (Page Up and Page Down) through the towers, a flagged-only filter, and the
photos by slot (click one for the full view).

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
