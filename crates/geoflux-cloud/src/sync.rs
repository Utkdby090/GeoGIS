//! Project › Sync: sending a survey from GeoGIS to the server.
//!
//! Three steps, each safe to retry over a weak connection:
//!
//! 1. `POST /surveys` with a [`SurveyManifest`]: the towers, their forms and severity, and each
//!    photo by SHA-256. The server answers with the photos it doesn't have yet. Sending the same
//!    `clientSurveyId` and `revision` again returns the same survey, so a retry never duplicates.
//! 2. `PUT /surveys/{id}/photos/{sha256}` for each missing photo. The server streams the body to a
//!    temporary file while hashing it, refuses it if the hash or size differs, then hands the file
//!    to [`crate::ports::BlobStore`]. Memory use doesn't grow with the photo.
//! 3. `POST /surveys/{id}/commit`. Only then does the survey appear on the map and the dashboard;
//!    if photos are still missing it answers [`crate::ErrorCode::Incomplete`] with the list.
//!
//! Every upload is a new revision: nothing is overwritten, so trends can compare surveys.

use std::collections::{BTreeSet, HashSet};

use geoflux_domain::Asset;
use geoflux_domain::annotation::AnnotationSet;
use serde::{Deserialize, Serialize};

use crate::Sha256;
use crate::error::FieldError;

/// The manifest format this build writes and reads.
pub const MANIFEST_FORMAT: u32 = 1;
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
        if self.format != MANIFEST_FORMAT {
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
                SurveyAsset { asset, photos: vec![photo("thermal", b"one"), photo("structure", b"two")] },
                // The same photo under another tower: counted and stored once.
                SurveyAsset { asset: second, photos: vec![photo("thermal", b"one")] },
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
}
