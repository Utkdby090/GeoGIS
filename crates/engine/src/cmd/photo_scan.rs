//! GeoFlux › Scan Raw Photos: a survey day's folder of drone photos as a point layer of where
//! each was taken, ready to be sorted onto the line's towers.
//!
//! [`scan`] touches only files, so the app runs it on a worker thread with progress and cancel;
//! [`apply`] then adds the layer. `geoflux.scanPhotos` does both in one command.
//!
//! Only the head of each photo is read (its EXIF sits in the first segments of the JPEG), into
//! one buffer reused for every photo, so a day of 740 photos of 8 MB reads about 100 MB from the
//! disk, not 6 GB, and memory holds one head at a time. Pictures are not decoded and no
//! thumbnails are made: the layer is positions and paths only.

use geoflux_domain::survey_photos::{PhotoKind, shot_id};
use geoflux_report::Progress;
use rustgis_data::{Dataset, Field, FieldType, Geometry, GeometryType, Value as V};
use rustgis_geom::Coord;
use serde_json::{Value, json};

use super::ps;
use crate::{CmdResult, CommandSpec as C, Session};

/// How much of each photo is read: the EXIF segment is at most 64 KB and comes first.
pub const HEAD_BYTES: u64 = 128 * 1024;
/// Most photos in one scan; the result says when more were left out.
pub const MAX_PHOTOS: usize = 200_000;
/// How deep below the chosen folder photos are looked for (day / flight / photo…).
#[cfg(not(target_arch = "wasm32"))]
const DEPTH: usize = 6;
/// Most folder entries looked at, files or not.
#[cfg(not(target_arch = "wasm32"))]
const MAX_ENTRIES: usize = 1_000_000;
/// How many unreadable photos are described one by one.
#[cfg(not(target_arch = "wasm32"))]
const MAX_DETAILS: usize = 10;

pub fn commands() -> Vec<C> {
    vec![C::new("geoflux.scanPhotos", "Scan Raw Photos", "GeoFlux › Inspection", scan_photos).params(
        r#"{"folder": "a survey day's folder of drone photos (sub-folders are scanned too)", "name"?: "the layer (default: the folder's name)"}"#,
    )]
}

/// One photo with a GPS position.
#[derive(Clone, Debug, PartialEq)]
pub struct ScannedPhoto {
    /// Where it is, as found under the scanned folder.
    pub path: String,
    pub lon: f64,
    pub lat: f64,
    pub time: Option<String>,
    pub altitude: Option<f64>,
    pub direction: Option<f64>,
    pub camera: Option<String>,
}

/// A scanned folder, ready for [`apply`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PhotoScan {
    /// The layer name when none is given: the folder's own name.
    pub name: String,
    pub photos: Vec<ScannedPhoto>,
    /// JPEGs read whose EXIF has no position (often a phone's or an edited copy).
    pub without_position: usize,
    /// JPEGs that could not be opened or read, and the first few with why.
    pub unreadable: usize,
    pub unreadable_details: Vec<String>,
    /// Set when the folder held more than [`MAX_PHOTOS`] photos or [`MAX_ENTRIES`] entries.
    pub truncated: bool,
}

fn scan_photos(s: &mut Session, p: &Value) -> CmdResult {
    let folder = ps(p, "folder").map(str::trim).filter(|x| !x.is_empty()).ok_or("folder is required (a folder of drone photos)")?;
    let scan = scan(folder, &Progress::new())?;
    add(s, scan, ps(p, "name"))
}

/// Adds a scanned folder as a photo layer, as one step to undo.
pub fn apply(s: &mut Session, scan: PhotoScan, name: Option<&str>) -> CmdResult {
    s.guarded("Scan Raw Photos", true, move |s| add(s, scan, name))
}

/// The fields of a raw photo layer: `kind` is a [`PhotoKind`] name and `shot` its
/// [`shot_id`], so a thermal photo and the visual one taken with it can be kept together.
pub fn fields() -> Vec<Field> {
    vec![
        Field::new("name", FieldType::Text),
        Field::new("kind", FieldType::Text),
        Field::new("shot", FieldType::Text),
        Field::new("time", FieldType::Text),
        Field::new("altitude", FieldType::Double),
        Field::new("direction", FieldType::Double),
        Field::new("camera", FieldType::Text),
        Field::new("path", FieldType::Text),
    ]
}

/// Whether a dataset is a raw photo layer made by `geoflux.scanPhotos`.
pub fn is_raw_photo_layer(ds: &Dataset) -> bool {
    ds.geometry_type == GeometryType::Point && ds.fields == fields()
}

fn add(s: &mut Session, scan: PhotoScan, name: Option<&str>) -> CmdResult {
    if scan.photos.is_empty() {
        let mut why = format!("no photo with a GPS position in {}", scan.name);
        if scan.without_position > 0 {
            why.push_str(&format!(" ({} photos have no position)", scan.without_position));
        }
        return Err(why);
    }
    let name = name.map(str::trim).filter(|n| !n.is_empty()).unwrap_or(&scan.name).to_string();
    let mut ds = Dataset::new(name, GeometryType::Point, fields());
    let mut kinds = [0usize; PhotoKind::ALL.len()];
    for ph in &scan.photos {
        let kind = PhotoKind::of(&ph.path);
        if let Some(n) = PhotoKind::ALL.iter().position(|k| *k == kind).and_then(|i| kinds.get_mut(i)) {
            *n += 1;
        }
        let leaf = ph.path.rsplit(['/', '\\']).next().unwrap_or(&ph.path);
        let text = |v: &Option<String>| v.clone().map(V::Text).unwrap_or(V::Null);
        let num = |v: Option<f64>| v.map(V::Float).unwrap_or(V::Null);
        ds.push_next(
            Some(Geometry::Point(Coord::new(ph.lon, ph.lat))),
            vec![
                V::Text(leaf.to_string()),
                V::Text(kind.as_str().to_string()),
                V::Text(shot_id(leaf).to_string()),
                text(&ph.time),
                num(ph.altitude),
                num(ph.direction),
                text(&ph.camera),
                V::Text(ph.path.clone()),
            ],
        );
    }
    let id = super::layer::add_dataset(s, ds);
    // A colour per kind, so thermal and visual shots read apart on the map.
    let _ = super::layer::symbology(s, &json!({"layer": id, "type": "unique", "field": "kind"}));
    if let Some(e) = s.map().layer(id).map(|l| l.extent()).filter(|e| !e.is_empty()) {
        s.zoom_to(&e);
    }
    let mut notes = Vec::new();
    if scan.without_position > 0 {
        notes.push(format!("{} photos have no GPS position and were left out", scan.without_position));
    }
    if scan.unreadable > 0 {
        notes.push(format!("{} photos could not be read: {}", scan.unreadable, scan.unreadable_details.join("; ")));
    }
    if scan.truncated {
        notes.push(format!("the folder holds more than {MAX_PHOTOS} photos; the rest were not scanned"));
    }
    if !notes.is_empty() {
        s.notify(notes.join(". "), false);
    }
    let by_kind: serde_json::Map<String, Value> =
        PhotoKind::ALL.iter().zip(kinds).filter(|(_, n)| *n > 0).map(|(k, n)| (k.as_str().to_string(), json!(n))).collect();
    Ok(json!({
        "layer": id,
        "photos": scan.photos.len(),
        "kinds": by_kind,
        "withoutPosition": scan.without_position,
        "unreadable": scan.unreadable,
        "unreadableDetails": scan.unreadable_details,
        "truncated": scan.truncated,
    }))
}

/// Reads the position of every JPEG under `folder` (sub-folders included, up to a depth of
/// [`DEPTH`]), in the order a person reads the names (`DJI_0009` before `DJI_0010`).
/// `progress` counts photos; a cancel stops between photos with the error `cancelled`.
pub fn scan(folder: &str, progress: &Progress) -> Result<PhotoScan, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = progress;
        Err(format!("{folder}: the web app can't read folders"))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::io::Read;

        let root = std::path::Path::new(folder);
        let name = root.file_name().and_then(|n| n.to_str()).unwrap_or(folder).to_string();
        let (files, truncated) = list_jpegs(root)?;
        progress.start(files.len());
        let mut out = PhotoScan { name, truncated, ..PhotoScan::default() };
        let mut head = Vec::with_capacity(HEAD_BYTES as usize);
        for path in files {
            head.clear();
            let read = std::fs::File::open(&path).and_then(|f| f.take(HEAD_BYTES).read_to_end(&mut head));
            let shown = path.display().to_string();
            match read {
                Err(e) => unreadable(&mut out, format!("{shown}: {e}")),
                Ok(_) => match rustgis_data::photo::read_exif(&head) {
                    None => unreadable(&mut out, format!("{shown}: no EXIF (not a camera's JPEG)")),
                    Some(x) => match x.position {
                        None => out.without_position += 1,
                        Some((lon, lat)) => {
                            let camera = match (x.make, x.model) {
                                (Some(m), Some(n)) if n.to_ascii_lowercase().starts_with(&m.to_ascii_lowercase()) => Some(n),
                                (Some(m), Some(n)) => Some(format!("{m} {n}")),
                                (m, n) => m.or(n),
                            };
                            out.photos.push(ScannedPhoto {
                                path: shown,
                                lon,
                                lat,
                                time: x.time,
                                altitude: x.altitude,
                                direction: x.direction,
                                camera,
                            });
                        }
                    },
                },
            }
            progress.step().map_err(|_| "cancelled".to_string())?;
        }
        Ok(out)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn unreadable(out: &mut PhotoScan, detail: String) {
    out.unreadable += 1;
    if out.unreadable_details.len() < MAX_DETAILS {
        out.unreadable_details.push(detail);
    }
}

/// The JPEGs under `root`, in natural name order folder by folder, and whether the walk was cut
/// at [`MAX_PHOTOS`] or [`MAX_ENTRIES`]. Symbolic links are not followed, so a looped folder
/// can't trap the walk; an unreadable sub-folder is skipped.
#[cfg(not(target_arch = "wasm32"))]
fn list_jpegs(root: &std::path::Path) -> Result<(Vec<std::path::PathBuf>, bool), String> {
    use geoflux_domain::sequence::natural_cmp;

    if !root.is_dir() {
        return Err(format!("{}: not a folder", root.display()));
    }
    let mut out = Vec::new();
    let mut seen = 0usize;
    let mut pending = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = pending.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if depth == 0 => return Err(format!("{}: {e}", root.display())),
            Err(_) => continue,
        };
        let mut files = Vec::new();
        let mut dirs = Vec::new();
        for entry in entries.flatten() {
            seen += 1;
            if seen > MAX_ENTRIES {
                return Ok((out, true));
            }
            let Ok(kind) = entry.file_type() else { continue };
            let Ok(name) = entry.file_name().into_string() else { continue };
            if kind.is_dir() && depth < DEPTH {
                dirs.push(name);
            } else if kind.is_file() && rustgis_data::photo::is_photo_name(&name) {
                files.push(name);
            }
        }
        files.sort_by(|a, b| natural_cmp(a, b));
        for f in files {
            if out.len() >= MAX_PHOTOS {
                return Ok((out, true));
            }
            out.push(dir.join(f));
        }
        // Popped last-first, so sub-folders are walked in name order.
        dirs.sort_by(|a, b| natural_cmp(b, a));
        pending.extend(dirs.into_iter().map(|d| (dir.join(d), depth + 1)));
    }
    Ok((out, false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Project;

    /// A minimal JPEG whose EXIF holds a GPS position (or none), then `pad` bytes of picture.
    fn jpeg(position: Option<(f64, f64)>, pad: usize) -> Vec<u8> {
        // Little-endian TIFF: IFD0 with a GPS pointer; the GPS IFD with lat/lon refs and values.
        let mut t: Vec<u8> = b"II*\0".to_vec();
        t.extend(8u32.to_le_bytes());
        let entry = |t: &mut Vec<u8>, tag: u16, ty: u16, count: u32, value: u32| {
            t.extend(tag.to_le_bytes());
            t.extend(ty.to_le_bytes());
            t.extend(count.to_le_bytes());
            t.extend(value.to_le_bytes());
        };
        // IFD0 at 8: one entry (GPS pointer), next IFD 0.
        t.extend(1u16.to_le_bytes());
        let gps_at = 8 + 2 + 12 + 4;
        entry(&mut t, 0x8825, 4, 1, gps_at);
        t.extend(0u32.to_le_bytes());
        // GPS IFD: 4 entries, then the rationals.
        let rationals_at = gps_at + 2 + 4 * 12 + 4;
        t.extend(4u16.to_le_bytes());
        let (lat, lon) = position.map_or((0.0, 0.0), |(lon, lat)| (lat, lon));
        entry(&mut t, 0x0001, 2, 2, u32::from_le_bytes(*b"N\0\0\0"));
        entry(&mut t, 0x0002, 5, 3, rationals_at);
        entry(&mut t, 0x0003, 2, 2, u32::from_le_bytes(*b"E\0\0\0"));
        entry(&mut t, 0x0004, 5, 3, rationals_at + 24);
        t.extend(0u32.to_le_bytes());
        for v in [lat, lon] {
            // Degrees as a rational in millionths, minutes and seconds zero.
            t.extend(((v * 1e6).round() as u32).to_le_bytes());
            t.extend(1_000_000u32.to_le_bytes());
            for _ in 0..2 {
                t.extend(0u32.to_le_bytes());
                t.extend(1u32.to_le_bytes());
            }
        }
        let mut j = vec![0xFF, 0xD8, 0xFF, 0xE1];
        j.extend(((t.len() + 8) as u16).to_be_bytes());
        j.extend(b"Exif\0\0");
        j.extend(t);
        j.extend([0xFF, 0xDA, 0, 2]);
        j.extend(std::iter::repeat_n(0u8, pad));
        j.extend([0xFF, 0xD9]);
        j
    }

    /// A folder removed when dropped.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let p = std::env::temp_dir().join(format!("geoflux-scan-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).expect("temp dir");
            TempDir(p)
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn day(name: &str) -> TempDir {
        let dir = TempDir::new(name);
        let flight = dir.path().join("flight 2");
        std::fs::create_dir(&flight).expect("sub-folder");
        // Large enough that reading whole photos would show, small enough for a test.
        std::fs::write(dir.path().join("DJI_0010_T.JPG"), jpeg(Some((75.102, 27.503)), 300_000)).expect("write");
        std::fs::write(dir.path().join("DJI_0010_V.JPG"), jpeg(Some((75.1021, 27.5031)), 10)).expect("write");
        std::fs::write(dir.path().join("DJI_0009_T.JPG"), jpeg(Some((75.100, 27.500)), 10)).expect("write");
        std::fs::write(dir.path().join("edited.jpg"), jpeg(None, 10)).expect("write");
        std::fs::write(dir.path().join("broken.jpg"), b"not a jpeg").expect("write");
        std::fs::write(dir.path().join("notes.txt"), b"ignored").expect("write");
        std::fs::write(flight.join("DJI_0001_V.JPG"), jpeg(Some((75.104, 27.506)), 10)).expect("write");
        dir
    }

    #[test]
    fn scan_reads_positions_from_photo_heads_in_name_order() {
        let dir = day("order");
        let progress = Progress::new();
        let scan = scan(dir.path().to_str().expect("utf-8"), &progress).expect("scan");
        let names: Vec<&str> = scan.photos.iter().map(|p| p.path.rsplit(['/', '\\']).next().unwrap_or("")).collect();
        assert_eq!(names, ["DJI_0009_T.JPG", "DJI_0010_T.JPG", "DJI_0010_V.JPG", "DJI_0001_V.JPG"]);
        assert!((scan.photos[1].lon - 75.102).abs() < 1e-6 && (scan.photos[1].lat - 27.503).abs() < 1e-6);
        assert_eq!(scan.without_position, 1);
        assert_eq!(scan.unreadable, 1);
        assert!(scan.unreadable_details[0].contains("broken.jpg"));
        assert_eq!(progress.get(), (6, 6));
        assert!(!scan.truncated);
    }

    #[test]
    fn the_command_adds_a_photo_layer_with_kinds_and_shots() {
        let dir = day("layer");
        let mut s = Session::new(Project::default());
        let r = s.run("geoflux.scanPhotos", &json!({"folder": dir.path(), "name": "01-06-2026"})).expect("scan");
        assert_eq!(r["photos"], 4);
        assert_eq!(r["kinds"], json!({"thermal": 2, "visual": 2}));
        assert_eq!(r["withoutPosition"], 1);
        let layer = s.map().layer(r["layer"].as_u64().expect("layer") as u32).expect("layer");
        assert_eq!(layer.name, "01-06-2026");
        assert!(is_raw_photo_layer(&layer.dataset));
        let ds = &layer.dataset;
        let col = |n: &str| ds.field_index(n).expect("field");
        assert_eq!(ds.value(1, col("kind")).to_string(), "thermal");
        assert_eq!(ds.value(1, col("shot")).to_string(), "DJI_0010");
        assert_eq!(ds.value(2, col("shot")).to_string(), "DJI_0010");
        assert!(ds.value(0, col("path")).to_string().ends_with("DJI_0009_T.JPG"));
        assert!(s.can_undo());
    }

    #[test]
    fn a_cancelled_scan_stops_and_bad_folders_are_refused() {
        let dir = day("cancel");
        let progress = Progress::new();
        progress.cancel();
        assert_eq!(scan(dir.path().to_str().expect("utf-8"), &progress).err().as_deref(), Some("cancelled"));
        assert!(scan(&dir.path().join("missing").display().to_string(), &Progress::new()).is_err());

        let empty = TempDir::new("empty");
        let mut s = Session::new(Project::default());
        let e = s.run("geoflux.scanPhotos", &json!({"folder": empty.path()})).expect_err("nothing to add");
        assert!(e.contains("no photo with a GPS position"), "{e}");
        assert!(s.run("geoflux.scanPhotos", &json!({})).is_err());
    }
}
