//! GeoFlux › Sync to Cloud: a layer's towers, their photos and markup as a survey for the account
//! server.
//!
//! Like the bulk report, the work is split: [`plan`] reads the project on the session (text only),
//! and [`prepare`] reads and hashes the photos, which can take minutes for a long line, so the app
//! runs it on a worker thread with a progress bar. The upload itself is `geoflux-client`'s.
//!
//! A project keeps one survey id ([`CloudLink`]), so each sync of the same project is a new
//! revision of the same survey. Every sync sends a revision never sent before: the server keeps
//! photos by hash, so a sync after an interrupted one still sends only the photos it lacks, and a
//! half-sent revision whose photos have since changed can never block the project.

use std::collections::{HashMap, HashSet};

use geoflux_cloud::Sha256;
#[cfg(not(target_arch = "wasm32"))]
use geoflux_cloud::sync::{MANIFEST_FORMAT, MAX_PHOTO_BYTES, PhotoRef, PhotoType, SurveyAsset};
use geoflux_cloud::sync::{MAX_ASSETS, MAX_PHOTOS_PER_ASSET, SurveyManifest};
use geoflux_domain::annotation::AnnotationSet;
use geoflux_domain::{Asset, Severity};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::geoflux::{self, SEVERITY_FIELD};
use super::inspection_report::markup_key;
use super::need_feature_layer;
use crate::{CmdResult, CommandSpec as C, Session};

/// The survey a project uploads as. Saved with the project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudLink {
    /// The project's survey id on the server (`clientSurveyId`), made on the first sync.
    pub survey: String,
    /// The last revision the server committed.
    #[serde(default)]
    pub revision: u32,
    /// The last revision sent, committed or not; the next sync sends one more.
    #[serde(default)]
    pub attempt: u32,
}

impl CloudLink {
    /// The revision the next sync sends.
    pub fn next_revision(&self) -> u32 {
        self.revision.max(self.attempt).saturating_add(1)
    }
}

pub fn commands() -> Vec<C> {
    vec![C::new("geoflux.syncCheck", "Check Cloud Sync", "GeoFlux › Cloud", check).view().when(need_feature_layer).params(r#"{"layer"?}"#)]
}

/// What a sync of the layer would send, without reading any photo.
fn check(s: &mut Session, p: &Value) -> CmdResult {
    let id = s.layer_id(p)?;
    let plan = plan(s, id)?;
    let photos: usize = plan.assets.iter().map(|a| a.photos.len()).sum();
    Ok(json!({
        "layer": id,
        "survey": plan.survey,
        "revision": plan.revision,
        "assets": plan.assets.len(),
        "photos": photos,
        "skipped": plan.skipped,
    }))
}

/// A photo to send, before it is read.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedPhoto {
    pub slot: String,
    pub path: String,
    pub markup: Option<AnnotationSet>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlannedAsset {
    pub asset: Asset,
    pub photos: Vec<PlannedPhoto>,
}

/// A survey ready to prepare: everything but the photos' hashes and sizes.
#[derive(Clone, Debug, PartialEq)]
pub struct SyncPlan {
    pub survey: String,
    pub revision: u32,
    pub project: String,
    pub line: Option<String>,
    pub schema_id: String,
    pub assets: Vec<PlannedAsset>,
    /// Towers left out, with why (no label, a label used twice).
    pub skipped: Vec<String>,
}

/// A new survey id: 32 hex digits. Ids only need to be unique within an organisation: two hashers
/// with keys from the system's random source, over the time and the project's name.
fn new_survey_id(seed: &str) -> String {
    use std::hash::{BuildHasher, Hasher};
    let now = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let half = |n: u8| {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u8(n);
        h.write(seed.as_bytes());
        h.write_u128(now);
        h.finish()
    };
    format!("{:016x}{:016x}", half(1), half(2))
}

/// The project's survey id, made and kept (the project then needs saving) on the first sync.
pub fn ensure_link(s: &mut Session) -> CloudLink {
    if s.project.geoflux.cloud.is_none() {
        s.project.geoflux.cloud = Some(CloudLink { survey: new_survey_id(&s.project.name), revision: 0, attempt: 0 });
        s.dirty = true;
    }
    s.project.geoflux.cloud.clone().unwrap_or(CloudLink { survey: String::new(), revision: 0, attempt: 0 })
}

/// Records that `revision` is being sent, so no later sync reuses it.
pub fn attempted(s: &mut Session, survey: &str, revision: u32) {
    if let Some(link) = s.project.geoflux.cloud.as_mut().filter(|l| l.survey == survey && revision > l.attempt) {
        link.attempt = revision;
        s.dirty = true;
    }
}

/// Records that the server committed `revision`.
pub fn committed(s: &mut Session, survey: &str, revision: u32) {
    attempted(s, survey, revision);
    if let Some(link) = s.project.geoflux.cloud.as_mut().filter(|l| l.survey == survey && revision > l.revision) {
        link.revision = revision;
        s.dirty = true;
    }
}

/// The layer's towers in walking order, with their photos and markup. Towers without a label, or
/// with a label already used, are skipped: the server keys towers by label.
pub fn plan(s: &Session, layer: u32) -> Result<SyncPlan, String> {
    let link = s.project.geoflux.cloud.clone().ok_or("the project has no cloud survey yet")?;
    let schema = s.project.geoflux.schema(s.project.geoflux.schema_of(layer).ok_or("this layer has no inspection schema")?)?;
    let stops = geoflux::sequence(s, layer, false)?;
    let l = s.map().layer(layer).ok_or("no such layer")?;
    let ds = &l.dataset;
    let sev_col = ds.field_index(SEVERITY_FIELD);
    let index = geoflux::PhotoIndex::new(&s.project.geoflux, layer);
    let mut marks: HashMap<(String, &str, String), &AnnotationSet> = HashMap::new();
    for m in s.project.geoflux.markup.iter().filter(|m| m.layer == layer) {
        marks.entry((m.label.trim().to_lowercase(), m.slot.as_str(), m.photo.clone())).or_insert(&m.shapes);
    }
    let mut seen = HashSet::new();
    let mut out = SyncPlan {
        revision: link.next_revision(),
        survey: link.survey,
        project: text_or(&s.project.name, "Untitled project"),
        line: Some(text_or(&l.name, "Line")),
        schema_id: schema.id.clone(),
        assets: Vec::new(),
        skipped: Vec::new(),
    };
    for stop in &stops {
        let label = stop.label.trim();
        if label.is_empty() {
            out.skipped.push(format!("row {}: no {}", stop.row + 1, schema.label_key()));
            continue;
        }
        if !seen.insert(label.to_lowercase()) {
            out.skipped.push(format!("{label}: the label is used twice; only the first is sent"));
            continue;
        }
        if out.assets.len() >= MAX_ASSETS {
            out.skipped.push(format!("{label}: more than {MAX_ASSETS} towers"));
            continue;
        }
        let mut asset = geoflux::asset_of(ds, &schema, stop.row);
        asset.id = label.chars().filter(|c| !c.is_control()).take(200).collect();
        asset.severity = sev_col.map_or(Severity::None, |c| Severity::from_name(&ds.value(stop.row, c).to_string()));
        let key = label.to_lowercase();
        let mut photos = Vec::new();
        for sp in index.slots(&schema, label) {
            for path in sp.photos {
                if photos.len() >= MAX_PHOTOS_PER_ASSET {
                    break;
                }
                let markup = marks.get(&(key.clone(), sp.slot.id.as_str(), markup_key(s, layer, &path))).map(|m| (*m).clone());
                photos.push(PlannedPhoto { slot: sp.slot.id.clone(), path, markup });
            }
        }
        out.assets.push(PlannedAsset { asset, photos });
    }
    if out.assets.is_empty() {
        return Err("the layer has no labelled towers to send".into());
    }
    Ok(out)
}

fn text_or(text: &str, default: &str) -> String {
    let t: String = text.trim().chars().filter(|c| !c.is_control()).take(200).collect();
    if t.is_empty() { default.to_string() } else { t }
}

/// A survey ready to upload, and where each photo is on disk.
#[derive(Debug)]
pub struct Prepared {
    pub manifest: SurveyManifest,
    pub files: HashMap<Sha256, String>,
    /// Photos left out (missing, unreadable, not JPEG or PNG, too large), with why.
    pub skipped: Vec<String>,
}

/// Reads and hashes every photo of `plan`, one at a time in 64 KiB pieces, so memory stays flat
/// whatever the photos weigh. A photo used twice is read once. `progress` counts photos and is
/// checked between them for Cancel.
#[cfg(not(target_arch = "wasm32"))]
pub fn prepare(plan: SyncPlan, progress: &geoflux_report::Progress) -> Result<Prepared, String> {
    let total = plan.assets.iter().map(|a| a.photos.len()).sum();
    progress.start(total);
    let mut read: HashMap<String, Result<(Sha256, u64, PhotoType), String>> = HashMap::new();
    let mut files = HashMap::new();
    let mut skipped = plan.skipped;
    let mut assets = Vec::with_capacity(plan.assets.len());
    for a in plan.assets {
        let mut photos = Vec::with_capacity(a.photos.len());
        for ph in a.photos {
            let r = read.entry(ph.path.clone()).or_insert_with(|| hash_file(&ph.path)).clone();
            progress.step().map_err(|_| "cancelled".to_string())?;
            match r {
                Ok((sha, bytes, kind)) => {
                    files.entry(sha.clone()).or_insert_with(|| ph.path.clone());
                    let file_name = std::path::Path::new(&ph.path)
                        .file_name()
                        .map_or_else(|| "photo".to_string(), |n| n.to_string_lossy().chars().take(255).collect());
                    photos.push(PhotoRef { slot: ph.slot, sha256: sha, bytes, kind, file_name, markup: ph.markup });
                }
                Err(why) => skipped.push(format!("{}: {}: {why}", a.asset.id, ph.path)),
            }
        }
        assets.push(SurveyAsset { asset: a.asset, photos, findings: Vec::new() });
    }
    let manifest = SurveyManifest {
        format: MANIFEST_FORMAT,
        client_survey_id: plan.survey,
        revision: plan.revision,
        project: plan.project,
        line: plan.line,
        schema_id: plan.schema_id,
        surveyed: None,
        assets,
    };
    if let Err(fields) = manifest.validate() {
        let list: Vec<String> = fields.iter().take(5).map(|f| format!("{}: {}", f.field, f.reason)).collect();
        return Err(format!("the survey can't be sent: {}", list.join("; ")));
    }
    Ok(Prepared { manifest, files, skipped })
}

/// A photo's hash, size and type, reading it in pieces.
#[cfg(not(target_arch = "wasm32"))]
fn hash_file(path: &str) -> Result<(Sha256, u64, PhotoType), String> {
    use std::io::Read;

    use sha2::Digest;
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    if len == 0 {
        return Err("the file is empty".into());
    }
    if len > MAX_PHOTO_BYTES {
        return Err(format!("larger than {} MB", MAX_PHOTO_BYTES >> 20));
    }
    let mut hasher = sha2::Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut head = Vec::new();
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        let chunk = buf.get(..n).unwrap_or_default();
        if head.len() < 16 {
            head.extend(chunk.iter().take(16 - head.len()));
        }
        hasher.update(chunk);
        total += n as u64;
        // The file grew while being read: never send more than the limit.
        if total > MAX_PHOTO_BYTES {
            return Err(format!("larger than {} MB", MAX_PHOTO_BYTES >> 20));
        }
    }
    let kind = [PhotoType::Jpeg, PhotoType::Png].into_iter().find(|k| k.matches(&head)).ok_or("not a JPEG or PNG")?;
    Ok((Sha256::from_digest(hasher.finalize().into()), total, kind))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::Project;

    const SHEET: &str = "Tower No.,Line Name,CKT,Lat.,Long.,Ambeint Temp.,Fault Temp.,Fault Description,report_Type,Fault,Company Name\n\
        76_0,Bhadla-Sikar,1,27.50,75.16,30,92,Hot clamp,tower_fault,Fault,Acme Grid\n\
        77_0,Bhadla-Sikar,1,27.51,75.17,30,,,tower,,Acme Grid\n\
        77_0,Bhadla-Sikar,1,27.52,75.18,30,,,tower,,Acme Grid\n\
        ,Bhadla-Sikar,1,27.53,75.19,30,,,tower,,Acme Grid\n";

    /// Rows with one label twice (and one without a label, which the import leaves out), photos for 76_0: two JPEGs and a text
    /// file posing as one.
    fn line(tag: &str) -> (Session, u32, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("geoflux-sync-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("76_0")).unwrap();
        std::fs::write(dir.join("76_0/DJI_0001_T.JPG"), [0xff, 0xd8, 0xff, 0xe0, 1, 2, 3]).unwrap();
        std::fs::write(dir.join("76_0/DJI_0001_Z.JPG"), [0xff, 0xd8, 0xff, 0xe0, 4, 5, 6]).unwrap();
        std::fs::write(dir.join("76_0/DJI_0001_W.JPG"), b"not a photo").unwrap();
        let mut s = Session::new(Project::default());
        let r = s.run("geoflux.importCsv", &json!({"text": SHEET})).unwrap();
        let id = r["layer"].as_u64().unwrap() as u32;
        s.run("geoflux.linkPhotos", &json!({"layer": id, "folder": dir.join("").to_string_lossy()})).unwrap();
        (s, id, dir)
    }

    #[test]
    fn a_line_becomes_a_survey_with_hashed_photos() {
        let (mut s, id, dir) = line("plan");
        assert!(plan(&s, id).is_err(), "no survey id yet");
        let link = ensure_link(&mut s);
        assert_eq!(link.survey.len(), 32);
        assert_eq!(ensure_link(&mut s), link, "kept");
        let p = plan(&s, id).unwrap();
        assert_eq!(p.revision, 1);
        assert_eq!(p.assets.iter().map(|a| a.asset.id.as_str()).collect::<Vec<_>>(), ["76_0", "77_0"]);
        assert_eq!(p.skipped, ["77_0: the label is used twice; only the first is sent"]);
        assert_eq!(p.assets[0].photos.len(), 3);

        let prepared = prepare(p, &geoflux_report::Progress::new()).unwrap();
        let photos = &prepared.manifest.assets[0].photos;
        assert_eq!(photos.len(), 2, "the text file is left out");
        assert!(prepared.skipped.iter().any(|w| w.contains("not a JPEG or PNG")), "{:?}", prepared.skipped);
        assert_eq!(photos[0].bytes, 7);
        assert!(prepared.files.contains_key(&photos[0].sha256));
        assert_eq!(prepared.manifest.client_survey_id, link.survey);

        // Each sync sends a new revision, sent or committed.
        attempted(&mut s, &link.survey, 1);
        assert_eq!(plan(&s, id).unwrap().revision, 2);
        committed(&mut s, &link.survey, 2);
        let l = s.project.geoflux.cloud.clone().unwrap();
        assert_eq!((l.revision, l.attempt, l.next_revision()), (2, 2, 3));
        committed(&mut s, "another survey", 9);
        assert_eq!(s.project.geoflux.cloud.as_ref().unwrap().revision, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancel_stops_between_photos() {
        let (mut s, id, dir) = line("cancel");
        ensure_link(&mut s);
        let p = plan(&s, id).unwrap();
        let progress = geoflux_report::Progress::new();
        progress.cancel();
        assert_eq!(prepare(p, &progress).unwrap_err(), "cancelled");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_link_is_saved_with_the_project() {
        let (mut s, _, dir) = line("save");
        let link = ensure_link(&mut s);
        let text = serde_json::to_string(&s.project.geoflux).unwrap();
        let back: geoflux::GeoFlux = serde_json::from_str(&text).unwrap();
        assert_eq!(back.cloud, Some(link));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
