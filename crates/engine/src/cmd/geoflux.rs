//! GeoFlux › Inspection: industry schemas, survey CSV import, severity rating and form checks.
//!
//! The domain rules live in `geoflux-domain`; these commands connect them to layers. A layer
//! imported with a schema remembers it (in the project's [`GeoFlux`] state), so later commands
//! know its fields, CSV mapping and severity rule.

use geoflux_domain::schema::core_fields;
use geoflux_domain::{AssetDraft, DomainSchema, FieldType as SchemaType, Properties, Severity, csv_import, form};
use rustgis_data::{Dataset, Field, FieldType, Geometry, GeometryType, Value as V};
use rustgis_geom::Coord;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{need_feature_layer, ps};
use crate::{CmdResult, CommandSpec as C, Session};

/// The schema used when an import names none: power-line inspection.
pub const DEFAULT_SCHEMA: &str = "com.geo.telecom";
/// The attribute holding each feature's severity.
pub const SEVERITY_FIELD: &str = "severity";
/// Most schemas a project carries, and most layer links it keeps.
const MAX_SCHEMAS: usize = 64;
const MAX_LINKS: usize = 4_096;
/// Most issues `geoflux.validate` lists one by one.
const MAX_LISTED: usize = 200;

pub fn commands() -> Vec<C> {
    vec![
        C::new("geoflux.schemas", "Inspection Schemas", "GeoFlux › Inspection", schemas).view(),
        C::new("geoflux.addSchema", "Add Inspection Schema", "GeoFlux › Inspection", add_schema)
            .params(r#"{"path"?: "a schema .json file", "json"?: "the schema itself, as text"}"#),
        C::new("geoflux.importCsv", "Import Survey CSV", "GeoFlux › Inspection", import_csv).params(
            r#"{"path"?: "a survey CSV", "text"?: "the CSV itself, as text", "schema"?: "a schema id (default com.geo.telecom)", "name"?: "the layer (default: the file name, or the schema's name)"}"#,
        ),
        C::new("geoflux.classify", "Rate Severity", "GeoFlux › Inspection", classify)
            .when(need_feature_layer)
            .params(r#"{"layer"?: name|id (default: the active layer)}"#),
        C::new("geoflux.validate", "Check Inspection Data", "GeoFlux › Inspection", validate)
            .view()
            .when(need_feature_layer)
            .params(r#"{"layer"?: name|id (default: the active layer)}"#),
    ]
}

/// A layer that follows a schema.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerLink {
    pub layer: u32,
    pub schema: String,
}

/// What a project keeps for GeoFlux: schemas added to it (built-in ones are not copied) and the
/// layers that follow a schema.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GeoFlux {
    #[serde(default)]
    pub schemas: Vec<DomainSchema>,
    #[serde(default)]
    pub links: Vec<LayerLink>,
}

impl GeoFlux {
    pub fn is_empty(&self) -> bool {
        self.schemas.is_empty() && self.links.is_empty()
    }

    /// Files skip the commands' checks: schemas that fail them are dropped, and lists are bounded.
    pub fn clamp(&mut self) {
        self.schemas.retain(|s| s.check().is_ok());
        self.schemas.truncate(MAX_SCHEMAS);
        self.links.truncate(MAX_LINKS);
    }

    /// A schema by id: one the project carries first, else a built-in one.
    pub fn schema(&self, id: &str) -> Result<DomainSchema, String> {
        if let Some(s) = self.schemas.iter().find(|s| s.id == id) {
            return Ok(s.clone());
        }
        match geoflux_domain::builtin_schema(id) {
            Some(r) => r.map_err(|e| format!("built-in schema {id}: {e}")),
            None => Err(format!("no inspection schema '{id}' (geoflux.schemas lists them)")),
        }
    }

    pub fn schema_of(&self, layer: u32) -> Option<&str> {
        self.links.iter().find(|l| l.layer == layer).map(|l| l.schema.as_str())
    }

    fn link(&mut self, layer: u32, schema: &str) {
        self.links.retain(|l| l.layer != layer);
        if self.links.len() < MAX_LINKS {
            self.links.push(LayerLink { layer, schema: schema.to_string() });
        }
    }
}

fn schemas(s: &mut Session, _: &Value) -> CmdResult {
    let describe = |sc: &DomainSchema, origin: &str| json!({"id": sc.id, "name": sc.name, "version": sc.version, "fields": sc.fields.len(), "origin": origin});
    let mut list: Vec<Value> = s.project.geoflux.schemas.iter().map(|sc| describe(sc, "project")).collect();
    for (id, _) in geoflux_domain::BUILTIN_SCHEMAS {
        if s.project.geoflux.schemas.iter().any(|sc| sc.id == *id) {
            continue; // the project's own copy replaces the built-in one
        }
        if let Some(Ok(sc)) = geoflux_domain::builtin_schema(id) {
            list.push(describe(&sc, "built-in"));
        }
    }
    let links: Vec<Value> = s
        .project
        .geoflux
        .links
        .iter()
        .filter_map(|l| s.map().layer(l.layer).map(|layer| json!({"layer": layer.name, "id": l.layer, "schema": l.schema})))
        .collect();
    Ok(json!({"schemas": list, "layers": links}))
}

fn add_schema(s: &mut Session, p: &Value) -> CmdResult {
    let text = match (ps(p, "json"), ps(p, "path")) {
        (Some(t), _) => t.to_string(),
        (None, Some(path)) => {
            String::from_utf8(read_file(path, geoflux_domain::schema::MAX_SCHEMA_BYTES)?).map_err(|_| format!("{path}: not UTF-8 text"))?
        }
        (None, None) => return Err("give \"path\" (a schema .json file) or \"json\"".into()),
    };
    let schema = DomainSchema::from_json(&text).map_err(|e| e.to_string())?;
    let id = schema.id.clone();
    let list = &mut s.project.geoflux.schemas;
    if let Some(slot) = list.iter_mut().find(|x| x.id == id) {
        *slot = schema;
    } else if list.len() >= MAX_SCHEMAS {
        return Err(format!("a project carries at most {MAX_SCHEMAS} schemas"));
    } else {
        list.push(schema);
    }
    Ok(json!({"schema": id}))
}

/// A file's bytes, refusing files larger than `limit` before reading them.
fn read_file(path: &str, limit: usize) -> Result<Vec<u8>, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = limit;
        Err(format!("{path}: the web app has no file paths; pass the file's text instead"))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let meta = std::fs::metadata(path).map_err(|e| format!("{path}: {e}"))?;
        if !meta.is_file() {
            return Err(format!("{path}: not a file"));
        }
        if meta.len() > limit as u64 {
            return Err(format!("{path}: larger than {} MB", limit >> 20));
        }
        std::fs::read(path).map_err(|e| format!("{path}: {e}"))
    }
}

fn import_csv(s: &mut Session, p: &Value) -> CmdResult {
    let schema_id = ps(p, "schema").map(str::trim).filter(|x| !x.is_empty()).unwrap_or(DEFAULT_SCHEMA);
    let schema = s.project.geoflux.schema(schema_id)?;
    let profile = schema.csv.as_ref().ok_or_else(|| format!("schema '{schema_id}' has no CSV mapping"))?;

    let (bytes, default_name) = match (ps(p, "text"), ps(p, "path")) {
        (Some(t), _) => (t.as_bytes().to_vec(), schema.name.clone()),
        (None, Some(path)) => {
            let stem = std::path::Path::new(path).file_stem().and_then(|n| n.to_str()).unwrap_or(&schema.name).to_string();
            (read_file(path, csv_import::MAX_BYTES)?, stem)
        }
        (None, None) => return Err("give \"path\" (a survey CSV) or \"text\"".into()),
    };
    let report = csv_import::read(&bytes, profile, &mut |_| {}).map_err(|e| e.to_string())?;
    drop(bytes);
    if report.drafts.is_empty() {
        let why = report.skipped_details.first().map(|d| format!(" ({d})")).unwrap_or_default();
        return Err(format!("no row could become an asset{why}"));
    }

    let name = ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).map_or(default_name, str::to_string);
    let mut ds = dataset(&schema, &name, &report.drafts);
    let rated = rate(&schema, &mut ds);
    let id = super::layer::add_dataset(s, ds);
    s.project.geoflux.link(id, &schema.id);
    if rated {
        // Unique values on severity, so findings stand out at once.
        let _ = super::layer::symbology(s, &json!({"layer": id, "type": "unique", "field": SEVERITY_FIELD}));
    }
    if let Some(e) = s.map().layer(id).map(|l| l.extent()).filter(|e| !e.is_empty()) {
        s.zoom_to(&e);
    }
    if report.skipped_rows > 0 {
        s.notify(format!("{} rows were skipped: {}", report.skipped_rows, report.skipped_details.join("; ")), false);
    }
    if report.rows_without_position > 0 {
        s.notify(format!("{} assets have no readable position yet", report.rows_without_position), false);
    }
    Ok(json!({
        "layer": id,
        "schema": schema.id,
        "assets": report.drafts.len(),
        "skipped": report.skipped_rows,
        "skippedDetails": report.skipped_details,
        "withoutPosition": report.rows_without_position,
        "truncated": report.truncated,
    }))
}

/// The layer's table: the schema's fields (labels as aliases), other mapped CSV columns, extra
/// columns in the order first seen, then the severity. A number or integer field becomes a
/// numeric column only when every value reads as one, so nothing typed into a sheet is lost.
fn dataset(schema: &DomainSchema, name: &str, drafts: &[AssetDraft]) -> Dataset {
    let mut keys: Vec<(String, Option<String>, SchemaType)> = Vec::new();
    let mut push = |key: &str, alias: Option<&str>, ty: SchemaType| {
        if !keys.iter().any(|(k, _, _)| k == key) {
            keys.push((key.to_string(), alias.map(str::to_string), ty));
        }
    };
    for f in schema.fields.iter().filter(|f| !core_fields::is_coordinate(&f.key)) {
        push(&f.key, Some(&f.label), f.field_type);
    }
    if let Some(csv) = &schema.csv {
        for c in &csv.columns {
            push(&c.key, Some(&c.header), SchemaType::Text);
        }
    }
    for d in drafts {
        for k in d.properties.keys() {
            push(k, None, SchemaType::Text);
        }
    }

    let column_type = |key: &str, ty: SchemaType| -> FieldType {
        let values = || drafts.iter().map(|d| d.properties.get(key)).filter(|v| !v.trim().is_empty());
        match ty {
            SchemaType::Integer if values().all(|v| v.trim().parse::<i64>().is_ok()) => FieldType::Integer,
            SchemaType::Number if values().all(|v| number(v).is_some()) => FieldType::Double,
            _ => FieldType::Text,
        }
    };
    let mut fields: Vec<Field> =
        keys.iter().map(|(k, alias, ty)| Field { name: k.clone(), ty: column_type(k, *ty), alias: alias.clone() }).collect();
    fields.push(Field { name: SEVERITY_FIELD.into(), ty: FieldType::Text, alias: Some("Severity".into()) });

    let mut ds = Dataset::new(name, GeometryType::Point, fields);
    for d in drafts {
        let mut attrs: Vec<V> = ds.fields.iter().take(keys.len()).map(|f| cell(d.properties.get(&f.name), f.ty)).collect();
        attrs.push(V::Text(Severity::None.as_str().into()));
        let geometry = d.location.map(|g| Geometry::Point(Coord::new(g.lon, g.lat)));
        ds.push_next(geometry, attrs);
    }
    ds
}

fn number(text: &str) -> Option<f64> {
    text.trim().replace(',', ".").parse::<f64>().ok().filter(|v| v.is_finite())
}

fn cell(text: &str, ty: FieldType) -> V {
    let t = text.trim();
    if t.is_empty() {
        return V::Null;
    }
    match ty {
        FieldType::Integer => t.parse().map(V::Int).unwrap_or(V::Null),
        FieldType::Double => number(t).map(V::Float).unwrap_or(V::Null),
        _ => V::Text(text.to_string()),
    }
}

/// A feature's attributes as properties (blank for nulls), for the domain rules.
fn properties(ds: &Dataset, row: usize) -> Properties {
    ds.fields
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let v = match ds.value(row, i) {
                V::Null => String::new(),
                v => v.to_string(),
            };
            (f.name.clone(), v)
        })
        .collect()
}

/// Writes every feature's severity from the schema's rule. False when the schema has no rule.
fn rate(schema: &DomainSchema, ds: &mut Dataset) -> bool {
    let Some(rule) = &schema.severity else { return false };
    let col = ds.ensure_field(SEVERITY_FIELD, FieldType::Text);
    let levels: Vec<Severity> = (0..ds.features.len()).map(|row| rule.classify(&properties(ds, row))).collect();
    for (feature, level) in ds.features.iter_mut().zip(levels) {
        if let Some(slot) = feature.attrs.get_mut(col) {
            *slot = V::Text(level.as_str().into());
        }
    }
    true
}

/// The schema of the layer `p` names (default: the active layer).
fn layer_schema(s: &Session, p: &Value) -> Result<(u32, DomainSchema), String> {
    let id = s.layer_id(p)?;
    let schema_id = s.project.geoflux.schema_of(id).ok_or("this layer has no inspection schema (import it with geoflux.importCsv)")?;
    Ok((id, s.project.geoflux.schema(schema_id)?))
}

fn classify(s: &mut Session, p: &Value) -> CmdResult {
    let (id, schema) = layer_schema(s, p)?;
    if schema.severity.is_none() {
        return Err(format!("schema '{}' has no severity rule", schema.id));
    }
    let layer = s.map_mut().layer_mut(id).ok_or("no such layer")?;
    if layer.dataset.windowed {
        return Err("this layer is read in place; load all its features first".into());
    }
    rate(&schema, &mut layer.dataset);
    let col = layer.dataset.field_index(SEVERITY_FIELD);
    let mut counts = serde_json::Map::new();
    for level in Severity::ALL {
        let n = col.map_or(0, |c| {
            (0..layer.dataset.features.len()).filter(|&r| matches!(layer.dataset.value(r, c), V::Text(t) if t == level.as_str())).count()
        });
        counts.insert(level.as_str().into(), json!(n));
    }
    Ok(json!({"layer": id, "counts": counts}))
}

fn validate(s: &mut Session, p: &Value) -> CmdResult {
    let (id, schema) = layer_schema(s, p)?;
    let layer = s.map().layer(id).ok_or("no such layer")?;
    let ds = &layer.dataset;
    let mut issues = Vec::new();
    let mut total = 0usize;
    let mut rows_with_issues = 0usize;
    for row in 0..ds.features.len() {
        let mut asset =
            geoflux_domain::Asset::from_draft("", &schema.id, row as u32, AssetDraft { location: None, properties: properties(ds, row) });
        if let Some(Geometry::Point(c)) = ds.features.get(row).and_then(|f| f.geometry.as_ref()) {
            asset.location = Some(geoflux_domain::GeoPoint { lat: c.y, lon: c.x });
        }
        let fields = schema.fields_for(&asset);
        let found = form::validate(&fields, &form::values(&asset, &fields));
        if !found.is_empty() {
            rows_with_issues += 1;
        }
        total += found.len();
        let fid = ds.features.get(row).map_or(0, |f| f.id);
        for issue in found {
            if issues.len() < MAX_LISTED {
                issues.push(json!({"feature": fid, "field": issue.key, "issue": issue.kind}));
            }
        }
    }
    Ok(json!({"layer": id, "features": ds.features.len(), "featuresWithIssues": rows_with_issues, "issues": total, "listed": issues}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Project;

    const SHEET: &str = "Tower No.,Line Name,Lat.,Long.,Ambeint Temp.,Fault Temp.,Rise Temp.,report_Type,Fault,Load A\n\
        76_0,Bhadla-Sikar,27°30'N,75°10'E,30,75,,tower_fault,Fault,120\n\
        77_0,Bhadla-Sikar,27.51,75.17,30,,2,,,130\n\
        78_0,Bhadla-Sikar,,,30,,,,,\n\
        ,Bhadla-Sikar,27.52,75.18,,,,,,\n";

    fn session() -> Session {
        Session::new(Project::default())
    }

    #[test]
    fn import_builds_a_rated_point_layer_and_links_the_schema() {
        let mut s = session();
        let r = s.run("geoflux.importCsv", &json!({"text": SHEET, "name": "Line 1"})).expect("import");
        assert_eq!(r["assets"], 3);
        assert_eq!(r["skipped"], 1);
        assert_eq!(r["withoutPosition"], 1);
        let id = r["layer"].as_u64().expect("layer id") as u32;
        let ds = &s.map().layer(id).expect("layer").dataset;
        assert_eq!(ds.features.len(), 3);
        let sev = ds.field_index(SEVERITY_FIELD).expect("severity field");
        let levels: Vec<String> = (0..3).map(|r| ds.value(r, sev).to_string()).collect();
        assert_eq!(levels, vec!["critical", "low", "none"]);
        let tower = ds.field_index("towerNumber").expect("tower field");
        assert_eq!(ds.fields[tower].display(), "Tower No");
        assert!(ds.field_index("load.Load A").is_some());
        // No position yet: no geometry, but the asset is kept.
        assert!(ds.features[2].geometry.is_none());
        assert_eq!(s.project.geoflux.schema_of(id), Some(DEFAULT_SCHEMA));
        // Undo removes the layer and the link with it (the project is one snapshot).
        assert!(s.can_undo());
    }

    #[test]
    fn classify_follows_edits_and_validate_lists_issues() {
        let mut s = session();
        let r = s.run("geoflux.importCsv", &json!({"text": SHEET})).expect("import");
        let id = r["layer"].as_u64().expect("id") as u32;
        {
            let ds = &mut s.map_mut().layer_mut(id).expect("layer").dataset;
            let rise = ds.field_index("riseTemp").expect("rise");
            ds.features[2].attrs[rise] = V::Text("20".into());
        }
        let c = s.run("geoflux.classify", &json!({"layer": id})).expect("classify");
        assert_eq!(c["counts"]["high"], 1);
        let v = s.run("geoflux.validate", &json!({"layer": id})).expect("validate");
        // The third tower has no position: latitude and longitude are required.
        assert_eq!(v["featuresWithIssues"], 1);
        assert_eq!(v["issues"], 2);
    }

    #[test]
    fn project_schemas_round_trip_and_replace_built_ins() {
        let mut s = session();
        let mut custom = geoflux_domain::builtin_schema(DEFAULT_SCHEMA).expect("listed").expect("valid");
        custom.name = "Custom grid".into();
        s.run("geoflux.addSchema", &json!({"json": custom.to_json()})).expect("add");
        let list = s.run("geoflux.schemas", &Value::Null).expect("list");
        let names: Vec<&str> = list["schemas"].as_array().expect("array").iter().filter_map(|x| x["name"].as_str()).collect();
        assert_eq!(names, vec!["Custom grid"]);
        let saved = serde_json::to_string(&s.project).expect("save");
        let back: Project = serde_json::from_str(&saved).expect("load");
        assert_eq!(back.geoflux.schemas.len(), 1);
        assert!(s.run("geoflux.addSchema", &json!({"json": "{}"})).is_err());
        assert!(s.run("geoflux.importCsv", &json!({"text": SHEET, "schema": "nope"})).is_err());
    }

    #[test]
    fn layers_without_a_schema_are_refused() {
        let mut s = session();
        s.run("layer.addSample", &json!({"name": "cities"})).expect("sample");
        assert!(s.run("geoflux.classify", &Value::Null).is_err());
        assert!(s.run("geoflux.validate", &Value::Null).is_err());
    }
}
