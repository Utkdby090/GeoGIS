//! Where on an asset a finding is: the schema's list of parts (for a tower: earthwire peaks,
//! insulator strings by circuit and phase, body, plates, legs). The dashboard's 3D tower places a
//! finding's marker on its part, the report groups by it, and a model can learn which part a
//! defect is on. Like defect types, parts are data in the schema, never free text.
//!
//! A part also names the point on the 3D model its markers sit at ([`PartDef::node`]), so one
//! static model serves every tower and a better model can replace it without touching findings.

use serde::{Deserialize, Serialize};

use crate::asset::Severity;
use crate::finding::FindingStatus;

/// Most parts in one schema.
pub const MAX_PARTS: usize = 200;
/// Longest part id, and longest model node name.
pub const MAX_PART_ID: usize = 64;

/// A part's id: lowercase words of letters, digits and `_`, joined by dots, family first
/// (`insulator.left.r`, `leg.a`). Kept forever once findings use it; rename the label, never the id.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PartId(String);

impl PartId {
    pub fn parse(text: &str) -> Option<Self> {
        crate::defect::is_dotted_id(text, MAX_PART_ID).then(|| PartId(text.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The family, the part before the first dot (`insulator` for `insulator.left.r`).
    pub fn family(&self) -> &str {
        self.0.split('.').next().unwrap_or("")
    }

    /// Whether this part is `filter` or inside it: `insulator` and `insulator.left` both match
    /// `insulator.left.r`; `insul` doesn't.
    pub fn is_within(&self, filter: &str) -> bool {
        self.0 == filter || (self.0.starts_with(filter) && self.0.as_bytes().get(filter.len()) == Some(&b'.'))
    }
}

impl std::fmt::Display for PartId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for PartId {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        PartId::parse(&s).ok_or_else(|| format!("'{}' isn't a part id like insulator.left.r", s.chars().take(80).collect::<String>()))
    }
}

impl From<PartId> for String {
    fn from(id: PartId) -> String {
        id.0
    }
}

/// One part in a schema's `parts` list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PartDef {
    pub id: PartId,
    pub label: String,
    /// The node of the 3D model its markers sit at. Empty: a node named like the id.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub node: String,
}

impl PartDef {
    /// The 3D model node this part's markers sit at.
    pub fn node_name(&self) -> &str {
        if self.node.is_empty() { self.id.as_str() } else { &self.node }
    }
}

/// How one part of one asset is: its worst counted finding and how many are counted or waiting.
/// `part: None` gathers the findings that name no part (the whole asset).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartHealth {
    pub part: Option<PartId>,
    /// The worst counted finding's severity; [`Severity::None`] when only proposals are waiting.
    pub severity: Severity,
    /// Confirmed and edited findings.
    pub counted: u32,
    /// Proposals waiting for a person.
    pub pending: u32,
}

/// Each part's health from its findings' `(part, severity, status)`, worst first, then by part id
/// (no part last). Rejected findings are left out; a part with only rejected findings isn't listed.
pub fn part_health<'a>(findings: impl IntoIterator<Item = (Option<&'a PartId>, Severity, FindingStatus)>) -> Vec<PartHealth> {
    let mut out: Vec<PartHealth> = Vec::new();
    for (part, severity, status) in findings {
        if status == FindingStatus::Rejected {
            continue;
        }
        let at = match out.iter().position(|h| h.part.as_ref() == part) {
            Some(i) => i,
            None => {
                out.push(PartHealth { part: part.cloned(), severity: Severity::None, counted: 0, pending: 0 });
                out.len() - 1
            }
        };
        let h = &mut out[at];
        if status.counts() {
            h.counted = h.counted.saturating_add(1);
            h.severity = h.severity.max(severity);
        } else {
            h.pending = h.pending.saturating_add(1);
        }
    }
    out.sort_by(|a, b| {
        b.severity.cmp(&a.severity).then_with(|| match (&a.part, &b.part) {
            (Some(x), Some(y)) => x.cmp(y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        })
    });
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn p(s: &str) -> PartId {
        PartId::parse(s).unwrap()
    }

    #[test]
    fn ids_are_dotted_lowercase_words() {
        for good in ["body", "leg.a", "insulator.left.r", "earthwire.right"] {
            assert!(PartId::parse(good).is_some(), "{good}");
        }
        for bad in ["", "Leg.A", "leg a", "leg.", ".a", "leg-a", &"a".repeat(65)] {
            assert!(PartId::parse(bad).is_none(), "{bad}");
        }
        assert_eq!(p("insulator.left.r").family(), "insulator");
        assert!(serde_json::from_str::<PartId>("\"Leg A\"").is_err());
    }

    #[test]
    fn within_matches_whole_words_only() {
        let id = p("insulator.left.r");
        assert!(id.is_within("insulator"));
        assert!(id.is_within("insulator.left"));
        assert!(id.is_within("insulator.left.r"));
        assert!(!id.is_within("insul"));
        assert!(!id.is_within("insulator.left.r.x"));
        assert!(!id.is_within("insulator.right"));
    }

    #[test]
    fn node_defaults_to_the_id() {
        let d: PartDef = serde_json::from_str(r#"{"id":"leg.a","label":"Leg A"}"#).unwrap();
        assert_eq!(d.node_name(), "leg.a");
        let d: PartDef = serde_json::from_str(r#"{"id":"leg.a","label":"Leg A","node":"LegA_Anchor"}"#).unwrap();
        assert_eq!(d.node_name(), "LegA_Anchor");
        assert!(serde_json::from_str::<PartDef>(r#"{"id":"leg.a","label":"Leg A","pos":[0,0,0]}"#).is_err());
    }

    #[test]
    fn health_is_worst_first_and_ignores_rejected() {
        use FindingStatus::*;
        let (a, b) = (p("leg.a"), p("insulator.left.r"));
        let h = part_health([
            (Some(&a), Severity::Low, Confirmed),
            (Some(&b), Severity::Critical, Edited),
            (Some(&b), Severity::Low, Proposed),
            (None, Severity::High, Confirmed),
            (Some(&a), Severity::Critical, Rejected),
            (Some(&a), Severity::Medium, Confirmed),
            (Some(&p("body")), Severity::High, Rejected),
        ]);
        let order: Vec<Option<&str>> = h.iter().map(|x| x.part.as_ref().map(PartId::as_str)).collect();
        assert_eq!(order, [Some("insulator.left.r"), None, Some("leg.a")]);
        assert_eq!((h[0].severity, h[0].counted, h[0].pending), (Severity::Critical, 1, 1));
        assert_eq!((h[2].severity, h[2].counted), (Severity::Medium, 2), "rejected critical doesn't count");
        let only_pending = part_health([(Some(&a), Severity::High, Proposed)]);
        assert_eq!((only_pending[0].severity, only_pending[0].pending), (Severity::None, 1));
    }
}
