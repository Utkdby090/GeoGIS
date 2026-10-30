//! GeoFlux › Sort Photos to Towers and Make Tower Folders: a scanned day of raw photos (see
//! [`super::photo_scan`]) given to the line's towers, then copied into one folder per tower.
//!
//! Sorting links each photo in place: it is added to its tower's image slot (thermal photos to
//! the thermal slot, visual and zoom photos to the RGB slot, wide ones to the structure slot), so
//! the Inspection pane shows it at once and nothing is copied. The photo layer gains `tower` and
//! `metres` columns, so the map shows where each photo went and how far it was. Sorting again
//! (after moving a tower, or with more towers) replaces what the same photos were given before.
//!
//! Tower folders are optional copies for people who work in folders: `<folder>/34_1/C1/…`, one
//! per tower and visit. [`plan_folders`] reads the session; [`copy_folders`] touches only files,
//! so the app runs it on a worker thread with progress and cancel. Originals are never moved or
//! changed, and a copy that is already there is not made again, so a stopped copy can be resumed.

use std::collections::HashSet;
use std::path::PathBuf;

use geoflux_domain::images;
use geoflux_domain::survey_photos::PhotoKind;
use geoflux_domain::tower_photos::{self, DEFAULT_VISIT, PhotoPoint, TowerPoint};
use geoflux_report::Progress;
use rustgis_data::{FieldType, Geometry, Value as V};
use serde_json::{Value, json};

use super::geoflux::{self, layer_schema};
use super::photo_scan::is_raw_photo_layer;
use super::ps;
use crate::{CmdResult, CommandSpec as C, Session};

/// The columns sorting adds to a photo layer.
pub const TOWER_FIELD: &str = "tower";
pub const METRES_FIELD: &str = "metres";
/// A photo farther than this from every tower is flagged: it was probably taken on another line
/// (or the tower's position in the KML is wrong). It is still sorted; nothing is blocked.
pub const FAR_METRES: f64 = 500.0;
/// How many copy failures are described one by one.
#[cfg(not(target_arch = "wasm32"))]
const MAX_DETAILS: usize = 10;

pub fn commands() -> Vec<C> {
    vec![
        C::new("geoflux.sortPhotos", "Sort Photos to Towers", "GeoFlux › Inspection", sort_photos).params(
            r#"{"layer"?: "the tower layer (default: the active layer)", "photos"?: "a layer made by Scan Raw Photos (default: the newest)"}"#,
        ),
        C::new("geoflux.towerFolders", "Make Tower Folders", "GeoFlux › Inspection", tower_folders).params(
            r#"{"folder": "where the tower folders go", "photos"?: "a sorted photo layer (default: the newest)", "visit"?: "the visit folder inside each tower's (default C1)"}"#,
        ),
    ]
}

/// The raw photo layer `p.photos` names, else the newest one in the map.
fn photo_layer(s: &Session, p: &Value) -> Result<u32, String> {
    if let Some(named) = p.get("photos").filter(|v| !v.is_null()) {
        let id = s.layer_id(&json!({ "layer": named }))?;
        let ok = s.map().layer(id).is_some_and(|l| is_raw_photo_layer(&l.dataset));
        return if ok { Ok(id) } else { Err("that layer was not made by Scan Raw Photos".into()) };
    }
    s.map()
        .layers
        .iter()
        .filter(|l| is_raw_photo_layer(&l.dataset))
        .map(|l| l.id)
        .max()
        .ok_or_else(|| "scan a folder of raw photos first (Scan Raw Photos)".to_string())
}

/// The slot a kind of photo goes in: found by the words the schema's slots match, so another
/// industry's schema sorts the same way.
fn slot_for_kind(slots: &[geoflux_domain::ImageSlotDef], kind: PhotoKind) -> Option<&str> {
    let word = match kind {
        PhotoKind::Thermal => "thermal",
        PhotoKind::Wide => "wide",
        PhotoKind::Visual | PhotoKind::Zoom | PhotoKind::Other => "rgb",
    };
    images::slot_for(slots, word)
}

fn sort_photos(s: &mut Session, p: &Value) -> CmdResult {
    let (tower_id, schema) = layer_schema(s, p)?;
    let photos_id = photo_layer(s, p)?;
    if tower_id == photos_id {
        return Err("choose the tower layer, not the photo layer".into());
    }

    // The towers with a number and a position, and the label of each.
    let (labels, towers): (Vec<String>, Vec<TowerPoint>) = {
        let ds = &s.map().layer(tower_id).ok_or("no such layer")?.dataset;
        let col = ds.field_index(schema.label_key()).ok_or("the tower layer has no tower number column")?;
        ds.features
            .iter()
            .enumerate()
            .filter_map(|(row, f)| {
                let label = ds.value(row, col).to_string();
                let Some(Geometry::Point(c)) = &f.geometry else { return None };
                (!label.trim().is_empty() && !matches!(ds.value(row, col), V::Null))
                    .then(|| (label.trim().to_string(), TowerPoint { lat: c.y, lon: c.x }))
            })
            .unzip()
    };
    if towers.is_empty() {
        return Err("the tower layer has no towers with a number and a position".into());
    }

    let (paths, kinds, points): (Vec<String>, Vec<PhotoKind>, Vec<PhotoPoint>) = {
        let ds = &s.map().layer(photos_id).ok_or("no such layer")?.dataset;
        let col = |n: &str| ds.field_index(n).ok_or_else(|| format!("the photo layer has no {n} column"));
        let (path, kind, shot) = (col("path")?, col("kind")?, col("shot")?);
        let mut paths = Vec::with_capacity(ds.features.len());
        let mut kinds = Vec::with_capacity(ds.features.len());
        let mut points = Vec::with_capacity(ds.features.len());
        for (row, f) in ds.features.iter().enumerate() {
            let (lat, lon) = match &f.geometry {
                Some(Geometry::Point(c)) => (c.y, c.x),
                _ => (f64::NAN, f64::NAN),
            };
            paths.push(ds.value(row, path).to_string());
            kinds.push(PhotoKind::from_name(&ds.value(row, kind).to_string()));
            points.push(PhotoPoint { shot: ds.value(row, shot).to_string(), lat, lon });
        }
        (paths, kinds, points)
    };
    let placed = tower_photos::assign(&towers, &points);
    drop(points);

    // The photo layer shows where each photo went.
    {
        let layer = s.map_mut().layer_mut(photos_id).ok_or("no such layer")?;
        let ds = &mut layer.dataset;
        let tc = ds.ensure_field(TOWER_FIELD, FieldType::Text);
        let mc = ds.ensure_field(METRES_FIELD, FieldType::Double);
        for (f, place) in ds.features.iter_mut().zip(&placed) {
            let (tower, metres) = match place.and_then(|pl| labels.get(pl.tower).map(|l| (l, pl.metres))) {
                Some((l, m)) => (V::Text(l.clone()), V::Float((m * 10.0).round() / 10.0)),
                None => (V::Null, V::Null),
            };
            if let Some(a) = f.attrs.get_mut(tc) {
                *a = tower;
            }
            if let Some(a) = f.attrs.get_mut(mc) {
                *a = metres;
            }
        }
        // Its label (off until turned on in Style › Label) now names each photo's tower.
        super::geoflux_style::set_photo_label(layer, TOWER_FIELD);
    }

    // What these photos were given before goes, so sorting again doesn't double them.
    let ours: HashSet<&str> = paths.iter().map(String::as_str).collect();
    let g = &mut s.project.geoflux;
    g.picked.retain(|q| !(q.layer == tower_id && ours.contains(q.path.as_str())));
    let mut sorted = 0usize;
    let mut not_added = 0usize;
    let mut no_slot = 0usize;
    let mut towers_with = HashSet::new();
    let mut farthest = 0.0f64;
    // Rows of the photo layer farther than FAR_METRES from every tower.
    let far: Vec<usize> = placed.iter().enumerate().filter(|(_, p)| p.is_some_and(|p| p.metres > FAR_METRES)).map(|(row, _)| row).collect();
    for ((path, kind), place) in paths.iter().zip(&kinds).zip(&placed) {
        let Some(place) = place else { continue };
        let Some(label) = labels.get(place.tower) else { continue };
        let Some(slot) = slot_for_kind(&schema.image_slots, *kind) else {
            no_slot += 1;
            continue;
        };
        if geoflux::add_picked(g, tower_id, label, slot, path) {
            sorted += 1;
            towers_with.insert(place.tower);
            farthest = farthest.max(place.metres);
        } else {
            not_added += 1;
        }
    }
    let unplaced = placed.iter().filter(|p| p.is_none()).count();
    if not_added > 0 {
        s.notify(format!("{not_added} photos were not added: a tower's slot or the project holds as many photos as it can"), false);
    }
    if no_slot > 0 {
        s.notify(format!("{no_slot} photos have no slot in this schema to go in"), false);
    }
    if !far.is_empty() {
        let placed_count = placed.iter().filter(|p| p.is_some()).count();
        let worst = placed.iter().flatten().map(|p| p.metres).fold(0.0f64, f64::max);
        let km = (worst / 100.0).round() / 10.0;
        let text = if far.len() == placed_count {
            format!(
                "all {} photos are more than {FAR_METRES:.0} m from every tower (up to {km} km): they look like another line's photos. Check the line KML, then Undo",
                far.len()
            )
        } else {
            format!(
                "{} photos are more than {FAR_METRES:.0} m from every tower (up to {km} km); they are selected on the photo layer. Check them before inspecting",
                far.len()
            )
        };
        s.notify(text, true);
        // Selected, so the table's Selected only shows just these.
        if let Some(layer) = s.map_mut().layer_mut(photos_id) {
            layer.selection = far.clone();
        }
    }
    Ok(json!({
        "towerLayer": tower_id,
        "photoLayer": photos_id,
        "sorted": sorted,
        "towers": towers_with.len(),
        "unplaced": unplaced,
        "notAdded": not_added,
        "noSlot": no_slot,
        "farthestMetres": (farthest * 10.0).round() / 10.0,
        "far": far.len(),
        "farMetres": FAR_METRES,
    }))
}

/// The copies to make: each sorted photo to `<folder>/<tower>/<visit>/<its file name>`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FolderPlan {
    pub copies: Vec<(PathBuf, PathBuf)>,
    /// How many tower folders the copies go to.
    pub towers: usize,
    /// Photos in the layer not sorted to a tower (left out).
    pub unsorted: usize,
}

/// What copying did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FolderReport {
    pub copied: usize,
    /// Already there with the same size (from an earlier or stopped copy).
    pub already: usize,
    /// A different file of the same name was there; it was kept and the photo not copied.
    pub clashes: usize,
    pub failed: usize,
    pub details: Vec<String>,
}

/// The tower folder copies for the photo layer `p.photos` names (default: the newest), into
/// `folder`. Fails when the layer was never sorted.
pub fn plan_folders(s: &Session, p: &Value, folder: &str) -> Result<FolderPlan, String> {
    let folder = folder.trim();
    if folder.is_empty() {
        return Err("folder is required (where the tower folders go)".into());
    }
    let visit = tower_photos::folder_name(ps(p, "visit").map(str::trim).filter(|v| !v.is_empty()).unwrap_or(DEFAULT_VISIT));
    let id = photo_layer(s, p)?;
    let ds = &s.map().layer(id).ok_or("no such layer")?.dataset;
    let tower = ds.field_index(TOWER_FIELD).ok_or("sort the photos to towers first (Sort Photos to Towers)")?;
    let path = ds.field_index("path").ok_or("the photo layer has no path column")?;
    let root = PathBuf::from(folder);
    let mut plan = FolderPlan::default();
    let mut towers = HashSet::new();
    for row in 0..ds.features.len() {
        let V::Text(label) = ds.value(row, tower) else {
            plan.unsorted += 1;
            continue;
        };
        let src = PathBuf::from(ds.value(row, path).to_string());
        let Some(file) = src.file_name() else { continue };
        let dir = tower_photos::folder_name(label);
        let dst = root.join(&dir).join(&visit).join(file);
        towers.insert(dir);
        plan.copies.push((src, dst));
    }
    plan.towers = towers.len();
    if plan.copies.is_empty() {
        return Err("no photo is sorted to a tower yet".into());
    }
    Ok(plan)
}

/// Makes the copies in `plan`. A copy is written beside its place and renamed into it when
/// whole, so a stopped copy never leaves half a photo under the photo's name. `progress` counts
/// photos; a cancel stops between photos with the error `cancelled` (the copies made stay).
pub fn copy_folders(plan: &FolderPlan, progress: &Progress) -> Result<FolderReport, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (plan, progress);
        Err("the web app can't write folders".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        progress.start(plan.copies.len());
        let mut r = FolderReport::default();
        for (src, dst) in &plan.copies {
            match copy_one(src, dst) {
                Ok(Copied::Made) => r.copied += 1,
                Ok(Copied::Already) => r.already += 1,
                Ok(Copied::Clash) => {
                    r.clashes += 1;
                    detail(&mut r, format!("{}: a different file of that name is already there", dst.display()));
                }
                Err(e) => {
                    r.failed += 1;
                    detail(&mut r, format!("{}: {e}", src.display()));
                }
            }
            progress.step().map_err(|_| "cancelled".to_string())?;
        }
        Ok(r)
    }
}

#[cfg(not(target_arch = "wasm32"))]
enum Copied {
    Made,
    Already,
    Clash,
}

#[cfg(not(target_arch = "wasm32"))]
fn copy_one(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<Copied> {
    let len = std::fs::metadata(src)?.len();
    if let Ok(there) = std::fs::metadata(dst) {
        return Ok(if there.is_file() && there.len() == len { Copied::Already } else { Copied::Clash });
    }
    if let Some(dir) = dst.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut part = dst.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    if let Err(e) = std::fs::copy(src, &part).and_then(|_| std::fs::rename(&part, dst)) {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    Ok(Copied::Made)
}

#[cfg(not(target_arch = "wasm32"))]
fn detail(r: &mut FolderReport, d: String) {
    if r.details.len() < MAX_DETAILS {
        r.details.push(d);
    }
}

fn tower_folders(s: &mut Session, p: &Value) -> CmdResult {
    let folder = ps(p, "folder").unwrap_or_default();
    let plan = plan_folders(s, p, folder)?;
    let r = copy_folders(&plan, &Progress::new())?;
    Ok(folder_result(&plan, &r))
}

/// A copy's outcome, as the command and the app report it.
pub fn folder_result(plan: &FolderPlan, r: &FolderReport) -> Value {
    json!({
        "towers": plan.towers,
        "copied": r.copied,
        "already": r.already,
        "clashes": r.clashes,
        "failed": r.failed,
        "unsorted": plan.unsorted,
        "details": r.details,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Project;

    const LINE: &str = r#"<kml><Document><name>L</name>
      <Placemark><name>34/1</name><Point><coordinates>75.100,27.500</coordinates></Point></Placemark>
      <Placemark><name>34/2</name><Point><coordinates>75.104,27.500</coordinates></Point></Placemark>
    </Document></kml>"#;

    /// A folder removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let p = std::env::temp_dir().join(format!("geoflux-sort-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).expect("temp dir");
            TempDir(p)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A session with the line's towers and a photo layer of three shots: a pair at tower 34/1,
    /// one thermal photo at 34/2. The photo files exist under `raw`.
    fn session(raw: &std::path::Path) -> (Session, u32, u32) {
        let mut s = Session::new(Project::default());
        let towers = s.run("geoflux.importLineKml", &json!({"text": LINE})).expect("line")["layer"].as_u64().expect("id") as u32;
        let mut ds = rustgis_data::Dataset::new("day", rustgis_data::GeometryType::Point, super::super::photo_scan::fields());
        for (name, lon) in [("DJI_0001_T.JPG", 75.1001), ("DJI_0001_V.JPG", 75.1002), ("DJI_0002_T.JPG", 75.1039)] {
            let path = raw.join(name);
            std::fs::write(&path, name.as_bytes()).expect("photo");
            let kind = PhotoKind::of(name).as_str();
            let shot = geoflux_domain::survey_photos::shot_id(name);
            ds.push_next(
                Some(Geometry::Point(rustgis_geom::Coord::new(lon, 27.5))),
                vec![
                    V::Text(name.into()),
                    V::Text(kind.into()),
                    V::Text(shot.into()),
                    V::Null,
                    V::Null,
                    V::Null,
                    V::Null,
                    V::Text(path.display().to_string()),
                ],
            );
        }
        let photos = super::super::layer::add_dataset(&mut s, ds);
        s.active_layer = Some(towers);
        (s, towers, photos)
    }

    #[test]
    fn sorting_links_each_photo_to_its_nearest_towers_slot() {
        let raw = TempDir::new("link");
        let (mut s, towers, photos) = session(&raw.0);
        let r = s.run("geoflux.sortPhotos", &json!({})).expect("sort");
        assert_eq!(r["sorted"], 3);
        assert_eq!(r["towers"], 2);
        assert_eq!(r["photoLayer"], photos);
        let picked: Vec<(String, String)> =
            s.project.geoflux.picked.iter().filter(|p| p.layer == towers).map(|p| (p.label.clone(), p.slot.clone())).collect();
        assert_eq!(
            picked,
            [("34/1".to_string(), "THERMAL".to_string()), ("34/1".into(), "RGB_ZOOM".into()), ("34/2".into(), "THERMAL".into())]
        );
        let ds = &s.map().layer(photos).expect("photos").dataset;
        assert_eq!(ds.value(2, ds.field_index(TOWER_FIELD).expect("tower")).to_string(), "34/2");
        assert!(is_raw_photo_layer(ds), "still a photo layer with the new columns");

        // Again: replaced, not doubled.
        s.run("geoflux.sortPhotos", &json!({"layer": towers, "photos": photos})).expect("again");
        assert_eq!(s.project.geoflux.picked.len(), 3);
    }

    #[test]
    fn photos_far_from_every_tower_are_flagged_and_selected_but_still_sorted() {
        let raw = TempDir::new("far");
        let (mut s, _, photos) = session(&raw.0);
        let move_to = |s: &mut Session, row: usize, lon: f64, lat: f64| {
            let ds = &mut s.map_mut().layer_mut(photos).expect("photos").dataset;
            ds.features[row].geometry = Some(Geometry::Point(rustgis_geom::Coord::new(lon, lat)));
        };
        // One photo about 1,500 km away (another line).
        move_to(&mut s, 2, 78.24, 15.64);
        let mark = s.notice_mark();
        let r = s.run("geoflux.sortPhotos", &json!({})).expect("sorted anyway");
        assert_eq!((r["sorted"].as_u64(), r["far"].as_u64()), (Some(3), Some(1)), "{r}");
        assert_eq!(s.map().layer(photos).expect("photos").selection, vec![2], "the far one is selected");
        let said: Vec<_> = s.notices_since(mark).iter().map(|n| (n.text.clone(), n.error)).collect();
        assert!(said.iter().any(|(t, e)| *e && t.starts_with("1 photos are more than 500 m from every tower (up to 1")), "{said:?}");

        // All of them far: the whole folder looks like another line's.
        move_to(&mut s, 0, 78.24, 15.64);
        move_to(&mut s, 1, 78.2401, 15.64);
        let mark = s.notice_mark();
        let r = s.run("geoflux.sortPhotos", &json!({})).expect("sorted anyway");
        assert_eq!(r["far"], 3);
        let said: Vec<String> = s.notices_since(mark).iter().map(|n| n.text.clone()).collect();
        assert!(said.iter().any(|t| t.starts_with("all 3 photos are more than 500 m") && t.contains("another line")), "{said:?}");

        // Near again: no warning, and the selection isn't touched.
        for row in 0..3 {
            move_to(&mut s, row, 75.1001, 27.5);
        }
        s.map_mut().layer_mut(photos).expect("photos").selection.clear();
        let r = s.run("geoflux.sortPhotos", &json!({})).expect("sorted");
        assert_eq!(r["far"], 0);
        assert!(s.map().layer(photos).expect("photos").selection.is_empty());
    }

    #[test]
    fn towers_and_photos_stay_as_layers_and_look_apart() {
        let raw = TempDir::new("look");
        let (mut s, towers, photos) = session(&raw.0);
        s.run("geoflux.sortPhotos", &json!({})).expect("sort");
        // The line's towers are still there under the photos.
        assert!(s.map().layer(towers).is_some() && s.map().layer(photos).is_some());
        let t = s.map().layer(towers).expect("towers");
        assert!(t.labels.enabled && t.labels.field == "towerNumber", "towers show their number");
        match &t.renderer {
            rustgis_map::Renderer::Unique { field, classes, .. } => {
                assert_eq!(field, "severity");
                assert!(classes.iter().all(|(_, sym)| sym.marker == rustgis_map::Marker::Triangle));
            }
            _ => panic!("towers are drawn by severity"),
        }
        let p = s.map().layer(photos).expect("photos");
        assert_eq!(p.labels.field, TOWER_FIELD, "a photo's label names its tower");
        assert!(!p.labels.enabled, "off until turned on, so 700 labels don't hide the map");
    }

    #[test]
    fn tower_folders_copy_each_photo_once_and_never_overwrite() {
        let raw = TempDir::new("raw");
        let out = TempDir::new("out");
        let (mut s, _, _) = session(&raw.0);
        assert!(s.run("geoflux.towerFolders", &json!({"folder": out.0})).is_err(), "not sorted yet");
        s.run("geoflux.sortPhotos", &json!({})).expect("sort");

        let r = s.run("geoflux.towerFolders", &json!({"folder": out.0})).expect("copy");
        assert_eq!((r["copied"].as_u64(), r["towers"].as_u64()), (Some(3), Some(2)));
        let made = out.0.join("34_1").join("C1").join("DJI_0001_V.JPG");
        assert_eq!(std::fs::read(&made).expect("copied"), b"DJI_0001_V.JPG");
        assert!(out.0.join("34_2").join("C1").join("DJI_0002_T.JPG").is_file());
        assert!(raw.0.join("DJI_0001_V.JPG").is_file(), "the original stays");

        // Again: nothing copied twice; a different file of the same name is kept.
        std::fs::write(out.0.join("34_2").join("C1").join("DJI_0002_T.JPG"), b"a different photo").expect("clash");
        let r = s.run("geoflux.towerFolders", &json!({"folder": out.0})).expect("again");
        assert_eq!((r["copied"].as_u64(), r["already"].as_u64(), r["clashes"].as_u64()), (Some(0), Some(2), Some(1)));
        assert_eq!(std::fs::read(out.0.join("34_2").join("C1").join("DJI_0002_T.JPG")).expect("kept"), b"a different photo");

        let r = s.run("geoflux.towerFolders", &json!({"folder": out.0, "visit": "C2"})).expect("visit");
        assert_eq!(r["copied"], 3);
        assert!(out.0.join("34_1").join("C2").join("DJI_0001_T.JPG").is_file());
    }

    #[test]
    fn a_cancelled_copy_stops_between_photos() {
        let raw = TempDir::new("cancel");
        let out = TempDir::new("cancel-out");
        let (mut s, _, _) = session(&raw.0);
        s.run("geoflux.sortPhotos", &json!({})).expect("sort");
        let plan = plan_folders(&s, &json!({}), &out.0.display().to_string()).expect("plan");
        let progress = Progress::new();
        progress.cancel();
        assert_eq!(copy_folders(&plan, &progress).err().as_deref(), Some("cancelled"));
        assert_eq!(progress.get(), (1, 3));
    }

    #[test]
    fn the_chosen_report_photo_comes_first_and_survives_sorting_again() {
        let raw = TempDir::new("chosen");
        let (mut s, towers, _) = session(&raw.0);
        s.run("geoflux.sortPhotos", &json!({})).expect("sort");
        let extra = raw.0.join("DJI_0003_T.JPG");
        std::fs::write(&extra, b"x").expect("photo");
        let extra = extra.display().to_string();
        s.run("geoflux.addPhotos", &json!({"layer": towers, "row": 0, "slot": "THERMAL", "paths": [extra]})).expect("add");
        let thermal =
            |s: &Session| geoflux::photos(s, towers, 0).expect("photos").into_iter().find(|x| x.slot.id == "THERMAL").expect("slot").photos;
        assert!(thermal(&s)[0].ends_with("DJI_0001_T.JPG"));

        s.run("geoflux.reportPhoto", &json!({"layer": towers, "row": 0, "slot": "THERMAL", "path": extra})).expect("choose");
        assert_eq!(thermal(&s)[0], extra);
        assert_eq!(thermal(&s).len(), 2);
        s.run("geoflux.sortPhotos", &json!({})).expect("again");
        assert_eq!(thermal(&s)[0], extra, "kept through a new sort");

        let e =
            s.run("geoflux.reportPhoto", &json!({"layer": towers, "row": 0, "slot": "RGB_ZOOM", "path": extra})).expect_err("not there");
        assert!(e.contains("not in this slot"), "{e}");
        assert_eq!(s.project.geoflux.chosen.len(), 1);
    }

    #[test]
    fn sorting_needs_a_tower_layer_and_a_photo_layer() {
        let mut s = Session::new(Project::default());
        assert!(s.run("geoflux.sortPhotos", &json!({})).is_err());
        let towers = s.run("geoflux.importLineKml", &json!({"text": LINE})).expect("line")["layer"].clone();
        let e = s.run("geoflux.sortPhotos", &json!({"layer": towers})).expect_err("no photos");
        assert!(e.contains("Scan Raw Photos"), "{e}");
        let e = s.run("geoflux.sortPhotos", &json!({"layer": towers, "photos": towers})).expect_err("not a photo layer");
        assert!(e.contains("not made by Scan Raw Photos"), "{e}");
    }
}
