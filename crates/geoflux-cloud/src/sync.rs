//! Project › Sync: sending a survey from GeoGIS to the server.
//!
//! Three steps, each safe to retry over a weak connection:
//!
//! 1. `POST /surveys` with a [`SurveyManifest`]: the towers, their forms and severity, each
//!    photo by SHA-256, and each tower's findings ([`SyncedFinding`], format 2). The server answers with the photos it doesn't have yet. Sending the same
//!    `clientSurveyId` and `revision` again returns the same survey, so a retry never duplicates.
//! 2. `PUT /surveys/{id}/photos/{sha256}` for each missing photo. The server streams the body to a
//!    temporary file while hashing it, refuses it if the hash or size differs, then hands the file
//!    to [`crate::ports::BlobStore`]. Memory use doesn't grow with the photo.
//! 3. `POST /surveys/{id}/commit`. Only then does the survey appear on the map and the dashboard;
//!    if photos are still missing it answers [`crate::ErrorCode::Incomplete`] with the list.
//!
//! Every upload is a new revision: nothing is overwritten, so trends can compare surveys.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use geoflux_domain::annotation::AnnotationSet;
use geoflux_domain::finding::{self, Decision, FindingSource, FindingStatus, Region};
use geoflux_domain::{Asset, DefectId, Finding, FindingId, PartId, Severity};
use serde::{Deserialize, Serialize};

use crate::Sha256;
use crate::error::FieldError;

/// The manifest format this build writes. 2 added each tower's `findings`; the server still
/// reads 1 (an older GeoGIS), which has none.
pub const MANIFEST_FORMAT: u32 = 2;
/// Findings on one tower.
pub const MAX_FINDINGS_PER_ASSET: usize = 2_000;
/// Findings in one upload, the same as one layer holds.
pub const MAX_FINDINGS: usize = finding::MAX_FINDINGS;
/// Towers in one upload. A line has a few hundred to a few thousand.
pub const MAX_ASSETS: usize = 100_000;
pub const MAX_PHOTOS_PER_ASSET: usize = 64;
/// The same limit as `.geox` import.
pub const MAX_PHOTO_BYTES: u64 = geoflux_domain::legacy::MAX_PHOTO_BYTES as u64;
/// The manifest's JSON body; the server refuses a larger one before parsing it.
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_TEXT_CHARS: usize = 200;
pub const MAX_FILE_NAME_CHARS: usize = 255;
const MAX_MARKUP_SHAPES: usize = 500;

/// A photo's format. Only these are stored; the server checks the first bytes match.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PhotoType {
    Jpeg,
    Png,
}

impl PhotoType {
    pub fn content_type(self) -> &'static str {
        match self {
            PhotoType::Jpeg => "image/jpeg",
            PhotoType::Png => "image/png",
        }
    }

    /// Whether `head` (the first bytes of the upload) starts like this format.
    pub fn matches(self, head: &[u8]) -> bool {
        match self {
            PhotoType::Jpeg => head.starts_with(&[0xff, 0xd8, 0xff]),
            PhotoType::Png => head.starts_with(b"\x89PNG\r\n\x1a\n"),
        }
    }
}

/// One photo of one tower, by hash.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoRef {
    /// The schema's image slot (`thermal`, `structure`…).
    pub slot: String,
    pub sha256: Sha256,
    pub bytes: u64,
    #[serde(rename = "type")]
    pub kind: PhotoType,
    /// The original file name, for people; never used as a path.
    pub file_name: String,
    /// Markup drawn on it in GeoGIS (the photo itself is never changed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub markup: Option<AnnotationSet>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SurveyAsset {
    /// The tower as GeoGIS holds it: id, form values, severity, position.
    pub asset: Asset,
    #[serde(default)]
    pub photos: Vec<PhotoRef>,
    /// Its findings, rejected ones included (they are review history and negative examples).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<SyncedFinding>,
}

/// A finding as uploaded: the desktop [`Finding`] with its photo named by the SHA-256 the upload
/// uses (one of the same tower's [`PhotoRef`]s) instead of GeoGIS's own photo id. What an
/// inspector first proposed stays on the desktop.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncedFinding {
    pub id: FindingId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub photo: Option<Sha256>,
    #[serde(default)]
    pub region: Region,
    pub defect: DefectId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<PartId>,
    pub severity: Severity,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub measurements: BTreeMap<String, f64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    pub source: FindingSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    pub status: FindingStatus,
    /// Unix milliseconds, as GeoGIS records them.
    pub created: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided: Option<Decision>,
}

impl SyncedFinding {
    /// `photo` is the SHA-256 of the finding's photo as this upload lists it; `None` when the
    /// finding has no photo or its photo isn't being uploaded (the finding then covers the tower).
    pub fn of(f: &Finding, photo: Option<Sha256>) -> Self {
        let region = if photo.is_some() { f.region.clone() } else { Region::Whole };
        SyncedFinding {
            id: f.id.clone(),
            photo,
            region,
            defect: f.defect.clone(),
            part: f.part.clone(),
            severity: f.severity,
            measurements: f.measurements.clone(),
            note: f.note.clone(),
            source: f.source.clone(),
            confidence: f.confidence,
            status: f.status,
            created: f.created,
            decided: f.decided.clone(),
        }
    }

    /// The problems with this finding as `(field, reason)`; `photos` are its tower's photos.
    fn problems(&self, photos: &[PhotoRef]) -> Vec<(&'static str, &'static str)> {
        let mut out = Vec::new();
        match self.region.clone().checked() {
            Err(_) => out.push(("region", "invalid")),
            Ok(r) if r != self.region => out.push(("region", "out_of_range")),
            Ok(_) => {}
        }
        match &self.photo {
            Some(h) if !photos.iter().any(|p| p.sha256 == *h) => out.push(("photo", "not_listed")),
            None if self.region != Region::Whole => out.push(("region", "needs_photo")),
            _ => {}
        }
        if self.measurements.len() > finding::MAX_MEASUREMENTS
            || self.measurements.iter().any(|(k, v)| !finding::is_measure_key(k) || !v.is_finite())
        {
            out.push(("measurements", "invalid"));
        }
        if self.note.chars().count() > finding::MAX_NOTE_CHARS {
            out.push(("note", "too_long"));
        }
        if self.confidence.is_some_and(|c| !(c.is_finite() && (0.0..=1.0).contains(&c))) {
            out.push(("confidence", "out_of_range"));
        }
        let name_ok = |t: &str| !t.trim().is_empty() && t.chars().count() <= finding::MAX_NAME_CHARS;
        if self.source.inspector().is_some_and(|(id, v)| !name_ok(id) || !name_ok(v)) {
            out.push(("source", "invalid"));
        }
        out
    }
}

/// Step 1's body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SurveyManifest {
    pub format: u32,
    /// Made by GeoGIS once per project and kept in it, so revisions of one survey group together.
    pub client_survey_id: String,
    /// 1 for the first upload of the project, then one more each time.
    pub revision: u32,
    pub project: String,
    #[serde(default)]
    pub line: Option<String>,
    pub schema_id: String,
    /// When the survey was done in the field, if known (seconds since 1970).
    #[serde(default)]
    pub surveyed: Option<i64>,
    pub assets: Vec<SurveyAsset>,
}

fn text_ok(t: &str, max: usize) -> bool {
    let n = t.chars().count();
    n > 0 && n <= max && !t.chars().any(char::is_control)
}

impl SurveyManifest {
    /// Every rule the server applies before storing anything. All problems at once, so the
    /// desktop app can show them together; capped at 100.
    pub fn validate(&self) -> Result<(), Vec<FieldError>> {
        let mut errs: Vec<FieldError> = Vec::new();
        let mut push = |field: String, reason: &str| {
            if errs.len() < 100 {
                errs.push(FieldError::new(field, reason));
            }
        };
        if !(1..=MANIFEST_FORMAT).contains(&self.format) {
            push("format".into(), "unsupported");
        }
        let id_ok = (8..=64).contains(&self.client_survey_id.len())
            && self.client_survey_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if !id_ok {
            push("clientSurveyId".into(), "invalid");
        }
        if self.revision == 0 {
            push("revision".into(), "invalid");
        }
        if !text_ok(&self.project, MAX_TEXT_CHARS) {
            push("project".into(), "invalid");
        }
        if self.line.as_deref().is_some_and(|l| !text_ok(l, MAX_TEXT_CHARS)) {
            push("line".into(), "invalid");
        }
        if !text_ok(&self.schema_id, MAX_TEXT_CHARS) {
            push("schemaId".into(), "invalid");
        }
        if self.assets.is_empty() {
            push("assets".into(), "required");
        }
        if self.assets.len() > MAX_ASSETS {
            push("assets".into(), "too_many");
            return Err(errs);
        }
        if self.assets.iter().map(|a| a.findings.len()).fold(0usize, usize::saturating_add) > MAX_FINDINGS {
            push("assets".into(), "too_many_findings");
            return Err(errs);
        }
        let mut ids: HashSet<&str> = HashSet::with_capacity(self.assets.len());
        for (i, a) in self.assets.iter().enumerate() {
            let at = |f: &str| format!("assets[{i}].{f}");
            if !text_ok(&a.asset.id, MAX_TEXT_CHARS) {
                push(at("id"), "invalid");
            } else if !ids.insert(&a.asset.id) {
                push(at("id"), "duplicate");
            }
            if a.asset.schema_id != self.schema_id {
                push(at("schemaId"), "mismatch");
            }
            if let Some(p) = a.asset.location
                && !(p.lat.is_finite() && p.lon.is_finite() && (-90.0..=90.0).contains(&p.lat) && (-180.0..=180.0).contains(&p.lon))
            {
                push(at("location"), "out_of_range");
            }
            if a.photos.len() > MAX_PHOTOS_PER_ASSET {
                push(at("photos"), "too_many");
                continue;
            }
            if self.format < 2 && !a.findings.is_empty() {
                push(at("findings"), "unsupported");
            }
            if a.findings.len() > MAX_FINDINGS_PER_ASSET {
                push(at("findings"), "too_many");
            } else {
                let mut seen: HashSet<&FindingId> = HashSet::with_capacity(a.findings.len());
                for (k, f) in a.findings.iter().enumerate() {
                    if !seen.insert(&f.id) {
                        push(format!("assets[{i}].findings[{k}].id"), "duplicate");
                    }
                    for (field, reason) in f.problems(&a.photos) {
                        push(format!("assets[{i}].findings[{k}].{field}"), reason);
                    }
                }
            }
            for (j, ph) in a.photos.iter().enumerate() {
                let at = |f: &str| format!("assets[{i}].photos[{j}].{f}");
                if !text_ok(&ph.slot, 64) {
                    push(at("slot"), "invalid");
                }
                if ph.bytes == 0 || ph.bytes > MAX_PHOTO_BYTES {
                    push(at("bytes"), "out_of_range");
                }
                if !text_ok(&ph.file_name, MAX_FILE_NAME_CHARS) {
                    push(at("fileName"), "invalid");
                }
                if ph.markup.as_ref().is_some_and(|m| m.len() > MAX_MARKUP_SHAPES) {
                    push(at("markup"), "too_many");
                }
            }
        }
        if errs.is_empty() { Ok(()) } else { Err(errs) }
    }

    /// Each photo once, with the size and type every reference to it must agree on. `Err` names a
    /// hash listed with two different sizes or types (a corrupt manifest).
    pub fn photos(&self) -> Result<Vec<(Sha256, u64, PhotoType)>, Sha256> {
        let mut seen: std::collections::BTreeMap<&Sha256, (u64, PhotoType)> = std::collections::BTreeMap::new();
        for ph in self.assets.iter().flat_map(|a| &a.photos) {
            match seen.insert(&ph.sha256, (ph.bytes, ph.kind)) {
                Some(prev) if prev != (ph.bytes, ph.kind) => return Err(ph.sha256.clone()),
                _ => {}
            }
        }
        Ok(seen.into_iter().map(|(h, (b, k))| (h.clone(), b, k)).collect())
    }

    /// The total photo bytes, each photo counted once, for the plan's storage limit.
    pub fn photo_bytes(&self) -> u64 {
        let mut seen: BTreeSet<&Sha256> = BTreeSet::new();
        self.assets.iter().flat_map(|a| &a.photos).filter(|p| seen.insert(&p.sha256)).map(|p| p.bytes).fold(0u64, u64::saturating_add)
    }
}

/// Step 1's answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SurveyCreated {
    pub survey: crate::SurveyId,
    /// Photos the server still needs, in step 2.
    pub missing: Vec<Sha256>,
    /// True when this revision was already uploaded (a retry); nothing new was stored.
    pub existing: bool,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub(crate) mod tests {
    use super::*;
    use geoflux_domain::GeoPoint;

    pub(crate) fn manifest() -> SurveyManifest {
        let draft = geoflux_domain::AssetDraft { location: None, properties: Default::default() };
        let mut asset = Asset::from_draft("T-001", "com.geo.telecom", 0, draft);
        asset.location = Some(GeoPoint { lat: 25.6, lon: 85.1 });
        let photo = |slot: &str, bytes: &[u8]| PhotoRef {
            slot: slot.into(),
            sha256: Sha256::of(bytes),
            bytes: bytes.len() as u64,
            kind: PhotoType::Jpeg,
            file_name: format!("{slot}.jpg"),
            markup: None,
        };
        let mut second = asset.clone();
        second.id = "T-002".into();
        SurveyManifest {
            format: MANIFEST_FORMAT,
            client_survey_id: "proj-4f1c2a9e".into(),
            revision: 1,
            project: "Patna 132 kV".into(),
            line: Some("Line 4".into()),
            schema_id: "com.geo.telecom".into(),
            surveyed: None,
            assets: vec![
                SurveyAsset { asset, photos: vec![photo("thermal", b"one"), photo("structure", b"two")], findings: Vec::new() },
                // The same photo under another tower: counted and stored once.
                SurveyAsset { asset: second, photos: vec![photo("thermal", b"one")], findings: Vec::new() },
            ],
        }
    }

    #[test]
    fn a_good_manifest_passes_and_round_trips() {
        let m = manifest();
        assert_eq!(m.validate(), Ok(()));
        assert_eq!(m.photos().unwrap().len(), 2);
        assert_eq!(m.photo_bytes(), 6);
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("\"clientSurveyId\"") && json.contains("\"type\":\"jpeg\""));
        assert_eq!(serde_json::from_str::<SurveyManifest>(&json).unwrap(), m);
    }

    #[test]
    fn every_problem_is_reported_by_field() {
        let mut m = manifest();
        m.client_survey_id = "../../etc".into();
        m.revision = 0;
        m.assets[1].asset.id = "T-001".into();
        m.assets[0].asset.location = Some(GeoPoint { lat: f64::NAN, lon: 0.0 });
        m.assets[0].photos[0].bytes = MAX_PHOTO_BYTES + 1;
        m.assets[0].photos[1].file_name = "a\nb".into();
        let fields: Vec<String> = m.validate().unwrap_err().into_iter().map(|e| e.field).collect();
        for f in [
            "clientSurveyId",
            "revision",
            "assets[1].id",
            "assets[0].location",
            "assets[0].photos[0].bytes",
            "assets[0].photos[1].fileName",
        ] {
            assert!(fields.iter().any(|x| x == f), "{f} in {fields:?}");
        }
    }

    #[test]
    fn one_hash_with_two_sizes_is_refused() {
        let mut m = manifest();
        m.assets[1].photos[0].bytes = 99;
        assert_eq!(m.photos(), Err(Sha256::of(b"one")));
    }

    #[test]
    fn photo_types_check_the_first_bytes() {
        assert!(PhotoType::Jpeg.matches(&[0xff, 0xd8, 0xff, 0xe0]));
        assert!(!PhotoType::Jpeg.matches(b"\x89PNG\r\n\x1a\n"));
        assert!(PhotoType::Png.matches(b"\x89PNG\r\n\x1a\n...."));
    }

    pub(crate) fn finding(
        id: &str,
        photo: Option<&[u8]>,
        defect: &str,
        part: Option<&str>,
        severity: Severity,
        status: FindingStatus,
    ) -> SyncedFinding {
        use geoflux_domain::annotation::NormPoint;
        SyncedFinding {
            id: FindingId::parse(id).unwrap(),
            photo: photo.map(Sha256::of),
            region: if photo.is_some() { Region::Box { a: NormPoint::new(0.2, 0.2), b: NormPoint::new(0.4, 0.5) } } else { Region::Whole },
            defect: DefectId::parse(defect).unwrap(),
            part: part.map(|p| PartId::parse(p).unwrap()),
            severity,
            measurements: BTreeMap::new(),
            note: String::new(),
            source: FindingSource::Engineer { user: String::new() },
            confidence: None,
            status,
            created: 1,
            decided: None,
        }
    }

    #[test]
    fn findings_ride_with_their_tower_and_are_checked() {
        let mut m = manifest();
        m.assets[0].findings = vec![
            finding("f-1", Some(b"one"), "hotspot.joint", Some("insulator.left.r"), Severity::High, FindingStatus::Confirmed),
            finding("f-2", None, "bird_nest", Some("earthwire.left"), Severity::Low, FindingStatus::Proposed),
        ];
        assert_eq!(m.validate(), Ok(()));
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<SurveyManifest>(&json).unwrap(), m);

        let mut bad = m.clone();
        let fs = &mut bad.assets[0].findings;
        fs[1].id = fs[0].id.clone();
        fs[0].photo = Some(Sha256::of(b"not this tower's"));
        fs[1].region = Region::Point { at: geoflux_domain::annotation::NormPoint::new(0.5, 0.5) };
        fs[1].measurements.insert("riseC".into(), f64::INFINITY);
        fs[1].confidence = Some(1.5);
        let fields: Vec<String> = bad.validate().unwrap_err().into_iter().map(|e| format!("{} {}", e.field, e.reason)).collect();
        for f in [
            "assets[0].findings[0].photo not_listed",
            "assets[0].findings[1].id duplicate",
            "assets[0].findings[1].region needs_photo",
            "assets[0].findings[1].measurements invalid",
            "assets[0].findings[1].confidence out_of_range",
        ] {
            assert!(fields.iter().any(|x| x == f), "{f} in {fields:?}");
        }
    }

    #[test]
    fn an_older_geogis_still_uploads() {
        let mut m = manifest();
        m.format = 1;
        assert_eq!(m.validate(), Ok(()), "format 1 has no findings");
        m.assets[0].findings = vec![finding("f-1", None, "bird_nest", None, Severity::Low, FindingStatus::Confirmed)];
        assert!(m.validate().unwrap_err().iter().any(|e| e.field == "assets[0].findings" && e.reason == "unsupported"));
        m.format = MANIFEST_FORMAT + 1;
        assert!(m.validate().unwrap_err().iter().any(|e| e.field == "format"));
        // Format 1 JSON has no `findings` key at all.
        let v = serde_json::to_value(manifest()).unwrap();
        assert!(v["assets"][0].get("findings").is_none());
    }
}
