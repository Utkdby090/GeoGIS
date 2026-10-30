//! An industry's layer as data: its form fields, image slots, how its survey CSV maps onto fields,
//! and how a finding's severity is rated. One JSON file per industry, shared by desktop, web and
//! mobile, so a new tender's 30–40 layers are set up without code.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::asset::Asset;
use crate::defect::{DefectDef, DefectId, MAX_DEFECTS};

/// Form keys for the coordinates, which are core [`Asset`] fields rather than properties.
pub mod core_fields {
    pub const LATITUDE: &str = "core.latitude";
    pub const LONGITUDE: &str = "core.longitude";

    pub fn is_coordinate(key: &str) -> bool {
        key == LATITUDE || key == LONGITUDE
    }
}

/// The schema format this build reads. Files with a higher number come from a newer GeoFlux.
/// 2 added `defects` and `findingFields`; format 1 files read unchanged.
pub const SCHEMA_VERSION: u32 = 2;

/// Bounds on a schema, so a hostile or broken file cannot make the app allocate without limit.
pub const MAX_FIELDS: usize = 1_000;
pub const MAX_OPTIONS: usize = 1_000;
/// Most image slots, and most match words on one slot.
pub const MAX_SLOTS: usize = 64;
pub const MAX_MATCHES: usize = 64;
pub const MAX_SCHEMA_BYTES: usize = 4 * 1024 * 1024;

/// What kind of input a field takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FieldType {
    Text,
    Number,
    Integer,
    /// ISO 8601 date, `YYYY-MM-DD`.
    Date,
    /// One of `options`.
    Dropdown,
    /// Any of `options`, stored separated by `;`.
    MultiSelect,
    /// `true`/`false` (also `yes`/`no`, `1`/`0`).
    Boolean,
    /// A photo, stored as a path relative to the project.
    Photo,
}

fn one() -> f32 {
    1.0
}

/// One form field (e.g. "Tower No."). Fields are shown by `group` (in schema order), then by `row`
/// inside the group; fields on one row share the width by `weight`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FieldDef {
    pub key: String,
    pub label: String,
    #[serde(rename = "type")]
    pub field_type: FieldType,
    #[serde(default)]
    pub required: bool,
    /// The choices of a dropdown or multi-select field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub row: u32,
    #[serde(default = "one")]
    pub weight: f32,
    /// Bounds of a number or integer field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

/// One image slot on the form and the report. `thermal` marks a slot for radiometric images.
///
/// A linked photo goes in the slot one of whose `matches` words is a whole part of its file
/// name (split at `_`, `-`, `.`, spaces and case changes don't matter): `ir` matches
/// `76_0_IR.jpg` but not `wire.jpg`. Thermal slots are tried first, then the rest in order. A
/// photo no slot claims goes in the `fallback` slot, if there is one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageSlotDef {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub thermal: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matches: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fallback: bool,
}

/// One CSV column read into a field. `default` fills a blank cell.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CsvColumn {
    pub header: String,
    pub key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default: String,
}

/// How a survey CSV becomes assets. Headers match without regard to case or surrounding spaces.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CsvProfile {
    pub columns: Vec<CsvColumn>,
    /// Keys a row must have a value for; a row without them is skipped with a reason. Their
    /// columns must exist in the file.
    #[serde(default)]
    pub required: Vec<String>,
    pub latitude: String,
    pub longitude: String,
    /// Headers that are neither fields nor extra columns (row numbers and the like).
    #[serde(default)]
    pub ignore: Vec<String>,
    /// When set, every other column is kept as a property named `<prefix><header>`
    /// (per-circuit load data, for example). When absent, other columns are dropped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_prefix: Option<String>,
    /// The form group extra columns are shown in.
    #[serde(default)]
    pub extra_group: String,
}

/// Temperature rise (or any measured excess) at which a finding moves up a level.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Thresholds {
    pub low: f64,
    pub medium: f64,
    pub high: f64,
    pub critical: f64,
}

impl Default for Thresholds {
    /// Common electrical-inspection bands (roughly NETA-style). A utility's own standard should replace them.
    fn default() -> Self {
        Thresholds { low: 1.0, medium: 4.0, high: 16.0, critical: 40.0 }
    }
}

/// A property that must contain some text (ignoring case) for an asset to count as flagged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlagMatch {
    pub key: String,
    pub contains: String,
}

/// How a finding is rated. The excess is `rise`, or `measured − reference` when `rise` is blank.
/// An asset matching every `flaggedWhen` entry is never rated below LOW, and is MEDIUM when it
/// has no usable numbers (needs a look, cannot be rated).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SeverityRule {
    pub rise: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measured: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flagged_when: Vec<FlagMatch>,
    #[serde(default)]
    pub thresholds: Thresholds,
}

/// How a form field is filled from an asset's counted findings (confirmed or edited; see
/// [`crate::finding`]). The form then shows the findings, so it can never disagree with them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "from", rename_all = "camelCase")]
pub enum Derivation {
    /// The labels of the findings' defect types, worst first, joined by "; ".
    Defects,
    /// The findings' notes, worst first, joined by "; ".
    Notes,
    /// The highest value of one measurement (`maxC`, `riseC`…), one decimal.
    Max { measure: String },
    /// The asset's severity name (`high`…).
    Severity,
}

/// One form field filled from findings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DerivedField {
    pub key: String,
    #[serde(flatten)]
    pub from: Derivation,
}

/// An industry's schema.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DomainSchema {
    pub schema_version: u32,
    /// Stable id, e.g. `com.geo.telecom`.
    pub id: String,
    pub name: String,
    /// The schema's own version, e.g. `1.0.0`; bumped when fields change.
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// The field that names an asset (a tower number): the inspection pane's list, the order
    /// assets are walked in, and the folder its photos are in. Blank: the first field.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label_field: String,
    pub fields: Vec<FieldDef>,
    #[serde(default)]
    pub image_slots: Vec<ImageSlotDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub csv: Option<CsvProfile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<SeverityRule>,
    /// The defect types findings are classed by (format 2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub defects: Vec<DefectDef>,
    /// Form fields filled from findings rather than typed (format 2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub finding_fields: Vec<DerivedField>,
}

/// Why a schema file cannot be used.
#[derive(Debug, Error, PartialEq)]
pub enum SchemaError {
    #[error("the schema is not valid JSON: {0}")]
    Json(String),
    #[error("the schema is larger than {MAX_SCHEMA_BYTES} bytes")]
    TooLarge,
    #[error("the schema needs a newer GeoFlux (format {found}, this version reads {SCHEMA_VERSION})")]
    UnsupportedVersion { found: u32 },
    #[error("the schema has no {0}")]
    Missing(&'static str),
    #[error("the schema has more than {limit} {what}")]
    TooMany { what: &'static str, limit: usize },
    #[error("field key \"{0}\" appears more than once")]
    DuplicateKey(String),
    #[error("image slot \"{0}\" appears more than once")]
    DuplicateSlot(String),
    #[error("image slot \"{id}\": {problem}")]
    BadSlot { id: String, problem: &'static str },
    #[error("field \"{key}\": {problem}")]
    BadField { key: String, problem: &'static str },
    #[error("the CSV mapping: {0}")]
    BadCsv(String),
    #[error("the severity rule: {0}")]
    BadSeverity(String),
    #[error("defect type \"{id}\": {problem}")]
    BadDefect { id: String, problem: &'static str },
    #[error("findingFields: {0}")]
    BadFindingField(String),
}

impl DomainSchema {
    /// Reads and checks a schema file.
    pub fn from_json(text: &str) -> Result<Self, SchemaError> {
        if text.len() > MAX_SCHEMA_BYTES {
            return Err(SchemaError::TooLarge);
        }
        let schema: DomainSchema = serde_json::from_str(text).map_err(|e| SchemaError::Json(e.to_string()))?;
        schema.check()?;
        Ok(schema)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Checks the rules the format cannot express: unique keys, options where needed, sane bounds.
    pub fn check(&self) -> Result<(), SchemaError> {
        if self.schema_version == 0 {
            return Err(SchemaError::Missing("schemaVersion"));
        }
        if self.schema_version > SCHEMA_VERSION {
            return Err(SchemaError::UnsupportedVersion { found: self.schema_version });
        }
        if self.id.trim().is_empty() {
            return Err(SchemaError::Missing("id"));
        }
        if self.name.trim().is_empty() {
            return Err(SchemaError::Missing("name"));
        }
        if self.fields.is_empty() {
            return Err(SchemaError::Missing("fields"));
        }
        if self.fields.len() > MAX_FIELDS {
            return Err(SchemaError::TooMany { what: "fields", limit: MAX_FIELDS });
        }
        let mut keys = HashSet::new();
        for f in &self.fields {
            let bad = |problem| SchemaError::BadField { key: f.key.clone(), problem };
            if f.key.trim().is_empty() {
                return Err(SchemaError::Missing("key on every field"));
            }
            if !keys.insert(f.key.as_str()) {
                return Err(SchemaError::DuplicateKey(f.key.clone()));
            }
            if f.label.trim().is_empty() {
                return Err(bad("no label"));
            }
            if f.options.len() > MAX_OPTIONS {
                return Err(SchemaError::TooMany { what: "options on a field", limit: MAX_OPTIONS });
            }
            let choice = matches!(f.field_type, FieldType::Dropdown | FieldType::MultiSelect);
            if choice && f.options.is_empty() {
                return Err(bad("a dropdown or multi-select needs options"));
            }
            if !choice && !f.options.is_empty() {
                return Err(bad("only dropdown and multi-select fields take options"));
            }
            if !(f.weight.is_finite() && f.weight > 0.0) {
                return Err(bad("weight must be a positive number"));
            }
            let numeric = matches!(f.field_type, FieldType::Number | FieldType::Integer);
            if (f.min.is_some() || f.max.is_some()) && !numeric {
                return Err(bad("only number and integer fields take min and max"));
            }
            if f.min.is_some_and(|v| !v.is_finite()) || f.max.is_some_and(|v| !v.is_finite()) {
                return Err(bad("min and max must be finite"));
            }
            if let (Some(lo), Some(hi)) = (f.min, f.max)
                && lo > hi
            {
                return Err(bad("min is above max"));
            }
            if core_fields::is_coordinate(&f.key) && f.field_type != FieldType::Number {
                return Err(bad("coordinate fields must be numbers"));
            }
        }
        if !self.label_field.is_empty() && !keys.contains(self.label_field.as_str()) {
            return Err(SchemaError::BadField { key: self.label_field.clone(), problem: "labelField names no field" });
        }
        if self.image_slots.len() > MAX_SLOTS {
            return Err(SchemaError::TooMany { what: "image slots", limit: MAX_SLOTS });
        }
        let mut slots = HashSet::new();
        for s in &self.image_slots {
            if s.id.trim().is_empty() {
                return Err(SchemaError::Missing("id on every image slot"));
            }
            if !slots.insert(s.id.as_str()) {
                return Err(SchemaError::DuplicateSlot(s.id.clone()));
            }
            if s.matches.len() > MAX_MATCHES {
                return Err(SchemaError::TooMany { what: "match words on an image slot", limit: MAX_MATCHES });
            }
            if s.matches.iter().any(|w| w.is_empty() || !w.chars().all(char::is_alphanumeric)) {
                return Err(SchemaError::BadSlot { id: s.id.clone(), problem: "match words are letters and digits only" });
            }
        }
        if self.image_slots.iter().filter(|s| s.fallback).count() > 1 {
            return Err(SchemaError::BadSlot { id: String::new(), problem: "only one image slot can be the fallback" });
        }
        if let Some(csv) = &self.csv {
            check_csv(csv, &keys)?;
        }
        if let Some(rule) = &self.severity {
            check_severity(rule, &keys)?;
        }
        check_defects(&self.defects, &slots)?;
        check_finding_fields(&self.finding_fields, &keys)?;
        Ok(())
    }

    /// The defect type with this id.
    pub fn defect(&self, id: &DefectId) -> Option<&DefectDef> {
        self.defects.iter().find(|d| d.id == *id)
    }

    /// What people read for a defect id: its label, "Unclassified", or the id itself when the
    /// schema no longer lists it (a finding outlives a schema edit).
    pub fn defect_label<'a>(&'a self, id: &'a DefectId) -> &'a str {
        match self.defect(id) {
            Some(d) => &d.label,
            None if id.is_unclassified() => "Unclassified",
            None => id.as_str(),
        }
    }

    /// The key of the field that names an asset ([`DomainSchema::label_field`]).
    pub fn label_key(&self) -> &str {
        if self.label_field.is_empty() { self.fields.first().map_or("", |f| f.key.as_str()) } else { &self.label_field }
    }

    pub fn field(&self, key: &str) -> Option<&FieldDef> {
        self.fields.iter().find(|f| f.key == key)
    }

    /// The form fields for `asset`: the schema's fields plus, when the CSV mapping keeps extra
    /// columns, a text field for each extra property the asset has (three per row, in order).
    pub fn fields_for(&self, asset: &Asset) -> Vec<FieldDef> {
        let mut fields = self.fields.clone();
        let Some(csv) = &self.csv else { return fields };
        let Some(prefix) = csv.extra_prefix.as_deref().filter(|p| !p.is_empty()) else { return fields };
        let extra =
            asset.properties.keys().filter_map(|k| k.strip_prefix(prefix).map(|label| (k, label))).filter(|(k, _)| self.field(k).is_none());
        for (i, (key, label)) in extra.enumerate() {
            fields.push(FieldDef {
                key: key.to_string(),
                label: label.to_string(),
                field_type: FieldType::Text,
                required: false,
                options: Vec::new(),
                group: csv.extra_group.clone(),
                row: 1 + (i / 3) as u32,
                weight: 1.0,
                min: None,
                max: None,
            });
        }
        fields
    }
}

fn check_defects(defects: &[DefectDef], slots: &HashSet<&str>) -> Result<(), SchemaError> {
    if defects.len() > MAX_DEFECTS {
        return Err(SchemaError::TooMany { what: "defect types", limit: MAX_DEFECTS });
    }
    let mut ids = HashSet::new();
    for d in defects {
        let bad = |problem| Err(SchemaError::BadDefect { id: d.id.to_string(), problem });
        if d.id.is_unclassified() {
            return bad("this id is reserved for findings carried over from form values");
        }
        if !ids.insert(&d.id) {
            return bad("appears more than once");
        }
        if d.label.trim().is_empty() {
            return bad("no label");
        }
        if d.slots.len() > MAX_SLOTS {
            return Err(SchemaError::TooMany { what: "slots on a defect type", limit: MAX_SLOTS });
        }
        if d.slots.iter().any(|s| !slots.contains(s.as_str())) {
            return bad("names an image slot the schema doesn't have");
        }
    }
    Ok(())
}

fn check_finding_fields(derived: &[DerivedField], keys: &HashSet<&str>) -> Result<(), SchemaError> {
    let bad = |m: String| Err(SchemaError::BadFindingField(m));
    if derived.len() > MAX_FIELDS {
        return Err(SchemaError::TooMany { what: "finding fields", limit: MAX_FIELDS });
    }
    let mut seen = HashSet::new();
    for d in derived {
        if !keys.contains(d.key.as_str()) || core_fields::is_coordinate(&d.key) {
            return bad(format!("\"{}\" isn't a form field that findings can fill", d.key));
        }
        if !seen.insert(d.key.as_str()) {
            return bad(format!("\"{}\" is listed more than once", d.key));
        }
        if let Derivation::Max { measure } = &d.from
            && !crate::finding::is_measure_key(measure)
        {
            return bad(format!("\"{}\": \"{measure}\" isn't a measurement name", d.key));
        }
    }
    Ok(())
}

fn check_csv(csv: &CsvProfile, keys: &HashSet<&str>) -> Result<(), SchemaError> {
    let bad = |m: String| Err(SchemaError::BadCsv(m));
    if csv.latitude.trim().is_empty() || csv.longitude.trim().is_empty() {
        return bad("latitude and longitude headers are needed".into());
    }
    let mut headers = HashSet::new();
    let all_headers = csv
        .columns
        .iter()
        .map(|c| c.header.as_str())
        .chain([csv.latitude.as_str(), csv.longitude.as_str()])
        .chain(csv.ignore.iter().map(String::as_str));
    for h in all_headers {
        let norm = h.trim().to_lowercase();
        if norm.is_empty() {
            return bad("a column has an empty header".into());
        }
        if !headers.insert(norm) {
            return bad(format!("header \"{h}\" is listed more than once"));
        }
    }
    let mut mapped = HashSet::new();
    for c in &csv.columns {
        if core_fields::is_coordinate(&c.key) {
            return bad(format!("column \"{}\" maps to a coordinate; use latitude/longitude instead", c.header));
        }
        if !mapped.insert(c.key.as_str()) {
            return bad(format!("two columns map to \"{}\"", c.key));
        }
        if c.key.trim().is_empty() {
            return bad(format!("column \"{}\" has no key", c.header));
        }
    }
    for r in &csv.required {
        if !mapped.contains(r.as_str()) {
            return bad(format!("required key \"{r}\" has no column"));
        }
    }
    // Columns may fill hidden properties (report type, fault status), so a mapped key need not be
    // a form field; but a form field's key must not collide with the extra-column prefix.
    if let Some(prefix) = csv.extra_prefix.as_deref().filter(|p| !p.is_empty())
        && let Some(k) = keys.iter().find(|k| k.starts_with(prefix))
    {
        return bad(format!("field \"{k}\" starts with the extra-column prefix \"{prefix}\""));
    }
    Ok(())
}

fn check_severity(rule: &SeverityRule, keys: &HashSet<&str>) -> Result<(), SchemaError> {
    let t = rule.thresholds;
    let all = [t.low, t.medium, t.high, t.critical];
    if all.iter().any(|v| !v.is_finite()) {
        return Err(SchemaError::BadSeverity("thresholds must be finite".into()));
    }
    if !(t.low <= t.medium && t.medium <= t.high && t.high <= t.critical) {
        return Err(SchemaError::BadSeverity("thresholds must not decrease".into()));
    }
    if rule.measured.is_some() != rule.reference.is_some() {
        return Err(SchemaError::BadSeverity("measured and reference go together".into()));
    }
    for k in [Some(&rule.rise), rule.measured.as_ref(), rule.reference.as_ref()].into_iter().flatten() {
        if !keys.contains(k.as_str()) {
            return Err(SchemaError::BadSeverity(format!("\"{k}\" is not a field")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::{AssetDraft, Properties};

    const MINIMAL: &str = r#"{"schemaVersion":1,"id":"x","name":"X","version":"1","fields":[{"key":"a","label":"A","type":"text"}]}"#;

    fn with(edit: impl FnOnce(&mut serde_json::Value)) -> Result<DomainSchema, SchemaError> {
        let mut v: serde_json::Value = serde_json::from_str(MINIMAL).expect("json");
        edit(&mut v);
        DomainSchema::from_json(&v.to_string())
    }

    #[test]
    fn minimal_schema_is_valid_and_round_trips() {
        let s = DomainSchema::from_json(MINIMAL).expect("valid");
        assert_eq!(DomainSchema::from_json(&s.to_json()), Ok(s));
    }

    #[test]
    fn broken_schemas_are_refused_with_a_reason() {
        assert!(matches!(DomainSchema::from_json("{"), Err(SchemaError::Json(_))));
        assert!(matches!(with(|v| v["schemaVersion"] = 99.into()), Err(SchemaError::UnsupportedVersion { found: 99 })));
        assert!(matches!(with(|v| v["fields"] = serde_json::json!([])), Err(SchemaError::Missing("fields"))));
        assert!(matches!(
            with(|v| v["fields"] = serde_json::json!([{"key":"a","label":"A","type":"text"},{"key":"a","label":"B","type":"text"}])),
            Err(SchemaError::DuplicateKey(_))
        ));
        assert!(matches!(with(|v| v["fields"][0]["type"] = "dropdown".into()), Err(SchemaError::BadField { .. })));
        assert!(matches!(with(|v| v["fields"][0]["weight"] = (-1).into()), Err(SchemaError::BadField { .. })));
        assert!(matches!(with(|v| v["fields"][0]["min"] = 1.into()), Err(SchemaError::BadField { .. })));
        assert!(matches!(with(|v| v["fields"][0]["unknown"] = 1.into()), Err(SchemaError::Json(_))));
        assert!(matches!(
            with(|v| v["severity"] = serde_json::json!({"rise":"a","thresholds":{"low":5,"medium":1,"high":2,"critical":3}})),
            Err(SchemaError::BadSeverity(_))
        ));
        assert!(matches!(
            with(|v| v["csv"] =
                serde_json::json!({"columns":[{"header":"A","key":"a"}],"required":["b"],"latitude":"Lat","longitude":"Lon"})),
            Err(SchemaError::BadCsv(_))
        ));
        assert!(matches!(
            with(|v| v["csv"] = serde_json::json!({"columns":[{"header":"Lat","key":"a"}],"latitude":"lat","longitude":"Lon"})),
            Err(SchemaError::BadCsv(_))
        ));
        assert_eq!(DomainSchema::from_json(&" ".repeat(MAX_SCHEMA_BYTES + 1)), Err(SchemaError::TooLarge));
    }

    #[test]
    fn defect_types_and_finding_fields_are_checked() {
        let defects = |d: serde_json::Value| with(|v| v["defects"] = d);
        assert!(defects(serde_json::json!([{"id":"bird_nest","label":"Bird nest","severity":"low"}])).is_ok());
        assert!(matches!(defects(serde_json::json!([{"id":"Bird Nest","label":"B"}])), Err(SchemaError::Json(_))));
        assert!(matches!(
            defects(serde_json::json!([{"id":"a","label":"A"},{"id":"a","label":"B"}])),
            Err(SchemaError::BadDefect { problem: "appears more than once", .. })
        ));
        assert!(matches!(defects(serde_json::json!([{"id":"unclassified","label":"U"}])), Err(SchemaError::BadDefect { .. })));
        assert!(matches!(defects(serde_json::json!([{"id":"a","label":" "}])), Err(SchemaError::BadDefect { .. })));
        assert!(matches!(defects(serde_json::json!([{"id":"a","label":"A","slots":["NOPE"]}])), Err(SchemaError::BadDefect { .. })));

        let derived = |d: serde_json::Value| with(|v| v["findingFields"] = d);
        assert!(derived(serde_json::json!([{"key":"a","from":"max","measure":"maxC"}])).is_ok());
        assert!(matches!(derived(serde_json::json!([{"key":"b","from":"defects"}])), Err(SchemaError::BadFindingField(_))));
        assert!(matches!(
            derived(serde_json::json!([{"key":"a","from":"notes"},{"key":"a","from":"severity"}])),
            Err(SchemaError::BadFindingField(_))
        ));
        assert!(matches!(derived(serde_json::json!([{"key":"a","from":"max","measure":"max C"}])), Err(SchemaError::BadFindingField(_))));
        assert!(matches!(derived(serde_json::json!([{"key":"a","from":"guess"}])), Err(SchemaError::Json(_))));
    }

    #[test]
    fn telecom_lists_defect_types_and_derives_its_fault_fields() {
        let s = crate::builtin_schema("com.geo.telecom").expect("listed").expect("valid");
        assert_eq!(s.schema_version, SCHEMA_VERSION);
        let hot = DefectId::parse("hotspot.joint").expect("valid");
        assert_eq!(s.defect(&hot).map(|d| d.slots.clone()), Some(vec!["THERMAL".to_string()]));
        assert_eq!(s.defect_label(&DefectId::unclassified()), "Unclassified");
        let gone = DefectId::parse("retired.type").expect("valid");
        assert_eq!(s.defect_label(&gone), "retired.type");
        let keys: Vec<&str> = s.finding_fields.iter().map(|d| d.key.as_str()).collect();
        assert_eq!(keys, ["faultDescription", "faultTemp", "riseTemp"]);
    }

    #[test]
    fn extra_columns_become_fields_three_per_row() {
        let s = with(|v| {
            v["csv"] = serde_json::json!({"columns":[{"header":"A","key":"a"}],"latitude":"Lat","longitude":"Lon","extraPrefix":"load.","extraGroup":"Load"})
        })
        .expect("valid");
        let props: Properties = [("a", "1"), ("load.C1", "x"), ("load.C2", "y"), ("load.C3", "z"), ("load.C4", "w")].into_iter().collect();
        let asset = Asset::from_draft("1", "x", 0, AssetDraft { location: None, properties: props });
        let fields = s.fields_for(&asset);
        let extra: Vec<_> = fields.iter().skip(1).map(|f| (f.label.as_str(), f.row, f.group.as_str())).collect();
        assert_eq!(extra, vec![("C1", 1, "Load"), ("C2", 1, "Load"), ("C3", 1, "Load"), ("C4", 2, "Load")]);
    }
}
