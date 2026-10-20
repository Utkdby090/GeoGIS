//! What the web app reads (Phase 7b): one survey, its towers on the map, one tower with its photos,
//! and the reports. Everything here is built from what GeoGIS uploaded ([`SurveyAsset`]); the
//! server stores each uploaded tower as it came and never edits it.
//!
//! The map gets towers as GeoJSON (RFC 7946, longitude first), small enough per tower that a line
//! of tens of thousands still loads in one request. The full tower (form, photos, markup) is a
//! second request when one is clicked.

use geoflux_domain::{RecordStatus, Severity};
use serde::{Deserialize, Serialize};

use crate::model::SurveySummary;
use crate::ports::{DeviceCodeRecord, DeviceCodeState};
use crate::sync::SurveyAsset;

/// Most towers one survey can list, the same as the manifest allows.
pub const MAX_TOWERS: usize = crate::sync::MAX_ASSETS;

/// `GET /surveys/{id}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SurveyDetail {
    pub summary: SurveySummary,
    /// `[west, south, east, north]` of the towers that have a position; `None` when none has.
    pub bounds: Option<[f64; 4]>,
    /// Every revision of the same survey (by `clientSurveyId`), newest first, this one included.
    pub revisions: Vec<RevisionRef>,
}

/// Another upload of the same survey.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionRef {
    pub id: crate::SurveyId,
    pub revision: u32,
    pub committed: Option<i64>,
}

/// `GET /surveys/{id}/towers`: a GeoJSON FeatureCollection, in walking order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TowerCollection {
    #[serde(rename = "type")]
    pub kind: FeatureCollectionTag,
    pub features: Vec<TowerFeature>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeatureCollectionTag {
    FeatureCollection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeatureTag {
    Feature,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PointTag {
    Point,
}

/// One tower on the map. `id` is the tower's id in GeoGIS, which also names it in
/// `/surveys/{id}/assets/{asset}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TowerFeature {
    #[serde(rename = "type")]
    pub kind: FeatureTag,
    pub id: String,
    /// `null` for a tower with no position: listed, but not drawn.
    pub geometry: Option<Point>,
    pub properties: TowerProperties,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Point {
    #[serde(rename = "type")]
    pub kind: PointTag,
    /// `[longitude, latitude]`.
    pub coordinates: [f64; 2],
}

/// What the map styles and labels a tower by, without loading the tower.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TowerProperties {
    /// The schema's label field (the tower number), or the id when that is blank.
    pub label: String,
    pub position: u32,
    pub severity: Severity,
    pub status: RecordStatus,
    pub photos: u32,
}

impl TowerCollection {
    /// The map's view of the uploaded towers. `label_field` is the schema's; a tower whose label is
    /// blank is labelled by its id. A position outside the world (or not a number) is dropped, so
    /// one bad tower can't break the map; the tower stays in the list.
    pub fn from_assets(assets: &[SurveyAsset], label_field: &str) -> Self {
        let mut sorted: Vec<&SurveyAsset> = assets.iter().take(MAX_TOWERS).collect();
        sorted.sort_by(|a, b| a.asset.position.cmp(&b.asset.position).then_with(|| a.asset.id.cmp(&b.asset.id)));
        let features = sorted
            .into_iter()
            .map(|a| {
                let label = Some(a.asset.properties.get(label_field).trim()).filter(|l| !l.is_empty()).unwrap_or(&a.asset.id);
                TowerFeature {
                    kind: FeatureTag::Feature,
                    id: a.asset.id.clone(),
                    geometry: a
                        .asset
                        .location
                        .filter(|p| valid_lon_lat(p.lon, p.lat))
                        .map(|p| Point { kind: PointTag::Point, coordinates: [p.lon, p.lat] }),
                    properties: TowerProperties {
                        label: label.to_string(),
                        position: a.asset.position,
                        severity: a.asset.severity,
                        status: a.asset.status,
                        photos: u32::try_from(a.photos.len()).unwrap_or(u32::MAX),
                    },
                }
            })
            .collect();
        TowerCollection { kind: FeatureCollectionTag::FeatureCollection, features }
    }

    /// `[west, south, east, north]` of the drawn towers, for the map's first view.
    pub fn bounds(&self) -> Option<[f64; 4]> {
        let mut it = self.features.iter().filter_map(|f| f.geometry.as_ref()).map(|g| g.coordinates);
        let [x, y] = it.next()?;
        Some(it.fold([x, y, x, y], |[w, s, e, n], [x, y]| [w.min(x), s.min(y), e.max(x), n.max(y)]))
    }
}

fn valid_lon_lat(lon: f64, lat: f64) -> bool {
    lon.is_finite() && lat.is_finite() && (-180.0..=180.0).contains(&lon) && (-90.0..=90.0).contains(&lat)
}

/// `GET /surveys/{id}/assets/{asset}`: one tower as uploaded, with the URLs of its photos.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TowerDetail {
    pub survey: crate::SurveyId,
    #[serde(flatten)]
    pub uploaded: SurveyAsset,
}

/// `GET /device/pending?code=…`: what the approve page shows before the person says yes, so a code
/// someone else sent them can't silently sign a stranger's computer into their account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingDevice {
    /// The name GeoGIS sent (the computer's name), shown as text, never as HTML.
    pub device_name: String,
    pub expires: i64,
}

impl PendingDevice {
    /// Only a code still waiting for an answer, and not run out, is shown; any other is "not found",
    /// the same as a code that never existed.
    pub fn of(record: &DeviceCodeRecord, now: i64) -> Option<Self> {
        (record.state == DeviceCodeState::Pending && now < record.expires)
            .then(|| PendingDevice { device_name: record.device_name.clone(), expires: record.expires })
    }
}

/// The reports the web app downloads. Both are made by the server with the desktop's report
/// templates, so the files match what GeoGIS writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReportRequest {
    /// One tower's PDF.
    Tower { asset: String },
    /// A ZIP with one PDF per tower, in walking order.
    Line,
}

impl ReportRequest {
    pub fn content_type(&self) -> &'static str {
        match self {
            ReportRequest::Tower { .. } => "application/pdf",
            ReportRequest::Line => "application/zip",
        }
    }

    /// The download's file name, from the survey's own text made safe for every system.
    pub fn file_name(&self, summary: &SurveySummary) -> String {
        let base = safe_file_part(summary.line.as_deref().unwrap_or(&summary.project));
        match self {
            ReportRequest::Tower { asset } => format!("{base}-{}-r{}.pdf", safe_file_part(asset), summary.revision),
            ReportRequest::Line => format!("{base}-r{}.zip", summary.revision),
        }
    }
}

/// Letters, digits, `-` and `_` only (others become `_`), at most 80 characters, never empty.
pub fn safe_file_part(text: &str) -> String {
    let s: String = text.trim().chars().take(80).map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    if s.is_empty() { "survey".to_string() } else { s }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use geoflux_domain::GeoPoint;

    fn assets() -> Vec<SurveyAsset> {
        let mut m = crate::sync::tests::manifest();
        m.assets[0].asset.position = 2;
        m.assets[0].asset.properties.set("tower_no", " 101 ");
        m.assets[1].asset.position = 3;
        let mut third = m.assets[0].clone();
        third.asset.id = "a2".into();
        third.asset.position = 1;
        third.asset.location = Some(GeoPoint { lat: 95.0, lon: 10.0 });
        third.asset.properties = Default::default();
        m.assets.push(third);
        m.assets
    }

    #[test]
    fn towers_are_in_walking_order_with_bad_positions_dropped() {
        let fc = TowerCollection::from_assets(&assets(), "tower_no");
        let ids: Vec<&str> = fc.features.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["a2", "T-001", "T-002"]);
        assert!(fc.features[0].geometry.is_none(), "latitude 95 is off the world");
        assert_eq!(fc.features[0].properties.label, "a2", "blank label falls back to the id");
        assert_eq!(fc.features[1].properties.label, "101");
        assert_eq!(fc.features[1].properties.photos, 2);
        assert_eq!(fc.features[1].geometry.as_ref().unwrap().coordinates, [85.1, 25.6], "longitude first");
    }

    #[test]
    fn towers_serialize_as_geojson() {
        let fc = TowerCollection::from_assets(&assets(), "tower_no");
        let v = serde_json::to_value(&fc).unwrap();
        assert_eq!(v["type"], "FeatureCollection");
        assert_eq!(v["features"][1]["type"], "Feature");
        assert_eq!(v["features"][1]["geometry"]["type"], "Point");
        assert!(v["features"][0]["geometry"].is_null());
        let back: TowerCollection = serde_json::from_value(v).unwrap();
        assert_eq!(back, fc);
    }

    #[test]
    fn bounds_cover_drawn_towers_only() {
        let fc = TowerCollection::from_assets(&assets(), "tower_no");
        let [w, s, e, n] = fc.bounds().unwrap();
        assert!(w <= e && s <= n);
        assert!(n <= 90.0);
        assert!(TowerCollection::from_assets(&[], "x").bounds().is_none());
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(safe_file_part("../../etc/passwd"), "______etc_passwd");
        assert_eq!(safe_file_part("  "), "survey");
        assert_eq!(safe_file_part("Line 4 – Patna"), "Line_4___Patna");
        assert_eq!(safe_file_part(&"x".repeat(500)).len(), 80);
    }
}
