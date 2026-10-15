//! Between an [`Asset`] and the flat key → text map an asset form edits.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::asset::{Asset, GeoPoint};
use crate::coords;
use crate::schema::{FieldDef, FieldType, core_fields};

/// What is wrong with a field's value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IssueKind {
    /// Required but blank.
    Missing,
    NotANumber,
    NotAnInteger,
    /// Below `min` or above `max`, or a latitude/longitude out of range.
    OutOfRange,
    /// Not `YYYY-MM-DD`, or not a real date.
    NotADate,
    /// Not one of the field's options.
    NotAnOption,
    NotABoolean,
    /// Neither decimal degrees nor degrees/minutes/seconds.
    NotACoordinate,
}

/// One invalid field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldIssue {
    pub key: String,
    pub kind: IssueKind,
}

/// The form values of `asset` for `fields`: coordinates from its location, the rest from its properties.
pub fn values(asset: &Asset, fields: &[FieldDef]) -> BTreeMap<String, String> {
    fields
        .iter()
        .map(|f| {
            let v = match f.key.as_str() {
                core_fields::LATITUDE => asset.location.map(|p| p.lat.to_string()).unwrap_or_default(),
                core_fields::LONGITUDE => asset.location.map(|p| p.lon.to_string()).unwrap_or_default(),
                key => asset.property(key).to_string(),
            };
            (f.key.clone(), v)
        })
        .collect()
}

/// The invalid fields, in schema order. Blank optional fields are fine.
pub fn validate(fields: &[FieldDef], values: &BTreeMap<String, String>) -> Vec<FieldIssue> {
    fields
        .iter()
        .filter_map(|f| {
            let value = values.get(&f.key).map_or("", |v| v.trim());
            check(f, value).map(|kind| FieldIssue { key: f.key.clone(), kind })
        })
        .collect()
}

fn check(f: &FieldDef, value: &str) -> Option<IssueKind> {
    if value.is_empty() {
        return f.required.then_some(IssueKind::Missing);
    }
    if core_fields::is_coordinate(&f.key) {
        let Some(c) = coords::parse(value) else { return Some(IssueKind::NotACoordinate) };
        let limit = if f.key == core_fields::LATITUDE { 90.0 } else { 180.0 };
        return (c.degrees.abs() > limit).then_some(IssueKind::OutOfRange);
    }
    let in_range = |v: f64| f.min.is_none_or(|lo| v >= lo) && f.max.is_none_or(|hi| v <= hi);
    match f.field_type {
        FieldType::Text | FieldType::Photo => None,
        FieldType::Number => match crate::number(value) {
            None => Some(IssueKind::NotANumber),
            Some(v) => (!in_range(v)).then_some(IssueKind::OutOfRange),
        },
        FieldType::Integer => match value.parse::<i64>() {
            Err(_) => Some(IssueKind::NotAnInteger),
            Ok(v) => (!in_range(v as f64)).then_some(IssueKind::OutOfRange),
        },
        FieldType::Date => (!is_iso_date(value)).then_some(IssueKind::NotADate),
        FieldType::Dropdown => (!f.options.iter().any(|o| o == value)).then_some(IssueKind::NotAnOption),
        FieldType::MultiSelect => value
            .split(';')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .any(|p| !f.options.iter().any(|o| o == p))
            .then_some(IssueKind::NotAnOption),
        FieldType::Boolean => parse_bool(value).is_none().then_some(IssueKind::NotABoolean),
    }
}

/// `true`/`false`, `yes`/`no`, `1`/`0`, ignoring case.
pub fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "1" => Some(true),
        "false" | "no" | "0" => Some(false),
        _ => None,
    }
}

/// A real calendar date written `YYYY-MM-DD`.
pub fn is_iso_date(value: &str) -> bool {
    let b = value.as_bytes();
    if b.len() != 10 || b.get(4) != Some(&b'-') || b.get(7) != Some(&b'-') {
        return false;
    }
    let num =
        |r: std::ops::Range<usize>| value.get(r).filter(|s| s.bytes().all(|c| c.is_ascii_digit())).and_then(|s| s.parse::<u32>().ok());
    let (Some(y), Some(m), Some(d)) = (num(0..4), num(5..7), num(8..10)) else { return false };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&d)
}

/// `asset` with the edited `values` applied. Properties not in the form are kept. The location
/// changes only when both coordinates are readable; a cleared pair removes it.
pub fn apply(asset: &Asset, values: &BTreeMap<String, String>) -> Asset {
    let mut next = asset.clone();
    for (key, value) in values {
        if !core_fields::is_coordinate(key) {
            next.properties.set(key.as_str(), value.as_str());
        }
    }
    let lat = values.get(core_fields::LATITUDE);
    let lon = values.get(core_fields::LONGITUDE);
    if let (Some(lat), Some(lon)) = (lat, lon) {
        if lat.trim().is_empty() && lon.trim().is_empty() {
            next.location = None;
        } else if let (Some(a), Some(o)) = (coords::parse(lat), coords::parse(lon)) {
            next.location = Some(GeoPoint { lat: a.degrees, lon: o.degrees });
        }
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::{AssetDraft, Properties};

    fn field(key: &str, t: FieldType) -> FieldDef {
        FieldDef {
            key: key.into(),
            label: key.into(),
            field_type: t,
            required: false,
            options: vec![],
            group: String::new(),
            row: 0,
            weight: 1.0,
            min: None,
            max: None,
        }
    }

    fn vals(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn each_type_is_checked() {
        let mut req = field("name", FieldType::Text);
        req.required = true;
        let mut num = field("n", FieldType::Number);
        num.min = Some(0.0);
        num.max = Some(100.0);
        let mut drop = field("d", FieldType::Dropdown);
        drop.options = vec!["A".into(), "B".into()];
        let mut multi = field("m", FieldType::MultiSelect);
        multi.options = vec!["X".into(), "Y".into()];
        let fields = vec![
            req,
            num,
            field("i", FieldType::Integer),
            field("dt", FieldType::Date),
            drop,
            multi,
            field("b", FieldType::Boolean),
            field(core_fields::LATITUDE, FieldType::Number),
        ];
        let bad = vals(&[
            ("n", "150"),
            ("i", "1.5"),
            ("dt", "2026-02-30"),
            ("d", "C"),
            ("m", "X; Z"),
            ("b", "maybe"),
            (core_fields::LATITUDE, "95"),
        ]);
        let kinds: Vec<_> = validate(&fields, &bad).into_iter().map(|i| (i.key, i.kind)).collect();
        assert_eq!(
            kinds,
            vec![
                ("name".into(), IssueKind::Missing),
                ("n".into(), IssueKind::OutOfRange),
                ("i".into(), IssueKind::NotAnInteger),
                ("dt".into(), IssueKind::NotADate),
                ("d".into(), IssueKind::NotAnOption),
                ("m".into(), IssueKind::NotAnOption),
                ("b".into(), IssueKind::NotABoolean),
                (core_fields::LATITUDE.into(), IssueKind::OutOfRange),
            ]
        );
        let good = vals(&[
            ("name", "x"),
            ("n", "12,5"),
            ("i", "-3"),
            ("dt", "2024-02-29"),
            ("d", "A"),
            ("m", "X;Y"),
            ("b", "Yes"),
            (core_fields::LATITUDE, "27°30'N"),
        ]);
        assert_eq!(validate(&fields, &good), vec![]);
    }

    #[test]
    fn apply_keeps_other_properties_and_reads_dms() {
        let props: Properties = [("a", "1"), ("hidden", "keep")].into_iter().collect();
        let asset = Asset::from_draft("1", "x", 0, AssetDraft { location: None, properties: props });
        let next = apply(&asset, &vals(&[("a", "2"), (core_fields::LATITUDE, "27°30'N"), (core_fields::LONGITUDE, "75.5")]));
        assert_eq!(next.property("a"), "2");
        assert_eq!(next.property("hidden"), "keep");
        assert_eq!(next.location, Some(GeoPoint { lat: 27.5, lon: 75.5 }));
        // An unreadable pair leaves the location alone; a cleared pair removes it.
        let same = apply(&next, &vals(&[(core_fields::LATITUDE, "junk"), (core_fields::LONGITUDE, "75")]));
        assert_eq!(same.location, next.location);
        let cleared = apply(&next, &vals(&[(core_fields::LATITUDE, ""), (core_fields::LONGITUDE, " ")]));
        assert_eq!(cleared.location, None);
    }

    #[test]
    fn values_round_trip_through_apply() {
        let fields = vec![
            field("a", FieldType::Text),
            field(core_fields::LATITUDE, FieldType::Number),
            field(core_fields::LONGITUDE, FieldType::Number),
        ];
        let props: Properties = [("a", "x")].into_iter().collect();
        let asset = Asset::from_draft("1", "x", 0, AssetDraft { location: Some(GeoPoint { lat: 1.25, lon: -2.5 }), properties: props });
        assert_eq!(apply(&asset, &values(&asset, &fields)), asset);
    }
}
