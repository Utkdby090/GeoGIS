//! GeoFlux 1.x projects (`.geox`): how a tower row maps onto a GeoGIS asset.
//!
//! A `.geox` is a zip of a GeoFlux project folder. Its towers are in [`PROJECT_DB`] (plain SQLite),
//! table [`TOWER_TABLE`], one row per tower, with up to four photos stored in the row itself. This
//! module is the mapping only: the engine opens the archive and the database read-only and hands
//! rows over one at a time, so a project with thousands of embedded photos never sits in memory.
//!
//! Nothing is lost: columns GeoGIS has no form field for are kept as properties
//! ([`EXTRA_COLUMNS`]), and every embedded photo is written out as a file.

use thiserror::Error;

use crate::{AssetDraft, GeoPoint, Properties};

/// The tower database inside a GeoFlux project folder (`AppRouter.kt`).
pub const PROJECT_DB: &str = "local_geodata.db";
/// The tower table (`GeoDataTable.kt`).
pub const TOWER_TABLE: &str = "geo_data";
/// GeoFlux projects are power-line surveys: the built-in telecom schema has the same fields.
pub const LEGACY_SCHEMA: &str = "com.geo.telecom";

/// Most entries a `.geox` may list; GeoFlux wrote one per file in the project folder.
pub const MAX_ENTRIES: usize = 200_000;
/// Largest tower database extracted; photos are inside it, so it can be large.
pub const MAX_DB_BYTES: u64 = 16 * 1024 * 1024 * 1024;
/// Largest single embedded photo written out; bigger ones are reported and skipped.
pub const MAX_PHOTO_BYTES: usize = 64 * 1024 * 1024;
/// Most towers read from one project, the same cap as a survey CSV.
pub const MAX_TOWERS: usize = crate::csv_import::MAX_ROWS;

/// `geo_data` columns that are fields of the telecom schema, as (column, field key), in the
/// form's order. Older GeoFlux databases lack some of them (`phase`, `rise_temp`…): GeoFlux added
/// columns without migrating old files, so the engine reads only the columns a file has.
pub const FIELD_COLUMNS: &[(&str, &str)] = &[
    ("line_name", "lineName"),
    ("tower_number", "towerNumber"),
    ("circuit", "circuit"),
    ("phase", "phase"),
    ("side", "side"),
    ("direction", "direction"),
    ("captured_date", "capturedDate"),
    ("captured_time", "capturedTime"),
    ("humidity", "humidity"),
    ("emissivity", "emissivity"),
    ("ambient_temp", "ambientTemp"),
    ("fault_desc", "faultDescription"),
    ("fault_temp", "faultTemp"),
    ("rise_temp", "riseTemp"),
];

/// The position columns (REAL), read separately from the text ones.
pub const LATITUDE_COLUMN: &str = "latitude";
pub const LONGITUDE_COLUMN: &str = "longitude";

/// Columns with no GeoGIS form field, kept as properties so they show in the attribute table and
/// can be mapped later: GeoFlux's report type and fault flag (its PDF title came from them), the
/// company name, the per-circuit load values (JSON), and its record status and last edit time.
pub const EXTRA_COLUMNS: &[(&str, &str)] = &[
    ("report_type", "reportType"),
    ("fault_status", "faultStatus"),
    ("company_name", "companyName"),
    ("dynamic_circuits", "dynamicCircuits"),
    ("status", "geofluxStatus"),
    ("updated_at", "geofluxUpdatedAt"),
];

/// GeoFlux's four photo columns and the image slots of the telecom schema they fill, as
/// `DetailView.kt` placed them on the form.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LegacySlot {
    /// `img_visual`, shown as "Location".
    Location,
    /// `img_thermal`.
    Thermal,
    /// `img_tower`, shown as Tower, Span, Sleeve or Earth Wire by report type.
    Structure,
    /// `img_extra`, shown as "RGB Image".
    RgbZoom,
}

impl LegacySlot {
    pub const ALL: [LegacySlot; 4] = [LegacySlot::Location, LegacySlot::Thermal, LegacySlot::Structure, LegacySlot::RgbZoom];

    pub fn column(self) -> &'static str {
        match self {
            LegacySlot::Location => "img_visual",
            LegacySlot::Thermal => "img_thermal",
            LegacySlot::Structure => "img_tower",
            LegacySlot::RgbZoom => "img_extra",
        }
    }

    /// The telecom schema's slot id.
    pub fn slot_id(self) -> &'static str {
        match self {
            LegacySlot::Location => "LOCATION",
            LegacySlot::Thermal => "THERMAL",
            LegacySlot::Structure => "STRUCTURE",
            LegacySlot::RgbZoom => "RGB_ZOOM",
        }
    }

    /// The word put in a written photo's file name; the schema's slot matching sends the file back
    /// to this slot when the folder is linked.
    pub fn file_word(self) -> &'static str {
        match self {
            LegacySlot::Location => "location",
            LegacySlot::Thermal => "thermal",
            LegacySlot::Structure => "tower",
            LegacySlot::RgbZoom => "rgb",
        }
    }
}

/// One photo column of one row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegacyPhoto<'a> {
    /// NULL: GeoFlux looked for the photo in the tower's folder on disk instead. Such photos are
    /// not in the `.geox`; linking the survey's photo folder finds them as before.
    NotSet,
    /// An empty value: the photo was removed from this slot in GeoFlux.
    Removed,
    /// The photo's bytes, borrowed from the database for the duration of one row.
    Embedded(&'a [u8]),
}

impl<'a> LegacyPhoto<'a> {
    pub fn from_blob(blob: Option<&'a [u8]>) -> Self {
        match blob {
            None => LegacyPhoto::NotSet,
            Some([]) => LegacyPhoto::Removed,
            Some(bytes) => LegacyPhoto::Embedded(bytes),
        }
    }
}

/// The file extension for embedded photo bytes, from their signature: GeoFlux stored original
/// JPEGs, or a PNG after an edit in its image editor. None for anything else.
pub fn photo_extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("png")
    } else {
        None
    }
}

/// One `geo_data` row as the engine read it.
#[derive(Clone, Debug, PartialEq)]
pub struct LegacyRow<'a> {
    /// GeoFlux's row id, for messages and for naming photos of a tower with no number.
    pub id: i64,
    /// (column, value) for the text columns of [`FIELD_COLUMNS`] and [`EXTRA_COLUMNS`] this file
    /// has; NULL reads as "".
    pub text: Vec<(&'static str, String)>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    /// In [`LegacySlot::ALL`] order.
    pub photos: [LegacyPhoto<'a>; 4],
}

impl LegacyRow<'_> {
    fn value(&self, column: &str) -> &str {
        self.text.iter().find(|(c, _)| *c == column).map_or("", |(_, v)| v.as_str())
    }

    /// The asset this row becomes: form fields in the schema's order, the position as
    /// `core.latitude`/`core.longitude`, then the extra columns. A missing, out-of-range or (0, 0)
    /// position (GeoFlux's placeholder) leaves the asset unplaced, as a CSV row without one is.
    pub fn to_draft(&self) -> AssetDraft {
        let mut properties: Properties = FIELD_COLUMNS.iter().map(|(column, key)| (*key, self.value(column).trim())).collect();
        let location = match (self.latitude, self.longitude) {
            (Some(lat), Some(lon))
                if lat.is_finite() && lon.is_finite() && lat.abs() <= 90.0 && lon.abs() <= 180.0 && (lat, lon) != (0.0, 0.0) =>
            {
                Some(GeoPoint { lat, lon })
            }
            _ => None,
        };
        let coordinate = |v: Option<f64>| if location.is_some() { v.map(|v| v.to_string()).unwrap_or_default() } else { String::new() };
        properties.set("core.latitude", coordinate(self.latitude));
        properties.set("core.longitude", coordinate(self.longitude));
        for (column, key) in EXTRA_COLUMNS {
            let v = self.value(column);
            if !v.is_empty() {
                properties.set(*key, v);
            }
        }
        AssetDraft { location, properties }
    }

    /// The folder its photos are written to: the tower number, so people browsing the photos find
    /// each tower's; or `row-<id>` for a row with no tower number, so its photos aren't lost and
    /// can be added by hand. The engine also uses `row-<id>` when the number can't be a folder
    /// name as it is (`76/0`). Written photos are added to their tower as picked photos, kept by
    /// tower number, so they stay when GeoFlux's own photo folder is linked afterwards.
    pub fn photo_folder(&self) -> String {
        match self.value("tower_number").trim() {
            "" => format!("row-{}", self.id),
            tower => tower.to_string(),
        }
    }
}

/// What reading a project produced, for the summary shown after the import.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LegacyReport {
    pub towers: usize,
    /// Rows with no usable position (imported, placed later).
    pub towers_without_position: usize,
    pub photos_written: usize,
    /// NULL photo columns: the photo was in the survey folder, not in the project.
    pub photos_in_survey_folder: usize,
    /// Photos written to `row-<id>` folders because the tower had no number.
    pub photos_without_tower: usize,
    /// Embedded values that aren't a JPEG or PNG, or are too large, with the row and slot.
    pub photos_skipped: Vec<String>,
    /// Database columns the file lacked (an older GeoFlux version).
    pub missing_columns: Vec<String>,
    /// Rows past [`MAX_TOWERS`] were left out.
    pub truncated: bool,
}

/// Why a `.geox` could not be imported at all.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum LegacyError {
    #[error("this is not a GeoFlux project file (.geox is a zip)")]
    NotAZip,
    #[error("the project has no tower database ({PROJECT_DB})")]
    NoDatabase,
    #[error("the project has more than one tower database; import one project at a time")]
    SeveralDatabases,
    /// SQLite says "file is not a database": GeoFlux's encrypted app database, not a project.
    #[error("the tower database is encrypted or damaged and can't be read")]
    Unreadable,
    #[error("the tower database has no {TOWER_TABLE} table")]
    NoTowerTable,
    #[error("{what} is too large (at most {max})")]
    TooLarge { what: &'static str, max: u64 },
    #[error("could not read the project: {0}")]
    Io(String),
    #[error("cancelled")]
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row<'a>(text: &[(&'static str, &str)], lat: Option<f64>, lon: Option<f64>, photos: [LegacyPhoto<'a>; 4]) -> LegacyRow<'a> {
        LegacyRow { id: 7, text: text.iter().map(|(c, v)| (*c, v.to_string())).collect(), latitude: lat, longitude: lon, photos }
    }

    const NONE: [LegacyPhoto<'static>; 4] = [LegacyPhoto::NotSet; 4];

    #[test]
    fn columns_map_to_the_telecom_schema() {
        let Some(Ok(schema)) = crate::builtin_schema(LEGACY_SCHEMA) else { panic!("telecom schema") };
        for (_, key) in FIELD_COLUMNS {
            assert!(schema.field(key).is_some(), "{key} is not a telecom field");
        }
        for (_, key) in EXTRA_COLUMNS {
            assert!(schema.field(key).is_none(), "{key} would overwrite a form field");
        }
        for slot in LegacySlot::ALL {
            let def = schema.image_slots.iter().find(|s| s.id == slot.slot_id());
            assert!(def.is_some_and(|d| d.matches.iter().any(|m| m == slot.file_word())), "{slot:?}");
        }
    }

    #[test]
    fn a_row_becomes_a_draft() {
        let r = row(
            &[
                ("line_name", "220kV Jaipur"),
                ("tower_number", " 76/0 "),
                ("rise_temp", "21.5"),
                ("report_type", "TOWER"),
                ("company_name", ""),
            ],
            Some(27.5),
            Some(75.2),
            NONE,
        );
        let d = r.to_draft();
        assert_eq!(d.location, Some(GeoPoint { lat: 27.5, lon: 75.2 }));
        assert_eq!(d.properties.get("towerNumber"), "76/0");
        assert_eq!(d.properties.get("riseTemp"), "21.5");
        assert_eq!(d.properties.get("core.latitude"), "27.5");
        assert_eq!(d.properties.get("reportType"), "TOWER");
        assert!(!d.properties.contains("companyName"));
        // Missing columns read as empty fields, so the form flags them.
        assert_eq!(d.properties.get("phase"), "");
        assert_eq!(r.photo_folder(), "76/0");
    }

    #[test]
    fn placeholder_positions_leave_the_tower_unplaced() {
        for (lat, lon) in [(Some(0.0), Some(0.0)), (Some(91.0), Some(75.0)), (Some(f64::NAN), Some(75.0)), (None, Some(75.0))] {
            let d = row(&[("tower_number", "1")], lat, lon, NONE).to_draft();
            assert_eq!(d.location, None, "{lat:?} {lon:?}");
            assert_eq!(d.properties.get("core.latitude"), "");
        }
        assert_eq!(row(&[], None, None, NONE).photo_folder(), "row-7");
    }

    #[test]
    fn photo_columns_and_kinds() {
        assert_eq!(LegacyPhoto::from_blob(None), LegacyPhoto::NotSet);
        assert_eq!(LegacyPhoto::from_blob(Some(&[])), LegacyPhoto::Removed);
        assert_eq!(LegacyPhoto::from_blob(Some(&[1, 2])), LegacyPhoto::Embedded(&[1, 2]));
        assert_eq!(photo_extension(&[0xFF, 0xD8, 0xFF, 0xE1]), Some("jpg"));
        assert_eq!(photo_extension(b"\x89PNG\r\n\x1a\n...."), Some("png"));
        assert_eq!(photo_extension(b"GIF89a"), None);
        assert_eq!(photo_extension(&[]), None);
    }
}
