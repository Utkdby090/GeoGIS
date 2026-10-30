//! GeoFlux › Reports: photo markup, and inspection reports as PDFs (one asset, or a ZIP of every
//! asset of a layer).
//!
//! Layout and PDF writing live in `geoflux-report`; these commands turn an asset, its template,
//! photos and markup into [`PlannedPage`]s. Planning is text only and runs on the session; the
//! photos are read one page at a time while the PDF or ZIP is written, so the app can do that part
//! on a worker thread ([`plan`] then [`write_bulk_file`]) with a progress bar and cancel.

use std::collections::HashMap;

use geoflux_domain::annotation::{Annotation, AnnotationSet};
use geoflux_domain::{DomainSchema, Severity, form};
use geoflux_report::page::MAX_SMALL_IMAGE_BYTES;
use geoflux_report::{BulkEntry, CellPhoto, Image, PageInput, PhotoIssue, PhotoSource, PlannedPage, ReportTemplate, Summary, plan_page};
use serde_json::{Value, json};

use super::geoflux::{self, MAX_MARKUP, PhotoMarkup, SEVERITY_FIELD};
use super::{need_feature_layer, ps};
use crate::{CmdResult, CommandSpec as C, Session};

/// The longest template given as text.
const MAX_TEMPLATE_BYTES: usize = 256 * 1024;
/// Most photos listed as missing in a command's result.
const MAX_LISTED: usize = 50;

pub fn commands() -> Vec<C> {
    vec![
        C::new("geoflux.annotate", "Mark Up Photo", "GeoFlux › Reports", annotate)
            .when(need_feature_layer)
            .params(r#"{"layer"?, "row": number, "slot": "an image slot id", "photo": "the photo, as geoflux.photos lists it", "shapes": [annotation]}"#),
        C::new("geoflux.markup", "Photo Markup", "GeoFlux › Reports", markup_cmd)
            .view()
            .when(need_feature_layer)
            .params(r#"{"layer"?, "row": number, "slot": "an image slot id", "photo": "the photo, as geoflux.photos lists it"}"#),
        C::new("geoflux.reportTemplate", "Report Template", "GeoFlux › Reports", template_cmd)
            .view()
            .when(need_feature_layer)
            .params(r#"{"layer"?, "template"?: template to check (object or JSON text); without it, the layer's own}"#),
        C::new("geoflux.report", "Inspection Report", "GeoFlux › Reports", report)
            .view()
            .when(need_feature_layer)
            .params(r#"{"layer"?, "row": number, "path": "*.pdf", "template"?: template (object or JSON text)}"#),
        C::new("geoflux.reportBulk", "Inspection Reports (ZIP)", "GeoFlux › Reports", report_bulk)
            .view()
            .when(need_feature_layer)
            .params(r#"{"layer"?, "path": "*.zip", "template"?: template, "all"?: bool (include assets whose form has issues)}"#),
    ]
}

/// The key markup is kept by for a photo as `geoflux::photos` lists it: relative to the layer's
/// linked folder (so markup survives the folder being moved and linked again), else the full path.
pub fn markup_key(s: &Session, layer: u32, photo: &str) -> String {
    let folder = s.project.geoflux.photos_of(layer).map(|l| l.folder.as_str()).unwrap_or_default();
    if !folder.is_empty()
        && let Ok(rel) = std::path::Path::new(photo).strip_prefix(folder)
    {
        let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        if !parts.is_empty() {
            return parts.join("/");
        }
    }
    photo.to_string()
}

/// The markup on one photo of one asset (empty when it has none).
pub fn markup(s: &Session, layer: u32, label: &str, slot: &str, photo: &str) -> AnnotationSet {
    let key = markup_key(s, layer, photo);
    let label = label.trim();
    s.project
        .geoflux
        .markup
        .iter()
        .find(|m| m.layer == layer && m.slot == slot && m.photo == key && m.label.trim().eq_ignore_ascii_case(label))
        .map(|m| m.shapes.clone())
        .unwrap_or_default()
}

/// The asset's label, and the slot and photo named by the parameters (the photo must be one the
/// slot lists).
fn photo_target(s: &Session, layer: u32, row: usize, p: &Value) -> Result<(String, String, String), String> {
    let (schema, asset) = geoflux::asset(s, layer, row)?;
    let label = asset.property(schema.label_key()).trim().to_string();
    if label.is_empty() {
        return Err(format!("this asset has no {} to keep its markup by", schema.label_key()));
    }
    let slot = ps(p, "slot").ok_or("slot is required")?;
    let photo = ps(p, "photo").map(str::trim).filter(|x| !x.is_empty()).ok_or("photo is required")?;
    let slots = geoflux::photos(s, layer, row)?;
    let sp = slots.iter().find(|sp| sp.slot.id == slot).ok_or_else(|| format!("schema '{}' has no image slot '{slot}'", schema.id))?;
    if !sp.photos.iter().any(|x| x == photo) {
        return Err(format!("{photo} is not one of this slot's photos (geoflux.photos lists them)"));
    }
    Ok((label, sp.slot.id.clone(), photo.to_string()))
}

/// Replaces one photo's markup (an empty list removes it), as one undoable edit.
fn annotate(s: &mut Session, p: &Value) -> CmdResult {
    let id = s.layer_id(p)?;
    let row = p.get("row").and_then(Value::as_u64).ok_or("row is required")? as usize;
    let (label, slot, photo) = photo_target(s, id, row, p)?;
    let given = p.get("shapes").filter(|v| v.is_array()).ok_or("shapes is required (a list of shapes; [] removes the markup)")?;
    let shapes: Vec<Annotation> = serde_json::from_value(given.clone()).map_err(|e| format!("shapes: {e}"))?;
    let set = AnnotationSet::from_shapes(shapes).map_err(|e| e.to_string())?;
    let key = markup_key(s, id, &photo);
    let count = set.len();
    let g = &mut s.project.geoflux;
    let at = g.markup.iter().position(|m| m.layer == id && m.slot == slot && m.photo == key && m.label.trim().eq_ignore_ascii_case(&label));
    match (at, set.is_empty()) {
        (Some(i), true) => {
            g.markup.remove(i);
        }
        (Some(i), false) => {
            if let Some(m) = g.markup.get_mut(i) {
                m.shapes = set;
            }
        }
        (None, true) => {}
        (None, false) => {
            if g.markup.len() >= MAX_MARKUP {
                return Err(format!("a project holds markup on at most {MAX_MARKUP} photos"));
            }
            g.markup.push(PhotoMarkup { layer: id, label, slot: slot.clone(), photo: key, shapes: set });
        }
    }
    Ok(json!({"layer": id, "row": row, "slot": slot, "photo": photo, "shapes": count}))
}

fn markup_cmd(s: &mut Session, p: &Value) -> CmdResult {
    let id = s.layer_id(p)?;
    let row = p.get("row").and_then(Value::as_u64).ok_or("row is required")? as usize;
    let (label, slot, photo) = photo_target(s, id, row, p)?;
    let set = markup(s, id, &label, &slot, &photo);
    Ok(json!({"layer": id, "row": row, "slot": slot, "photo": photo, "shapes": set}))
}

/// The template a command uses: `template` when given (an object or JSON text), else the schema's.
fn template(s: &Session, layer: u32, p: &Value) -> Result<(DomainSchema, ReportTemplate), String> {
    let schema = s.project.geoflux.schema(s.project.geoflux.schema_of(layer).ok_or("this layer has no inspection schema")?)?;
    let t = match p.get("template") {
        None | Some(Value::Null) => ReportTemplate::for_schema(&schema),
        Some(Value::String(text)) if text.len() <= MAX_TEMPLATE_BYTES => ReportTemplate::from_json(text).map_err(|e| e.to_string())?,
        Some(Value::String(_)) => return Err(format!("the template is larger than {} KB", MAX_TEMPLATE_BYTES >> 10)),
        Some(v) => serde_json::from_value(v.clone()).map_err(|e| format!("the report template is not usable: {e}"))?,
    };
    t.check(&schema).map_err(|e| e.to_string())?;
    Ok((schema, t))
}

fn template_cmd(s: &mut Session, p: &Value) -> CmdResult {
    let id = s.layer_id(p)?;
    let (_, t) = template(s, id, p)?;
    Ok(json!({"layer": id, "template": t}))
}

/// Reads report photos and logos from disk, never more than asked.
pub struct DiskPhotos;

impl PhotoSource for DiskPhotos {
    fn read(&self, path: &str, max_bytes: usize) -> Result<Vec<u8>, PhotoIssue> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (path, max_bytes);
            Err(PhotoIssue::Unreadable("the web app can't read files by path".into()))
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            use std::io::Read;
            let file = std::fs::File::open(path).map_err(|e| PhotoIssue::Unreadable(e.to_string()))?;
            let len = file.metadata().map_err(|e| PhotoIssue::Unreadable(e.to_string()))?.len();
            if len > max_bytes as u64 {
                return Err(PhotoIssue::TooLarge { max_mb: max_bytes >> 20 });
            }
            // The file may grow after the size check: never read more than the limit.
            let mut bytes = Vec::with_capacity(len as usize);
            file.take(max_bytes as u64 + 1).read_to_end(&mut bytes).map_err(|e| PhotoIssue::Unreadable(e.to_string()))?;
            if bytes.len() > max_bytes {
                return Err(PhotoIssue::TooLarge { max_mb: max_bytes >> 20 });
            }
            Ok(bytes)
        }
    }
}

/// The pages of a report: every asset of the layer (in walking order) or only `row`.
pub struct Plan {
    pub pages: Vec<PlannedPage>,
    /// Assets left out because their form has issues (when the template wants ready ones only).
    pub not_ready: Vec<String>,
}

/// Plans the report pages of `rows` (None: every asset of the layer). Text only: photos are read
/// when each page is written.
pub fn plan(s: &Session, layer: u32, rows: Option<&[usize]>, p: &Value, include_not_ready: bool) -> Result<Plan, String> {
    let (schema, t) = template(s, layer, p)?;
    let stops = geoflux::sequence(s, layer, false)?;
    let logo = match t.branding.logo.trim() {
        "" => None,
        path => match DiskPhotos.read(path, MAX_SMALL_IMAGE_BYTES).and_then(Image::from_bytes) {
            Ok(img) => Some(img),
            Err(e) => return Err(format!("the logo {path}: {e}")),
        },
    };
    let ds = &s.map().layer(layer).ok_or("no such layer")?.dataset;
    let sev_col = ds.field_index(SEVERITY_FIELD);
    let mut out = Plan { pages: Vec::new(), not_ready: Vec::new() };
    let wanted: Vec<usize> = match rows {
        Some(r) => r.to_vec(),
        None => stops.iter().map(|st| st.row).collect(),
    };
    // Everything looked up per asset is indexed once, so a line of thousands of towers plans in
    // one pass: walking order, linked and picked photos, and markup.
    let order: HashMap<usize, usize> = stops.iter().enumerate().map(|(i, st)| (st.row, i)).collect();
    let index = geoflux::PhotoIndex::new(&s.project.geoflux, layer);
    let mut marks: HashMap<(String, &str, String), &AnnotationSet> = HashMap::new();
    for m in s.project.geoflux.markup.iter().filter(|m| m.layer == layer) {
        marks.entry((m.label.trim().to_lowercase(), m.slot.as_str(), m.photo.clone())).or_insert(&m.shapes);
    }
    let found = super::findings::ReportFindings::new(&s.project.geoflux, layer);
    for row in wanted {
        if row >= ds.features.len() {
            return Err("no such row".into());
        }
        let asset = geoflux::asset_of(ds, &schema, row);
        let fields = schema.fields_for(&asset);
        let values = form::values(&asset, &fields);
        if rows.is_none() && t.ready_only && !include_not_ready && !form::validate(&fields, &values).is_empty() {
            out.not_ready.push(asset.property(schema.label_key()).to_string());
            continue;
        }
        let label = asset.property(schema.label_key()).trim().to_string();
        let severity = sev_col.map_or(Severity::None, |c| Severity::from_name(&ds.value(row, c).to_string()));
        let at = order.get(&row).copied();
        let near = |i: Option<usize>| i.and_then(|i| stops.get(i)).map(|st| st.label.clone()).filter(|l| !l.trim().is_empty());
        let slots = index.slots(&schema, &label);
        let label_key = label.to_lowercase();
        let input = PageInput {
            schema: &schema,
            template: &t,
            asset: &asset,
            severity,
            previous: near(at.and_then(|i| i.checked_sub(1))),
            next: near(at.map(|i| i + 1)),
            logo: logo.clone(),
        };
        out.pages.push(plan_page(input, &mut |slot| {
            let path = slots.iter().find(|sp| sp.slot.id == slot).and_then(|sp| sp.photos.first().cloned())?;
            let markup = marks.get(&(label_key.clone(), slot, markup_key(s, layer, &path))).map(|set| (*set).clone()).unwrap_or_default();
            let markup = found.add_to(markup, &label, &path, &schema);
            Some(CellPhoto { key: path, markup })
        }));
    }
    Ok(out)
}

/// Writes what `write` produces to `path` through a part file beside it, renamed only on success, so
/// a failed or cancelled export never leaves a half-written file under the real name.
#[cfg(not(target_arch = "wasm32"))]
fn write_through_part<T>(
    path: &str,
    write: impl FnOnce(&mut std::fs::File) -> Result<T, geoflux_report::ReportError>,
) -> Result<T, String> {
    /// Removes the part file unless it was renamed, also when the writer panics.
    struct Part(Option<String>);
    impl Drop for Part {
        fn drop(&mut self) {
            if let Some(p) = self.0.take() {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    // The process id keeps two GeoGIS windows exporting to the same file from sharing a part file.
    let part = Part(Some(format!("{path}.{}.part", std::process::id())));
    let result = (|| {
        let name = part.0.as_deref().unwrap_or_default();
        let mut f = std::fs::File::create(name)?;
        let out = write(&mut f)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(name, path)?;
        Ok::<T, geoflux_report::ReportError>(out)
    })();
    if result.is_ok() {
        let mut part = part;
        part.0 = None;
    }
    result.map_err(|e| match e {
        geoflux_report::ReportError::Cancelled => "cancelled".to_string(),
        e => format!("{path}: {e}"),
    })
}

/// Writes a planned bulk export to `path` (a ZIP), reading photos page by page. Safe to call off
/// the session's thread: it touches only the plan and the file.
#[cfg(not(target_arch = "wasm32"))]
pub fn write_bulk_file(path: &str, pages: Vec<PlannedPage>, progress: &geoflux_report::Progress) -> Result<Summary, String> {
    let total = pages.len();
    write_through_part(path, |f| {
        let out = std::io::BufWriter::new(f);
        geoflux_report::write_bulk(out, total, pages.into_iter().map(|pp| pp.load(&DiskPhotos)), progress)
    })
}

/// What a written report says back: pages, size, photos left out and characters replaced.
pub fn summary_json(summary: &Summary) -> Value {
    let missing: Vec<Value> = summary
        .photos_missing
        .iter()
        .take(MAX_LISTED)
        .map(|(asset, caption, why)| json!({"asset": asset, "photo": caption, "why": why.to_string()}))
        .collect();
    json!({
        "pages": summary.pages,
        "bytes": summary.bytes,
        "photosMissing": summary.photos_missing.len(),
        "photosMissingListed": missing,
        "replacedCharacters": summary.replaced_chars.iter().collect::<String>(),
    })
}

fn report(s: &mut Session, p: &Value) -> CmdResult {
    let id = s.layer_id(p)?;
    let row = p.get("row").and_then(Value::as_u64).ok_or("row is required")? as usize;
    let path = ps(p, "path").map(str::trim).filter(|x| !x.is_empty()).ok_or("path is required")?.to_string();
    let mut plan = plan(s, id, Some(&[row]), p, true)?;
    let page = plan.pages.pop().ok_or("no such row")?;
    let entry: BulkEntry = page.load(&DiskPhotos);
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (entry, path);
        Err("the web app saves reports through the browser's download".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let summary = write_through_part(&path, |f| {
            let mut out = std::io::BufWriter::new(f);
            let summary = geoflux_report::write_pdf(&entry.page, &mut out)?;
            std::io::Write::flush(&mut out)?;
            Ok(summary)
        })?;
        s.notify(format!("Wrote {path}"), false);
        let mut out = summary_json(&summary);
        out["path"] = json!(path);
        Ok(out)
    }
}

fn report_bulk(s: &mut Session, p: &Value) -> CmdResult {
    let id = s.layer_id(p)?;
    let path = ps(p, "path").map(str::trim).filter(|x| !x.is_empty()).ok_or("path is required")?.to_string();
    let all = p.get("all").and_then(Value::as_bool).unwrap_or(false);
    let plan = plan(s, id, None, p, all)?;
    if plan.pages.is_empty() {
        return Err(format!(
            "no asset is ready for a report ({} have form issues; pass \"all\": true to include them)",
            plan.not_ready.len()
        ));
    }
    let skipped = plan.not_ready.len();
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (plan, path, skipped);
        Err("the web app saves reports through the browser's download".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let reports = plan.pages.len();
        let summary = write_bulk_file(&path, plan.pages, &geoflux_report::Progress::new())?;
        s.notify(format!("Wrote {reports} reports to {path}"), false);
        let mut out = summary_json(&summary);
        out["path"] = json!(path);
        out["reports"] = json!(reports);
        out["skippedNotReady"] = json!(skipped);
        out["skippedListed"] = json!(plan.not_ready.iter().take(MAX_LISTED).collect::<Vec<_>>());
        Ok(out)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::io::Read;

    use image::{ExtendedColorType, ImageEncoder};

    use super::*;
    use crate::Project;

    const SHEET: &str = "Tower No.,Line Name,CKT,Lat.,Long.,Ambeint Temp.,Fault Temp.,Fault Description,report_Type,Fault,Company Name\n\
        76_0,Bhadla-Sikar,1,27.50,75.16,30,92,Hot clamp on the R phase jumper,tower_fault,Fault,Acme Grid\n\
        77_0,Bhadla-Sikar,1,27.51,75.17,30,,,tower,,Acme Grid\n\
        78_0,Bhadla-Sikar,1,,,30,,,tower,,Acme Grid\n";

    fn jpeg() -> Vec<u8> {
        let rgb: Vec<u8> = (0..64 * 48).flat_map(|i: u32| [(i % 251) as u8, 80, 160]).collect();
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut out).write_image(&rgb, 64, 48, ExtendedColorType::Rgb8).unwrap();
        out
    }

    /// A line of three towers with photos for 76_0 and 77_0.
    fn line(tag: &str) -> (Session, u32, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("geoflux-report-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("76_0")).unwrap();
        std::fs::create_dir_all(dir.join("77_0")).unwrap();
        for f in ["76_0/DJI_0001_T.JPG", "76_0/DJI_0001_Z.JPG", "77_0/DJI_0002_T.JPG"] {
            std::fs::write(dir.join(f), jpeg()).unwrap();
        }
        let mut s = Session::new(Project::default());
        let r = s.run("geoflux.importCsv", &json!({"text": SHEET})).unwrap();
        let id = r["layer"].as_u64().unwrap() as u32;
        s.run("geoflux.linkPhotos", &json!({"layer": id, "folder": dir.join("").to_string_lossy()})).unwrap();
        (s, id, dir)
    }

    fn thermal(s: &Session, id: u32, row: usize) -> String {
        geoflux::photos(s, id, row).unwrap().into_iter().find(|x| x.slot.id == "THERMAL").unwrap().photos[0].clone()
    }

    #[test]
    fn markup_is_kept_relative_to_the_folder_and_undoes() {
        let (mut s, id, dir) = line("markup");
        let photo = thermal(&s, id, 0);
        let shapes = json!([{"kind": "ellipse", "a": {"x": 0.2, "y": 0.2}, "b": {"x": 0.6, "y": 0.5}, "color": "#ff0000", "width": 6}]);
        let r = s.run("geoflux.annotate", &json!({"layer": id, "row": 0, "slot": "THERMAL", "photo": photo, "shapes": shapes})).unwrap();
        assert_eq!(r["shapes"], 1);
        assert_eq!(s.project.geoflux.markup[0].photo, "76_0/DJI_0001_T.JPG");
        let got = s.run("geoflux.markup", &json!({"layer": id, "row": 0, "slot": "THERMAL", "photo": photo})).unwrap();
        assert_eq!(got["shapes"].as_array().unwrap().len(), 1);
        // Not a photo of the slot, and shapes that don't check out, are refused.
        assert!(
            s.run("geoflux.annotate", &json!({"layer": id, "row": 0, "slot": "THERMAL", "photo": "/etc/passwd", "shapes": []})).is_err()
        );
        let bad =
            json!([{"kind": "text", "at": {"x": 0.1, "y": 0.1}, "text": "x".repeat(500), "size": 40, "color": "#ff0000", "width": 4}]);
        assert!(s.run("geoflux.annotate", &json!({"layer": id, "row": 0, "slot": "THERMAL", "photo": photo, "shapes": bad})).is_err());
        // An empty list removes it; undo brings it back.
        s.run("geoflux.annotate", &json!({"layer": id, "row": 0, "slot": "THERMAL", "photo": photo, "shapes": []})).unwrap();
        assert!(s.project.geoflux.markup.is_empty());
        s.run("edit.undo", &json!({})).unwrap();
        assert_eq!(s.project.geoflux.markup.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_report_is_a_pdf_with_its_photos_and_names() {
        let (mut s, id, dir) = line("one");
        let photo = thermal(&s, id, 0);
        let shapes = json!([{"kind": "arrow", "from": {"x": 0.9, "y": 0.9}, "to": {"x": 0.5, "y": 0.5}, "color": "#ff0000", "width": 6}]);
        s.run("geoflux.annotate", &json!({"layer": id, "row": 0, "slot": "THERMAL", "photo": photo, "shapes": shapes})).unwrap();
        let plan = plan(&s, id, Some(&[0]), &json!({}), true).unwrap();
        let page = &plan.pages[0];
        assert_eq!(page.path, "tower_fault/FAULT_tower_fault_76_0_Report.pdf");
        assert_eq!(page.page.header, "Bhadla-Sikar | 76_0 | 1");
        assert_eq!(page.page.next.as_deref(), Some("77_0"));
        assert_eq!(page.page.footer, "Report created by Acme Grid");
        assert_eq!(page.page.photos[1].annotations.len(), 1, "the thermal cell carries its markup");
        assert_eq!(page.photos.iter().flatten().count(), 2);

        let out = dir.join("76_0.pdf");
        let r = s.run("geoflux.report", &json!({"layer": id, "row": 0, "path": out.to_string_lossy()})).unwrap();
        assert_eq!(r["pages"], 1);
        assert_eq!(r["photosMissing"], 0);
        let pdf = std::fs::read(&out).unwrap();
        assert!(pdf.starts_with(b"%PDF-"));
        assert!(std::fs::read_dir(&dir).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".part")));
        // A path that can't be written leaves nothing behind.
        let bad = dir.join("no-such-folder").join("x.pdf");
        assert!(s.run("geoflux.report", &json!({"layer": id, "row": 0, "path": bad.to_string_lossy()})).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bulk_writes_ready_assets_into_a_zip() {
        let (mut s, id, dir) = line("bulk");
        let out = dir.join("reports.zip");
        let r = s.run("geoflux.reportBulk", &json!({"layer": id, "path": out.to_string_lossy()})).unwrap();
        // 78_0 has no position, which its form requires.
        assert_eq!(r["reports"], 2, "{r}");
        assert_eq!(r["skippedNotReady"], 1);
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
        assert_eq!(zip.len(), 2);
        let mut first = Vec::new();
        zip.by_index(0).unwrap().read_to_end(&mut first).unwrap();
        assert!(first.starts_with(b"%PDF-"));
        let r = s.run("geoflux.reportBulk", &json!({"layer": id, "path": out.to_string_lossy(), "all": true})).unwrap();
        assert_eq!(r["reports"], 3);
        // A cancelled export removes its part file and never touches the finished one.
        let before = std::fs::read(&out).unwrap();
        let progress = geoflux_report::Progress::new();
        progress.cancel();
        let pages = plan(&s, id, None, &json!({}), true).unwrap().pages;
        assert_eq!(write_bulk_file(&out.to_string_lossy(), pages, &progress), Err("cancelled".into()));
        assert_eq!(std::fs::read(&out).unwrap(), before);
        assert!(std::fs::read_dir(&dir).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".part")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn templates_are_checked() {
        let (mut s, id, dir) = line("template");
        let r = s.run("geoflux.reportTemplate", &json!({"layer": id})).unwrap();
        assert_eq!(r["template"]["schemaId"], "com.geo.telecom");
        let mut t = r["template"].clone();
        t["photos"][0]["slot"] = json!("NOPE");
        let out = dir.join("x.pdf");
        let err = s.run("geoflux.report", &json!({"layer": id, "row": 0, "path": out.to_string_lossy(), "template": t})).unwrap_err();
        assert!(err.contains("NOPE"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
