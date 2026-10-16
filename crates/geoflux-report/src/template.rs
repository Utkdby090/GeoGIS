//! What a report shows and how it is laid out, as data: one template per schema, stored in the
//! project or a `.gfreport.json` file, so a new tender's report is a file, not code.

use geoflux_domain::DomainSchema;
use geoflux_domain::annotation::Color;
use serde::{Deserialize, Serialize};

use crate::ReportError;

/// Most photo cells on a page (the 2×2 grid).
pub const MAX_PHOTOS: usize = 4;
/// Most field sections, and fields in one.
pub const MAX_SECTIONS: usize = 12;
pub const MAX_SECTION_FIELDS: usize = 40;
/// Most header values ("LINE | TOWER | CKT").
pub const MAX_HEADER: usize = 5;
/// Longest title, caption, footer or company name, in characters.
pub const MAX_TEXT: usize = 200;

/// The paper size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PageSize {
    #[default]
    A4,
    Letter,
}

impl PageSize {
    /// Width and height in PDF points (1/72 inch).
    pub fn points(self) -> (f32, f32) {
        match self {
            PageSize::A4 => (595.28, 841.89),
            PageSize::Letter => (612.0, 792.0),
        }
    }
}

/// One cell of the photo grid: which image slot fills it and the caption under it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PhotoCell {
    /// An image slot id of the schema (`THERMAL`).
    pub slot: String,
    /// Shown under the photo; blank uses the slot's label.
    #[serde(default)]
    pub caption: String,
}

/// A titled block of the field table, laid out as label/value pairs, two pairs to a row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FieldSection {
    pub title: String,
    /// Field keys of the schema, in order. A field with a blank value shows "N/A".
    pub fields: Vec<String>,
    /// Keys whose value takes a whole row (long descriptions).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wide: Vec<String>,
}

/// Who the report is from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Branding {
    /// Printed in the footer; blank uses the asset's `companyName` field when it has one.
    #[serde(default)]
    pub company: String,
    /// A PNG or JPEG file drawn at the top right; blank for none. Read when the report is made.
    #[serde(default)]
    pub logo: String,
    /// The header bar and section headings.
    #[serde(default = "accent")]
    pub accent: Color,
    /// The footer line; `{company}` is replaced.
    #[serde(default = "footer")]
    pub footer: String,
}

impl Default for Branding {
    fn default() -> Self {
        Self { company: String::new(), logo: String::new(), accent: accent(), footer: footer() }
    }
}

fn accent() -> Color {
    Color([0x2B, 0x2F, 0x36, 0xFF])
}

fn footer() -> String {
    "Report created by {company}".into()
}

/// A report layout: GeoFlux's page (title, a dark header bar, a 2×2 photo grid with the previous
/// and next asset, a sectioned field table and a footer), plus severity, a location map and a logo.
///
/// Text with `{key}` tokens takes the asset's values: any field key, `{label}` (the asset's
/// label), `{severity}` (`none` to `critical`) and `{status}` (`FAULT` when the asset is flagged by
/// the schema's severity rule, else `NORMAL`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReportTemplate {
    /// The schema this template is for.
    pub schema_id: String,
    #[serde(default)]
    pub page: PageSize,
    pub title: String,
    /// Field keys whose values fill the header bar, joined with " | ".
    pub header: Vec<String>,
    pub photos: Vec<PhotoCell>,
    pub sections: Vec<FieldSection>,
    /// Show the previous and next asset beside the photos.
    #[serde(default = "yes")]
    pub neighbours: bool,
    /// Show the severity as a coloured badge in the header.
    #[serde(default = "yes")]
    pub severity: bool,
    /// Show a small map of the asset's location (drawn by the app, embedded as an image).
    #[serde(default)]
    pub map: bool,
    #[serde(default)]
    pub branding: Branding,
    /// The PDF's file name.
    #[serde(default = "file_name")]
    pub file_name: String,
    /// The folder a bulk export puts the PDF in; blank for the top of the ZIP.
    #[serde(default)]
    pub folder: String,
    /// Which assets a bulk export includes: only `ready` ones (as GeoFlux does) or all.
    #[serde(default = "yes")]
    pub ready_only: bool,
}

fn yes() -> bool {
    true
}

fn file_name() -> String {
    "{label}_Report.pdf".into()
}

/// Templates that ship with GeoFlux, as `(schema id, JSON)`.
pub const BUILTIN_TEMPLATES: &[(&str, &str)] = &[("com.geo.telecom", include_str!("../templates/telecom.json"))];

impl ReportTemplate {
    pub fn from_json(text: &str) -> Result<Self, ReportError> {
        let t: ReportTemplate = serde_json::from_str(text).map_err(|e| ReportError::Template(e.to_string()))?;
        Ok(t)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// The template for `schema`: the built-in one when there is one, else one made from the
    /// schema (its label field in the header, its first four image slots, one section per field group).
    pub fn for_schema(schema: &DomainSchema) -> Self {
        if let Some(Ok(t)) = BUILTIN_TEMPLATES.iter().find(|(id, _)| *id == schema.id).map(|(_, json)| Self::from_json(json)) {
            return t;
        }
        let mut sections: Vec<FieldSection> = Vec::new();
        for f in &schema.fields {
            match sections.iter_mut().find(|s| s.title == f.group) {
                Some(s) => s.fields.push(f.key.clone()),
                None => sections.push(FieldSection { title: f.group.clone(), fields: vec![f.key.clone()], wide: Vec::new() }),
            }
        }
        sections.truncate(MAX_SECTIONS);
        sections.iter_mut().for_each(|s| s.fields.truncate(MAX_SECTION_FIELDS));
        Self {
            schema_id: schema.id.clone(),
            page: PageSize::A4,
            title: format!("{} Report", schema.name),
            header: vec![schema.label_key().to_string()],
            photos: schema.image_slots.iter().take(MAX_PHOTOS).map(|s| PhotoCell { slot: s.id.clone(), caption: String::new() }).collect(),
            sections,
            neighbours: true,
            severity: schema.severity.is_some(),
            map: false,
            branding: Branding::default(),
            file_name: file_name(),
            folder: String::new(),
            ready_only: true,
        }
    }

    /// Checks the template against its schema: every key and slot exists, and nothing is over a limit.
    pub fn check(&self, schema: &DomainSchema) -> Result<(), ReportError> {
        let bad = |m: String| Err(ReportError::Template(m));
        if self.schema_id != schema.id {
            return bad(format!("it is for schema {}, not {}", self.schema_id, schema.id));
        }
        if self.photos.len() > MAX_PHOTOS {
            return bad(format!("at most {MAX_PHOTOS} photos fit a page"));
        }
        if self.header.len() > MAX_HEADER || self.sections.len() > MAX_SECTIONS {
            return bad(format!("at most {MAX_HEADER} header values and {MAX_SECTIONS} sections"));
        }
        for p in &self.photos {
            if !schema.image_slots.iter().any(|s| s.id == p.slot) {
                return bad(format!("the schema has no image slot {}", p.slot));
            }
        }
        let known = |k: &str| schema.field(k).is_some() || schema.csv.as_ref().is_some_and(|c| c.columns.iter().any(|col| col.key == k));
        for s in &self.sections {
            if s.fields.len() > MAX_SECTION_FIELDS {
                return bad(format!("section {} has more than {MAX_SECTION_FIELDS} fields", s.title));
            }
            if let Some(k) = s.fields.iter().chain(&s.wide).find(|k| !known(k)) {
                return bad(format!("the schema has no field {k}"));
            }
        }
        if let Some(k) = self.header.iter().find(|k| !known(k)) {
            return bad(format!("the schema has no field {k}"));
        }
        let texts = [&self.title, &self.branding.company, &self.branding.footer, &self.file_name, &self.folder];
        if texts.iter().any(|t| t.chars().count() > MAX_TEXT) || self.photos.iter().any(|p| p.caption.chars().count() > MAX_TEXT) {
            return bad(format!("a title, caption, footer or name is longer than {MAX_TEXT} characters"));
        }
        if !self.file_name.to_ascii_lowercase().ends_with(".pdf") {
            return bad("the file name must end in .pdf".into());
        }
        Ok(())
    }
}

/// `text` with each `{key}` replaced by `value(key)`; an unknown or unclosed token is kept as written.
pub fn expand(text: &str, value: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('{') {
        out.push_str(rest.get(..i).unwrap_or_default());
        let after = rest.get(i + 1..).unwrap_or_default();
        match after.find('}').and_then(|j| after.get(..j).map(|k| (j, k))) {
            Some((j, key)) => {
                match value(key) {
                    Some(v) => out.push_str(&v),
                    None => {
                        out.push('{');
                        out.push_str(key);
                        out.push('}');
                    }
                }
                rest = after.get(j + 1..).unwrap_or_default();
            }
            None => {
                out.push_str(rest.get(i..).unwrap_or_default());
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// A file or folder name made safe for every system and for a ZIP: path separators, reserved
/// characters and control characters become `_`, leading dots and trailing dots/spaces go, and the
/// name is at most 120 characters. Never empty.
pub fn safe_name(name: &str) -> String {
    let mapped: String = name.chars().map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '_' } else { c }).take(120).collect();
    let trimmed = mapped.trim_start_matches('.').trim_end_matches(['.', ' ']).trim();
    // Windows reserves these names whatever the extension.
    let stem = trimmed.split('.').next().unwrap_or_default().to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.ends_with(|c: char| c.is_ascii_digit()));
    match (trimmed.is_empty(), reserved) {
        (true, _) => "_".into(),
        (_, true) => format!("_{trimmed}"),
        _ => trimmed.to_string(),
    }
}
