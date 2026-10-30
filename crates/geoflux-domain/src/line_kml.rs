//! A transmission line's KML (the towers as named points, the spans as lines) to tower drafts.
//!
//! The KML itself is read elsewhere (the desktop's KML reader, or the browser's); this takes its
//! point placemarks, so the rules stay pure and testable. A placemark is a tower when its name has
//! a digit in it (`34/1`, `AP8`, `T-12`), and its number is the name without the full stops
//! surveyors often leave at its end (`8/0.` is tower `8/0`); the rest (`STORE`, `Site office`) are other places on
//! the line and left out, with the reason, like duplicated tower numbers and unusable positions.

use std::collections::HashSet;
use std::fmt;

use crate::asset::{AssetDraft, GeoPoint, Properties};
use crate::schema::DomainSchema;

/// Most towers one line brings in; the report says when more were left out.
pub const MAX_TOWERS: usize = 100_000;
/// How many left-out placemarks are described one by one.
pub const MAX_DETAILS: usize = 10;
/// Longest left-out name described, in characters.
pub const MAX_NAME_CHARS: usize = 60;
/// The property the line's name goes to, when the schema has such a field.
pub const LINE_NAME_KEY: &str = "lineName";

/// One point placemark from the line's KML.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Placemark {
    pub name: String,
    /// The folders it sits in, outermost first, joined with ` / ` (empty at the top level).
    pub folder: String,
    pub lon: f64,
    pub lat: f64,
}

/// Why a placemark did not become a tower.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeftOutReason {
    /// No name, so no tower number.
    NoName,
    /// A name without a digit: a store, an office, a crossing.
    NotATower,
    /// The same tower number as an earlier placemark.
    Duplicate,
    /// A position that is not a longitude and latitude in range.
    BadPosition,
    /// Past [`MAX_TOWERS`].
    TooMany,
}

impl fmt::Display for LeftOutReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            LeftOutReason::NoName => "no name",
            LeftOutReason::NotATower => "not a tower number",
            LeftOutReason::Duplicate => "same tower number as an earlier point",
            LeftOutReason::BadPosition => "position out of range",
            LeftOutReason::TooMany => "too many towers in one line",
        })
    }
}

/// A placemark left out, by name (empty when it had none).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeftOut {
    pub name: String,
    pub reason: LeftOutReason,
}

impl fmt::Display for LeftOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.name.is_empty() { write!(f, "a point: {}", self.reason) } else { write!(f, "\"{}\": {}", self.name, self.reason) }
    }
}

/// What a line's placemarks produced. `left_out_count` counts every placemark left out;
/// `left_out` describes the first [`MAX_DETAILS`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineReport {
    /// One per tower, in the KML's order.
    pub towers: Vec<AssetDraft>,
    pub left_out: Vec<LeftOut>,
    pub left_out_count: usize,
}

impl LineReport {
    /// The left-out placemarks as one sentence part: `"STORE": not a tower number; …`, with a
    /// count of the rest past [`MAX_DETAILS`].
    pub fn left_out_summary(&self) -> String {
        let mut s = self.left_out.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ");
        let rest = self.left_out_count.saturating_sub(self.left_out.len());
        if rest > 0 {
            s.push_str(&format!("; and {rest} more"));
        }
        s
    }

    fn leave_out(&mut self, name: &str, reason: LeftOutReason) {
        self.left_out_count += 1;
        if self.left_out.len() < MAX_DETAILS {
            // A name is a label here, so a pasted paragraph doesn't flood the note.
            let name = match name.char_indices().nth(MAX_NAME_CHARS) {
                Some((at, _)) => format!("{}…", &name[..at]),
                None => name.to_string(),
            };
            self.left_out.push(LeftOut { name, reason });
        }
    }
}

/// The towers among `placemarks`, as drafts of `schema`: the tower number (the placemark's name)
/// goes to the schema's label field, `line_name` to [`LINE_NAME_KEY`], and every field the
/// schema's CSV mapping gives a default gets it, so a KML line reads like an imported survey sheet.
pub fn towers(placemarks: &[Placemark], schema: &DomainSchema, line_name: &str) -> LineReport {
    let label_key = schema.label_key();
    let defaults: Vec<(&str, &str)> = schema
        .csv
        .iter()
        .flat_map(|p| p.columns.iter())
        .filter(|c| !c.default.is_empty() && c.key != label_key && c.key != LINE_NAME_KEY)
        .map(|c| (c.key.as_str(), c.default.as_str()))
        .collect();
    let line_name = line_name.trim();

    let mut report = LineReport::default();
    // Tower numbers seen, compared as people read them: `34/1` and ` 34/1 ` are the same tower.
    let mut seen: HashSet<String> = HashSet::new();
    for pm in placemarks {
        let name = tower_number(&pm.name);
        let reason = if name.is_empty() {
            Some(LeftOutReason::NoName)
        } else if !name.bytes().any(|b| b.is_ascii_digit()) {
            Some(LeftOutReason::NotATower)
        } else if !in_range(pm.lon, pm.lat) {
            Some(LeftOutReason::BadPosition)
        } else if seen.contains(&key(name)) {
            Some(LeftOutReason::Duplicate)
        } else if report.towers.len() >= MAX_TOWERS {
            Some(LeftOutReason::TooMany)
        } else {
            None
        };
        if let Some(reason) = reason {
            report.leave_out(name, reason);
            continue;
        }
        seen.insert(key(name));

        let mut properties = Properties::new();
        properties.set(label_key, name);
        if !line_name.is_empty() {
            properties.set(LINE_NAME_KEY, line_name);
        }
        for (k, v) in &defaults {
            properties.set(*k, *v);
        }
        report.towers.push(AssetDraft { location: Some(GeoPoint { lat: pm.lat, lon: pm.lon }), properties });
    }
    report
}

/// A placemark's name as a tower number: trimmed, without full stops at its end.
pub fn tower_number(name: &str) -> &str {
    name.trim().trim_end_matches(|c: char| c == '.' || c.is_whitespace())
}

fn in_range(lon: f64, lat: f64) -> bool {
    lon.is_finite() && lat.is_finite() && (-180.0..=180.0).contains(&lon) && (-90.0..=90.0).contains(&lat)
}

/// A tower number for comparing: trimmed, inner runs of spaces as one, case ignored.
fn key(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> DomainSchema {
        crate::builtin_schema("com.geo.telecom").expect("listed").expect("valid")
    }

    fn pm(name: &str, lon: f64, lat: f64) -> Placemark {
        Placemark { name: name.into(), folder: String::new(), lon, lat }
    }

    #[test]
    fn named_points_with_a_digit_become_towers_in_kml_order() {
        let pms = [pm("AP8", 77.1, 28.5), pm("34/1", 77.2, 28.6), pm("STORE", 77.3, 28.7), pm("T-12", 77.4, 28.8)];
        let r = towers(&pms, &schema(), "TW 01");
        let names: Vec<&str> = r.towers.iter().map(|t| t.properties.get("towerNumber")).collect();
        assert_eq!(names, ["AP8", "34/1", "T-12"]);
        assert_eq!(r.towers[1].location, Some(GeoPoint { lat: 28.6, lon: 77.2 }));
        assert_eq!(r.towers[0].properties.get("lineName"), "TW 01");
        assert_eq!(r.left_out_count, 1);
        assert_eq!(r.left_out[0], LeftOut { name: "STORE".into(), reason: LeftOutReason::NotATower });
    }

    #[test]
    fn full_stops_at_the_end_of_a_name_are_not_part_of_the_number() {
        let r = towers(&[pm("8/0.", 77.0, 28.0), pm(" 8/0 ", 77.0, 28.0), pm("10/7. .", 77.0, 28.1)], &schema(), "L");
        let names: Vec<&str> = r.towers.iter().map(|t| t.properties.get("towerNumber")).collect();
        assert_eq!(names, ["8/0", "10/7"]);
        assert_eq!(r.left_out[0].reason, LeftOutReason::Duplicate);
        assert_eq!(tower_number("AP-73"), "AP-73");
        assert_eq!(tower_number("..."), "");
    }

    #[test]
    fn towers_get_the_schemas_csv_defaults() {
        let r = towers(&[pm("1", 77.0, 28.0)], &schema(), "L");
        assert_eq!(r.towers[0].properties.get("reportType"), "tower");
        assert_eq!(r.towers[0].properties.get("faultStatus"), "normal");
    }

    #[test]
    fn duplicates_unnamed_and_out_of_range_points_are_left_out_with_reasons() {
        let pms = [
            pm("34/1", 77.0, 28.0),
            pm(" 34/1 ", 77.0, 28.0),
            pm("", 77.0, 28.0),
            pm("35", 200.0, 28.0),
            pm("36", 77.0, f64::NAN),
            pm("37", 77.0, 28.1),
        ];
        let r = towers(&pms, &schema(), "L");
        assert_eq!(r.towers.len(), 2);
        let reasons: Vec<LeftOutReason> = r.left_out.iter().map(|l| l.reason).collect();
        assert_eq!(reasons, [LeftOutReason::Duplicate, LeftOutReason::NoName, LeftOutReason::BadPosition, LeftOutReason::BadPosition]);
        assert_eq!(
            r.left_out_summary(),
            "\"34/1\": same tower number as an earlier point; a point: no name; \"35\": position out of range; \"36\": position out of range"
        );
    }

    #[test]
    fn long_left_out_names_are_shortened() {
        let r = towers(&[pm(&"ä".repeat(100), 77.0, 28.0)], &schema(), "L");
        assert_eq!(r.left_out[0].name, format!("{}…", "ä".repeat(MAX_NAME_CHARS)));
    }

    #[test]
    fn an_empty_line_name_is_not_written() {
        let r = towers(&[pm("1", 77.0, 28.0)], &schema(), "  ");
        assert!(!r.towers[0].properties.contains("lineName"));
    }

    #[test]
    fn details_stop_at_the_limit_and_the_summary_counts_the_rest() {
        let pms: Vec<Placemark> = (0..MAX_DETAILS + 3).map(|i| pm(&format!("STORE {}", "x".repeat(i)), 77.0, 28.0)).collect();
        let r = towers(&pms, &schema(), "L");
        assert_eq!(r.left_out_count, MAX_DETAILS + 3);
        assert_eq!(r.left_out.len(), MAX_DETAILS);
        assert!(r.left_out_summary().ends_with("; and 3 more"));
    }
}
