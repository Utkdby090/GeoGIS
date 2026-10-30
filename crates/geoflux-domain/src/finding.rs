//! Findings: what is wrong with an asset, as records rather than form text.
//!
//! A [`Finding`] says which photo ([`PhotoId`]), where on it ([`Region`]), what ([`DefectId`] from
//! the schema's taxonomy), how bad ([`Severity`]), what was measured, who or what found it
//! ([`FindingSource`]), how sure it was, and whether a person has decided on it
//! ([`FindingStatus`]). An engineer's finding is confirmed when made; a rule's or model's finding
//! is only *proposed* until a person confirms, edits or rejects it, and only confirmed and edited
//! findings count towards the asset's severity, its form fields and the report.
//!
//! So the engineer's work today and a model's work later go through one record and one review
//! step, and every confirmed finding with a photo and a defect type is a labelled example.
//!
//! [`FindingSet`] holds one layer's findings and the inspection runs that made them, and is the
//! only way to change them, so its rules hold however it is used: ids are unique, a run of one
//! inspector version on one photo happens once, a model's original proposal is kept when a person
//! edits it, and nothing grows without bound. Time is passed in (Unix milliseconds), never read.

use std::collections::{BTreeMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::annotation::{Annotation, Color, MAX_PEN_POINTS, MAX_TEXT_CHARS, NormPoint, Shape};
use crate::asset::{Properties, Severity};
use crate::defect::DefectId;
use crate::photo_id::PhotoId;
use crate::schema::{Derivation, DomainSchema};

/// The stored format of a [`FindingSet`]. Higher numbers come from a newer GeoFlux.
pub const FINDINGS_VERSION: u32 = 1;
/// Most findings in one layer (a 300-tower line at ~50 findings a tower is 15,000).
pub const MAX_FINDINGS: usize = 100_000;
/// Most findings one inspector run may add on one photo; more are dropped and counted.
pub const MAX_PER_RUN: usize = 200;
/// Most inspection runs remembered in one layer.
pub const MAX_RUNS: usize = 500_000;
/// Longest note, in characters.
pub const MAX_NOTE_CHARS: usize = 2_000;
/// Longest name (an asset label, a user, an inspector id or version), in characters.
pub const MAX_NAME_CHARS: usize = 200;
/// Most measurements on one finding.
pub const MAX_MEASUREMENTS: usize = 32;
/// Most corners of a polygon region.
pub const MAX_POLYGON_POINTS: usize = 256;

/// Measurement names that GeoFlux's own inspectors and the built-in schemas use.
pub mod measure {
    /// The hottest temperature in the region, °C.
    pub const MAX_C: &str = "maxC";
    /// The rise over the reference (ambient or a healthy component), °C.
    pub const RISE_C: &str = "riseC";
    /// The reference temperature the rise is over, °C.
    pub const REFERENCE_C: &str = "referenceC";
}

/// Whether `key` can name a measurement: 1 to 32 ASCII letters and digits, starting with a letter.
pub fn is_measure_key(key: &str) -> bool {
    (1..=32).contains(&key.len())
        && key.bytes().next().is_some_and(|b| b.is_ascii_alphabetic())
        && key.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// A finding's id: 1 to 64 ASCII letters, digits, `-` and `_`. Ids are made by
/// [`FindingId::derived`], so the same input always gives the same id.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct FindingId(String);

impl FindingId {
    pub fn parse(text: &str) -> Option<Self> {
        let ok = (1..=64).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        ok.then(|| FindingId(text.to_string()))
    }

    /// `f-` and 32 hex digits of SHA-256 over the parts (each ended by a zero byte, so
    /// `["ab", "c"]` and `["a", "bc"]` differ).
    pub fn derived(parts: &[&str]) -> Self {
        let mut h = Sha256::new();
        for p in parts {
            h.update(p.as_bytes());
            h.update([0]);
        }
        let mut text = String::with_capacity(34);
        text.push_str("f-");
        for b in &h.finalize()[..16] {
            let _ = fmt::Write::write_fmt(&mut text, format_args!("{b:02x}"));
        }
        FindingId(text)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FindingId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for FindingId {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        FindingId::parse(&s).ok_or_else(|| format!("'{}' isn't a finding id", s.chars().take(80).collect::<String>()))
    }
}

impl From<FindingId> for String {
    fn from(id: FindingId) -> String {
        id.0
    }
}

/// Where on the photo a finding is, as fractions of the photo's width and height (0 to 1 from
/// the top left, after EXIF orientation), like [`crate::annotation`]. They fit any size of the
/// photo and export straight to COCO/YOLO boxes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Region {
    /// The whole photo, or no photo.
    #[default]
    Whole,
    Point {
        at: NormPoint,
    },
    /// A box with corners `a` and `b` (any two opposite corners).
    Box {
        a: NormPoint,
        b: NormPoint,
    },
    Polygon {
        points: Vec<NormPoint>,
    },
}

impl Region {
    /// The region with points kept on the photo, or why it can't be used.
    pub fn checked(self) -> Result<Region, FindingError> {
        let fix = |p: NormPoint| -> Result<NormPoint, FindingError> {
            if p.x.is_finite() && p.y.is_finite() {
                Ok(NormPoint::new(p.x.clamp(0.0, 1.0), p.y.clamp(0.0, 1.0)))
            } else {
                Err(FindingError::BadRegion("a position isn't a number"))
            }
        };
        Ok(match self {
            Region::Whole => Region::Whole,
            Region::Point { at } => Region::Point { at: fix(at)? },
            Region::Box { a, b } => {
                let (a, b) = (fix(a)?, fix(b)?);
                if a.x == b.x || a.y == b.y {
                    return Err(FindingError::BadRegion("a box needs two different corners"));
                }
                Region::Box { a, b }
            }
            Region::Polygon { points } => {
                if !(3..=MAX_POLYGON_POINTS).contains(&points.len()) {
                    return Err(FindingError::BadRegion("a polygon needs 3 to 256 corners"));
                }
                let polygon = Region::Polygon { points: points.into_iter().map(fix).collect::<Result<_, _>>()? };
                if polygon.bbox().is_none() {
                    return Err(FindingError::BadRegion("a polygon needs an area"));
                }
                polygon
            }
        })
    }

    /// The bounding box as `[x, y, width, height]` fractions; `None` for the whole photo and a point.
    pub fn bbox(&self) -> Option<[f32; 4]> {
        let pts: &[NormPoint] = match self {
            Region::Whole | Region::Point { .. } => return None,
            Region::Box { a, b } => &[*a, *b],
            Region::Polygon { points } => points,
        };
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in pts {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        (x1 > x0 && y1 > y0).then_some([x0, y0, x1 - x0, y1 - y0])
    }
}

/// Who or what made a finding. An inspector is named by id and version, so findings from
/// different versions of a model can be told apart and compared.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FindingSource {
    /// A person. `user` is their account (email or id) when signed in, else empty.
    Engineer {
        #[serde(default)]
        user: String,
    },
    /// A fixed rule (the thermal hottest-point and ΔT rating).
    Rule { inspector: String, version: String },
    /// A trained model.
    Model { inspector: String, version: String },
}

impl FindingSource {
    /// The inspector's `(id, version)`; `None` for an engineer.
    pub fn inspector(&self) -> Option<(&str, &str)> {
        match self {
            FindingSource::Engineer { .. } => None,
            FindingSource::Rule { inspector, version } | FindingSource::Model { inspector, version } => Some((inspector, version)),
        }
    }
}

/// Where a finding is in review. Only [`FindingStatus::counts`] findings reach the form and report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FindingStatus {
    /// Made by a rule or model; no person has decided yet.
    Proposed,
    /// A person made it, or agreed with a proposal as it was.
    Confirmed,
    /// A person agreed with a proposal after changing it (the original is kept).
    Edited,
    /// A person said it is wrong. Kept, never shown in reports: a rejected proposal is a
    /// negative example for training.
    Rejected,
}

impl FindingStatus {
    pub fn counts(self) -> bool {
        matches!(self, FindingStatus::Confirmed | FindingStatus::Edited)
    }
}

/// Who decided on a finding, and when (Unix milliseconds).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    #[serde(default)]
    pub by: String,
    pub at: i64,
}

/// What was seen on one photo: what an inspector returns and what an engineer enters.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Observation {
    #[serde(default)]
    pub region: Region,
    pub defect: Option<DefectId>,
    /// `None`: the defect type's default severity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub measurements: BTreeMap<String, f64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// How sure an inspector is, 0 to 1. Engineers leave it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
}

/// One finding. Change it only through [`FindingSet`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub id: FindingId,
    /// The asset's label (a tower number).
    pub asset: String,
    /// The photo it is on; `None` for a finding without one (carried over from form values).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub photo: Option<PhotoId>,
    /// The image slot of the photo; empty without a photo.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub slot: String,
    #[serde(default)]
    pub region: Region,
    pub defect: DefectId,
    pub severity: Severity,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub measurements: BTreeMap<String, f64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    pub source: FindingSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    pub status: FindingStatus,
    /// What the inspector proposed, kept the first time a person edits its finding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed: Option<Observation>,
    /// When it was made, Unix milliseconds.
    pub created: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided: Option<Decision>,
}

impl Finding {
    /// Its observation part, as it is now.
    pub fn observation(&self) -> Observation {
        Observation {
            region: self.region.clone(),
            defect: Some(self.defect.clone()),
            severity: Some(self.severity),
            measurements: self.measurements.clone(),
            note: self.note.clone(),
            confidence: self.confidence,
        }
    }

    pub fn measurement(&self, key: &str) -> Option<f64> {
        self.measurements.get(key).copied()
    }

    /// Usable as a training label: counted, on a photo, and classed.
    pub fn is_labelled_example(&self) -> bool {
        self.status.counts() && self.photo.is_some() && !self.defect.is_unclassified()
    }
}

/// A change a person makes to a finding; `None` leaves that part as it is.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingEdit {
    #[serde(default)]
    pub region: Option<Region>,
    #[serde(default)]
    pub defect: Option<DefectId>,
    #[serde(default)]
    pub severity: Option<Severity>,
    #[serde(default)]
    pub measurements: Option<BTreeMap<String, f64>>,
    #[serde(default)]
    pub note: Option<String>,
}

/// How one inspector version's run on one photo of one asset ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RunOutcome {
    /// It proposed `count` findings.
    Found { count: u32 },
    /// It looked and found nothing (remembered, so a re-run doesn't look again).
    Nothing,
    /// The photo isn't one it can read (a thermal rule on a photo without thermal data).
    Skipped { reason: String },
    /// It failed; the next run tries again.
    Failed { reason: String },
}

/// One inspector run on one photo of one asset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectionRun {
    pub asset: String,
    pub photo: PhotoId,
    pub inspector: String,
    pub version: String,
    pub at: i64,
    pub outcome: RunOutcome,
}

/// Why an inspector produced no observations for a photo.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InspectError {
    /// Not a photo this inspector reads; recorded so it isn't tried again.
    #[error("not applicable: {0}")]
    NotApplicable(String),
    /// The file couldn't be read or decoded; tried again next run.
    #[error("couldn't read the photo: {0}")]
    Unreadable(String),
    /// The inspector itself failed; tried again next run.
    #[error("{0}")]
    Failed(String),
    /// Stopped by the user; nothing is recorded.
    #[error("cancelled")]
    Cancelled,
}

/// Why a finding or a change to one was refused.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum FindingError {
    #[error("there is no finding {0}")]
    NotFound(String),
    #[error("pick a defect type")]
    NoDefect,
    #[error("\"{0}\" isn't a defect type in this schema")]
    UnknownDefect(String),
    #[error("\"{defect}\" isn't found on {slot} photos")]
    DefectNotForSlot { defect: String, slot: String },
    #[error("the region: {0}")]
    BadRegion(&'static str),
    #[error("a finding without a photo covers the whole asset; it can't have a region")]
    RegionWithoutPhoto,
    #[error("measurement \"{0}\": the name is letters and digits, the value a number")]
    BadMeasurement(String),
    #[error("a finding can have at most {MAX_MEASUREMENTS} measurements")]
    TooManyMeasurements,
    #[error("a note can be at most {MAX_NOTE_CHARS} characters")]
    NoteTooLong,
    #[error("confidence must be between 0 and 1")]
    BadConfidence,
    #[error("{0} is empty or longer than {MAX_NAME_CHARS} characters")]
    BadName(&'static str),
    #[error("this layer already has {MAX_FINDINGS} findings")]
    Full,
    #[error("findings made by an inspector are rejected, not deleted, so the review stays on record")]
    InspectorFindingsAreKept,
    #[error("these findings were saved by a newer GeoFlux (format {0})")]
    NewerFormat(u32),
}

/// An inspector as findings name it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectorInfo {
    /// Stable id, e.g. `geoflux.thermal.hotspot`.
    pub id: String,
    /// Bumped whenever its output can change (new rule constants, new model weights).
    pub version: String,
    pub kind: InspectorKind,
    /// What people read, e.g. "Thermal hot spots".
    pub label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InspectorKind {
    Rule,
    Model,
}

impl InspectorInfo {
    pub fn source(&self) -> FindingSource {
        let (inspector, version) = (self.id.clone(), self.version.clone());
        match self.kind {
            InspectorKind::Rule => FindingSource::Rule { inspector, version },
            InspectorKind::Model => FindingSource::Model { inspector, version },
        }
    }

    fn check(&self) -> Result<(), FindingError> {
        name(&self.id, "the inspector id")?;
        name(&self.version, "the inspector version")?;
        Ok(())
    }
}

/// What [`FindingSet::record_run`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunReport {
    /// This inspector version had already run on this photo; nothing changed.
    pub already_done: bool,
    pub added: usize,
    /// Undecided proposals of an older version of the same inspector, replaced by this run.
    pub replaced: usize,
    /// Observations that broke the rules (unknown defect type, bad region…), with the first reason.
    pub invalid: usize,
    pub first_invalid: Option<String>,
    /// Observations past [`MAX_PER_RUN`].
    pub over_limit: usize,
    /// How the run ended, as recorded; `None` when it was cancelled or already done (nothing recorded).
    pub outcome: Option<RunOutcome>,
}

/// One asset's findings in the order they matter: counted ones worst first.
#[derive(Debug, Default)]
pub struct AssetFindings<'a> {
    /// Confirmed and edited findings, worst severity first, then oldest first.
    pub counted: Vec<&'a Finding>,
    /// Proposals waiting for a person.
    pub pending: usize,
    pub rejected: usize,
}

impl AssetFindings<'_> {
    /// The asset's severity: its worst counted finding's, `None` without one. Proposals and
    /// rejected findings never count.
    pub fn severity(&self) -> Severity {
        self.counted.first().map_or(Severity::None, |f| f.severity)
    }

    /// The values of the schema's `findingFields` for this asset, as `(field key, text)`. A field
    /// with nothing to show is `""`, so stale text is cleared.
    pub fn form_values(&self, schema: &DomainSchema) -> Vec<(String, String)> {
        schema
            .finding_fields
            .iter()
            .map(|d| {
                let text = match &d.from {
                    Derivation::Defects => join(self.counted.iter().map(|f| {
                        let label = schema.defect_label(&f.defect);
                        match (f.defect.is_unclassified(), f.note.is_empty()) {
                            (true, false) => f.note.clone(),
                            (false, false) => format!("{label} ({})", f.note),
                            _ => label.to_string(),
                        }
                    })),
                    Derivation::Notes => join(self.counted.iter().filter(|f| !f.note.is_empty()).map(|f| f.note.clone())),
                    Derivation::Max { measure } => self
                        .counted
                        .iter()
                        .filter_map(|f| f.measurement(measure))
                        .fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v))))
                        .map_or(String::new(), |v| format!("{v:.1}")),
                    Derivation::Severity => {
                        if self.counted.is_empty() {
                            String::new()
                        } else {
                            self.severity().as_str().to_string()
                        }
                    }
                };
                (d.key.clone(), text)
            })
            .collect()
    }
}

/// The parts in first-seen order, a repeated one once with its count ("Bird nest ×2").
fn join(parts: impl Iterator<Item = String>) -> String {
    let mut counted: Vec<(String, usize)> = Vec::new();
    for p in parts {
        match counted.iter_mut().find(|(q, _)| *q == p) {
            Some((_, n)) => *n += 1,
            None => counted.push((p, 1)),
        }
    }
    counted.into_iter().map(|(p, n)| if n > 1 { format!("{p} ×{n}") } else { p }).collect::<Vec<_>>().join("; ")
}

fn name(text: &str, what: &'static str) -> Result<(), FindingError> {
    if text.trim().is_empty() || text.chars().count() > MAX_NAME_CHARS { Err(FindingError::BadName(what)) } else { Ok(()) }
}

/// An observation made safe to store: region on the photo, a defect type the schema has and the
/// slot allows, finite measurements, a bounded note and a confidence between 0 and 1.
fn checked(o: Observation, photo: bool, slot: &str, schema: &DomainSchema) -> Result<(Observation, DefectId, Severity), FindingError> {
    let region = o.region.checked()?;
    if !photo && region != Region::Whole {
        return Err(FindingError::RegionWithoutPhoto);
    }
    let defect = o.defect.ok_or(FindingError::NoDefect)?;
    let default = if defect.is_unclassified() {
        Severity::None
    } else {
        let def = schema.defect(&defect).ok_or_else(|| FindingError::UnknownDefect(defect.to_string()))?;
        if !def.applies_to(slot) {
            return Err(FindingError::DefectNotForSlot { defect: defect.to_string(), slot: slot.to_string() });
        }
        def.severity
    };
    if o.measurements.len() > MAX_MEASUREMENTS {
        return Err(FindingError::TooManyMeasurements);
    }
    if let Some((k, _)) = o.measurements.iter().find(|(k, v)| !is_measure_key(k) || !v.is_finite()) {
        return Err(FindingError::BadMeasurement(k.chars().take(40).collect()));
    }
    let note = o.note.trim().to_string();
    if note.chars().count() > MAX_NOTE_CHARS {
        return Err(FindingError::NoteTooLong);
    }
    if o.confidence.is_some_and(|c| !(c.is_finite() && (0.0..=1.0).contains(&c))) {
        return Err(FindingError::BadConfidence);
    }
    let severity = o.severity.unwrap_or(default);
    Ok((Observation { region, defect: Some(defect.clone()), severity: Some(severity), note, ..o }, defect, severity))
}

/// One layer's findings and inspection runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FindingSet {
    version: u32,
    #[serde(default)]
    findings: Vec<Finding>,
    #[serde(default)]
    runs: Vec<InspectionRun>,
    /// Whether form values typed before findings existed have been carried over (once per layer).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    migrated: bool,
}

impl Default for FindingSet {
    fn default() -> Self {
        Self { version: FINDINGS_VERSION, findings: Vec::new(), runs: Vec::new(), migrated: false }
    }
}

impl FindingSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes a set read from a project file safe to use: refuses a newer format, and drops
    /// findings with a bad region, non-finite numbers or a repeated id, and anything past the
    /// limits. Returns how many entries were dropped. Defect types the schema no longer lists
    /// are kept (the schema may have been edited since); they show as their id.
    pub fn sanitize(&mut self) -> Result<usize, FindingError> {
        if self.version > FINDINGS_VERSION {
            return Err(FindingError::NewerFormat(self.version));
        }
        self.version = FINDINGS_VERSION;
        let before = self.findings.len() + self.runs.len();
        let mut ids = HashSet::new();
        self.findings.retain_mut(|f| {
            let Ok(region) = std::mem::take(&mut f.region).checked() else { return false };
            f.region = region;
            f.photo.is_some() || f.region == Region::Whole
        });
        self.findings.retain(|f| {
            ids.insert(f.id.clone())
                && name(&f.asset, "").is_ok()
                && f.measurements.len() <= MAX_MEASUREMENTS
                && f.measurements.iter().all(|(k, v)| is_measure_key(k) && v.is_finite())
                && f.confidence.is_none_or(|c| c.is_finite() && (0.0..=1.0).contains(&c))
                && f.note.chars().count() <= MAX_NOTE_CHARS
        });
        self.findings.truncate(MAX_FINDINGS);
        self.runs.truncate(MAX_RUNS);
        Ok(before - self.findings.len() - self.runs.len())
    }

    /// Whether this build can change the set: false for one saved by a newer GeoFlux, which is
    /// kept as it is so nothing this version can't read is lost.
    pub fn is_writable(&self) -> bool {
        self.version <= FINDINGS_VERSION
    }

    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }

    pub fn runs(&self) -> &[InspectionRun] {
        &self.runs
    }

    pub fn is_empty(&self) -> bool {
        self.findings.is_empty() && self.runs.is_empty()
    }

    pub fn get(&self, id: &FindingId) -> Option<&Finding> {
        self.findings.iter().find(|f| f.id == *id)
    }

    /// The asset's findings, counted ones worst first.
    pub fn for_asset(&self, asset: &str) -> AssetFindings<'_> {
        let mut out = AssetFindings::default();
        for f in self.findings.iter().filter(|f| f.asset == asset) {
            match f.status {
                FindingStatus::Proposed => out.pending += 1,
                FindingStatus::Rejected => out.rejected += 1,
                _ => out.counted.push(f),
            }
        }
        out.counted.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.created.cmp(&b.created)).then(a.id.cmp(&b.id)));
        out
    }

    /// Every asset's findings, in one pass (what [`FindingSet::for_asset`] gives for each).
    pub fn by_asset(&self) -> std::collections::BTreeMap<&str, AssetFindings<'_>> {
        let mut out: std::collections::BTreeMap<&str, AssetFindings<'_>> = std::collections::BTreeMap::new();
        for f in &self.findings {
            let a = out.entry(f.asset.as_str()).or_default();
            match f.status {
                FindingStatus::Proposed => a.pending += 1,
                FindingStatus::Rejected => a.rejected += 1,
                _ => a.counted.push(f),
            }
        }
        for a in out.values_mut() {
            a.counted.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.created.cmp(&b.created)).then(a.id.cmp(&b.id)));
        }
        out
    }

    /// Findings on one photo of one asset, in the order they were made.
    pub fn on_photo<'a>(&'a self, asset: &'a str, photo: &'a PhotoId) -> impl Iterator<Item = &'a Finding> + 'a {
        self.findings.iter().filter(move |f| f.asset == asset && f.photo.as_ref() == Some(photo))
    }

    /// Confirmed and edited findings on photos with a defect type: the training set.
    pub fn labelled_examples(&self) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(|f| f.is_labelled_example())
    }

    /// Whether this inspector version still has to look at this photo of this asset: it hasn't
    /// run there, or its last run there failed.
    pub fn needs_run(&self, asset: &str, photo: &PhotoId, inspector: &InspectorInfo) -> bool {
        !self.runs.iter().any(|r| {
            r.asset == asset
                && r.photo == *photo
                && r.inspector == inspector.id
                && r.version == inspector.version
                && !matches!(r.outcome, RunOutcome::Failed { .. })
        })
    }

    /// An engineer's finding, confirmed as made. Returns its id.
    pub fn add(
        &mut self,
        asset: &str,
        photo: Option<PhotoId>,
        slot: &str,
        observation: Observation,
        user: &str,
        now: i64,
        schema: &DomainSchema,
    ) -> Result<FindingId, FindingError> {
        name(asset, "the asset")?;
        if self.findings.len() >= MAX_FINDINGS {
            return Err(FindingError::Full);
        }
        let slot = if photo.is_some() { slot } else { "" };
        let (o, defect, severity) = checked(observation, photo.is_some(), slot, schema)?;
        let photo_text = photo.as_ref().map_or("", PhotoId::as_str);
        let stamp = now.to_string();
        let mut n = 0u32;
        let id = loop {
            let id = FindingId::derived(&["engineer", user, asset, photo_text, &stamp, &n.to_string()]);
            if self.get(&id).is_none() {
                break id;
            }
            n += 1;
        };
        self.findings.push(Finding {
            id: id.clone(),
            asset: asset.to_string(),
            photo,
            slot: slot.to_string(),
            region: o.region,
            defect,
            severity,
            measurements: o.measurements,
            note: o.note,
            source: FindingSource::Engineer { user: user.to_string() },
            confidence: None,
            status: FindingStatus::Confirmed,
            proposed: None,
            created: now,
            decided: Some(Decision { by: user.to_string(), at: now }),
        });
        Ok(id)
    }

    /// Records an inspector's result for one photo of one asset. Its observations become
    /// proposals; undecided proposals of an older version of the same inspector on that photo
    /// are replaced, decided ones are kept. Running the same version again changes nothing, a
    /// failure is retried next time, and a cancel records nothing.
    pub fn record_run(
        &mut self,
        asset: &str,
        photo: &PhotoId,
        slot: &str,
        inspector: &InspectorInfo,
        result: Result<Vec<Observation>, InspectError>,
        now: i64,
        schema: &DomainSchema,
    ) -> Result<RunReport, FindingError> {
        inspector.check()?;
        name(asset, "the asset")?;
        let mut report = RunReport::default();
        if !self.needs_run(asset, photo, inspector) {
            report.already_done = true;
            return Ok(report);
        }
        if self.runs.len() >= MAX_RUNS {
            return Err(FindingError::Full);
        }
        if let Ok(o) = &result
            && self.findings.len() + o.len().min(MAX_PER_RUN) > MAX_FINDINGS
        {
            return Err(FindingError::Full);
        }
        let outcome = match result {
            Err(InspectError::Cancelled) => return Ok(report),
            Err(InspectError::NotApplicable(reason)) => RunOutcome::Skipped { reason },
            Err(e @ (InspectError::Unreadable(_) | InspectError::Failed(_))) => RunOutcome::Failed { reason: e.to_string() },
            Ok(observations) => {
                let stale = |f: &Finding| {
                    f.status == FindingStatus::Proposed
                        && f.asset == asset
                        && f.photo.as_ref() == Some(photo)
                        && f.source.inspector().is_some_and(|(id, _)| id == inspector.id)
                };
                let before = self.findings.len();
                self.findings.retain(|f| !stale(f));
                report.replaced = before - self.findings.len();
                // Ids include the photo, so only this photo's findings could clash (they can't while
                // the runs are intact; this keeps ids unique even if a stored file lost its runs).
                let taken: HashSet<FindingId> =
                    self.findings.iter().filter(|f| f.photo.as_ref() == Some(photo)).map(|f| f.id.clone()).collect();
                let source = inspector.source();
                for (i, o) in observations.into_iter().enumerate() {
                    if report.added >= MAX_PER_RUN {
                        report.over_limit += 1;
                        continue;
                    }
                    let (o, defect, severity) = match checked(o, true, slot, schema) {
                        Ok(c) => c,
                        Err(e) => {
                            report.invalid += 1;
                            report.first_invalid.get_or_insert_with(|| e.to_string());
                            continue;
                        }
                    };
                    let id = FindingId::derived(&[asset, photo.as_str(), &inspector.id, &inspector.version, &i.to_string()]);
                    if taken.contains(&id) {
                        continue;
                    }
                    self.findings.push(Finding {
                        id,
                        asset: asset.to_string(),
                        photo: Some(photo.clone()),
                        slot: slot.to_string(),
                        region: o.region,
                        defect,
                        severity,
                        measurements: o.measurements,
                        note: o.note,
                        source: source.clone(),
                        confidence: o.confidence,
                        status: FindingStatus::Proposed,
                        proposed: None,
                        created: now,
                        decided: None,
                    });
                    report.added += 1;
                }
                if report.added == 0 { RunOutcome::Nothing } else { RunOutcome::Found { count: report.added as u32 } }
            }
        };
        // Keep one run per (asset, photo, inspector, version): a retried failure replaces the failure.
        self.runs.retain(|r| !(r.asset == asset && r.photo == *photo && r.inspector == inspector.id && r.version == inspector.version));
        self.runs.push(InspectionRun {
            asset: asset.to_string(),
            photo: photo.clone(),
            inspector: inspector.id.clone(),
            version: inspector.version.clone(),
            at: now,
            outcome: outcome.clone(),
        });
        report.outcome = Some(outcome);
        Ok(report)
    }

    fn find_mut(&mut self, id: &FindingId) -> Result<&mut Finding, FindingError> {
        self.findings.iter_mut().find(|f| f.id == *id).ok_or_else(|| FindingError::NotFound(id.to_string()))
    }

    /// A person agrees with a finding. A changed proposal stays [`FindingStatus::Edited`].
    pub fn confirm(&mut self, id: &FindingId, by: &str, now: i64) -> Result<(), FindingError> {
        let f = self.find_mut(id)?;
        f.status = if f.proposed.is_some() { FindingStatus::Edited } else { FindingStatus::Confirmed };
        f.decided = Some(Decision { by: by.to_string(), at: now });
        Ok(())
    }

    /// A person says a finding is wrong. It is kept (and never counted).
    pub fn reject(&mut self, id: &FindingId, by: &str, now: i64) -> Result<(), FindingError> {
        let f = self.find_mut(id)?;
        f.status = FindingStatus::Rejected;
        f.decided = Some(Decision { by: by.to_string(), at: now });
        Ok(())
    }

    /// A person changes a finding, which also accepts it. The first change to an inspector's
    /// finding keeps what it proposed.
    pub fn edit(&mut self, id: &FindingId, edit: FindingEdit, by: &str, now: i64, schema: &DomainSchema) -> Result<(), FindingError> {
        let f = self.find_mut(id)?;
        let mut o = f.observation();
        if let Some(r) = edit.region {
            o.region = r;
        }
        if let Some(d) = edit.defect {
            o.defect = Some(d);
            // A new type starts at its own default unless a severity is given too.
            o.severity = edit.severity;
        }
        if let Some(s) = edit.severity {
            o.severity = Some(s);
        }
        if let Some(m) = edit.measurements {
            o.measurements = m;
        }
        if let Some(n) = edit.note {
            o.note = n;
        }
        let (o, defect, severity) = checked(o, f.photo.is_some(), &f.slot.clone(), schema)?;
        let machine = f.source.inspector().is_some();
        if machine && f.proposed.is_none() {
            f.proposed = Some(f.observation());
        }
        f.region = o.region;
        f.defect = defect;
        f.severity = severity;
        f.measurements = o.measurements;
        f.note = o.note;
        f.status = if machine { FindingStatus::Edited } else { FindingStatus::Confirmed };
        f.decided = Some(Decision { by: by.to_string(), at: now });
        Ok(())
    }

    /// Deletes an engineer's finding. An inspector's finding is rejected instead, so the review
    /// stays on record (and the inspector doesn't propose it again).
    pub fn remove(&mut self, id: &FindingId) -> Result<Finding, FindingError> {
        let at = self.findings.iter().position(|f| f.id == *id).ok_or_else(|| FindingError::NotFound(id.to_string()))?;
        if self.findings[at].source.inspector().is_some() {
            return Err(FindingError::InspectorFindingsAreKept);
        }
        Ok(self.findings.remove(at))
    }

    /// Carries the layer's fault values typed before findings existed into findings, once: each
    /// asset with fault values gets one confirmed, unclassified finding without a photo, from the
    /// `findingFields` values (a description as its note, numbers as measurements) and the
    /// schema's severity rule. Later calls do nothing, so it is safe on every load, and a carried
    /// finding an engineer deletes stays deleted. Returns the ids made.
    pub fn migrate_layer<'p>(
        &mut self,
        assets: impl IntoIterator<Item = (&'p str, &'p Properties)>,
        schema: &DomainSchema,
        now: i64,
    ) -> Vec<FindingId> {
        if self.migrated {
            return Vec::new();
        }
        self.migrated = true;
        assets.into_iter().filter_map(|(asset, props)| self.migrate_form(asset, props, schema, now)).collect()
    }

    pub fn is_migrated(&self) -> bool {
        self.migrated
    }

    fn migrate_form(&mut self, asset: &str, props: &Properties, schema: &DomainSchema, now: i64) -> Option<FindingId> {
        if name(asset, "").is_err() || self.findings.len() >= MAX_FINDINGS || self.findings.iter().any(|f| f.asset == asset) {
            return None;
        }
        let mut note = Vec::new();
        let mut measurements = BTreeMap::new();
        for d in &schema.finding_fields {
            let value = props.get(&d.key).trim();
            match &d.from {
                Derivation::Defects | Derivation::Notes if !value.is_empty() => note.push(value),
                Derivation::Max { measure } => {
                    if let Some(v) = crate::number(value) {
                        measurements.insert(measure.clone(), v);
                    }
                }
                _ => {}
            }
        }
        let severity = schema.severity.as_ref().map_or(Severity::None, |rule| rule.classify(props));
        if note.is_empty() && measurements.is_empty() && severity == Severity::None {
            return None;
        }
        let note: String = note.join("; ").chars().take(MAX_NOTE_CHARS).collect();
        let id = FindingId::derived(&["migrated", asset]);
        self.findings.push(Finding {
            id: id.clone(),
            asset: asset.to_string(),
            photo: None,
            slot: String::new(),
            region: Region::Whole,
            defect: DefectId::unclassified(),
            severity,
            measurements,
            note,
            source: FindingSource::Engineer { user: String::new() },
            confidence: None,
            status: FindingStatus::Confirmed,
            proposed: None,
            created: now,
            decided: Some(Decision { by: String::new(), at: now }),
        });
        Some(id)
    }
}

/// The colour a finding of `severity` is drawn in, on photos and in reports (the map's colours).
pub fn severity_color(severity: Severity) -> Color {
    Color(match severity {
        Severity::None => [0xF8, 0xF9, 0xFA, 0xFF],
        Severity::Low => [0xFF, 0xD4, 0x3B, 0xFF],
        Severity::Medium => [0xFF, 0x92, 0x2B, 0xFF],
        Severity::High => [0xF0, 0x3E, 0x3E, 0xFF],
        Severity::Critical => [0x86, 0x0F, 0x0F, 0xFF],
    })
}

/// Findings drawn as markup: each region in its severity's colour with the defect's label above
/// it, so a report photo shows what the page's text says. Findings on the whole photo draw nothing.
pub fn findings_markup<'a>(findings: impl IntoIterator<Item = &'a Finding>, schema: &DomainSchema) -> Vec<Annotation> {
    /// Text height and line width, in thousandths of the photo's shorter side.
    const TEXT: f32 = 32.0;
    const LINE: f32 = 6.0;
    /// A point finding's ring radius, as a fraction of the photo.
    const RING: f32 = 0.035;
    let mut out = Vec::new();
    for f in findings {
        let color = severity_color(f.severity);
        let (shape, top_left) = match &f.region {
            Region::Whole => continue,
            Region::Point { at } => {
                let a = NormPoint::new((at.x - RING).max(0.0), (at.y - RING).max(0.0));
                let b = NormPoint::new((at.x + RING).min(1.0), (at.y + RING).min(1.0));
                (Shape::Ellipse { a, b }, a)
            }
            Region::Box { a, b } => {
                let tl = NormPoint::new(a.x.min(b.x), a.y.min(b.y));
                (Shape::Rect { a: *a, b: *b }, tl)
            }
            Region::Polygon { points } => {
                let Some(first) = points.first() else { continue };
                let mut ring: Vec<NormPoint> = points.iter().copied().take(MAX_PEN_POINTS - 1).collect();
                ring.push(*first);
                let tl = ring.iter().fold(*first, |m, p| NormPoint::new(m.x.min(p.x), m.y.min(p.y)));
                (Shape::Pen { points: ring }, tl)
            }
        };
        out.push(Annotation { shape, color, width: LINE });
        let label: String = schema.defect_label(&f.defect).chars().take(MAX_TEXT_CHARS.min(60)).collect();
        // Above the region, or inside its top when it touches the photo's top edge.
        let y = if top_left.y >= 0.05 { top_left.y - 0.045 } else { top_left.y + 0.01 };
        out.push(Annotation { shape: Shape::Text { at: NormPoint::new(top_left.x, y), text: label, size: TEXT }, color, width: LINE });
    }
    out
}

#[cfg(test)]
mod tests;
