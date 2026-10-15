//! One inspected item and what every industry has in common: a position, a status, a severity
//! and the schema-defined properties.

use serde::{Deserialize, Serialize};

/// A WGS 84 position in degrees.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GeoPoint {
    pub lat: f64,
    pub lon: f64,
}

/// How far an asset's record is: missing data, complete, or exported.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecordStatus {
    #[default]
    Draft,
    Ready,
    Archived,
}

/// How urgent an inspection finding is. Ordered: later is more severe.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    #[default]
    None,
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    pub const ALL: [Severity; 5] = [Severity::None, Severity::Low, Severity::Medium, Severity::High, Severity::Critical];

    /// The stored name (`none`, `low`…), as attribute values and project files hold it.
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::None => "none",
            Severity::Low => "low",
            Severity::Medium => "medium",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }

    /// Reads a stored name, ignoring case; anything unknown (an older or newer version wrote it) is [`Severity::None`].
    pub fn from_name(name: &str) -> Severity {
        Severity::ALL.into_iter().find(|s| s.as_str().eq_ignore_ascii_case(name.trim())).unwrap_or(Severity::None)
    }
}

/// Schema-defined values of an asset, in the order they were added (CSV column order for extra columns).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Properties(Vec<(String, String)>);

impl Properties {
    pub fn new() -> Self {
        Self::default()
    }

    /// The value, or "" when absent.
    pub fn get(&self, key: &str) -> &str {
        self.0.iter().find(|(k, _)| k == key).map_or("", |(_, v)| v.as_str())
    }

    pub fn contains(&self, key: &str) -> bool {
        self.0.iter().any(|(k, _)| k == key)
    }

    /// Sets a value, keeping the key's place when it already exists.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key = key.into();
        let value = value.into();
        match self.0.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => self.0.push((key, value)),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(k, _)| k.as_str())
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for Properties {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut p = Properties::new();
        for (k, v) in iter {
            p.set(k, v);
        }
        p
    }
}

/// A row read by a CSV import, before it gets an id and a place in the project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssetDraft {
    /// None when the row had no readable position yet.
    pub location: Option<GeoPoint>,
    pub properties: Properties,
}

/// One inspected item (a tower, a span, a turbine…). Only what every industry has is a field; the
/// rest is in [`Asset::properties`], whose keys the schema named by [`Asset::schema_id`] defines.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub id: String,
    pub schema_id: String,
    /// Display and import order (ids carry no order).
    pub position: u32,
    #[serde(default)]
    pub status: RecordStatus,
    pub location: Option<GeoPoint>,
    #[serde(default)]
    pub properties: Properties,
    #[serde(default)]
    pub severity: Severity,
}

impl Asset {
    /// An asset from an imported draft.
    pub fn from_draft(id: impl Into<String>, schema_id: impl Into<String>, position: u32, draft: AssetDraft) -> Self {
        Asset {
            id: id.into(),
            schema_id: schema_id.into(),
            position,
            status: RecordStatus::Draft,
            location: draft.location,
            properties: draft.properties,
            severity: Severity::None,
        }
    }

    /// The property value, or "" when absent.
    pub fn property(&self, key: &str) -> &str {
        self.properties.get(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn properties_keep_insertion_order_and_replace_in_place() {
        let mut p = Properties::new();
        p.set("b", "1");
        p.set("a", "2");
        p.set("b", "3");
        assert_eq!(p.iter().collect::<Vec<_>>(), vec![("b", "3"), ("a", "2")]);
        assert_eq!(p.get("missing"), "");
    }

    #[test]
    fn severity_names_round_trip_and_unknown_is_none() {
        for s in Severity::ALL {
            assert_eq!(Severity::from_name(s.as_str()), s);
        }
        assert_eq!(Severity::from_name("HIGH"), Severity::High);
        assert_eq!(Severity::from_name("extreme"), Severity::None);
        assert!(Severity::Critical > Severity::High);
    }

    #[test]
    fn asset_json_round_trip() {
        let draft =
            AssetDraft { location: Some(GeoPoint { lat: 27.5, lon: 75.1 }), properties: [("towerNumber", "76_0")].into_iter().collect() };
        let asset = Asset::from_draft("a1", "com.geo.telecom", 0, draft);
        let json = serde_json::to_string(&asset).expect("serialize");
        let back: Asset = serde_json::from_str(&json).expect("parse");
        assert_eq!(back, asset);
        assert_eq!(back.property("towerNumber"), "76_0");
    }
}
