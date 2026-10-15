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

Severity is rated from `rise`, or from `measured − reference` when `rise` is blank, against
`thresholds` (low, medium, high, critical). An asset matching every `flaggedWhen` entry is never
rated below low, and is rated medium when it has no usable numbers.
