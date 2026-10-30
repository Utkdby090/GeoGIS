//! What can be wrong with an asset, as data in the schema: the list engineers pick from and models
//! predict from. A finding's class is always one of these ids, never free text (free text goes in
//! the finding's note), so every confirmed finding is a usable training label.

use serde::{Deserialize, Serialize};

use crate::asset::Severity;

/// Most defect types in one schema.
pub const MAX_DEFECTS: usize = 500;
/// Longest defect id.
pub const MAX_DEFECT_ID: usize = 64;

/// A defect type's id: lowercase words of letters, digits and `_`, joined by dots, family first
/// (`hotspot.joint`, `insulator.broken_disc`). Ids are kept forever once findings use them;
/// rename the label, never the id.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DefectId(String);

impl DefectId {
    /// The class of a finding carried over from form values written before findings existed.
    /// Always valid, whatever the schema lists, and never exported as a training label.
    pub const UNCLASSIFIED: &'static str = "unclassified";

    pub fn parse(text: &str) -> Option<Self> {
        let ok = !text.is_empty()
            && text.len() <= MAX_DEFECT_ID
            && text.split('.').all(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'));
        ok.then(|| DefectId(text.to_string()))
    }

    pub fn unclassified() -> Self {
        DefectId(Self::UNCLASSIFIED.to_string())
    }

    pub fn is_unclassified(&self) -> bool {
        self.0 == Self::UNCLASSIFIED
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The family, the part before the first dot (`hotspot` for `hotspot.joint`).
    pub fn family(&self) -> &str {
        self.0.split('.').next().unwrap_or("")
    }
}

impl std::fmt::Display for DefectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for DefectId {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        DefectId::parse(&s).ok_or_else(|| format!("'{}' isn't a defect id like hotspot.joint", s.chars().take(80).collect::<String>()))
    }
}

impl From<DefectId> for String {
    fn from(id: DefectId) -> String {
        id.0
    }
}

/// One defect type in a schema's `defects` list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefectDef {
    pub id: DefectId,
    pub label: String,
    /// The severity a new finding of this type starts at; the engineer (or a rule) can change it.
    #[serde(default)]
    pub severity: Severity,
    /// The image slots it is seen in; empty means any slot. Thermal defects list the thermal slot,
    /// so the review screen offers them only there and a model trained on them only sees those.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slots: Vec<String>,
}

impl DefectDef {
    /// Whether this type can be found on a photo in `slot` ("" for a finding without a photo).
    pub fn applies_to(&self, slot: &str) -> bool {
        self.slots.is_empty() || slot.is_empty() || self.slots.iter().any(|s| s == slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_dotted_lowercase_words() {
        for good in ["hotspot", "hotspot.joint", "insulator.broken_disc", "a1.b_2.c3"] {
            assert!(DefectId::parse(good).is_some(), "{good}");
        }
        for bad in ["", "Hotspot", "hot spot", "hotspot.", ".joint", "a..b", "hot-spot", &"a".repeat(65)] {
            assert!(DefectId::parse(bad).is_none(), "{bad}");
        }
        assert_eq!(DefectId::parse("insulator.broken_disc").expect("valid").family(), "insulator");
        assert!(serde_json::from_str::<DefectId>("\"Bad Id\"").is_err());
    }

    #[test]
    fn slots_limit_where_a_type_applies() {
        let d = DefectDef {
            id: DefectId::parse("hotspot.joint").expect("valid"),
            label: "Hot joint".into(),
            severity: Severity::Medium,
            slots: vec!["THERMAL".into()],
        };
        assert!(d.applies_to("THERMAL"));
        assert!(!d.applies_to("RGB_ZOOM"));
        assert!(d.applies_to(""));
    }
}
