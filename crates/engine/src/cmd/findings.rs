//! GeoFlux › Findings: what is wrong with each tower, as records (see
//! [`geoflux_domain::finding`]) instead of typed fault text.
//!
//! A layer gets findings the first time one is added. At that moment the fault values typed into
//! its forms before (description, temperatures) are carried into one finding per tower, once, so
//! nothing is lost. From then on the layer is in findings mode:
//!
//! - the form fields the schema's `findingFields` name (Fault Description, Fault Temp, Rise Temp)
//!   are filled from each tower's confirmed findings and can't be typed over;
//! - a tower's severity is its worst confirmed finding;
//! - Record Thermal Finding adds a finding, and Scan Line Thermal *proposes* one per hot thermal
//!   photo, which a person confirms, edits or rejects.
//!
//! A finding refers to its photo by content ([`PhotoId`]), so moving or relinking the photo folder
//! never loses it. Where each photo was last seen is kept beside, so a finding can show its photo.

use std::collections::BTreeMap;

use geoflux_domain::finding::{FindingEdit, InspectError, InspectorInfo, InspectorKind, measure};
use geoflux_domain::{DefectId, DomainSchema, FindingId, FindingSet, Observation, PhotoId, Region, Severity};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::geoflux::{self, GeoFlux, layer_schema};
use super::{need_feature_layer, ps};
use crate::{CmdResult, CommandSpec as C, Session};

/// Most layers with findings in one project.
const MAX_LAYERS: usize = 4_096;
/// Most remembered photo locations.
const MAX_PHOTO_PATHS: usize = 200_000;
/// Longest remembered photo path.
const MAX_PATH: usize = 4_096;

/// The thermal rules as an inspector: the hottest point of a thermal photo and its rise over
/// ambient, rated by the schema's thresholds. Bump the version when its output can change.
pub fn thermal_rule() -> InspectorInfo {
    InspectorInfo {
        id: "geoflux.thermal.hotspot".into(),
        version: "1".into(),
        kind: InspectorKind::Rule,
        label: "Thermal hot spots".into(),
    }
}

/// One layer's findings, as the project keeps them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LayerFindings {
    pub layer: u32,
    pub set: FindingSet,
}

pub fn commands() -> Vec<C> {
    vec![
        C::new("geoflux.findings", "Tower Findings", "GeoFlux › Findings", findings_cmd).view().when(need_feature_layer).params(
            r#"{"layer"?, "row": number, "path"?: "a photo of the tower: also list the findings on it"}"#,
        ),
        C::new("geoflux.addFinding", "Add Finding", "GeoFlux › Findings", add_finding).when(need_feature_layer).params(
            r#"{"layer"?, "row": number, "path"?: "the tower photo it is on (leave out for the whole tower)", "defect": "a defect type id from the schema", "region"?: {"kind": "box", "a": {"x","y"}, "b": {"x","y"}} (fractions of the photo), "severity"?: "none|low|medium|high|critical", "note"?, "measurements"?: {"maxC": number…}, "by"?: "who made it (the signed-in account)"}"#,
        ),
        C::new("geoflux.decideFinding", "Confirm or Reject Finding", "GeoFlux › Findings", decide_finding)
            .when(need_feature_layer)
            .params(r#"{"layer"?, "id": "finding id", "decision": "confirm|reject", "by"?}"#),
        C::new("geoflux.editFinding", "Edit Finding", "GeoFlux › Findings", edit_finding).when(need_feature_layer).params(
            r#"{"layer"?, "id": "finding id", "defect"?, "region"?, "severity"?, "note"?, "measurements"?, "by"?}"#,
        ),
        C::new("geoflux.removeFinding", "Delete Finding", "GeoFlux › Findings", remove_finding)
            .when(need_feature_layer)
            .params(r#"{"layer"?, "id": "finding id (an engineer's; an inspector's is rejected instead)"}"#),
    ]
}

/// Bounds what a project file brings: findings are sanitized (a set from a newer GeoFlux is kept
/// untouched and can't be changed), and the lists are bounded.
pub(crate) fn clamp(g: &mut GeoFlux) {
    g.findings.truncate(MAX_LAYERS);
    for f in &mut g.findings {
        let _ = f.set.sanitize();
    }
    g.photo_paths.retain(|_, p| !p.trim().is_empty() && p.len() <= MAX_PATH);
    while g.photo_paths.len() > MAX_PHOTO_PATHS {
        g.photo_paths.pop_last();
    }
}

/// The keys of the form fields filled from findings, when the layer has findings.
pub(crate) fn derived_keys<'a>(g: &GeoFlux, layer: u32, schema: &'a DomainSchema) -> Vec<&'a str> {
    if g.findings_of(layer).is_none() {
        return Vec::new();
    }
    schema.finding_fields.iter().map(|d| d.key.as_str()).collect()
}

pub(crate) fn now_ms() -> i64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// The content id of a photo file: reads its first and last 64 KiB only.
#[cfg(not(target_arch = "wasm32"))]
pub fn photo_id_of(path: &str) -> Result<PhotoId, String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
    let len = file.metadata().map_err(|e| format!("{path}: {e}"))?.len();
    let head_range = PhotoId::head_range(len);
    let tail_range = PhotoId::tail_range(len);
    let mut head = vec![0u8; (head_range.end - head_range.start) as usize];
    file.read_exact(&mut head).map_err(|e| format!("{path}: {e}"))?;
    let mut tail = vec![0u8; (tail_range.end - tail_range.start) as usize];
    file.seek(SeekFrom::Start(tail_range.start)).map_err(|e| format!("{path}: {e}"))?;
    file.read_exact(&mut tail).map_err(|e| format!("{path}: {e}"))?;
    PhotoId::from_samples(len, &head, &tail).map_err(|e| format!("{path}: {e}"))
}

#[cfg(target_arch = "wasm32")]
pub fn photo_id_of(path: &str) -> Result<PhotoId, String> {
    Err(format!("{path}: the web app can't read photo files"))
}

/// Remembers where a photo was seen.
fn remember(g: &mut GeoFlux, id: &PhotoId, path: &str) {
    if path.len() <= MAX_PATH && (g.photo_paths.len() < MAX_PHOTO_PATHS || g.photo_paths.contains_key(id)) {
        g.photo_paths.insert(id.clone(), path.to_string());
    }
}

/// The asset's label at `row`, refused when it has none (findings are kept by label).
fn label_of(s: &Session, layer: u32, row: usize, schema: &DomainSchema) -> Result<String, String> {
    let ds = &s.map().layer(layer).ok_or("no such layer")?.dataset;
    if row >= ds.features.len() {
        return Err("no such row".into());
    }
    let label = geoflux::label_at(ds, schema, row);
    if label.is_empty() {
        return Err(format!("this tower has no {} to keep its findings by", schema.label_key()));
    }
    Ok(label)
}

/// A photo of the tower at `row` and the slot it is in. The path must be one of the tower's
/// photos, as the Inspection pane lists them.
fn photo_of(s: &mut Session, layer: u32, row: usize, path: &str) -> Result<(PhotoId, String), String> {
    let slot = geoflux::photos(s, layer, row)?
        .into_iter()
        .find(|sp| sp.photos.iter().any(|p| p == path))
        .map(|sp| sp.slot.id)
        .ok_or("that photo is not one of this tower's photos")?;
    let id = photo_id_of(path)?;
    remember(&mut s.project.geoflux, &id, path);
    Ok((id, slot))
}

/// The layer's findings, created on first use: the fault values typed into its forms before are
/// carried over once, one finding per tower.
fn set_mut<'a>(s: &'a mut Session, layer: u32, schema: &DomainSchema) -> Result<&'a mut FindingSet, String> {
    if s.project.geoflux.findings_of(layer).is_none() {
        if s.project.geoflux.findings.len() >= MAX_LAYERS {
            return Err(format!("a project keeps findings for at most {MAX_LAYERS} layers"));
        }
        let ds = &s.map().layer(layer).ok_or("no such layer")?.dataset;
        let rows: Vec<(String, geoflux_domain::Properties)> = (0..ds.features.len())
            .map(|row| {
                let asset = geoflux::asset_of(ds, schema, row);
                (geoflux::label_at(ds, schema, row), asset.properties)
            })
            .filter(|(label, _)| !label.is_empty())
            .collect();
        let mut set = FindingSet::new();
        set.migrate_layer(rows.iter().map(|(l, p)| (l.as_str(), p)), schema, now_ms());
        s.project.geoflux.findings.push(LayerFindings { layer, set });
    }
    let set = s.project.geoflux.findings.iter_mut().find(|f| f.layer == layer).map(|f| &mut f.set).ok_or("no such layer")?;
    // A set from a newer GeoFlux is kept as it is; changing it here would lose what this version can't read.
    if !set.is_writable() {
        return Err("this layer's findings were saved by a newer GeoFlux; update GeoGIS to change them".into());
    }
    Ok(set)
}

/// Writes the derived form fields and severity of every tower with one of `labels`.
fn refresh(s: &mut Session, layer: u32, schema: &DomainSchema, labels: &[String]) -> Result<(), String> {
    let wanted: std::collections::HashSet<&str> = labels.iter().map(String::as_str).collect();
    // Each tower's values, worked out in one pass over the findings.
    let rows: Vec<(usize, Vec<(String, String)>)> = {
        let ds = &s.map().layer(layer).ok_or("no such layer")?.dataset;
        let Some(set) = s.project.geoflux.findings_of(layer) else { return Ok(()) };
        let grouped = set.by_asset();
        let none = geoflux_domain::finding::AssetFindings::default();
        (0..ds.features.len())
            .filter_map(|row| {
                let label = geoflux::label_at(ds, schema, row);
                wanted.contains(label.as_str()).then(|| (row, grouped.get(label.as_str()).unwrap_or(&none).form_values(schema)))
            })
            .collect()
    };
    for (row, values) in rows {
        geoflux::write_derived(s, layer, row, schema, values)?;
        geoflux::rate_row(s, layer, row, schema);
    }
    Ok(())
}

/// Every tower of the layer: after the first finding the carried-over findings decide severity.
fn refresh_all(s: &mut Session, layer: u32, schema: &DomainSchema) -> Result<(), String> {
    let labels: Vec<String> = {
        let ds = &s.map().layer(layer).ok_or("no such layer")?.dataset;
        (0..ds.features.len()).map(|row| geoflux::label_at(ds, schema, row)).collect()
    };
    refresh(s, layer, schema, &labels)
}

fn row_param(p: &Value) -> Result<usize, String> {
    p.get("row").and_then(Value::as_u64).map(|r| r as usize).ok_or_else(|| "row is required".to_string())
}

fn id_param(p: &Value) -> Result<FindingId, String> {
    ps(p, "id").and_then(FindingId::parse).ok_or_else(|| "id is required (a finding id)".to_string())
}

fn severity_param(p: &Value) -> Result<Option<Severity>, String> {
    match ps(p, "severity").map(str::trim) {
        None | Some("") => Ok(None),
        Some(name) => Severity::ALL
            .into_iter()
            .find(|s| s.as_str().eq_ignore_ascii_case(name))
            .map(Some)
            .ok_or_else(|| format!("severity '{name}' isn't one of none, low, medium, high, critical")),
    }
}

fn region_param(p: &Value) -> Result<Option<Region>, String> {
    match p.get("region").filter(|v| !v.is_null()) {
        None => Ok(None),
        Some(v) => serde_json::from_value(v.clone()).map(Some).map_err(|e| format!("region: {e}")),
    }
}

fn measurements_param(p: &Value) -> Result<Option<BTreeMap<String, f64>>, String> {
    match p.get("measurements").filter(|v| !v.is_null()) {
        None => Ok(None),
        Some(v) => serde_json::from_value(v.clone()).map(Some).map_err(|_| "measurements must be {name: number}".to_string()),
    }
}

fn defect_param(p: &Value) -> Result<Option<DefectId>, String> {
    match ps(p, "defect").map(str::trim).filter(|d| !d.is_empty()) {
        None => Ok(None),
        Some(d) => DefectId::parse(d).map(Some).ok_or_else(|| format!("'{d}' isn't a defect type id like hotspot.joint")),
    }
}

/// A finding as the app shows it: its fields, plus where its photo was last seen.
fn finding_json(g: &GeoFlux, schema: &DomainSchema, f: &geoflux_domain::Finding) -> Value {
    let mut v = serde_json::to_value(f).unwrap_or(Value::Null);
    if let Some(obj) = v.as_object_mut() {
        obj.insert("defectLabel".into(), json!(schema.defect_label(&f.defect)));
        obj.insert("counts".into(), json!(f.status.counts()));
        if let Some(path) = f.photo.as_ref().and_then(|id| g.photo_paths.get(id)) {
            obj.insert("path".into(), json!(path));
        }
    }
    v
}

/// One tower's findings: counted ones worst first, then proposals, then rejected; with the
/// schema's defect types and, for `path`, the ids of the findings on that photo.
fn findings_cmd(s: &mut Session, p: &Value) -> CmdResult {
    let (layer, schema) = layer_schema(s, p)?;
    let row = row_param(p)?;
    let label = label_of(s, layer, row, &schema)?;
    let photo = match ps(p, "path").map(str::trim).filter(|x| !x.is_empty()) {
        Some(path) => Some(photo_of(s, layer, row, path)?),
        None => None,
    };
    let g = &s.project.geoflux;
    let mut list = Vec::new();
    let (mut severity, mut pending, mut rejected) = (Severity::None, 0, 0);
    let mut on_photo = Vec::new();
    if let Some(set) = g.findings_of(layer) {
        let summary = set.for_asset(&label);
        severity = summary.severity();
        (pending, rejected) = (summary.pending, summary.rejected);
        list.extend(summary.counted.iter().map(|f| finding_json(g, &schema, f)));
        let rest = set.findings().iter().filter(|f| f.asset == label && !f.status.counts());
        list.extend(rest.map(|f| finding_json(g, &schema, f)));
        if let Some((id, _)) = &photo {
            on_photo.extend(set.on_photo(&label, id).map(|f| f.id.to_string()));
        }
    }
    Ok(json!({
        "layer": layer,
        "row": row,
        "tower": label,
        "findingsMode": g.findings_of(layer).is_some(),
        "severity": severity,
        "pending": pending,
        "rejected": rejected,
        "findings": list,
        "photo": photo.as_ref().map(|(id, slot)| json!({"id": id, "slot": slot})),
        "onPhoto": on_photo,
        "defects": schema.defects,
        "derivedFields": schema.finding_fields.iter().map(|d| d.key.as_str()).collect::<Vec<_>>(),
    }))
}

/// An engineer's finding on one tower, confirmed as made.
fn add_finding(s: &mut Session, p: &Value) -> CmdResult {
    let (layer, schema) = layer_schema(s, p)?;
    let row = row_param(p)?;
    let label = label_of(s, layer, row, &schema)?;
    let (photo, slot) = match ps(p, "path").map(str::trim).filter(|x| !x.is_empty()) {
        Some(path) => {
            let (id, slot) = photo_of(s, layer, row, path)?;
            (Some(id), slot)
        }
        None => (None, String::new()),
    };
    let observation = Observation {
        region: region_param(p)?.unwrap_or_default(),
        defect: Some(defect_param(p)?.ok_or("defect is required (a defect type id from the schema)")?),
        severity: severity_param(p)?,
        measurements: measurements_param(p)?.unwrap_or_default(),
        note: ps(p, "note").unwrap_or("").to_string(),
        confidence: None,
    };
    let user = user(p);
    let first = s.project.geoflux.findings_of(layer).is_none();
    let set = set_mut(s, layer, &schema)?;
    let id = set.add(&label, photo, &slot, observation, &user, now_ms(), &schema).map_err(|e| e.to_string())?;
    if first {
        refresh_all(s, layer, &schema)?
    } else {
        refresh(s, layer, &schema, std::slice::from_ref(&label))?
    }
    Ok(json!({"layer": layer, "row": row, "id": id, "severity": severity_of(s, layer, &label)}))
}

fn severity_of(s: &Session, layer: u32, label: &str) -> Severity {
    s.project.geoflux.findings_of(layer).map_or(Severity::None, |set| set.for_asset(label).severity())
}

/// The finding `id` in the layer, and its tower.
fn find(s: &Session, layer: u32, id: &FindingId) -> Result<String, String> {
    s.project
        .geoflux
        .findings_of(layer)
        .and_then(|set| set.get(id))
        .map(|f| f.asset.clone())
        .ok_or_else(|| format!("there is no finding {id} on this layer"))
}

/// Who is deciding: the signed-in account the app passes as `by`, else nobody named.
fn user(p: &Value) -> String {
    ps(p, "by").map(str::trim).filter(|b| b.chars().count() <= 200).unwrap_or("").to_string()
}

fn decide_finding(s: &mut Session, p: &Value) -> CmdResult {
    let (layer, schema) = layer_schema(s, p)?;
    let id = id_param(p)?;
    let tower = find(s, layer, &id)?;
    let by = user(p);
    let set = set_mut(s, layer, &schema)?;
    match ps(p, "decision").map(str::trim) {
        Some("confirm") => set.confirm(&id, &by, now_ms()),
        Some("reject") => set.reject(&id, &by, now_ms()),
        _ => return Err("decision is confirm or reject".into()),
    }
    .map_err(|e| e.to_string())?;
    refresh(s, layer, &schema, std::slice::from_ref(&tower))?;
    Ok(json!({"layer": layer, "id": id, "tower": tower, "severity": severity_of(s, layer, &tower)}))
}

fn edit_finding(s: &mut Session, p: &Value) -> CmdResult {
    let (layer, schema) = layer_schema(s, p)?;
    let id = id_param(p)?;
    let tower = find(s, layer, &id)?;
    let edit = FindingEdit {
        region: region_param(p)?,
        defect: defect_param(p)?,
        severity: severity_param(p)?,
        measurements: measurements_param(p)?,
        note: ps(p, "note").map(str::to_string),
    };
    let by = user(p);
    set_mut(s, layer, &schema)?.edit(&id, edit, &by, now_ms(), &schema).map_err(|e| e.to_string())?;
    refresh(s, layer, &schema, std::slice::from_ref(&tower))?;
    Ok(json!({"layer": layer, "id": id, "tower": tower, "severity": severity_of(s, layer, &tower)}))
}

fn remove_finding(s: &mut Session, p: &Value) -> CmdResult {
    let (layer, schema) = layer_schema(s, p)?;
    let id = id_param(p)?;
    let tower = find(s, layer, &id)?;
    set_mut(s, layer, &schema)?.remove(&id).map_err(|e| e.to_string())?;
    refresh(s, layer, &schema, std::slice::from_ref(&tower))?;
    Ok(json!({"layer": layer, "id": id, "tower": tower, "severity": severity_of(s, layer, &tower)}))
}

/// The defect type a thermal hot spot is classed as: `hotspot.other` when the schema has it for
/// the slot, else the first `hotspot` type for the slot, else the first type for the slot, else
/// unclassified (the engineer picks the exact type when confirming).
pub(crate) fn thermal_defect(schema: &DomainSchema, slot: &str) -> DefectId {
    let fits = |d: &&geoflux_domain::DefectDef| d.applies_to(slot) && !d.slots.is_empty();
    schema
        .defects
        .iter()
        .filter(fits)
        .find(|d| d.id.as_str() == "hotspot.other")
        .or_else(|| schema.defects.iter().filter(fits).find(|d| d.id.family() == "hotspot"))
        .or_else(|| schema.defects.iter().find(fits))
        .map_or_else(DefectId::unclassified, |d| d.id.clone())
}

/// A thermal measurement as an observation: the box it was found in (or the hottest point), its
/// temperatures, and a severity from the schema's thresholds when there is a rise.
pub(crate) fn thermal_observation(schema: &DomainSchema, slot: &str, max_c: f32, reference_c: Option<f32>, region: Region) -> Observation {
    let mut measurements = BTreeMap::new();
    if max_c.is_finite() {
        measurements.insert(measure::MAX_C.to_string(), round1(max_c));
    }
    let rise = reference_c.filter(|r| r.is_finite()).map(|r| {
        measurements.insert(measure::REFERENCE_C.to_string(), round1(r));
        max_c - r
    });
    if let Some(r) = rise.filter(|r| r.is_finite()) {
        measurements.insert(measure::RISE_C.to_string(), round1(r));
    }
    let severity = match (rise, &schema.severity) {
        (Some(r), Some(rule)) => Some(rule.thresholds.level(f64::from(r))),
        _ => None,
    };
    Observation { region, defect: Some(thermal_defect(schema, slot)), severity, measurements, note: String::new(), confidence: None }
}

fn round1(v: f32) -> f64 {
    (f64::from(v) * 10.0).round() / 10.0
}

/// Records a thermal measurement an engineer made on a tower's thermal photo as a confirmed finding.
pub(crate) fn record_thermal(
    s: &mut Session,
    p: &Value,
    layer: u32,
    row: usize,
    path: Option<&str>,
    observation: Observation,
) -> Result<(FindingId, Severity), String> {
    let (_, schema) = layer_schema(s, &json!({"layer": layer}))?;
    let label = label_of(s, layer, row, &schema)?;
    let (photo, slot) = match path {
        Some(path) => {
            let (id, slot) = photo_of(s, layer, row, path)?;
            (Some(id), slot)
        }
        None => (None, String::new()),
    };
    let observation = if photo.is_none() { Observation { region: Region::Whole, ..observation } } else { observation };
    let by = user(p);
    let first = s.project.geoflux.findings_of(layer).is_none();
    let set = set_mut(s, layer, &schema)?;
    let id = set.add(&label, photo, &slot, observation, &by, now_ms(), &schema).map_err(|e| e.to_string())?;
    if first {
        refresh_all(s, layer, &schema)?
    } else {
        refresh(s, layer, &schema, std::slice::from_ref(&label))?
    }
    Ok((id, severity_of(s, layer, &label)))
}

/// What one tower's thermal rule run gave.
pub(crate) enum RuleResult {
    /// The hottest point and the reference it is rated over, when there is one.
    Measured {
        max_c: f32,
        reference_c: Option<f32>,
        region: Region,
    },
    Failed(String),
}

/// What [`propose_thermal`] did across a line.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProposeReport {
    pub proposed: usize,
    /// Photos the rule had already looked at in this version.
    pub already: usize,
    /// Photos it looked at without finding a hot spot worth proposing.
    pub nothing: usize,
    pub failed: usize,
}

/// Records the thermal rule's results as proposals (see [`thermal_rule`]): a hot spot whose rise
/// reaches the schema's lowest threshold, or any hot spot when it can't be rated. Photos this
/// version already looked at are left alone, so scanning a line again is cheap and never duplicates.
pub(crate) fn propose_thermal(s: &mut Session, layer: u32, results: Vec<(usize, String, RuleResult)>) -> Result<ProposeReport, String> {
    let (_, schema) = layer_schema(s, &json!({"layer": layer}))?;
    let rule = thermal_rule();
    let first = s.project.geoflux.findings_of(layer).is_none();
    let mut report = ProposeReport::default();
    let mut touched = Vec::new();
    for (row, path, result) in results {
        let Ok(label) = label_of(s, layer, row, &schema) else { continue };
        let (photo, slot) = match photo_of(s, layer, row, &path) {
            Ok(p) => p,
            Err(_) => {
                report.failed += 1;
                continue;
            }
        };
        let outcome = match result {
            RuleResult::Failed(e) => Err(InspectError::Unreadable(e)),
            RuleResult::Measured { max_c, reference_c, region } => {
                let o = thermal_observation(&schema, &slot, max_c, reference_c, region);
                let low = schema.severity.as_ref().map(|r| r.thresholds.low);
                let rise = o.measurements.get(measure::RISE_C).copied();
                let worth = match (rise, low) {
                    (Some(r), Some(low)) => r >= low,
                    _ => true,
                };
                Ok(if worth { vec![o] } else { Vec::new() })
            }
        };
        let set = set_mut(s, layer, &schema)?;
        let r = set.record_run(&label, &photo, &slot, &rule, outcome, now_ms(), &schema).map_err(|e| e.to_string())?;
        match r.outcome {
            _ if r.already_done => report.already += 1,
            Some(geoflux_domain::finding::RunOutcome::Found { count }) => report.proposed += count as usize,
            Some(geoflux_domain::finding::RunOutcome::Failed { .. }) => report.failed += 1,
            _ => report.nothing += 1,
        }
        touched.push(label);
    }
    if first && s.project.geoflux.findings_of(layer).is_some() {
        refresh_all(s, layer, &schema)?;
    } else {
        touched.sort();
        touched.dedup();
        refresh(s, layer, &schema, &touched)?;
    }
    Ok(report)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
