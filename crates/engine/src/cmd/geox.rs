//! GeoFlux › Import GeoFlux Project: a GeoFlux 1.x project (`.geox`) as a tower layer.
//!
//! [`read`] touches only files, so the app can run it on a worker thread with a progress bar and
//! cancel; [`apply`] then adds the layer on the session. `geoflux.importGeox` does both in one
//! command. The mapping rules are in [`geoflux_domain::legacy`].
//!
//! The archive is treated as hostile. Only the tower database is taken from it, found by name and
//! copied into a private temporary file: entry names are never used as paths. SQLite opens that
//! copy read-only, with nothing in it allowed to run. Rows are read one at a time and each
//! embedded photo is written out as soon as it is read, so memory holds one tower's photos at once.
//!
//! Embedded photos are the ones a person put in a slot in GeoFlux; the rest stayed in GeoFlux's
//! photo folder on disk. So written photos are added to their towers as picked photos, which
//! stay when that photo folder is linked with Link Photos afterwards.

use geoflux_domain::AssetDraft;
use geoflux_domain::legacy::{LEGACY_SCHEMA, LegacyReport};
use geoflux_report::Progress;
use serde_json::{Value, json};

use super::geoflux;
use super::ps;
use crate::{CmdResult, CommandSpec as C, Session};

/// Most skipped photos listed one by one in the summary (all are counted).
#[cfg(not(target_arch = "wasm32"))]
const MAX_SKIPPED_LISTED: usize = 50;

pub fn commands() -> Vec<C> {
    vec![C::new("geoflux.importGeox", "Import GeoFlux Project", "GeoFlux › Inspection", import_geox).params(
        r#"{"path": "a GeoFlux project (.geox)", "photos"?: "a new or empty folder for the photos stored in the project (default: '<file name> photos' beside it)", "name"?: "the layer (default: the file name)"}"#,
    )]
}

/// A read project, ready for [`apply`]. Dropping it without applying removes the photos written.
pub struct GeoxProject {
    /// The layer name when none is given: the file name without `.geox`.
    pub name: String,
    drafts: Vec<AssetDraft>,
    /// (tower label, slot id, photo path) for each written photo of a tower with a number.
    picked: Vec<(String, &'static str, String)>,
    pub report: LegacyReport,
    photos: PhotoFolder,
}

impl GeoxProject {
    pub fn towers(&self) -> usize {
        self.drafts.len()
    }
}

fn import_geox(s: &mut Session, p: &Value) -> CmdResult {
    let path = ps(p, "path").map(str::trim).filter(|x| !x.is_empty()).ok_or("path is required (a GeoFlux project, .geox)")?;
    let project = read(path, ps(p, "photos").map(str::trim).filter(|x| !x.is_empty()), &Progress::new())?;
    add(s, project, ps(p, "name"))
}

/// Adds a read project as a tower layer, as one step to undo.
pub fn apply(s: &mut Session, project: GeoxProject, name: Option<&str>) -> CmdResult {
    s.guarded("Import GeoFlux Project", true, move |s| add(s, project, name))
}

fn add(s: &mut Session, mut project: GeoxProject, name: Option<&str>) -> CmdResult {
    if project.drafts.is_empty() {
        return Err("the GeoFlux project has no towers".into());
    }
    let schema = s.project.geoflux.schema(LEGACY_SCHEMA)?;
    let name = name.map(str::trim).filter(|n| !n.is_empty()).unwrap_or(&project.name).to_string();
    let id = geoflux::add_inspection_layer(s, &schema, &name, &project.drafts);
    let mut not_added = 0usize;
    for (label, slot, path) in &project.picked {
        if !geoflux::add_picked(&mut s.project.geoflux, id, label, slot, path) {
            not_added += 1;
        }
    }
    project.photos.keep();
    let r = &project.report;
    let folder = project.photos.written().then(|| project.photos.root.to_string_lossy().into_owned());
    let mut note = format!("Imported {} towers from the GeoFlux project", r.towers);
    if let Some(f) = &folder {
        note.push_str(&format!("; {} photos stored in it were written to {f}", r.photos_written));
    }
    if r.photos_in_survey_folder > 0 {
        note.push_str(&format!(
            "; {} photos were in GeoFlux's photo folder, not in the project: link that folder with Link Photos",
            r.photos_in_survey_folder
        ));
    }
    s.notify(note, false);
    if r.towers_without_position > 0 {
        s.notify(format!("{} towers have no position yet", r.towers_without_position), false);
    }
    if r.photos_without_tower > 0 {
        s.notify(
            format!("{} photos belong to rows with no tower number; they are in the photo folder's row-… folders", r.photos_without_tower),
            false,
        );
    }
    if !r.photos_skipped.is_empty() {
        s.notify(format!("Photos left out: {}", r.photos_skipped.join("; ")), false);
    }
    if not_added > 0 {
        s.notify(
            format!("{not_added} written photos could not be added to their towers (too many photos); link the photo folder instead"),
            true,
        );
    }
    if r.truncated {
        s.notify(format!("Only the first {} towers were imported", geoflux_domain::legacy::MAX_TOWERS), true);
    }
    Ok(json!({
        "layer": id,
        "towers": r.towers,
        "withoutPosition": r.towers_without_position,
        "photosFolder": folder,
        "photosWritten": r.photos_written,
        "photosAdded": project.picked.len() - not_added,
        "photosInSurveyFolder": r.photos_in_survey_folder,
        "photosWithoutTower": r.photos_without_tower,
        "photosSkipped": r.photos_skipped,
        "missingColumns": r.missing_columns,
        "truncated": r.truncated,
    }))
}

/// The folder embedded photos are written to. It is made only when the first photo is written,
/// and removed again (or emptied, when it was an empty folder already) unless [`Self::keep`] is
/// called, so a failed or cancelled import leaves nothing behind.
struct PhotoFolder {
    root: std::path::PathBuf,
    /// The folder was there (empty) before the import.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    existed: bool,
    made: bool,
    kept: bool,
}

impl PhotoFolder {
    fn keep(&mut self) {
        self.kept = true;
    }

    fn written(&self) -> bool {
        self.made
    }
}

impl Drop for PhotoFolder {
    fn drop(&mut self) {
        if self.kept || !self.made {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            // Everything in it was written by this import: the folder was new or empty.
            let _ = std::fs::remove_dir_all(&self.root);
            if self.existed {
                let _ = std::fs::create_dir(&self.root);
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub fn read(path: &str, _photos: Option<&str>, _progress: &Progress) -> Result<GeoxProject, String> {
    Err(format!("{path}: importing GeoFlux projects needs the desktop app"))
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::read;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::fs::{self, File, OpenOptions};
    use std::io::{self, Read, Write};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use geoflux_domain::legacy::{
        EXTRA_COLUMNS, FIELD_COLUMNS, LATITUDE_COLUMN, LONGITUDE_COLUMN, LegacyError, LegacyPhoto, LegacyReport, LegacyRow, LegacySlot,
        MAX_DB_BYTES, MAX_ENTRIES, MAX_PHOTO_BYTES, MAX_TOWERS, PROJECT_DB, TOWER_TABLE, photo_extension,
    };
    use geoflux_report::Progress;
    use rusqlite::types::ValueRef;
    use rusqlite::{Connection, OpenFlags};

    use super::{GeoxProject, MAX_SKIPPED_LISTED, PhotoFolder};

    /// Most "photos" folders tried beside the project before giving up (`x photos 2`…).
    const MAX_FOLDER_TRIES: usize = 100;
    /// Most photos of one slot with the same name in one tower folder (`thermal_2.jpg`…).
    const MAX_SAME_NAME: usize = 1_000;

    /// Reads the project at `path`, writing its embedded photos into `photos` (a new or empty
    /// folder; by default `<name> photos` beside the project). Safe to call off the session's
    /// thread: it touches only the project, a temporary file and the photo folder.
    pub fn read(path: &str, photos: Option<&str>, progress: &Progress) -> Result<GeoxProject, String> {
        let geox = Path::new(path);
        let name = geox.file_stem().map_or_else(|| "GeoFlux project".to_string(), |s| s.to_string_lossy().into_owned());
        let photos = photo_folder(geox, &name, photos)?;
        let db = extract_db(geox).map_err(|e| message(path, e))?;
        let conn = open_db(&db.0).map_err(|e| message(path, e))?;
        let mut project = GeoxProject { name, drafts: Vec::new(), picked: Vec::new(), report: LegacyReport::default(), photos };
        read_rows(&conn, &mut project, progress).map_err(|e| message(path, e))?;
        // The connection closes before the temporary copy is removed.
        drop(conn);
        drop(db);
        Ok(project)
    }

    fn message(path: &str, e: LegacyError) -> String {
        match e {
            LegacyError::Cancelled => "cancelled".into(),
            e => format!("{path}: {e}"),
        }
    }

    fn io_error(e: io::Error) -> LegacyError {
        LegacyError::Io(e.to_string())
    }

    /// The photo folder: the one given, which must be new or empty, else the first free
    /// `<name> photos`, `<name> photos 2`… beside the project, so an earlier import's photos are
    /// never mixed in or overwritten.
    fn photo_folder(geox: &Path, name: &str, given: Option<&str>) -> Result<PhotoFolder, String> {
        let folder = |root: PathBuf, existed: bool| PhotoFolder { root, existed, made: false, kept: false };
        if let Some(given) = given {
            let root = PathBuf::from(given);
            return match fs::read_dir(&root).map(|mut entries| entries.next().is_none()) {
                Ok(true) => Ok(folder(root, true)),
                Ok(false) => Err(format!("{given}: choose a new or empty folder for the project's photos")),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(folder(root, false)),
                Err(e) => Err(format!("{given}: {e}")),
            };
        }
        let dir = geox.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
        for i in 1..=MAX_FOLDER_TRIES {
            let root = dir.join(if i == 1 { format!("{name} photos") } else { format!("{name} photos {i}") });
            match fs::symlink_metadata(&root) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(folder(root, false)),
                _ => continue,
            }
        }
        Err(format!("{}: no free folder for the project's photos; give one with \"photos\"", dir.display()))
    }

    impl PhotoFolder {
        /// The tower's folder, made on first use. The root is made with `create_dir`, so a folder
        /// that appeared since it was chosen is never written into.
        fn tower_dir(&mut self, tower: &str) -> io::Result<PathBuf> {
            if !self.made {
                if !self.existed {
                    if let Some(parent) = self.root.parent().filter(|p| !p.as_os_str().is_empty()) {
                        fs::create_dir_all(parent)?;
                    }
                    fs::create_dir(&self.root)?;
                }
                self.made = true;
            }
            let dir = self.root.join(tower);
            match fs::create_dir(&dir) {
                Err(e) if e.kind() != io::ErrorKind::AlreadyExists => Err(e),
                _ => Ok(dir),
            }
        }
    }

    /// A temporary file removed when dropped.
    pub(super) struct TempFile(pub(super) PathBuf);

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    /// A new, empty file in the temporary folder that only this user can read.
    fn temp_file() -> io::Result<(TempFile, File)> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir();
        let mut last = None;
        for _ in 0..16 {
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = dir.join(format!("geogis-geox-{}-{n}.db", std::process::id()));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(f) => return Ok((TempFile(path), f)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap_or_else(|| io::Error::other("no free temporary file name")))
    }

    /// Copies the project's one tower database out of the zip into a temporary file. Entry names
    /// are only compared, never used as paths.
    pub(super) fn extract_db(geox: &Path) -> Result<TempFile, LegacyError> {
        let file = File::open(geox).map_err(io_error)?;
        let mut zip = zip::ZipArchive::new(io::BufReader::new(file)).map_err(|e| match e {
            zip::result::ZipError::Io(e) => io_error(e),
            _ => LegacyError::NotAZip,
        })?;
        if zip.len() > MAX_ENTRIES {
            return Err(LegacyError::TooLarge { what: "the project's file list", max: MAX_ENTRIES as u64 });
        }
        let mut found = None;
        for i in 0..zip.len() {
            let Some(name) = zip.name_for_index(i) else { continue };
            let last = name.rsplit(['/', '\\']).next().unwrap_or(name);
            if last.eq_ignore_ascii_case(PROJECT_DB) {
                if found.is_some() {
                    return Err(LegacyError::SeveralDatabases);
                }
                found = Some(i);
            }
        }
        let index = found.ok_or(LegacyError::NoDatabase)?;
        let entry = zip.by_index(index).map_err(|e| match e {
            zip::result::ZipError::Io(e) => io_error(e),
            _ => LegacyError::Unreadable,
        })?;
        if entry.size() > MAX_DB_BYTES {
            return Err(LegacyError::TooLarge { what: "the tower database", max: MAX_DB_BYTES });
        }
        let (temp, out) = temp_file().map_err(io_error)?;
        let mut out = io::BufWriter::new(out);
        // The size in the zip is the writer's claim: the copy is cut off past the limit anyway.
        let copied = io::copy(&mut entry.take(MAX_DB_BYTES + 1), &mut out).map_err(|e| match e.kind() {
            // A bad checksum or deflate stream.
            io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof => LegacyError::Unreadable,
            _ => io_error(e),
        })?;
        if copied > MAX_DB_BYTES {
            return Err(LegacyError::TooLarge { what: "the tower database", max: MAX_DB_BYTES });
        }
        out.flush().map_err(io_error)?;
        Ok(temp)
    }

    /// Opens the copied database read-only. Nothing it defines (views, triggers) may call
    /// functions, nothing writes, and its page cache is capped.
    pub(super) fn open_db(path: &Path) -> Result<Connection, LegacyError> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
            .map_err(|_| LegacyError::Unreadable)?;
        let _ = conn.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false);
        let _ = conn.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true);
        conn.execute_batch("PRAGMA cache_size = -65536; PRAGMA query_only = ON;").map_err(|_| LegacyError::Unreadable)?;
        // Reading the schema proves it is a database ("file is not a database" otherwise).
        conn.query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get::<_, i64>(0)).map_err(|_| LegacyError::Unreadable)?;
        Ok(conn)
    }

    /// A column's value as text: numbers as written, NULL and blobs as "".
    fn text(v: ValueRef<'_>) -> String {
        match v {
            ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
            ValueRef::Integer(i) => i.to_string(),
            ValueRef::Real(f) => f.to_string(),
            ValueRef::Null | ValueRef::Blob(_) => String::new(),
        }
    }

    fn real(v: ValueRef<'_>) -> Option<f64> {
        match v {
            ValueRef::Real(f) => Some(f),
            ValueRef::Integer(i) => Some(i as f64),
            ValueRef::Text(t) => std::str::from_utf8(t).ok()?.trim().parse().ok(),
            ValueRef::Null | ValueRef::Blob(_) => None,
        }
    }

    /// Whether `name` can be a folder name on Windows, macOS and Linux as it is, so the written
    /// folder carries the tower number unchanged.
    pub(super) fn is_folder_name(name: &str) -> bool {
        const RESERVED: &[&str] = &["CON", "PRN", "AUX", "NUL"];
        let stem = name.split('.').next().unwrap_or(name).trim_end().to_ascii_uppercase();
        let device = RESERVED.contains(&stem.as_str())
            || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit());
        !name.is_empty()
            && name.len() <= 120
            && name != "."
            && name != ".."
            && !name.starts_with(' ')
            && !name.ends_with(['.', ' '])
            && !name.chars().any(|c| c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'))
            && !device
    }

    /// Writes `bytes` as `<word>.<ext>` in `dir`, or `<word>_2.<ext>`… when that name is taken,
    /// never replacing a file. A file only partly written is removed.
    fn write_photo(dir: &Path, word: &str, ext: &str, bytes: &[u8]) -> io::Result<PathBuf> {
        for i in 1..=MAX_SAME_NAME {
            let path = dir.join(if i == 1 { format!("{word}.{ext}") } else { format!("{word}_{i}.{ext}") });
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    if let Err(e) = f.write_all(bytes) {
                        drop(f);
                        let _ = fs::remove_file(&path);
                        return Err(e);
                    }
                    return Ok(path);
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::other(format!("{}: too many photos named {word}", dir.display())))
    }

    /// The photo columns as read: the value, unless it was too large to take.
    enum Photo<'a> {
        Read(LegacyPhoto<'a>),
        TooLarge,
    }

    fn read_rows(conn: &Connection, project: &mut GeoxProject, progress: &Progress) -> Result<(), LegacyError> {
        let db = |_| LegacyError::Unreadable;
        let columns: Vec<String> = {
            let mut st = conn.prepare("SELECT name FROM pragma_table_info(?1)").map_err(db)?;
            let rows = st.query_map([TOWER_TABLE], |r| r.get::<_, String>(0)).map_err(db)?;
            rows.filter_map(Result::ok).map(|c| c.to_ascii_lowercase()).collect()
        };
        if columns.is_empty() {
            return Err(LegacyError::NoTowerTable);
        }
        let has = |c: &str| columns.iter().any(|x| x == c);
        let report = &mut project.report;
        let text_columns: Vec<&'static str> = FIELD_COLUMNS.iter().chain(EXTRA_COLUMNS).map(|(c, _)| *c).filter(|c| has(c)).collect();
        let photo_columns = LegacySlot::ALL.map(|s| has(s.column()));
        for c in FIELD_COLUMNS.iter().map(|(c, _)| *c).chain([LATITUDE_COLUMN, LONGITUDE_COLUMN]).chain(LegacySlot::ALL.map(|s| s.column()))
        {
            if !has(c) {
                report.missing_columns.push(c.to_string());
            }
        }

        // Column names are this module's own constants, checked against the table above.
        let id = if has("id") { "\"id\"" } else { "rowid" };
        let mut select = vec![id.to_string()];
        select.extend(text_columns.iter().map(|c| format!("\"{c}\"")));
        for c in [LATITUDE_COLUMN, LONGITUDE_COLUMN] {
            select.push(if has(c) { format!("\"{c}\"") } else { "NULL".into() });
        }
        for (slot, present) in LegacySlot::ALL.iter().zip(photo_columns) {
            let c = slot.column();
            if present {
                // A value too large to take is left in the database: only its size is read.
                select.push(format!("octet_length(\"{c}\")"));
                select.push(format!("CASE WHEN octet_length(\"{c}\") <= {MAX_PHOTO_BYTES} THEN \"{c}\" END"));
            } else {
                select.extend(["NULL".to_string(), "NULL".to_string()]);
            }
        }
        let total: i64 = conn.query_row(&format!("SELECT count(*) FROM \"{TOWER_TABLE}\""), [], |r| r.get(0)).map_err(db)?;
        progress.start(usize::try_from(total).unwrap_or(0).min(MAX_TOWERS));
        let sql = format!("SELECT {} FROM \"{TOWER_TABLE}\" ORDER BY {id} LIMIT {}", select.join(", "), MAX_TOWERS + 1);
        let mut st = conn.prepare(&sql).map_err(db)?;
        let mut rows = st.query([]).map_err(db)?;
        let lat_at = 1 + text_columns.len();
        let mut n = 0usize;
        while let Some(row) = rows.next().map_err(db)? {
            if n == MAX_TOWERS {
                project.report.truncated = true;
                break;
            }
            n += 1;
            let get = |i: usize| row.get_ref(i).unwrap_or(ValueRef::Null);
            let id = match get(0) {
                ValueRef::Integer(i) => i,
                _ => n as i64,
            };
            let photos: [Photo<'_>; 4] = std::array::from_fn(|k| {
                let at = lat_at + 2 + k * 2;
                match (photo_columns[k], get(at), get(at + 1)) {
                    // An older file without the column: nothing to report.
                    (false, _, _) => Photo::Read(LegacyPhoto::Removed),
                    (true, ValueRef::Null, _) => Photo::Read(LegacyPhoto::NotSet),
                    (true, ValueRef::Integer(0), _) => Photo::Read(LegacyPhoto::Removed),
                    (true, _, ValueRef::Blob(b) | ValueRef::Text(b)) => Photo::Read(LegacyPhoto::from_blob(Some(b))),
                    (true, _, ValueRef::Null) => Photo::TooLarge,
                    // A number in a photo column: not a photo.
                    (true, _, ValueRef::Integer(_) | ValueRef::Real(_)) => Photo::Read(LegacyPhoto::Embedded(&[])),
                }
            });
            let legacy = LegacyRow {
                id,
                text: text_columns.iter().enumerate().map(|(i, c)| (*c, text(get(1 + i)))).collect(),
                latitude: real(get(lat_at)),
                longitude: real(get(lat_at + 1)),
                photos: [LegacyPhoto::NotSet; 4],
            };
            let draft = legacy.to_draft();
            let report = &mut project.report;
            report.towers += 1;
            if draft.location.is_none() {
                report.towers_without_position += 1;
            }
            let tower = draft.properties.get("towerNumber").to_string();
            // A tower number that can't be a folder name as it is gets a row folder instead.
            let folder = if tower.is_empty() || is_folder_name(&tower) { legacy.photo_folder() } else { format!("row-{id}") };
            let skip = |report: &mut LegacyReport, why: String| {
                if report.photos_skipped.len() < MAX_SKIPPED_LISTED {
                    report.photos_skipped.push(why);
                } else if report.photos_skipped.len() == MAX_SKIPPED_LISTED {
                    report.photos_skipped.push("and more".into());
                }
            };
            for (slot, photo) in LegacySlot::ALL.iter().zip(&photos) {
                // Only for the messages of skipped photos, so built only then.
                let who = || if tower.is_empty() { format!("row {id}") } else { format!("tower {tower}") };
                let bytes = match photo {
                    Photo::Read(LegacyPhoto::NotSet) => {
                        report.photos_in_survey_folder += 1;
                        continue;
                    }
                    Photo::Read(LegacyPhoto::Removed) => continue,
                    Photo::TooLarge => {
                        skip(report, format!("{} {}: larger than {} MB", who(), slot.file_word(), MAX_PHOTO_BYTES >> 20));
                        continue;
                    }
                    Photo::Read(LegacyPhoto::Embedded(b)) => *b,
                };
                let Some(ext) = photo_extension(bytes) else {
                    skip(report, format!("{} {}: not a JPEG or PNG", who(), slot.file_word()));
                    continue;
                };
                let dir = project.photos.tower_dir(&folder).map_err(io_error)?;
                let written = write_photo(&dir, slot.file_word(), ext, bytes).map_err(io_error)?;
                report.photos_written += 1;
                if tower.is_empty() {
                    report.photos_without_tower += 1;
                } else {
                    project.picked.push((tower.clone(), slot.slot_id(), written.to_string_lossy().into_owned()));
                }
            }
            project.drafts.push(draft);
            progress.step().map_err(|_| LegacyError::Cancelled)?;
        }
        Ok(())
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::io::Write;
    use std::path::Path;

    use super::native::{extract_db, is_folder_name};
    use super::*;
    use crate::Project;

    const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3];
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n0000";

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("geogis-geox-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A GeoFlux-style project database: an older layout without `phase` and `img_extra`.
    fn database(path: &Path, rows: &[(&str, f64, f64, Option<&[u8]>, Option<&[u8]>)]) {
        let c = rusqlite::Connection::open(path).unwrap();
        c.execute_batch(
            "CREATE TABLE geo_data (id INTEGER PRIMARY KEY AUTOINCREMENT, line_name VARCHAR(255), tower_number VARCHAR(255) DEFAULT '',
             latitude DOUBLE, longitude DOUBLE, rise_temp VARCHAR(255), report_type TEXT, img_thermal BLOB, img_visual BLOB, img_tower BLOB)",
        )
        .unwrap();
        for (tower, lat, lon, thermal, visual) in rows {
            c.execute(
                "INSERT INTO geo_data (line_name, tower_number, latitude, longitude, rise_temp, report_type, img_thermal, img_visual, img_tower)
                 VALUES ('220kV Bhadla', ?1, ?2, ?3, '21.5', 'TOWER', ?4, ?5, x'')",
                rusqlite::params![tower, lat, lon, thermal, visual],
            )
            .unwrap();
        }
    }

    /// Zips `files` as GeoFlux does: under the project's folder name.
    fn geox(path: &Path, files: &[(&str, &[u8])]) {
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, bytes) in files {
            z.start_file(format!("Line 1/{name}"), zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(bytes).unwrap();
        }
        z.finish().unwrap();
    }

    fn project_file(d: &Path, rows: &[(&str, f64, f64, Option<&[u8]>, Option<&[u8]>)]) -> String {
        let db = d.join("local_geodata.db");
        database(&db, rows);
        let bytes = std::fs::read(&db).unwrap();
        let path = d.join("Line 1.geox");
        geox(&path, &[("local_geodata.db", &bytes), ("notes.txt", b"hello")]);
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn a_project_becomes_a_rated_layer_with_its_photos() {
        let d = dir("import");
        let path = project_file(
            &d,
            &[
                ("76_0", 27.5, 75.2, Some(JPEG), None),
                ("76/0", 27.6, 75.3, Some(PNG), Some(JPEG)),
                ("", 0.0, 0.0, Some(JPEG), Some(b"GIF89a")),
            ],
        );
        let mut s = Session::new(Project::default());
        let r = s.run("geoflux.importGeox", &json!({"path": path})).expect("import");
        assert_eq!(r["towers"], 3);
        assert_eq!(r["withoutPosition"], 1);
        assert_eq!(r["photosWritten"], 4);
        assert_eq!(r["photosAdded"], 3, "the row with no tower number keeps its photo on disk only");
        assert_eq!(r["photosWithoutTower"], 1);
        assert_eq!(r["photosInSurveyFolder"], 1, "a NULL photo was in GeoFlux's folder");
        assert_eq!(r["photosSkipped"].as_array().map(Vec::len), Some(1), "the GIF");
        let missing: Vec<&str> = r["missingColumns"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        assert!(missing.contains(&"phase") && missing.contains(&"img_extra"), "{missing:?}");

        let folder = d.join("Line 1 photos");
        assert_eq!(r["photosFolder"].as_str(), Some(folder.to_string_lossy().as_ref()));
        assert!(folder.join("76_0").join("thermal.jpg").is_file());
        // `76/0` can't be a folder name: its photos go in a row folder and are added by tower.
        assert!(folder.join("row-2").join("thermal.png").is_file());
        assert!(folder.join("row-3").join("thermal.jpg").is_file());

        let id = r["layer"].as_u64().unwrap() as u32;
        let layer = s.map().layer(id).unwrap();
        assert_eq!(layer.name, "Line 1");
        assert_eq!(layer.dataset.features.len(), 3);
        assert_eq!(s.project.geoflux.schema_of(id), Some(LEGACY_SCHEMA));
        let slots = geoflux::photos(&s, id, 1).unwrap();
        let shown = |slot: &str| slots.iter().find(|sp| sp.slot.id == slot).map(|sp| sp.photos.clone()).unwrap_or_default();
        assert_eq!(shown("THERMAL").len(), 1);
        assert!(shown("THERMAL")[0].ends_with("thermal.png"));
        assert_eq!(shown("LOCATION").len(), 1);
        // The photos stay through linking GeoFlux's own photo folder later.
        let survey = dir("survey");
        std::fs::create_dir_all(survey.join("76_0")).unwrap();
        std::fs::write(survey.join("76_0").join("DJI_0001_T.JPG"), JPEG).unwrap();
        s.run("geoflux.linkPhotos", &json!({"layer": id, "folder": survey.to_string_lossy()})).expect("link");
        let slots = geoflux::photos(&s, id, 0).unwrap();
        assert_eq!(slots.iter().find(|sp| sp.slot.id == "THERMAL").map(|sp| sp.photos.len()), Some(2));

        // Undo (the link, then the import) removes the layer; a second import writes to a new folder.
        assert!(s.undo() && s.undo());
        assert!(s.map().layer(id).is_none());
        let r = s.run("geoflux.importGeox", &json!({"path": path, "name": "Again"})).expect("again");
        assert_eq!(r["photosFolder"].as_str(), Some(d.join("Line 1 photos 2").to_string_lossy().as_ref()));
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&survey);
    }

    #[test]
    fn a_cancelled_or_failed_import_leaves_nothing() {
        let d = dir("cancel");
        let path = project_file(&d, &[("1", 27.5, 75.2, Some(JPEG), None), ("2", 27.6, 75.3, Some(JPEG), None)]);
        let progress = Progress::new();
        progress.cancel();
        assert_eq!(read(&path, None, &progress).err().as_deref(), Some("cancelled"));
        assert!(!d.join("Line 1 photos").exists());
        // A read project dropped without being applied removes its photos too.
        let project = read(&path, None, &Progress::new()).expect("read");
        assert_eq!(project.towers(), 2);
        assert!(d.join("Line 1 photos").exists());
        drop(project);
        assert!(!d.join("Line 1 photos").exists());
        // A folder given must be new or empty; an empty one is left empty after a cancel.
        let full = d.join("full");
        std::fs::create_dir_all(&full).unwrap();
        std::fs::write(full.join("keep.txt"), b"mine").unwrap();
        assert!(read(&path, full.to_str(), &Progress::new()).is_err());
        assert!(full.join("keep.txt").is_file());
        let empty = d.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        drop(read(&path, empty.to_str(), &Progress::new()).expect("read"));
        assert!(empty.is_dir() && std::fs::read_dir(&empty).unwrap().next().is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn hostile_or_wrong_files_are_refused() {
        let d = dir("bad");
        let mut s = Session::new(Project::default());
        let run = |s: &mut Session, path: &Path| s.run("geoflux.importGeox", &json!({"path": path.to_string_lossy()})).unwrap_err();

        let not_zip = d.join("a.geox");
        std::fs::write(&not_zip, b"not a zip").unwrap();
        assert!(run(&mut s, &not_zip).contains("not a GeoFlux project"));

        let no_db = d.join("b.geox");
        geox(&no_db, &[("readme.txt", b"x")]);
        assert!(run(&mut s, &no_db).contains("no tower database"));

        let two = d.join("c.geox");
        geox(&two, &[("local_geodata.db", b"x"), ("old/local_geodata.db", b"y")]);
        assert!(run(&mut s, &two).contains("more than one"));

        // An entry trying to climb out is only ever compared by name.
        let slip = d.join("d.geox");
        geox(&slip, &[("../../local_geodata.db", b"SQLite format 3\0 but not really")]);
        assert!(run(&mut s, &slip).contains("encrypted or damaged"));
        assert!(!d.parent().unwrap().join("local_geodata.db").exists());

        let other = d.join("other.db");
        rusqlite::Connection::open(&other).unwrap().execute_batch("CREATE TABLE audit_logs (id INTEGER)").unwrap();
        let no_table = d.join("e.geox");
        geox(&no_table, &[("local_geodata.db", &std::fs::read(&other).unwrap())]);
        assert!(run(&mut s, &no_table).contains("no geo_data table"));

        assert!(extract_db(&d.join("missing.geox")).is_err());
        assert!(s.map().layers.is_empty(), "nothing was added");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn folder_names_follow_every_platform() {
        for ok in ["76_0", "Tower 12", "A-1.5", "LOC 3"] {
            assert!(is_folder_name(ok), "{ok}");
        }
        for bad in ["", "76/0", "a\\b", "x:y", "what?", "end.", "end ", " start", "CON", "con.txt", "COM1", "lpt9", "..", "tab\t"] {
            assert!(!is_folder_name(bad), "{bad}");
        }
    }
}
