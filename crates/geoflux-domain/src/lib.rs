//! GeoFlux L1: the inspection domain, independent of any UI or file system.
//!
//! - [`schema`]: an industry's layer as data (fields, image slots, CSV mapping, severity rule),
//!   read from JSON so a new tender's schema is a file, not code.
//! - [`asset`]: one inspected item (a tower, a span…) and its severity.
//! - [`coords`]: coordinates as survey sheets write them (decimal or degrees/minutes/seconds).
//! - [`form`]: validating and applying the values an asset form edits.
//! - [`csv_import`]: survey CSVs to asset drafts, with a reason for every skipped row.
//! - [`severity`]: how urgent a finding is, from the schema's rule.
//! - [`sequence`]: the natural order assets are walked in (`9_0` before `10_0`).
//! - [`images`]: a folder of inspection photos linked to assets and their image slots.
//!
//! Everything works on bytes and strings, so it builds for the browser too.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod asset;
pub mod coords;
pub mod csv_import;
pub mod form;
pub mod images;
pub mod schema;
pub mod sequence;
pub mod severity;

pub use asset::{Asset, AssetDraft, GeoPoint, Properties, RecordStatus, Severity};
pub use csv_import::{ImportError, ImportReport};
pub use form::{FieldIssue, IssueKind};
pub use schema::{
    CsvColumn, CsvProfile, DomainSchema, FieldDef, FieldType, FlagMatch, ImageSlotDef, SchemaError, SeverityRule, Thresholds,
};

/// The schemas that ship with GeoFlux, as `(id, JSON)`.
pub const BUILTIN_SCHEMAS: &[(&str, &str)] = &[("com.geo.telecom", include_str!("../schemas/telecom.json"))];

/// A built-in schema by id, parsed and checked.
pub fn builtin_schema(id: &str) -> Option<Result<DomainSchema, SchemaError>> {
    BUILTIN_SCHEMAS.iter().find(|(sid, _)| *sid == id).map(|(_, json)| DomainSchema::from_json(json))
}

/// Reads a number as people type it: surrounding spaces and a decimal comma are fine. Only finite values count.
pub(crate) fn number(text: &str) -> Option<f64> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    t.replace(',', ".").parse::<f64>().ok().filter(|v| v.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_schema_parses() {
        for (id, _) in BUILTIN_SCHEMAS {
            let schema = builtin_schema(id).expect("listed").expect("valid");
            assert_eq!(schema.id, *id);
        }
    }

    #[test]
    fn number_accepts_decimal_comma_and_rejects_non_finite() {
        assert_eq!(number(" 12,5 "), Some(12.5));
        assert_eq!(number("-3"), Some(-3.0));
        assert_eq!(number(""), None);
        assert_eq!(number("inf"), None);
        assert_eq!(number("NaN"), None);
        assert_eq!(number("35%"), None);
    }
}
