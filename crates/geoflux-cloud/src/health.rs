//! What the line health dashboard reads: one survey's towers rated by their findings, tallied by
//! defect type and tower part, and one tower's findings by part for the 3D tower.
//!
//! Built from what GeoGIS uploaded ([`SurveyAsset`] with its [`SyncedFinding`]s); nothing is
//! stored twice, so the dashboard always agrees with the survey and the reports. Only confirmed
//! and edited findings rate a tower, as on the desktop and in the PDF; proposals are counted as
//! waiting, and rejected findings never show.
//!
//! A tower uploaded without findings (by a GeoGIS from before findings were synced) keeps the
//! severity GeoGIS gave it, so older surveys still show on the dashboard; it has nothing to match
//! a filter with, so a filtered view rates it as having no findings.

use std::collections::BTreeMap;

use geoflux_domain::part::part_health;
use geoflux_domain::{DefectId, PartHealth, PartId, Severity};
use serde::{Deserialize, Serialize};

use crate::SurveyId;
use crate::error::FieldError;
use crate::sync::{SurveyAsset, SyncedFinding};
use crate::view::{tower_label, walking_order};

/// Most entries in one tally and most peaks, so a survey with odd ids can't make the answer huge.
pub const MAX_TALLY: usize = 500;
pub const MAX_PEAKS: usize = 32;

/// `GET /surveys/{id}/health?minSeverity=high&defect=hotspot&part=insulator`. Every filter is
/// optional; a defect or part filter is an id or the start of one at a dot (`insulator` matches
/// `insulator.left.r`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HealthQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_severity: Option<Severity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defect: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
}

impl HealthQuery {
    /// Refuses a defect or part filter that can't be an id, so a typo is an error, not an
    /// empty dashboard.
    pub fn check(&self) -> Result<(), Vec<FieldError>> {
        let mut errs = Vec::new();
        if self.defect.as_deref().is_some_and(|d| DefectId::parse(d).is_none()) {
            errs.push(FieldError::new("defect", "invalid"));
        }
        if self.part.as_deref().is_some_and(|p| PartId::parse(p).is_none()) {
            errs.push(FieldError::new("part", "invalid"));
        }
        if errs.is_empty() { Ok(()) } else { Err(errs) }
    }

    pub fn is_empty(&self) -> bool {
        self.min_severity.is_none() && self.defect.is_none() && self.part.is_none()
    }

    /// Whether a finding passes every filter. A part filter never matches a finding on no part.
    pub fn matches(&self, f: &SyncedFinding) -> bool {
        self.min_severity.is_none_or(|m| f.severity >= m)
            && self.defect.as_deref().is_none_or(|d| within(f.defect.as_str(), d))
            && self.part.as_deref().is_none_or(|p| f.part.as_ref().is_some_and(|x| x.is_within(p)))
    }
}

/// `id` is `filter` or starts with it at a dot.
fn within(id: &str, filter: &str) -> bool {
    id == filter || (id.starts_with(filter) && id.as_bytes().get(filter.len()) == Some(&b'.'))
}

/// `GET /surveys/{id}/health`: the whole line at a glance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineHealth {
    pub survey: SurveyId,
    /// The filters this was made with.
    pub query: HealthQuery,
    /// Towers in the survey.
    pub towers: u32,
    /// Towers with at least one photo or finding; the rest weren't inspected in this survey.
    pub inspected: u32,
    /// Inspected towers by their worst counted finding that matches, in [`Severity::ALL`] order
    /// (a healthy tower is `none`), like [`crate::model::SurveySummary::severity_counts`].
    pub severity_counts: [u32; 5],
    /// Counted findings that match.
    pub findings: u32,
    /// Proposals that match, waiting for a person.
    pub pending: u32,
    /// Counted findings that match, by defect type: most first.
    pub by_defect: Vec<DefectTally>,
    /// Counted findings that match, by tower part (`part: null` is the whole tower): most first.
    pub by_part: Vec<PartTally>,
    /// The highest of each measurement (`riseC`, `maxC`…) among counted findings that match.
    pub peaks: Vec<Peak>,
    /// Every tower, in walking order.
    pub rows: Vec<TowerHealthRow>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefectTally {
    pub defect: DefectId,
    pub count: u32,
    pub worst: Severity,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartTally {
    pub part: Option<PartId>,
    pub count: u32,
    pub worst: Severity,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Peak {
    pub measure: String,
    pub value: f64,
    /// The tower's id (the first in walking order when several share the value).
    pub tower: String,
}

/// One tower's line in the dashboard (the map, the along-the-line chart, the attention table).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TowerHealthRow {
    /// The tower's id, as in `/surveys/{id}/assets/{asset}`.
    pub id: String,
    pub label: String,
    pub position: u32,
    pub inspected: bool,
    /// Its worst counted finding that matches; `none` when healthy or not inspected.
    pub severity: Severity,
    pub findings: u32,
    pub pending: u32,
    /// Its worst counted finding that matches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top: Option<TopFinding>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopFinding {
    pub defect: DefectId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<PartId>,
    pub severity: Severity,
}

/// Worst first, then oldest, then by id: the order a tower's findings are listed and its top one
/// chosen, the same as the desktop's.
fn worst_first(a: &SyncedFinding, b: &SyncedFinding) -> std::cmp::Ordering {
    b.severity.cmp(&a.severity).then(a.created.cmp(&b.created)).then_with(|| a.id.cmp(&b.id))
}

fn bucket(severity: Severity) -> usize {
    Severity::ALL.iter().position(|s| *s == severity).unwrap_or(0)
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

impl LineHealth {
    /// The dashboard for one survey. `label_field` is the schema's; `query` should have passed
    /// [`HealthQuery::check`] (a bad filter simply matches nothing).
    pub fn build(survey: SurveyId, assets: &[SurveyAsset], label_field: &str, query: HealthQuery) -> Self {
        let mut sorted: Vec<&SurveyAsset> = assets.iter().take(crate::view::MAX_TOWERS).collect();
        sorted.sort_by(|a, b| walking_order(a, b));
        let mut out = LineHealth {
            survey,
            query,
            towers: count(sorted.len()),
            inspected: 0,
            severity_counts: [0; 5],
            findings: 0,
            pending: 0,
            by_defect: Vec::new(),
            by_part: Vec::new(),
            peaks: Vec::new(),
            rows: Vec::with_capacity(sorted.len()),
        };
        let mut defects: BTreeMap<&DefectId, (u32, Severity)> = BTreeMap::new();
        let mut parts: BTreeMap<Option<&PartId>, (u32, Severity)> = BTreeMap::new();
        let mut peaks: BTreeMap<&str, (f64, &str)> = BTreeMap::new();
        // One buffer for every tower's counted findings, so the loop doesn't allocate per tower.
        let mut counted: Vec<&SyncedFinding> = Vec::new();
        for a in sorted {
            let inspected = !a.photos.is_empty() || !a.findings.is_empty();
            counted.clear();
            let mut pending = 0usize;
            for f in a.findings.iter().filter(|f| out.query.matches(f)) {
                if f.status.counts() {
                    counted.push(f);
                } else if f.status == geoflux_domain::FindingStatus::Proposed {
                    pending += 1;
                }
            }
            counted.sort_by(|x, y| worst_first(x, y));
            for f in &counted {
                let d = defects.entry(&f.defect).or_insert((0, Severity::None));
                d.0 = d.0.saturating_add(1);
                d.1 = d.1.max(f.severity);
                let p = parts.entry(f.part.as_ref()).or_insert((0, Severity::None));
                p.0 = p.0.saturating_add(1);
                p.1 = p.1.max(f.severity);
                for (k, v) in &f.measurements {
                    let e = peaks.entry(k.as_str()).or_insert((*v, a.asset.id.as_str()));
                    if *v > e.0 {
                        *e = (*v, a.asset.id.as_str());
                    }
                }
            }
            let severity = match counted.first() {
                Some(f) => f.severity,
                None if a.findings.is_empty() && out.query.is_empty() => a.asset.severity,
                None => Severity::None,
            };
            if inspected {
                out.inspected += 1;
                out.severity_counts[bucket(severity)] += 1;
            }
            out.findings = out.findings.saturating_add(count(counted.len()));
            out.pending = out.pending.saturating_add(count(pending));
            out.rows.push(TowerHealthRow {
                id: a.asset.id.clone(),
                label: tower_label(a, label_field).to_string(),
                position: a.asset.position,
                inspected,
                severity: if inspected { severity } else { Severity::None },
                findings: count(counted.len()),
                pending: count(pending),
                top: counted.first().map(|f| TopFinding { defect: f.defect.clone(), part: f.part.clone(), severity: f.severity }),
            });
        }
        out.by_defect = defects.into_iter().map(|(d, (count, worst))| DefectTally { defect: d.clone(), count, worst }).collect();
        out.by_defect.sort_by(|a, b| b.count.cmp(&a.count).then(b.worst.cmp(&a.worst)).then_with(|| a.defect.cmp(&b.defect)));
        out.by_defect.truncate(MAX_TALLY);
        out.by_part = parts.into_iter().map(|(p, (count, worst))| PartTally { part: p.cloned(), count, worst }).collect();
        out.by_part.sort_by(|a, b| b.count.cmp(&a.count).then(b.worst.cmp(&a.worst)).then_with(|| a.part.cmp(&b.part)));
        out.by_part.truncate(MAX_TALLY);
        out.peaks = peaks
            .into_iter()
            .take(MAX_PEAKS)
            .map(|(m, (value, tower))| Peak { measure: m.to_string(), value, tower: tower.to_string() })
            .collect();
        out
    }
}

/// `GET /surveys/{id}/assets/{asset}/health`: one tower for the 3D view, unfiltered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TowerHealth {
    pub survey: SurveyId,
    pub id: String,
    pub label: String,
    /// Its worst counted finding (or GeoGIS's own rating when it uploaded no findings).
    pub severity: Severity,
    /// Each part's health, worst first; a marker per part on the 3D tower.
    pub parts: Vec<PartHealth>,
    /// Counted findings worst first, then proposals worst first; rejected ones are left out.
    pub findings: Vec<SyncedFinding>,
}

impl TowerHealth {
    pub fn of(survey: SurveyId, a: &SurveyAsset, label_field: &str) -> Self {
        let mut findings: Vec<SyncedFinding> =
            a.findings.iter().filter(|f| f.status != geoflux_domain::FindingStatus::Rejected).cloned().collect();
        findings.sort_by(|x, y| y.status.counts().cmp(&x.status.counts()).then_with(|| worst_first(x, y)));
        let severity = match findings.first() {
            Some(f) if f.status.counts() => f.severity,
            _ if a.findings.is_empty() => a.asset.severity,
            _ => Severity::None,
        };
        TowerHealth {
            survey,
            id: a.asset.id.clone(),
            label: tower_label(a, label_field).to_string(),
            severity,
            parts: part_health(findings.iter().map(|f| (f.part.as_ref(), f.severity, f.status))),
            findings,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::sync::tests::{finding, manifest};
    use geoflux_domain::FindingStatus::{Confirmed, Edited, Proposed, Rejected};

    fn survey() -> SurveyId {
        SurveyId::from_random([3; 16])
    }

    /// Tower T-001 (photos; a critical and a low counted, a high proposal, a rejected critical),
    /// T-002 (photos, healthy), T-003 (no photos, not inspected), T-004 (no findings uploaded,
    /// rated high by GeoGIS).
    fn assets() -> Vec<SurveyAsset> {
        let mut m = manifest();
        let mut hot = finding("f-1", Some(b"one"), "hotspot.joint", Some("insulator.left.y"), Severity::Critical, Edited);
        hot.measurements.insert("riseC".into(), 41.7);
        m.assets[0].findings = vec![
            hot,
            finding("f-2", None, "bird_nest", Some("earthwire.left"), Severity::Low, Confirmed),
            finding("f-3", Some(b"two"), "insulator.flashover", Some("insulator.right.r"), Severity::High, Proposed),
            finding("f-4", None, "tower.missing_member", Some("body"), Severity::Critical, Rejected),
        ];
        m.assets[0].asset.properties.set("tower_no", "8/0");
        m.assets[1].asset.position = 1;
        let mut third = m.assets[1].clone();
        third.asset.id = "T-003".into();
        third.asset.position = 2;
        third.photos.clear();
        let mut fourth = third.clone();
        fourth.asset.id = "T-004".into();
        fourth.asset.position = 3;
        fourth.photos = m.assets[1].photos.clone();
        fourth.asset.severity = Severity::High;
        m.assets.extend([third, fourth]);
        m.assets
    }

    #[test]
    fn the_line_is_rated_by_counted_findings() {
        let h = LineHealth::build(survey(), &assets(), "tower_no", HealthQuery::default());
        assert_eq!((h.towers, h.inspected), (4, 3));
        // none: T-002; high: T-004 (GeoGIS's own rating); critical: T-001.
        assert_eq!(h.severity_counts, [1, 0, 0, 1, 1]);
        assert_eq!((h.findings, h.pending), (2, 1), "rejected never shows");
        let rows: Vec<(&str, &str, Severity, bool)> =
            h.rows.iter().map(|r| (r.id.as_str(), r.label.as_str(), r.severity, r.inspected)).collect();
        assert_eq!(
            rows,
            [
                ("T-001", "8/0", Severity::Critical, true),
                ("T-002", "T-002", Severity::None, true),
                ("T-003", "T-003", Severity::None, false),
                ("T-004", "T-004", Severity::High, true)
            ]
        );
        let top = h.rows[0].top.as_ref().unwrap();
        assert_eq!((top.defect.as_str(), top.part.as_ref().map(PartId::as_str)), ("hotspot.joint", Some("insulator.left.y")));
        assert_eq!(h.by_defect.len(), 2);
        assert_eq!(h.by_part.iter().map(|p| p.part.as_ref().map(PartId::as_str)).collect::<Vec<_>>().len(), 2);
        assert_eq!(h.peaks, [Peak { measure: "riseC".into(), value: 41.7, tower: "T-001".into() }]);
    }

    #[test]
    fn filters_narrow_findings_and_ratings() {
        let q = |json: &str| serde_json::from_str::<HealthQuery>(json).unwrap();
        let by_part = LineHealth::build(survey(), &assets(), "tower_no", q(r#"{"part":"earthwire"}"#));
        assert_eq!((by_part.findings, by_part.rows[0].severity), (1, Severity::Low));
        assert_eq!(by_part.rows[3].severity, Severity::None, "no findings uploaded: nothing matches a filter");
        let insulators = LineHealth::build(survey(), &assets(), "tower_no", q(r#"{"part":"insulator"}"#));
        assert_eq!((insulators.findings, insulators.pending), (1, 1));
        let severe = LineHealth::build(survey(), &assets(), "tower_no", q(r#"{"minSeverity":"high"}"#));
        assert_eq!((severe.findings, severe.pending), (1, 1));
        let family = LineHealth::build(survey(), &assets(), "tower_no", q(r#"{"defect":"hotspot"}"#));
        assert_eq!(family.findings, 1);
        let not_a_word = LineHealth::build(survey(), &assets(), "tower_no", q(r#"{"defect":"hot"}"#));
        assert_eq!(not_a_word.findings, 0, "families match at a dot");
        assert!(q(r#"{"part":"Leg A"}"#).check().is_err());
        assert!(serde_json::from_str::<HealthQuery>(r#"{"colour":"red"}"#).is_err());
        assert!(q("{}").is_empty() && q("{}").check().is_ok());
    }

    #[test]
    fn a_tower_lists_parts_and_findings_worst_first() {
        let a = assets();
        let t = TowerHealth::of(survey(), &a[0], "tower_no");
        assert_eq!((t.label.as_str(), t.severity), ("8/0", Severity::Critical));
        let ids: Vec<&str> = t.findings.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["f-1", "f-2", "f-3"], "counted worst first, then the proposal");
        let parts: Vec<(&str, Severity, u32, u32)> =
            t.parts.iter().map(|p| (p.part.as_ref().map_or("", PartId::as_str), p.severity, p.counted, p.pending)).collect();
        assert_eq!(
            parts,
            [
                ("insulator.left.y", Severity::Critical, 1, 0),
                ("earthwire.left", Severity::Low, 1, 0),
                ("insulator.right.r", Severity::None, 0, 1)
            ]
        );
        let legacy = TowerHealth::of(survey(), &a[3], "tower_no");
        assert_eq!((legacy.severity, legacy.parts.len()), (Severity::High, 0));
    }

    #[test]
    fn it_serializes_for_the_web_app() {
        let h = LineHealth::build(survey(), &assets(), "tower_no", HealthQuery { part: Some("insulator".into()), ..Default::default() });
        let v = serde_json::to_value(&h).unwrap();
        assert_eq!(v["query"]["part"], "insulator");
        assert!(v["severityCounts"].is_array() && v["byDefect"].is_array() && v["rows"][0]["top"]["defect"].is_string());
        assert_eq!(serde_json::from_value::<LineHealth>(v).unwrap(), h);
    }
}
