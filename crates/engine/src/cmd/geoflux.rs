//! GeoFlux › Inspection: industry schemas, survey CSV import, severity rating, form checks and
//! edits, the order towers are walked in, and linked inspection photos.
//!
//! The domain rules live in `geoflux-domain`; these commands connect them to layers. A layer
//! imported with a schema remembers it (in the project's [`GeoFlux`] state), so later commands
//! know its fields, CSV mapping and severity rule.

use std::collections::BTreeMap;

use geoflux_domain::annotation::AnnotationSet;
use geoflux_domain::images::{self, AssetPhotos};
use geoflux_domain::schema::core_fields;
use geoflux_domain::{
    Asset, AssetDraft, DomainSchema, FieldIssue, FieldType as SchemaType, GeoPoint, ImageSlotDef, Properties, Severity, csv_import, form,
    sequence,
};
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
/// Most photo links a project keeps (one per layer), and most assets with photos in one.
const MAX_PHOTO_LINKS: usize = 256;
const MAX_PHOTO_ASSETS: usize = 100_000;
/// Most photos picked one by one in a project, and in one slot of one asset.
const MAX_PICKED: usize = 20_000;
const MAX_PICKED_PER_SLOT: usize = 100;
/// Most photos with markup in one project.
pub const MAX_MARKUP: usize = 20_000;

/// The longest picked photo path kept.
const MAX_PICKED_PATH: usize = 4_096;
/// How deep below the linked folder photos are looked for (line / tower / photo).
#[cfg(not(target_arch = "wasm32"))]
const PHOTO_DEPTH: usize = 3;
/// Most folder entries looked at in one link, files or not.
#[cfg(not(target_arch = "wasm32"))]
const MAX_ENTRIES: usize = 500_000;

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
        C::new("geoflux.setValues", "Edit Inspection Form", "GeoFlux › Inspection", set_values)
            .when(need_feature_layer)
            .params(r#"{"layer"?, "row": number, "values": {field key: text}}"#),
        C::new("geoflux.sequence", "Tower Sequence", "GeoFlux › Inspection", sequence_cmd)
            .view()
            .when(need_feature_layer)
            .params(r#"{"layer"?, "flaggedOnly"?: bool}"#),
        C::new("geoflux.goTo", "Go To Tower", "GeoFlux › Inspection", go_to)
            .view()
            .when(need_feature_layer)
            .params(r#"{"layer"?, "row": number}"#),
        C::new("geoflux.linkPhotos", "Link Inspection Photos", "GeoFlux › Inspection", link_photos)
            .when(need_feature_layer)
            .params(r#"{"layer"?, "folder": "a folder of photos: one sub-folder per tower, or files named after the tower"}"#),
        C::new("geoflux.photos", "Tower Photos", "GeoFlux › Inspection", photos_cmd)
            .view()
            .when(need_feature_layer)
            .params(r#"{"layer"?, "row": number}"#),
        C::new("geoflux.addPhotos", "Add Tower Photos", "GeoFlux › Inspection", add_photos)
            .when(need_feature_layer)
            .params(r#"{"layer"?, "row": number, "slot": "an image slot id", "paths": ["photo files"]}"#),
        C::new("geoflux.removePhoto", "Remove Tower Photo", "GeoFlux › Inspection", remove_photo)
            .when(need_feature_layer)
            .params(r#"{"layer"?, "row": number, "slot": "an image slot id", "path": "a photo added with geoflux.addPhotos"}"#),
    ]
}

/// A layer that follows a schema.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerLink {
    pub layer: u32,
    pub schema: String,
}

/// A layer's linked photos: the folder as linked, and each asset's photos relative to it, so a
/// project moved with its photo folder only needs the folder linked again.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PhotoLink {
    pub layer: u32,
    pub folder: String,
    pub assets: Vec<AssetPhotos>,
}

/// A photo picked for one slot of one asset, outside any folder link. Kept by the asset's label,
/// like linked photos, and as the full path picked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PickedPhoto {
    pub layer: u32,
    pub label: String,
    pub slot: String,
    pub path: String,
}

/// Markup on one photo (see [`geoflux_domain::annotation`]). Kept by the asset's label and the
/// photo as its slot lists it (relative to the linked folder, or the picked path), so it survives
/// a relink; markup whose photo is gone is kept until the project is saved without it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PhotoMarkup {
    pub layer: u32,
    pub label: String,
    pub slot: String,
    pub photo: String,
    pub shapes: AnnotationSet,
}

/// What a project keeps for GeoFlux: schemas added to it (built-in ones are not copied), the
/// layers that follow a schema, and their linked photos.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GeoFlux {
    #[serde(default)]
    pub schemas: Vec<DomainSchema>,
    #[serde(default)]
    pub links: Vec<LayerLink>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub photos: Vec<PhotoLink>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub picked: Vec<PickedPhoto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markup: Vec<PhotoMarkup>,
    /// The survey this project syncs to GeoFlux Cloud as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud: Option<super::cloud_sync::CloudLink>,
}

impl GeoFlux {
    pub fn is_empty(&self) -> bool {
        self.schemas.is_empty()
            && self.links.is_empty()
            && self.photos.is_empty()
            && self.picked.is_empty()
            && self.markup.is_empty()
            && self.cloud.is_none()
    }

    /// Files skip the commands' checks: schemas that fail them are dropped, photo paths that
    /// would leave their folder are dropped, and lists are bounded.
    pub fn clamp(&mut self) {
        self.schemas.retain(|s| s.check().is_ok());
        self.schemas.truncate(MAX_SCHEMAS);
        self.links.truncate(MAX_LINKS);
        self.photos.truncate(MAX_PHOTO_LINKS);
        for link in &mut self.photos {
            link.assets.truncate(MAX_PHOTO_ASSETS);
            for a in &mut link.assets {
                a.slots.truncate(geoflux_domain::schema::MAX_SLOTS);
                for slot in &mut a.slots {
                    slot.photos.retain(|p| images::is_safe_path(p));
                    slot.photos.truncate(images::MAX_FILES);
                }
            }
        }
        self.picked.retain(|p| !p.path.trim().is_empty() && p.path.len() <= MAX_PICKED_PATH && images::is_image(&p.path));
        self.picked.truncate(MAX_PICKED);
        // Shapes are checked again, as a command would: one bad entry drops only its own photo's markup.
        self.markup.retain_mut(|m| match AnnotationSet::from_shapes(std::mem::take(&mut m.shapes).into_shapes()) {
            Ok(set) => {
                m.shapes = set;
                !m.shapes.is_empty() && m.photo.len() <= MAX_PICKED_PATH
            }
            Err(_) => false,
        });
        self.markup.truncate(MAX_MARKUP);
    }

    /// The photos linked to a layer.
    pub fn photos_of(&self, layer: u32) -> Option<&PhotoLink> {
        self.photos.iter().find(|l| l.layer == layer)
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
pub(crate) fn read_file(path: &str, limit: usize) -> Result<Vec<u8>, String> {
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
    let id = add_inspection_layer(s, &schema, &name, &report.drafts);
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

/// Adds `drafts` as a point layer that follows `schema`, rated by severity and zoomed to.
pub(crate) fn add_inspection_layer(s: &mut Session, schema: &DomainSchema, name: &str, drafts: &[AssetDraft]) -> u32 {
    let mut ds = dataset(schema, name, drafts);
    let rated = rate(schema, &mut ds);
    let id = super::layer::add_dataset(s, ds);
    s.project.geoflux.link(id, &schema.id);
    if rated {
        // Unique values on severity, so findings stand out at once.
        let _ = super::layer::symbology(s, &json!({"layer": id, "type": "unique", "field": SEVERITY_FIELD}));
    }
    if let Some(e) = s.map().layer(id).map(|l| l.extent()).filter(|e| !e.is_empty()) {
        s.zoom_to(&e);
    }
    id
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
        let asset = asset_of(ds, &schema, row);
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

/// Row `row` of a layer's table as an asset of `schema`.
pub(crate) fn asset_of(ds: &Dataset, schema: &DomainSchema, row: usize) -> Asset {
    let mut asset = Asset::from_draft("", &schema.id, row as u32, AssetDraft { location: None, properties: properties(ds, row) });
    if let Some(Geometry::Point(c)) = ds.features.get(row).and_then(|f| f.geometry.as_ref()) {
        asset.location = Some(GeoPoint { lat: c.y, lon: c.x });
    }
    asset
}

/// The asset at `row` of a layer and its schema, for the inspection form.
pub fn asset(s: &Session, layer: u32, row: usize) -> Result<(DomainSchema, Asset), String> {
    let schema = s.project.geoflux.schema(s.project.geoflux.schema_of(layer).ok_or("this layer has no inspection schema")?)?;
    let ds = &s.map().layer(layer).ok_or("no such layer")?.dataset;
    if row >= ds.features.len() {
        return Err("no such row".into());
    }
    let asset = asset_of(ds, &schema, row);
    Ok((schema, asset))
}

/// One stop on the walk through a layer's assets.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Stop {
    pub row: usize,
    pub label: String,
    pub severity: Severity,
}

/// A layer's assets in natural order of label; only those rated above none when `flagged_only`.
pub fn sequence(s: &Session, layer: u32, flagged_only: bool) -> Result<Vec<Stop>, String> {
    let schema = s.project.geoflux.schema(s.project.geoflux.schema_of(layer).ok_or("this layer has no inspection schema")?)?;
    let ds = &s.map().layer(layer).ok_or("no such layer")?.dataset;
    let label_col = ds.field_index(schema.label_key());
    let sev_col = ds.field_index(SEVERITY_FIELD);
    let text = |row: usize, col: Option<usize>| match col.map(|c| ds.value(row, c)) {
        Some(V::Null) | None => String::new(),
        Some(v) => v.to_string(),
    };
    let stops: Vec<Stop> = (0..ds.features.len())
        .map(|row| Stop { row, label: text(row, label_col), severity: Severity::from_name(&text(row, sev_col)) })
        .filter(|st| !flagged_only || st.severity > Severity::None)
        .collect();
    let order = sequence::order(stops.iter().enumerate().map(|(i, st)| (i, st.label.as_str())));
    let mut slots: Vec<Option<Stop>> = stops.into_iter().map(Some).collect();
    Ok(order.into_iter().filter_map(|i| slots.get_mut(i).and_then(Option::take)).collect())
}

fn row_param(p: &Value) -> Result<usize, String> {
    p.get("row").and_then(Value::as_u64).map(|r| r as usize).ok_or_else(|| "row is required".to_string())
}

fn sequence_cmd(s: &mut Session, p: &Value) -> CmdResult {
    let id = s.layer_id(p)?;
    let flagged = p.get("flaggedOnly").and_then(Value::as_bool).unwrap_or(false);
    let stops = sequence(s, id, flagged)?;
    Ok(json!({"layer": id, "stops": stops}))
}

/// Selects the asset and centres the map on it, keeping the scale.
fn go_to(s: &mut Session, p: &Value) -> CmdResult {
    let id = s.layer_id(p)?;
    let row = row_param(p)?;
    let ds = &s.map().layer(id).ok_or("no such layer")?.dataset;
    let at = match ds.features.get(row).ok_or("no such row")?.geometry.as_ref() {
        Some(Geometry::Point(c)) => Some(*c),
        _ => None,
    };
    super::select::set(s, id, vec![row], super::select::Mode::New);
    if let Some(c) = at {
        super::navigate::go_to_xy(s, &json!({"lon": c.x, "lat": c.y}))?;
    }
    Ok(json!({"layer": id, "row": row, "located": at.is_some()}))
}

/// Applies an inspection form's values to one asset, as one undoable edit: every field is
/// checked first and nothing changes if any is invalid. The asset is rated again afterwards.
pub(crate) fn set_values(s: &mut Session, p: &Value) -> CmdResult {
    let id = s.layer_id(p)?;
    let row = row_param(p)?;
    let given = p.get("values").and_then(Value::as_object).ok_or("values is required ({field key: text})")?;
    let (schema, before) = asset(s, id, row)?;
    let fields = schema.fields_for(&before);
    // The form's current values, with the given ones over them, so a partial edit is checked whole.
    let mut values = form::values(&before, &fields);
    for (k, v) in given {
        let text = match v {
            Value::String(t) => t.clone(),
            Value::Null => String::new(),
            other => other.to_string(),
        };
        values.insert(k.clone(), text);
    }
    let issues = form::validate(&fields, &values);
    if !issues.is_empty() {
        return Err(describe_issues(&schema, &issues));
    }
    let after = form::apply(&before, &values);
    let layer = s.map_mut().layer_mut(id).ok_or("no such layer")?;
    if layer.dataset.windowed {
        return Err("this layer is read in place; load all its features first".into());
    }
    write_asset(&schema, &mut layer.dataset, row, &after, &values)?;
    let level = match &schema.severity {
        Some(rule) => {
            let level = rule.classify(&properties(&layer.dataset, row));
            let col = layer.dataset.ensure_field(SEVERITY_FIELD, FieldType::Text);
            if let Some(cell) = layer.dataset.features.get_mut(row).and_then(|f| f.attrs.get_mut(col)) {
                *cell = V::Text(level.as_str().into());
            }
            Some(level)
        }
        None => None,
    };
    Ok(json!({"layer": id, "row": row, "severity": level}))
}

/// The issues as one sentence, by field label.
fn describe_issues(schema: &DomainSchema, issues: &[FieldIssue]) -> String {
    let parts: Vec<String> = issues
        .iter()
        .take(10)
        .map(|i| {
            let label = schema.field(&i.key).map_or(i.key.as_str(), |f| f.label.as_str());
            let what = match i.kind {
                geoflux_domain::IssueKind::Missing => "is required",
                geoflux_domain::IssueKind::NotANumber => "is not a number",
                geoflux_domain::IssueKind::NotAnInteger => "is not a whole number",
                geoflux_domain::IssueKind::OutOfRange => "is out of range",
                geoflux_domain::IssueKind::NotADate => "is not a date (YYYY-MM-DD)",
                geoflux_domain::IssueKind::NotAnOption => "is not one of its options",
                geoflux_domain::IssueKind::NotABoolean => "is not yes or no",
                geoflux_domain::IssueKind::NotACoordinate => "is not a coordinate",
            };
            format!("{label} {what}")
        })
        .collect();
    let more = issues.len().saturating_sub(parts.len());
    if more > 0 { format!("{} (and {more} more)", parts.join("; ")) } else { parts.join("; ") }
}

/// Writes an edited asset back to its row: each form value to its column (added as text when
/// the layer has none yet), and the position to the geometry. A value a numeric column can't
/// hold is refused rather than dropped.
fn write_asset(
    schema: &DomainSchema,
    ds: &mut Dataset,
    row: usize,
    after: &Asset,
    values: &BTreeMap<String, String>,
) -> Result<(), String> {
    let mut cells: Vec<(usize, V)> = Vec::new();
    for (key, text) in values.iter().filter(|(k, _)| !core_fields::is_coordinate(k)) {
        let col = match ds.field_index(key) {
            Some(c) => c,
            None if text.trim().is_empty() => continue,
            None => {
                let alias = schema.field(key).map(|f| f.label.clone());
                let c = ds.ensure_field(key, FieldType::Text);
                if let Some(f) = ds.fields.get_mut(c) {
                    f.alias = alias;
                }
                c
            }
        };
        let ty = ds.fields.get(col).map_or(FieldType::Text, |f| f.ty);
        let v = cell(text, ty);
        if v == V::Null && !text.trim().is_empty() {
            let label = schema.field(key).map_or(key.as_str(), |f| f.label.as_str());
            return Err(format!("{label}: '{}' doesn't fit this column (it holds numbers)", text.trim()));
        }
        cells.push((col, v));
    }
    let f = ds.features.get_mut(row).ok_or("no such row")?;
    for (col, v) in cells {
        if let Some(c) = f.attrs.get_mut(col) {
            *c = v;
        }
    }
    f.geometry = after.location.map(|g| Geometry::Point(Coord::new(g.lon, g.lat)));
    Ok(())
}

/// One image slot of an asset and its photos as full paths (the first is the one shown).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SlotPhotos {
    pub slot: ImageSlotDef,
    pub photos: Vec<String>,
}

/// Every image slot of the asset at `row`, with the photos linked to it.
pub fn photos(s: &Session, layer: u32, row: usize) -> Result<Vec<SlotPhotos>, String> {
    let (schema, asset) = asset(s, layer, row)?;
    let label = asset.property(schema.label_key()).trim().to_lowercase();
    let link = s.project.geoflux.photos_of(layer);
    // Compared lowercase without allocating per asset.
    let found = link.and_then(|l| l.assets.iter().find(|a| a.label.trim().chars().flat_map(char::to_lowercase).eq(label.chars())));
    let folder = std::path::Path::new(link.map_or("", |l| l.folder.as_str()));
    let picked: Vec<&PickedPhoto> = s
        .project
        .geoflux
        .picked
        .iter()
        .filter(|p| p.layer == layer && p.label.trim().chars().flat_map(char::to_lowercase).eq(label.chars()))
        .collect();
    Ok(slot_photos(&schema, folder, found, &picked))
}

/// The slots of an asset with its folder photos (full paths), then its picked ones, once each.
fn slot_photos(schema: &DomainSchema, folder: &std::path::Path, found: Option<&AssetPhotos>, picked: &[&PickedPhoto]) -> Vec<SlotPhotos> {
    schema
        .image_slots
        .iter()
        .map(|slot| {
            let mut photos: Vec<String> = found
                .map(|a| a.slot(&slot.id))
                .unwrap_or_default()
                .iter()
                .map(|rel| rel.split('/').fold(folder.to_path_buf(), |p, part| p.join(part)).to_string_lossy().into_owned())
                .collect();
            // Picked photos come after the folder's, once each.
            for p in picked.iter().filter(|p| p.slot == slot.id) {
                if !photos.contains(&p.path) {
                    photos.push(p.path.clone());
                }
            }
            SlotPhotos { slot: slot.clone(), photos }
        })
        .collect()
}

/// A layer's photos by lowercase asset label, for reading every asset's photos in one pass
/// (a report of a whole line) without searching the links once per asset.
pub struct PhotoIndex<'a> {
    folder: &'a std::path::Path,
    assets: std::collections::HashMap<String, &'a AssetPhotos>,
    picked: std::collections::HashMap<String, Vec<&'a PickedPhoto>>,
}

impl<'a> PhotoIndex<'a> {
    pub fn new(g: &'a GeoFlux, layer: u32) -> Self {
        let link = g.photos_of(layer);
        let mut assets = std::collections::HashMap::new();
        for a in link.map_or(&[][..], |l| l.assets.as_slice()) {
            assets.entry(a.label.trim().to_lowercase()).or_insert(a);
        }
        let mut picked: std::collections::HashMap<String, Vec<&PickedPhoto>> = std::collections::HashMap::new();
        for p in g.picked.iter().filter(|p| p.layer == layer) {
            picked.entry(p.label.trim().to_lowercase()).or_default().push(p);
        }
        Self { folder: std::path::Path::new(link.map_or("", |l| l.folder.as_str())), assets, picked }
    }

    /// The slots of the asset labelled `label`, as [`photos`] gives them.
    pub fn slots(&self, schema: &DomainSchema, label: &str) -> Vec<SlotPhotos> {
        let key = label.trim().to_lowercase();
        let picked = self.picked.get(&key).map_or(&[][..], Vec::as_slice);
        slot_photos(schema, self.folder, self.assets.get(&key).copied(), picked)
    }
}

/// The asset's label and the slot named by `slot`, for the photo-picking commands.
fn picked_target(s: &Session, layer: u32, row: usize, p: &Value) -> Result<(String, String), String> {
    let (schema, asset) = asset(s, layer, row)?;
    let slot = ps(p, "slot").ok_or("slot is required")?;
    let slot =
        schema.image_slots.iter().find(|d| d.id == slot).ok_or_else(|| format!("schema '{}' has no image slot '{slot}'", schema.id))?;
    let label = asset.property(schema.label_key()).trim().to_string();
    if label.is_empty() {
        return Err(format!("this asset has no {} to keep its photos by", schema.label_key()));
    }
    Ok((label, slot.id.clone()))
}

/// Adds photo files to one slot of one asset, wherever they are.
fn add_photos(s: &mut Session, p: &Value) -> CmdResult {
    let id = s.layer_id(p)?;
    let row = row_param(p)?;
    let (label, slot) = picked_target(s, id, row, p)?;
    let paths: Vec<String> = p
        .get("paths")
        .and_then(Value::as_array)
        .ok_or("paths is required (a list of photo files)")?
        .iter()
        .map(|v| v.as_str().map(|s| s.trim().to_string()).ok_or("paths must be text"))
        .collect::<Result<_, _>>()?;
    if paths.is_empty() || paths.len() > MAX_PICKED_PER_SLOT {
        return Err(format!("paths lists 1 to {MAX_PICKED_PER_SLOT} photos"));
    }
    for path in &paths {
        if path.is_empty() || path.len() > MAX_PICKED_PATH || !images::is_image(path) {
            return Err(format!("{path}: not a photo (.jpg, .jpeg or .png)"));
        }
        #[cfg(not(target_arch = "wasm32"))]
        if !std::path::Path::new(path).is_file() {
            return Err(format!("{path}: no such file"));
        }
    }
    let shown = photos(s, id, row)?;
    let have = shown.iter().find(|sp| sp.slot.id == slot).map(|sp| sp.photos.clone()).unwrap_or_default();
    let g = &mut s.project.geoflux;
    let in_slot = g.picked.iter().filter(|q| q.layer == id && q.slot == slot && q.label.eq_ignore_ascii_case(&label)).count();
    let mut new: Vec<&String> = Vec::new();
    for p in &paths {
        if !have.contains(p) && !new.contains(&p) {
            new.push(p);
        }
    }
    if in_slot + new.len() > MAX_PICKED_PER_SLOT || g.picked.len() + new.len() > MAX_PICKED {
        return Err(format!("a slot holds at most {MAX_PICKED_PER_SLOT} added photos"));
    }
    let added = new.len();
    for path in new {
        g.picked.push(PickedPhoto { layer: id, label: label.clone(), slot: slot.clone(), path: path.clone() });
    }
    Ok(json!({"layer": id, "row": row, "slot": slot, "added": added, "slots": photos(s, id, row)?}))
}

/// Adds one photo to one slot of the asset labelled `label`, as `geoflux.addPhotos` would, for a
/// photo the caller has already checked. False when the project's limits are reached.
pub(crate) fn add_picked(g: &mut GeoFlux, layer: u32, label: &str, slot: &str, path: &str) -> bool {
    let in_slot = g.picked.iter().filter(|q| q.layer == layer && q.slot == slot && q.label.eq_ignore_ascii_case(label)).count();
    if in_slot >= MAX_PICKED_PER_SLOT || g.picked.len() >= MAX_PICKED || path.len() > MAX_PICKED_PATH {
        return false;
    }
    if !g.picked.iter().any(|q| q.layer == layer && q.slot == slot && q.path == path) {
        g.picked.push(PickedPhoto { layer, label: label.to_string(), slot: slot.to_string(), path: path.to_string() });
    }
    true
}

/// Takes a picked photo out of its slot (the file stays where it is).
fn remove_photo(s: &mut Session, p: &Value) -> CmdResult {
    let id = s.layer_id(p)?;
    let row = row_param(p)?;
    let (label, slot) = picked_target(s, id, row, p)?;
    let path = ps(p, "path").map(str::trim).ok_or("path is required")?.to_string();
    let g = &mut s.project.geoflux;
    let before = g.picked.len();
    g.picked.retain(|q| !(q.layer == id && q.slot == slot && q.path == path && q.label.eq_ignore_ascii_case(&label)));
    if g.picked.len() == before {
        return Err(format!("{path} was not added to this slot (photos from the linked folder change with Relink)"));
    }
    Ok(json!({"layer": id, "row": row, "slot": slot, "slots": photos(s, id, row)?}))
}

fn photos_cmd(s: &mut Session, p: &Value) -> CmdResult {
    let id = s.layer_id(p)?;
    let slots = photos(s, id, row_param(p)?)?;
    Ok(json!({"layer": id, "slots": slots}))
}

/// Links a folder of photos to a layer's assets, replacing any earlier link of the layer.
pub(crate) fn link_photos(s: &mut Session, p: &Value) -> CmdResult {
    let (id, schema) = layer_schema(s, p)?;
    if schema.image_slots.is_empty() {
        return Err(format!("schema '{}' has no image slots", schema.id));
    }
    let folder = ps(p, "folder").map(str::trim).filter(|f| !f.is_empty()).ok_or("folder is required")?.to_string();
    let files = list_photos(&folder)?;
    let ds = &s.map().layer(id).ok_or("no such layer")?.dataset;
    let col =
        ds.field_index(schema.label_key()).ok_or_else(|| format!("the layer has no '{}' field to match photos by", schema.label_key()))?;
    let labels: Vec<String> = (0..ds.features.len())
        .map(|r| match ds.value(r, col) {
            V::Null => String::new(),
            v => v.to_string(),
        })
        .collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    // The folder's own name counts too, so a single tower's folder links to that tower.
    let root = std::path::Path::new(&folder).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let report = images::link(&schema.image_slots, &refs, &root, &files);
    drop(files);
    let towers = report.assets.len();
    let mut assets = report.assets;
    assets.truncate(MAX_PHOTO_ASSETS);
    let g = &mut s.project.geoflux;
    g.photos.retain(|l| l.layer != id);
    if g.photos.len() >= MAX_PHOTO_LINKS {
        return Err(format!("a project links photos to at most {MAX_PHOTO_LINKS} layers"));
    }
    g.photos.push(PhotoLink { layer: id, folder, assets });
    if report.unplaced_count > 0 {
        s.notify(format!("{} files in the folder were not linked (geoflux.linkPhotos lists them)", report.unplaced_count), false);
    }
    Ok(json!({
        "layer": id,
        "towersWithPhotos": towers,
        "towers": labels.len(),
        "linked": report.linked,
        "unplaced": report.unplaced_count,
        "unplacedListed": report.unplaced,
        "truncated": report.truncated,
    }))
}

/// The photo files under `folder` (up to [`PHOTO_DEPTH`] folders down), as paths relative to
/// it with `/` between parts. Symbolic links are not followed, so a looped folder can't trap
/// the walk; at most `images::MAX_FILES + 1` paths are returned, so the link can say it was cut.
fn list_photos(folder: &str) -> Result<Vec<String>, String> {
    #[cfg(target_arch = "wasm32")]
    {
        Err(format!("{folder}: the web app can't read folders"))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let root = std::path::Path::new(folder);
        if !root.is_dir() {
            return Err(format!("{folder}: not a folder"));
        }
        let mut out = Vec::new();
        let mut seen = 0usize;
        let mut pending: Vec<(std::path::PathBuf, String, usize)> = vec![(root.to_path_buf(), String::new(), 0)];
        while let Some((dir, rel, depth)) = pending.pop() {
            // An unreadable sub-folder is skipped; only the linked folder itself must open.
            let entries = match std::fs::read_dir(&dir) {
                Ok(e) => e,
                Err(e) if depth == 0 => return Err(format!("{folder}: {e}")),
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                seen += 1;
                if seen > MAX_ENTRIES || out.len() > images::MAX_FILES {
                    return Ok(out);
                }
                let Ok(name) = entry.file_name().into_string() else { continue };
                let Ok(kind) = entry.file_type() else { continue };
                let path = if rel.is_empty() { name } else { format!("{rel}/{name}") };
                if kind.is_dir() && depth < PHOTO_DEPTH {
                    pending.push((entry.path(), path, depth + 1));
                } else if kind.is_file() {
                    out.push(path);
                }
            }
        }
        Ok(out)
    }
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

    fn import(s: &mut Session) -> u32 {
        let r = s.run("geoflux.importCsv", &json!({"text": SHEET})).expect("import");
        r["layer"].as_u64().expect("id") as u32
    }

    #[test]
    fn form_edits_are_checked_written_and_rated_as_one_undo() {
        let mut s = session();
        let id = import(&mut s);
        // Row 2 (78_0) has no position; a bad latitude is refused and nothing changes.
        let bad =
            s.run("geoflux.setValues", &json!({"layer": id, "row": 2, "values": {"core.latitude": "north", "core.longitude": "75.2"}}));
        assert!(bad.expect_err("refused").contains("Latitude is not a coordinate"));
        assert!(s.map().layer(id).expect("layer").dataset.features[2].geometry.is_none());
        let depth = s.undo_depth();
        let r = s
            .run("geoflux.setValues", &json!({"layer": id, "row": 2, "values": {"core.latitude": "27.53", "core.longitude": "75.19", "riseTemp": "20", "faultDescription": "Hot joint"}}))
            .expect("edit");
        assert_eq!(r["severity"], "high");
        assert_eq!(s.undo_depth(), depth + 1);
        let ds = &s.map().layer(id).expect("layer").dataset;
        assert!(matches!(ds.features[2].geometry, Some(Geometry::Point(c)) if (c.y - 27.53).abs() < 1e-9));
        let desc = ds.field_index("faultDescription").expect("added as text");
        assert_eq!(ds.value(2, desc).to_string(), "Hot joint");
        assert_eq!(ds.fields[desc].display(), "Fault Description");
        // A required field cleared is refused.
        assert!(s.run("geoflux.setValues", &json!({"layer": id, "row": 0, "values": {"towerNumber": " "}})).is_err());
    }

    #[test]
    fn sequence_is_natural_and_filters_flagged() {
        let mut s = session();
        let sheet =
            "Tower No.,Line Name,Lat.,Long.,Rise Temp.\n10_0,L,27.5,75.1,20\n9_0,L,27.5,75.1,\n76_1,L,27.5,75.1,2\n76_0,L,27.5,75.1,\n";
        let r = s.run("geoflux.importCsv", &json!({"text": sheet})).expect("import");
        let id = r["layer"].as_u64().expect("id") as u32;
        let labels = |st: Vec<Stop>| st.into_iter().map(|x| x.label).collect::<Vec<_>>();
        assert_eq!(labels(sequence(&s, id, false).expect("all")), ["9_0", "10_0", "76_0", "76_1"]);
        assert_eq!(labels(sequence(&s, id, true).expect("flagged")), ["10_0", "76_1"]);
        let g = s.run("geoflux.goTo", &json!({"layer": id, "row": 0})).expect("go");
        assert_eq!(g["located"], true);
        assert_eq!(s.map().layer(id).expect("layer").selection, vec![0]);
        assert!(s.run("geoflux.goTo", &json!({"layer": id, "row": 99})).is_err());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn photos_link_from_a_folder_and_survive_saving() {
        let dir = std::env::temp_dir().join(format!("geoflux-photos-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("76_0")).expect("dir");
        for f in ["76_0/DJI_0001_T.JPG", "76_0/DJI_0001_Z.JPG", "77_0_tower.jpg", "readme.txt", "12_0_thermal.jpg"] {
            std::fs::write(dir.join(f), b"x").expect("write");
        }
        let mut s = session();
        let id = import(&mut s);
        let r = s.run("geoflux.linkPhotos", &json!({"layer": id, "folder": dir.to_string_lossy()})).expect("link");
        assert_eq!(r["linked"], 3);
        assert_eq!(r["unplaced"], 2);
        let slots = photos(&s, id, 0).expect("photos");
        let thermal = slots.iter().find(|x| x.slot.id == "THERMAL").expect("slot");
        assert!(thermal.photos[0].ends_with("DJI_0001_T.JPG"));
        assert!(std::path::Path::new(&thermal.photos[0]).is_file());
        let zoom = slots.iter().find(|x| x.slot.id == "RGB_ZOOM").expect("slot");
        assert_eq!(zoom.photos.len(), 1);
        let tower = photos(&s, id, 1).expect("photos");
        assert_eq!(tower.iter().find(|x| x.slot.id == "STRUCTURE").expect("slot").photos.len(), 1);
        // Saved and reopened, the link holds; a path that would leave the folder is dropped.
        let mut saved: Project = serde_json::from_str(&serde_json::to_string(&s.project).expect("save")).expect("load");
        assert_eq!(saved.geoflux.photos.len(), 1);
        saved.geoflux.photos[0].assets[0].slots[0].photos.push("../../etc/passwd".into());
        saved.geoflux.clamp();
        assert!(saved.geoflux.photos[0].assets[0].slots[0].photos.iter().all(|p| !p.contains("..")));
        assert!(s.run("geoflux.linkPhotos", &json!({"layer": id, "folder": dir.join("readme.txt").to_string_lossy()})).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn markup_from_a_project_file_is_checked_again() {
        let text = r##"{"markup":[
            {"layer":1,"label":"76/0","slot":"THERMAL","photo":"76_0/T.JPG","shapes":[{"kind":"rect","a":{"x":-1,"y":0.1},"b":{"x":0.5,"y":0.5},"color":"#ff0000","width":900}]},
            {"layer":1,"label":"77/0","slot":"THERMAL","photo":"77_0/T.JPG","shapes":[{"kind":"pen","points":[{"x":0.2,"y":0.2}],"color":"#ff0000","width":4}]}
        ]}"##;
        let mut g: GeoFlux = serde_json::from_str(text).expect("parses");
        g.clamp();
        assert_eq!(g.markup.len(), 1, "the one-point stroke is dropped with its photo's markup");
        let a = &g.markup[0].shapes.shapes()[0];
        assert_eq!(a.width, geoflux_domain::annotation::WIDTH_RANGE.1);
        assert!(matches!(a.shape, geoflux_domain::annotation::Shape::Rect { a, .. } if a.x == 0.0));
        assert!(!g.is_empty());
    }

    #[test]
    fn a_towers_own_folder_and_picked_photos_fill_its_slots() {
        let dir = std::env::temp_dir().join(format!("geoflux-picked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("76_0")).expect("dir");
        for f in ["76_0/DJI_0001_T.JPG", "elsewhere.png", "notes.txt"] {
            std::fs::write(dir.join(f), b"x").expect("write");
        }
        let mut s = session();
        let id = import(&mut s);
        // The tower's own folder, picked on its own, links to that tower.
        let r = s.run("geoflux.linkPhotos", &json!({"layer": id, "folder": dir.join("76_0").to_string_lossy()})).expect("link");
        assert_eq!(r["linked"], 1);
        let slot = |s: &Session, sid: &str| photos(s, id, 0).expect("photos").into_iter().find(|x| x.slot.id == sid).expect("slot").photos;
        assert_eq!(slot(&s, "THERMAL").len(), 1);

        let png = dir.join("elsewhere.png").to_string_lossy().into_owned();
        let add = json!({"layer": id, "row": 0, "slot": "LOCATION", "paths": [png, png]});
        assert_eq!(s.run("geoflux.addPhotos", &add).expect("add")["added"], 1, "once each");
        assert_eq!(s.run("geoflux.addPhotos", &add).expect("again")["added"], 0);
        assert_eq!(slot(&s, "LOCATION"), std::slice::from_ref(&png));
        // Picked photos survive a relink and a save.
        s.run("geoflux.linkPhotos", &json!({"layer": id, "folder": dir.join("76_0").to_string_lossy()})).expect("relink");
        let saved: Project = serde_json::from_str(&serde_json::to_string(&s.project).expect("save")).expect("load");
        assert_eq!(saved.geoflux.picked.len(), 1);
        assert_eq!(slot(&s, "LOCATION"), std::slice::from_ref(&png));

        let bad = |slot: &str, path: &str| json!({"layer": id, "row": 0, "slot": slot, "paths": [path]});
        assert!(s.run("geoflux.addPhotos", &bad("LOCATION", &dir.join("notes.txt").to_string_lossy())).is_err());
        assert!(s.run("geoflux.addPhotos", &bad("LOCATION", &dir.join("gone.jpg").to_string_lossy())).is_err());
        assert!(s.run("geoflux.addPhotos", &bad("NOPE", &png)).is_err());

        let linked = slot(&s, "THERMAL")[0].clone();
        assert!(
            s.run("geoflux.removePhoto", &json!({"layer": id, "row": 0, "slot": "THERMAL", "path": linked})).is_err(),
            "folder photos stay"
        );
        s.run("geoflux.removePhoto", &json!({"layer": id, "row": 0, "slot": "LOCATION", "path": png})).expect("remove");
        assert!(slot(&s, "LOCATION").is_empty());
        assert!(std::path::Path::new(&png).is_file(), "the file is kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn layers_without_a_schema_are_refused() {
        let mut s = session();
        s.run("layer.addSample", &json!({"name": "cities"})).expect("sample");
        assert!(s.run("geoflux.classify", &Value::Null).is_err());
        assert!(s.run("geoflux.validate", &Value::Null).is_err());
    }
}
