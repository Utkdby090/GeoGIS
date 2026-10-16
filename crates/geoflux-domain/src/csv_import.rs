//! Survey CSVs (one row per tower, span…) to asset drafts, by a schema's [`CsvProfile`].
//!
//! Works on the file's bytes, so the caller decides how to read it (a path on the desktop, the
//! browser's file picker on the web) and the file is never held open here.

use std::borrow::Cow;
use std::collections::HashMap;

use thiserror::Error;

use crate::asset::{AssetDraft, Properties};
use crate::coords;
use crate::schema::CsvProfile;

/// Larger files are refused rather than read into memory.
pub const MAX_BYTES: usize = 256 * 1024 * 1024;
/// Rows beyond this are not read; the report says so.
pub const MAX_ROWS: usize = 1_000_000;
/// How many skipped rows are described one by one.
pub const MAX_DETAILS: usize = 10;

/// What reading a CSV produced. `skipped_rows` counts rows that could not become an asset;
/// `skipped_details` describes the first few. `truncated` is set when rows past [`MAX_ROWS`] were left out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportReport {
    pub drafts: Vec<AssetDraft>,
    pub skipped_rows: usize,
    pub skipped_details: Vec<String>,
    /// Data rows without a readable position (imported, placed later).
    pub rows_without_position: usize,
    pub truncated: bool,
}

/// Why a file could not be used at all.
#[derive(Debug, Error, PartialEq)]
pub enum ImportError {
    #[error("the CSV file is empty")]
    Empty,
    #[error("the CSV file is larger than {limit} bytes ({bytes} bytes)")]
    TooLarge { bytes: usize, limit: usize },
    #[error("the CSV has no {} column; columns found: {}", quoted(.missing), found_list(.found))]
    MissingColumns { missing: Vec<String>, found: Vec<String> },
    #[error("the CSV has a header row but no data rows")]
    NoDataRows,
    #[error("the CSV could not be read: {0}")]
    Unreadable(String),
}

fn quoted(names: &[String]) -> String {
    names.iter().map(|n| format!("\"{n}\"")).collect::<Vec<_>>().join(" and ")
}

fn found_list(found: &[String]) -> String {
    if found.is_empty() { "none".into() } else { found.join(", ") }
}

/// Reads `bytes` with `profile`. `on_progress` gets 0..=1 at most once per percent.
pub fn read(bytes: &[u8], profile: &CsvProfile, on_progress: &mut dyn FnMut(f32)) -> Result<ImportReport, ImportError> {
    if bytes.len() > MAX_BYTES {
        return Err(ImportError::TooLarge { bytes: bytes.len(), limit: MAX_BYTES });
    }
    let text = decode(bytes);
    if text.trim().is_empty() {
        return Err(ImportError::Empty);
    }

    let mut reader = csv::ReaderBuilder::new()
        .delimiter(detect_delimiter(&text))
        .has_headers(true)
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(text.as_bytes());
    let headers: Vec<String> =
        reader.headers().map_err(|e| ImportError::Unreadable(e.to_string()))?.iter().map(|h| h.trim().to_string()).collect();

    // Header → index, first occurrence wins, without regard to case.
    let mut index: HashMap<String, usize> = HashMap::new();
    for (i, h) in headers.iter().enumerate() {
        index.entry(h.to_lowercase()).or_insert(i);
    }
    let col = |header: &str| index.get(&header.trim().to_lowercase()).copied();

    let missing: Vec<String> = profile
        .required
        .iter()
        .filter_map(|key| profile.columns.iter().find(|c| &c.key == key))
        .filter(|c| col(&c.header).is_none())
        .map(|c| c.header.clone())
        .collect();
    if !missing.is_empty() {
        let found = headers.iter().filter(|h| !h.is_empty()).take(8).cloned().collect();
        return Err(ImportError::MissingColumns { missing, found });
    }

    let mapped: Vec<(Option<usize>, &str, &str)> =
        profile.columns.iter().map(|c| (col(&c.header), c.key.as_str(), c.default.as_str())).collect();
    let lat_col = col(&profile.latitude);
    let lon_col = col(&profile.longitude);
    let known: Vec<String> = profile
        .columns
        .iter()
        .map(|c| &c.header)
        .chain([&profile.latitude, &profile.longitude])
        .chain(profile.ignore.iter())
        .map(|h| h.trim().to_lowercase())
        .collect();
    let extra: Vec<(usize, String)> = match profile.extra_prefix.as_deref().filter(|p| !p.is_empty()) {
        Some(prefix) => headers
            .iter()
            .enumerate()
            .filter(|(_, h)| !h.is_empty() && !known.contains(&h.to_lowercase()))
            .map(|(i, h)| (i, format!("{prefix}{h}")))
            .collect(),
        None => Vec::new(),
    };
    let required_labels = quoted_labels(profile);

    // Records are parsed one at a time into one reused record, so rows are never all held at once.
    let total_hint = text.as_bytes().iter().filter(|&&b| b == b'\n').count().max(1);
    let mut report = ImportReport::default();
    let mut last_percent = None;
    let mut data_rows = 0usize;
    let mut record = csv::StringRecord::new();
    let mut row_number = 1usize; // the header is row 1
    loop {
        match reader.read_record(&mut record) {
            Ok(false) => break,
            Ok(true) => {}
            Err(e) => {
                row_number += 1;
                skip(&mut report, format!("row {row_number}: {e}"));
                continue;
            }
        }
        row_number += 1;
        if record.iter().all(|c| c.is_empty()) {
            continue; // a row of empty cells, as spreadsheets leave at the end of a sheet
        }
        if data_rows >= MAX_ROWS {
            report.truncated = true;
            break;
        }
        data_rows += 1;

        let cell = |i: Option<usize>| i.and_then(|i| record.get(i)).unwrap_or("");
        let mut properties = Properties::new();
        for (i, key, default) in &mapped {
            let v = cell(*i);
            properties.set(*key, if v.is_empty() { *default } else { v });
        }
        if profile.required.iter().any(|k| properties.get(k).is_empty()) {
            skip(&mut report, format!("row {row_number}: no {required_labels}"));
        } else {
            for (i, key) in &extra {
                // A short row just has fewer extra values.
                if let Some(v) = record.get(*i) {
                    properties.set(key.as_str(), v);
                }
            }
            let location = coords::parse_pair(cell(lat_col), cell(lon_col));
            if location.is_none() {
                report.rows_without_position += 1;
            }
            report.drafts.push(AssetDraft { location, properties });
        }

        let percent = (data_rows.min(total_hint) * 100 / total_hint) as u32;
        if last_percent != Some(percent) {
            last_percent = Some(percent);
            on_progress(percent as f32 / 100.0);
        }
    }
    if data_rows == 0 && report.skipped_rows == 0 {
        return Err(ImportError::NoDataRows);
    }
    on_progress(1.0);
    Ok(report)
}

fn skip(report: &mut ImportReport, detail: String) {
    report.skipped_rows += 1;
    if report.skipped_details.len() < MAX_DETAILS {
        report.skipped_details.push(detail);
    }
}

/// The headers of the required keys, for skip messages: `Line Name or Tower No.`.
fn quoted_labels(profile: &CsvProfile) -> String {
    let labels: Vec<&str> =
        profile.required.iter().filter_map(|k| profile.columns.iter().find(|c| &c.key == k)).map(|c| c.header.as_str()).collect();
    labels.join(" or ")
}

/// The text of a CSV as Excel and survey tools write it: UTF-8 (with or without a byte-order
/// mark), UTF-16 with a mark, or the Windows "ANSI" encoding of Excel's plain CSV export (where
/// "°" is one byte). Reading ANSI as UTF-8 would turn every "°" into a replacement character and
/// lose the position.
/// Valid UTF-8 is borrowed, not copied, so a large file is not held twice.
pub fn decode(bytes: &[u8]) -> Cow<'_, str> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return Cow::Owned(utf16(rest, u16::from_le_bytes));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return Cow::Owned(utf16(rest, u16::from_be_bytes));
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => Cow::Borrowed(s),
        Err(_) => Cow::Owned(bytes.iter().map(|&b| windows_1252(b)).collect()),
    }
}

fn utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> String {
    let units = bytes.as_chunks::<2>().0.iter().copied().map(unit);
    char::decode_utf16(units).map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER)).collect()
}

/// One Windows-1252 byte as a character. 0x80–0x9F hold typographic marks; the rest match Latin-1.
fn windows_1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž', '\u{8F}', '\u{90}', '‘', '’', '“', '”', '•',
        '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9D}', 'ž', 'Ÿ',
    ];
    match b {
        0x80..=0x9F => HIGH.get(usize::from(b - 0x80)).copied().unwrap_or(char::REPLACEMENT_CHARACTER),
        _ => char::from(b),
    }
}

/// The separator of a CSV: spreadsheets in many countries write ";" because "," is their decimal
/// mark, and tools sometimes write tabs. Judged on the header row, ignoring separators inside
/// quotes; a comma unless another separator is strictly more common.
pub fn detect_delimiter(text: &str) -> u8 {
    let Some(header) = text.lines().find(|l| !l.trim().is_empty()) else { return b',' };
    let (mut comma, mut semi, mut tab) = (0usize, 0usize, 0usize);
    let mut quoted = false;
    for c in header.chars() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => comma += 1,
            ';' if !quoted => semi += 1,
            '\t' if !quoted => tab += 1,
            _ => {}
        }
    }
    if semi > comma && semi >= tab {
        b';'
    } else if tab > comma && tab > semi {
        b'\t'
    } else {
        b','
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::GeoPoint;
    use crate::builtin_schema;

    fn telecom() -> CsvProfile {
        builtin_schema("com.geo.telecom").expect("listed").expect("valid").csv.expect("csv profile")
    }

    fn run(bytes: &[u8]) -> Result<ImportReport, ImportError> {
        read(bytes, &telecom(), &mut |_| {})
    }

    const SAMPLE: &str = "Sr. No.,Tower No.,Line Name,CKT,Lat.,Long.,Ambeint Temp.,Fault Temp.,Rise Temp.,report_Type,Fault,Load A,Load B\n\
        1,76_0,Bhadla-Sikar,1,27°30'N,75°10'E,30,45,15,tower_fault,Fault,120,130\n\
        2,77_0,Bhadla-Sikar,1,27.51,75.17,30,,,,,,\n\
        3,,Bhadla-Sikar,1,27.52,75.18,,,,,,,\n\
        ,,,,,,,,,,,,\n";

    #[test]
    fn reads_the_survey_sheet() {
        let mut progress = Vec::new();
        let report = read(SAMPLE.as_bytes(), &telecom(), &mut |p| progress.push(p)).expect("read");
        assert_eq!(report.drafts.len(), 2);
        assert_eq!(report.skipped_rows, 1);
        assert_eq!(report.skipped_details, vec!["row 4: no Tower No. or Line Name".to_string()]);
        let first = &report.drafts[0];
        assert_eq!(first.location, Some(GeoPoint { lat: 27.5, lon: 75.0 + 10.0 / 60.0 }));
        assert_eq!(first.properties.get("towerNumber"), "76_0");
        assert_eq!(first.properties.get("riseTemp"), "15");
        assert_eq!(first.properties.get("load.Load A"), "120");
        assert_eq!(first.properties.get("load.Load B"), "130");
        // Defaults fill blank cells; the row-number column is ignored.
        let second = &report.drafts[1];
        assert_eq!(second.properties.get("reportType"), "tower");
        assert_eq!(second.properties.get("faultStatus"), "normal");
        assert!(!second.properties.contains("load.Sr. No."));
        assert_eq!(progress.last(), Some(&1.0));
        assert!(progress.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn windows_ansi_degree_sign_and_semicolons_are_understood() {
        let mut bytes = b"Tower No.;Line Name;Lat.;Long.\n1;L;27".to_vec();
        bytes.push(0xB0); // "°" in Windows-1252
        bytes.extend_from_slice(b"30'N;75,5\n");
        let report = run(&bytes).expect("read");
        assert_eq!(report.drafts[0].location, Some(GeoPoint { lat: 27.5, lon: 75.5 }));
    }

    #[test]
    fn utf16_and_bom_files_are_decoded() {
        let text = "Tower No.,Line Name\n1,L\n";
        let mut le = vec![0xFF, 0xFE];
        le.extend(text.encode_utf16().flat_map(|u| u.to_le_bytes()));
        assert_eq!(run(&le).expect("utf16").drafts.len(), 1);
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice(text.as_bytes());
        let report = run(&bom).expect("bom");
        assert_eq!(report.drafts.len(), 1);
        assert_eq!(report.rows_without_position, 1);
    }

    #[test]
    fn unusable_files_say_why() {
        assert_eq!(run(b""), Err(ImportError::Empty));
        assert_eq!(run(b"Tower No.,Line Name\n"), Err(ImportError::NoDataRows));
        match run(b"Name,Lat.\nx,1\n") {
            Err(ImportError::MissingColumns { missing, found }) => {
                assert_eq!(missing, vec!["Tower No.".to_string(), "Line Name".to_string()]);
                assert_eq!(found, vec!["Name".to_string(), "Lat.".to_string()]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn delimiter_detection_ignores_quoted_separators() {
        assert_eq!(detect_delimiter("a;b;c"), b';');
        assert_eq!(detect_delimiter("\"a,b,c\";d;e"), b';');
        assert_eq!(detect_delimiter("a\tb\tc"), b'\t');
        assert_eq!(detect_delimiter("a,b;c"), b',');
        assert_eq!(detect_delimiter(""), b',');
    }

    #[test]
    fn hostile_bytes_never_panic() {
        let profile = telecom();
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        for len in [1usize, 7, 64, 512, 4096] {
            let bytes: Vec<u8> = (0..len)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    (seed & 0xFF) as u8
                })
                .collect();
            let _ = read(&bytes, &profile, &mut |_| {});
            let mut with_header = b"Tower No.,Line Name,Lat.,Long.\n".to_vec();
            with_header.extend_from_slice(&bytes);
            let _ = read(&with_header, &profile, &mut |_| {});
        }
        let _ = read(b"\"unterminated,Tower No.\n1,2", &profile, &mut |_| {});
        let _ = read(&[0xFF, 0xFE, 0x00], &profile, &mut |_| {});
    }
}
