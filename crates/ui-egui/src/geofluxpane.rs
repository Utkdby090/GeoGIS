//! GeoFlux › the Inspection pane: one asset's form laid out from its schema, a walk through the
//! layer's assets in natural order (Page Up / Page Down), and the asset's linked photos by slot.
//!
//! The pane only reads the session and runs `geoflux.*` commands; the rules live in
//! `geoflux-domain`. Photos are decoded off the frame, shrunk, and kept in a small cache that is
//! emptied when the pane closes, so memory stays bounded however many photos a line has.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use egui::{Color32, RichText, Ui, vec2};
use serde_json::{Map, Value, json};

use crate::theme::{Tokens, semibold};
use crate::{RustGisApp, Slot};
use rustgis_engine::cmd::geoflux::{self as gf, SlotPhotos, Stop};
use rustgis_engine::geoflux_domain::annotation::AnnotationSet;
use rustgis_engine::geoflux_domain::{FieldDef, FieldIssue, FieldType, IssueKind, Severity, form};

/// The longest side of a slot's thumbnail and of the full view, in pixels.
const THUMB_SIDE: u32 = 384;
const VIEW_SIDE: u32 = 1600;
/// Decoded photos kept: thumbnails (about 0.6 MB each) and full views (up to about 10 MB each).
const MAX_THUMBS: usize = 48;
const MAX_VIEWS: usize = 4;
/// Photos decoded at once (each holds the whole photo in memory while it decodes), and waiting.
const MAX_DECODING: usize = 2;
const MAX_QUEUED: usize = 32;
/// The largest photo file read.
#[cfg(not(target_arch = "wasm32"))]
const MAX_PHOTO_BYTES: u64 = 64 << 20;
/// Rows of the tower list shown before it scrolls.
const LIST_ROWS: f32 = 8.0;

type Decoded = Result<egui::ColorImage, String>;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    path: String,
    side: u32,
}

/// Decoded photos as textures, least recently used first, and the decodes in flight.
#[derive(Default)]
pub struct PhotoCache {
    ready: VecDeque<(Key, Result<egui::TextureHandle, String>)>,
    decoding: Vec<(Key, Slot<Decoded>)>,
    queued: VecDeque<Key>,
    /// Decode threads still running, including ones a [`PhotoCache::clear`] forgot: new ones
    /// start only below [`MAX_DECODING`], so closing and reopening the pane can't pile them up.
    running: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl PhotoCache {
    fn is_empty(&self) -> bool {
        self.ready.is_empty() && self.decoding.is_empty() && self.queued.is_empty()
    }

    /// Drops every texture and forgets decodes in flight (their threads finish and free what
    /// they read; nothing waits for them).
    fn clear(&mut self) {
        self.ready.clear();
        self.decoding.clear();
        self.queued.clear();
    }

    /// The photo at `side`, if decoded; otherwise it is queued and `None` comes back.
    fn get(&mut self, path: &str, side: u32) -> Option<&Result<egui::TextureHandle, String>> {
        let same = |k: &Key| k.side == side && k.path == path;
        if let Some(i) = self.ready.iter().position(|(k, _)| same(k)) {
            // Most recently used goes last, so eviction takes the oldest.
            let hit = self.ready.remove(i)?;
            self.ready.push_back(hit);
            return self.ready.back().map(|(_, r)| r);
        }
        if !self.decoding.iter().any(|(k, _)| same(k)) && !self.queued.iter().any(same) {
            self.queued.push_back(Key { path: path.to_string(), side });
            if self.queued.len() > MAX_QUEUED {
                self.queued.pop_front();
            }
        }
        None
    }

    /// Turns finished decodes into textures, evicts the oldest past the limits, and starts the
    /// newest queued decodes (the photo on screen now beats ones already scrolled past).
    fn poll(&mut self, ctx: &egui::Context) {
        let mut i = 0;
        while i < self.decoding.len() {
            let done = self.decoding.get(i).and_then(|(_, slot)| slot.lock().unwrap_or_else(|e| e.into_inner()).take());
            match done {
                Some(result) => {
                    let (key, _) = self.decoding.swap_remove(i);
                    let tex =
                        result.map(|img| ctx.load_texture(format!("geoflux:{}:{}", key.side, key.path), img, egui::TextureOptions::LINEAR));
                    self.ready.push_back((key, tex));
                }
                None => i += 1,
            }
        }
        for (side, limit) in [(THUMB_SIDE, MAX_THUMBS), (VIEW_SIDE, MAX_VIEWS)] {
            while self.ready.iter().filter(|(k, _)| k.side == side).count() > limit {
                match self.ready.iter().position(|(k, _)| k.side == side) {
                    Some(oldest) => {
                        self.ready.remove(oldest);
                    }
                    None => break,
                }
            }
        }
        while self.running.load(std::sync::atomic::Ordering::Acquire) < MAX_DECODING {
            let Some(key) = self.queued.pop_back() else { break };
            let slot = start_decode(&key, ctx, &self.running);
            self.decoding.push((key, slot));
        }
    }
}

/// Decodes a photo on its own thread. A failure to start one, or a panic inside the decoder,
/// becomes an error for that photo rather than a decode that never finishes.
#[cfg(not(target_arch = "wasm32"))]
fn start_decode(key: &Key, ctx: &egui::Context, running: &std::sync::Arc<std::sync::atomic::AtomicUsize>) -> Slot<Decoded> {
    use std::sync::atomic::Ordering;
    let slot: Slot<Decoded> = Slot::default();
    let land = slot.clone();
    let (path, side, ctx, count) = (key.path.clone(), key.side, ctx.clone(), running.clone());
    running.fetch_add(1, Ordering::AcqRel);
    let spawned = std::thread::Builder::new().name("geoflux-photo".into()).spawn(move || {
        let r = std::panic::catch_unwind(|| decode(&path, side)).unwrap_or_else(|_| Err(tl!("This photo could not be read.").to_string()));
        *land.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
        count.fetch_sub(1, Ordering::AcqRel);
        ctx.request_repaint();
    });
    if let Err(e) = spawned {
        running.fetch_sub(1, Ordering::AcqRel);
        *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(Err(e.to_string()));
    }
    slot
}

#[cfg(target_arch = "wasm32")]
fn start_decode(_: &Key, _: &egui::Context, _: &std::sync::Arc<std::sync::atomic::AtomicUsize>) -> Slot<Decoded> {
    std::sync::Arc::new(std::sync::Mutex::new(Some(Err(tl!("Linked photos show in the desktop app.").to_string()))))
}

/// Reads a photo and shrinks it to at most `side` pixels, within fixed size and memory limits.
#[cfg(not(target_arch = "wasm32"))]
fn decode(path: &str, side: u32) -> Decoded {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_PHOTO_BYTES {
        return Err(crate::i18n::fmt("Larger than {n} MB", &[("n", &(MAX_PHOTO_BYTES >> 20).to_string())]));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(256 << 20);
    reader.limits(limits);
    // Shown the way the camera held it (EXIF orientation), as markup positions and the PDF assume.
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let orientation = image::ImageDecoder::orientation(&mut decoder).unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = image::DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    img.apply_orientation(orientation);
    let img = if img.width() > side || img.height() > side { img.thumbnail(side, side) } else { img };
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Ok(egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

/// State of the Inspection pane.
#[derive(Default)]
pub struct InspectionState {
    /// The layer being inspected, and the row (asset) shown.
    pub layer: Option<u32>,
    pub row: Option<usize>,
    pub flagged_only: bool,
    /// The form: which asset (and project revision) it was read from, its fields as laid out
    /// (group → rows → field indexes), the values as read and as edited, and what's invalid.
    loaded: Option<(u32, usize, u64)>,
    fields: Vec<FieldDef>,
    layout: Vec<(String, Vec<Vec<usize>>)>,
    original: BTreeMap<String, String>,
    values: BTreeMap<String, String>,
    /// Fields the tower's findings fill (in findings mode): shown, not typed.
    locked: BTreeSet<String>,
    issues: Vec<FieldIssue>,
    message: Option<(String, bool)>,
    /// The walk, for (layer, flagged only, project revision).
    stops: Option<((u32, bool, u64), Vec<Stop>)>,
    /// The shown asset's photos, for (layer, row, project revision).
    photos: Option<((u32, usize, u64), Vec<SlotPhotos>)>,
    /// Which of a slot's photos is shown, by slot id.
    shown: HashMap<String, usize>,
    /// The photo open in the full view.
    viewer: Option<String>,
    folder: String,
    /// What the last link or photo change did, shown under the Photos buttons.
    photo_note: Option<(String, bool)>,
    cache: PhotoCache,
    /// The Thermal view of the tower's thermal photo.
    pub thermal: crate::thermalview::ThermalState,
    /// The shown asset's photo markup by (slot, photo), read with `photos`.
    marks: Vec<(String, String, AnnotationSet)>,
    /// The markup editor, while open (kept when the pane closes, so unsaved shapes aren't lost).
    pub(crate) markup: Option<crate::markupview::Editor>,
    /// A line's report ZIP being written, and what the last report did.
    pub(crate) reports: Option<crate::markupview::ReportJob>,
    pub(crate) report_note: Option<(String, bool)>,
    /// A GeoFlux project (.geox) being read on a worker thread.
    geox: Option<GeoxJob>,
    /// A folder of raw drone photos being scanned on a worker thread.
    scan: Option<ScanJob>,
    /// Sorted photos being copied into tower folders on a worker thread.
    copy: Option<CopyJob>,
    /// The Findings window, while open.
    pub(crate) review: Option<crate::findingsview::Review>,
    /// The shown tower's findings, read once per project revision.
    pub(crate) tower: Option<crate::findingsview::TowerCache>,
}

/// Tower folders being copied off the frame.
struct CopyJob {
    folder: String,
    plan: std::sync::Arc<rustgis_engine::cmd::photo_sort::FolderPlan>,
    progress: std::sync::Arc<rustgis_engine::geoflux_report::Progress>,
    result: Slot<Result<rustgis_engine::cmd::photo_sort::FolderReport, String>>,
}

/// A raw photo folder being scanned off the frame; its layer is added when it lands.
struct ScanJob {
    name: String,
    progress: std::sync::Arc<rustgis_engine::geoflux_report::Progress>,
    result: Slot<Result<rustgis_engine::cmd::photo_scan::PhotoScan, String>>,
}

/// A GeoFlux project being read off the frame; its layer is added when it lands.
struct GeoxJob {
    name: String,
    progress: std::sync::Arc<rustgis_engine::geoflux_report::Progress>,
    result: Slot<Result<rustgis_engine::cmd::geox::GeoxProject, String>>,
}

impl InspectionState {
    /// Reads the photos and their markup again on the next frame.
    pub(crate) fn forget_marks(&mut self) {
        self.photos = None;
    }

    pub(crate) fn dirty(&self) -> bool {
        self.values != self.original
    }

    /// Frees the photos (and the thermal frame, stopping the DJI helper) when the pane is closed.
    pub fn release(&mut self, thermal: &rustgis_engine::cmd::thermal::Thermal) {
        if !self.cache.is_empty() {
            self.cache.clear();
        }
        self.viewer = None;
        self.thermal.release(thermal);
    }

    /// The scene settings the form gives as typed, saved or not.
    pub(crate) fn form_overrides(&self) -> rustgis_engine::geoflux_thermal::SceneOverrides {
        rustgis_engine::geoflux_thermal::FormBinding::default().overrides(|k| self.values.get(k).filter(|v| !v.trim().is_empty()).cloned())
    }

    /// Types values into the form (unsaved), for fields it has.
    pub(crate) fn enter_values(&mut self, values: Vec<(String, String)>) {
        for (k, v) in values {
            if self.fields.iter().any(|f| f.key == k) {
                self.values.insert(k, v);
            }
        }
        self.issues = form::validate(&self.fields, &self.values);
    }

    /// The shown tower's photos by slot, if read for (`layer`, `row`).
    pub(crate) fn slots(&self, layer: u32, row: usize) -> Option<&[SlotPhotos]> {
        self.photos.as_ref().filter(|((l, r, _), _)| (*l, *r) == (layer, row)).map(|(_, v)| v.as_slice())
    }

    /// The photo shown in a slot (its strip's choice, else the first).
    pub(crate) fn shown_in<'a>(&self, slot: &'a SlotPhotos) -> Option<&'a String> {
        let i = self.shown.get(&slot.slot.id).copied().unwrap_or(0).min(slot.photos.len().saturating_sub(1));
        slot.photos.get(i)
    }

    /// Decoded photos, for windows over one photo.
    pub(crate) fn photo(&mut self, path: &str, full: bool) -> Option<&Result<egui::TextureHandle, String>> {
        self.cache.get(path, if full { VIEW_SIDE } else { THUMB_SIDE })
    }

    /// The photo picked in the asset's thermal slot, if its photos are read.
    pub(crate) fn thermal_slot_photo(&self, layer: u32, row: usize) -> Option<String> {
        let ((l, r, _), slots) = self.photos.as_ref()?;
        if (*l, *r) != (layer, row) {
            return None;
        }
        let slot = slots.iter().find(|s| s.slot.thermal)?;
        let i = self.shown.get(&slot.slot.id).copied().unwrap_or(0).min(slot.photos.len().saturating_sub(1));
        slot.photos.get(i).cloned()
    }
}

/// The layers that follow an inspection schema, as (id, name).
fn inspection_layers(app: &RustGisApp) -> Vec<(u32, String)> {
    app.session.project.geoflux.links.iter().filter_map(|l| app.session.map().layer(l.layer).map(|x| (x.id, x.name.clone()))).collect()
}

pub(crate) fn severity_color(s: Severity, t: &Tokens) -> Color32 {
    match s {
        Severity::None => t.text_dim,
        Severity::Low => t.green,
        Severity::Medium => t.amber,
        Severity::High => Color32::from_rgb(0xE0, 0x6C, 0x1C),
        Severity::Critical => t.red,
    }
}

pub(crate) fn severity_label(s: Severity) -> &'static str {
    match s {
        Severity::None => tl!("No finding"),
        Severity::Low => tl!("Low"),
        Severity::Medium => tl!("Medium"),
        Severity::High => tl!("High"),
        Severity::Critical => tl!("Critical"),
    }
}

fn issue_text(k: IssueKind) -> &'static str {
    match k {
        IssueKind::Missing => tl!("Required"),
        IssueKind::NotANumber => tl!("Not a number"),
        IssueKind::NotAnInteger => tl!("Not a whole number"),
        IssueKind::OutOfRange => tl!("Out of range"),
        IssueKind::NotADate => tl!("Use YYYY-MM-DD"),
        IssueKind::NotAnOption => tl!("Not one of the options"),
        IssueKind::NotABoolean => tl!("Yes or no"),
        IssueKind::NotACoordinate => tl!("Not a coordinate"),
    }
}

pub fn show(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    let ctx = ui.ctx().clone();
    app.inspection.cache.poll(&ctx);
    crate::thermalview::poll_scan(app);
    geox_progress(app, ui, t);
    scan_progress(app, ui, t);
    copy_progress(app, ui, t);
    let layers = inspection_layers(app);
    if layers.is_empty() {
        if app.inspection.geox.is_some() || app.inspection.scan.is_some() || app.inspection.copy.is_some() {
            return;
        }
        ui.label(RichText::new(tl!("No inspection layer yet. Import a line KML, a survey CSV or a GeoFlux project to start.")).weak());
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            if ui.button(tl!("Import Line KML…")).clicked() {
                let _ = app.run("ui.geofluxImportLineKml", Value::Null);
            }
            if ui.button(tl!("Import Survey CSV…")).clicked() {
                let _ = app.run("ui.geofluxImport", Value::Null);
            }
            if ui.button(tl!("Import GeoFlux Project…")).clicked() {
                let _ = app.run("ui.geofluxImportGeox", Value::Null);
            }
        });
        if let Some((m, err)) = &app.inspection.message {
            ui.label(RichText::new(m.as_str()).small().color(if *err { t.red } else { t.text_dim }));
        }
        return;
    }
    let st = &mut app.inspection;
    if !st.layer.is_some_and(|l| layers.iter().any(|(id, _)| *id == l)) {
        let active = app.session.active_layer.filter(|a| layers.iter().any(|(id, _)| id == a));
        st.layer = active.or(layers.first().map(|(id, _)| *id));
        st.row = None;
        st.loaded = None;
    }
    let Some(layer) = st.layer else { return };

    if let Some((m, err)) = &st.message {
        let c = if *err { t.red } else { t.text_dim };
        ui.label(RichText::new(m.as_str()).small().color(c));
    }
    let current = layers.iter().find(|(id, _)| *id == layer).map_or("", |(_, n)| n.as_str());
    let mut pick = None;
    egui::ComboBox::from_id_salt("geoflux_layer").selected_text(current).width(ui.available_width()).show_ui(ui, |ui| {
        for (id, name) in &layers {
            if ui.selectable_label(*id == layer, name.as_str()).clicked() {
                pick = Some(*id);
            }
        }
    });
    if let Some(id) = pick.filter(|id| *id != layer) {
        if st.dirty() {
            st.message = Some((tl!("Save or revert this tower's changes first.").into(), true));
        } else {
            st.layer = Some(id);
            st.row = None;
            st.loaded = None;
            st.shown.clear();
        }
        return;
    }

    let rev = app.session.revision;
    let flagged = st.flagged_only;
    if st.stops.as_ref().is_none_or(|(k, _)| *k != (layer, flagged, rev)) {
        match gf::sequence(&app.session, layer, flagged) {
            Ok(v) => st.stops = Some(((layer, flagged, rev), v)),
            Err(e) => {
                ui.label(RichText::new(e).color(t.red));
                return;
            }
        }
    }
    // A single feature selected on the map (or in the table) is followed, unless edits are open.
    if !st.dirty()
        && let Some(sel) = app.session.map().layer(layer).map(|l| l.selection.as_slice())
        && let [one] = sel
        && st.row != Some(*one)
    {
        st.row = Some(*one);
    }
    let stops: &[Stop] = st.stops.as_ref().map_or(&[], |(_, v)| v.as_slice());
    let rows = app.session.map().layer(layer).map_or(0, |l| l.dataset.features.len());
    if st.row.is_none_or(|r| r >= rows) {
        st.row = stops.first().map(|s| s.row);
    }

    let target = navigator(st, ui, t);
    if let Some(row) = target {
        go(app, layer, row);
        return;
    }
    let st = &mut app.inspection;
    let Some(row) = st.row else {
        ui.label(RichText::new(if st.flagged_only { tl!("No flagged towers.") } else { tl!("This layer has no towers.") }).weak());
        return;
    };

    // The form follows the asset; after an undo or another change it is read again unless the
    // user has unsaved edits.
    let stale = match st.loaded {
        Some((l, r, v)) => l != layer || r != row || (v != rev && !st.dirty()),
        None => true,
    };
    if stale {
        load(app, layer, row, rev);
    }
    let st = &mut app.inspection;
    let save = form_ui(st, ui, t);
    if save {
        save_form(app, layer, row);
    }
    ui.add_space(8.0);
    crate::findingsview::summary_ui(app, ui, t, layer, row);
    ui.add_space(8.0);
    photos_ui(app, ui, t, &ctx, layer, row, rev);
    ui.add_space(8.0);
    crate::markupview::reports_ui(app, ui, t, layer, row);
}

/// Reads the asset at `row` into the form.
fn load(app: &mut RustGisApp, layer: u32, row: usize, rev: u64) {
    let st = &mut app.inspection;
    st.loaded = Some((layer, row, rev));
    match gf::asset(&app.session, layer, row) {
        Ok((schema, asset)) => {
            st.locked = match app.session.project.geoflux.findings_of(layer) {
                Some(_) => schema.finding_fields.iter().map(|d| d.key.clone()).collect(),
                None => BTreeSet::new(),
            };
            st.fields = schema.fields_for(&asset);
            st.values = form::values(&asset, &st.fields);
            st.original = st.values.clone();
            st.issues = form::validate(&st.fields, &st.values);
            st.layout = layout(&st.fields);
        }
        Err(e) => {
            st.fields.clear();
            st.locked.clear();
            st.layout.clear();
            st.values.clear();
            st.original.clear();
            st.issues.clear();
            st.message = Some((e, true));
        }
    }
}

/// Fields by group (in order of first appearance), then by row number, keeping schema order.
fn layout(fields: &[FieldDef]) -> Vec<(String, Vec<Vec<usize>>)> {
    let mut groups: Vec<(String, BTreeMap<u32, Vec<usize>>)> = Vec::new();
    for (i, f) in fields.iter().enumerate() {
        let g = match groups.iter().position(|(name, _)| *name == f.group) {
            Some(g) => g,
            None => {
                groups.push((f.group.clone(), BTreeMap::new()));
                groups.len() - 1
            }
        };
        if let Some((_, rows)) = groups.get_mut(g) {
            rows.entry(f.row).or_default().push(i);
        }
    }
    groups.into_iter().map(|(name, rows)| (name, rows.into_values().collect())).collect()
}

/// Previous / next, the position in the walk, the flagged filter and the tower list. Returns
/// the row to go to, if one was picked.
fn navigator(st: &mut InspectionState, ui: &mut Ui, t: &Tokens) -> Option<usize> {
    let stops: &[Stop] = st.stops.as_ref().map_or(&[], |(_, v)| v.as_slice());
    let at = st.row.and_then(|r| stops.iter().position(|s| s.row == r));
    // The neighbour in the walk, stopping at the ends; from nowhere, the first (or last) stop.
    let step = |forward: bool| -> Option<usize> {
        let i = match (at, forward) {
            (Some(i), true) => i + 1,
            (Some(i), false) => i.checked_sub(1)?,
            (None, true) => 0,
            (None, false) => stops.len().checked_sub(1)?,
        };
        stops.get(i).map(|s| s.row)
    };
    let mut target = None;
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(at.is_some_and(|i| i > 0), egui::Button::new("‹").small())
            .on_hover_text(tl!("Previous tower (Page Up)"))
            .clicked()
        {
            target = step(false);
        }
        let here = at.and_then(|i| stops.get(i));
        let name = here.map_or("—", |s| if s.label.is_empty() { "?" } else { s.label.as_str() });
        ui.label(RichText::new(name).font(semibold(14.0)));
        if let Some(s) = here {
            ui.label(RichText::new(severity_label(s.severity)).small().color(severity_color(s.severity, t)));
        }
        let pos = at.map_or(String::new(), |i| {
            crate::i18n::fmt("{n} of {total}", &[("n", &(i + 1).to_string()), ("total", &stops.len().to_string())])
        });
        ui.label(RichText::new(pos).small().color(t.text_dim));
        if ui
            .add_enabled(at.is_none_or(|i| i + 1 < stops.len()) && !stops.is_empty(), egui::Button::new("›").small())
            .on_hover_text(tl!("Next tower (Page Down)"))
            .clicked()
        {
            target = step(true);
        }
    });
    // Page keys walk the line while no text box has the keyboard.
    if ui.ctx().memory(|m| m.focused().is_none()) {
        let (down, up) = ui.ctx().input(|i| (i.key_pressed(egui::Key::PageDown), i.key_pressed(egui::Key::PageUp)));
        if down || up {
            target = step(down);
        }
    }
    ui.checkbox(&mut st.flagged_only, tl!("Flagged towers only"));
    egui::CollapsingHeader::new(tl!("All towers")).id_salt("geoflux_towers").show(ui, |ui| {
        let h = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
        egui::ScrollArea::vertical().max_height(h * LIST_ROWS).auto_shrink([false, true]).show_rows(ui, h, stops.len(), |ui, range| {
            for s in stops.get(range).unwrap_or(&[]) {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), egui::Sense::hover());
                    ui.painter().circle_filled(r.center(), 4.0, severity_color(s.severity, t));
                    let label = if s.label.is_empty() { "?" } else { s.label.as_str() };
                    if ui.selectable_label(st.row == Some(s.row), label).clicked() {
                        target = Some(s.row);
                    }
                });
            }
        });
    });
    target.filter(|r| Some(*r) != st.row)
}

/// Moves to another asset: unsaved edits are saved first (and the move is cancelled if they
/// can't be), then the asset is selected and the map centred on it.
pub(crate) fn go(app: &mut RustGisApp, layer: u32, row: usize) {
    if app.inspection.dirty() {
        let Some(from) = app.inspection.row else { return };
        if !save_form(app, layer, from) {
            return;
        }
    }
    let st = &mut app.inspection;
    st.row = Some(row);
    st.shown.clear();
    st.message = None;
    if let Err(e) = app.run("geoflux.goTo", json!({"layer": layer, "row": row})) {
        app.inspection.message = Some((e, true));
    }
}

/// Saves the changed values as one undoable edit. False when they were refused.
fn save_form(app: &mut RustGisApp, layer: u32, row: usize) -> bool {
    let st = &app.inspection;
    let changed: Map<String, Value> =
        st.values.iter().filter(|(k, v)| st.original.get(*k) != Some(*v)).map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
    if changed.is_empty() {
        return true;
    }
    match app.run("geoflux.setValues", json!({"layer": layer, "row": row, "values": changed})) {
        Ok(_) => {
            let st = &mut app.inspection;
            st.loaded = None;
            st.message = Some((tl!("Saved.").into(), false));
            true
        }
        Err(e) => {
            app.inspection.message = Some((e, true));
            false
        }
    }
}

/// The form, group by group. Returns whether Save was pressed.
fn form_ui(st: &mut InspectionState, ui: &mut Ui, t: &Tokens) -> bool {
    let mut changed = false;
    let InspectionState { fields, layout, values, issues, locked, .. } = st;
    for (group, rows) in layout.iter() {
        ui.add_space(6.0);
        let title = if group.is_empty() { tl!("Details") } else { group.as_str() };
        ui.label(RichText::new(title).font(semibold(12.5)));
        for row in rows {
            let weights: f32 = row.iter().filter_map(|&i| fields.get(i)).map(|f| f.weight.max(0.1)).sum::<f32>().max(0.1);
            let gap = ui.spacing().item_spacing.x;
            let room = (ui.available_width() - gap * row.len().saturating_sub(1) as f32).max(60.0);
            ui.horizontal_top(|ui| {
                for &i in row {
                    let Some(f) = fields.get(i) else { continue };
                    let w = (room * f.weight.max(0.1) / weights).max(40.0);
                    let issue = issues.iter().find(|x| x.key == f.key).map(|x| x.kind);
                    ui.vertical(|ui| {
                        ui.set_width(w);
                        let mut label = RichText::new(if f.required { format!("{} *", f.label) } else { f.label.clone() }).small();
                        label = if issue.is_some() { label.color(t.red) } else { label.color(t.text_dim) };
                        ui.label(label);
                        if !values.contains_key(&f.key) {
                            values.insert(f.key.clone(), String::new());
                        }
                        if let Some(value) = values.get_mut(&f.key) {
                            if locked.contains(&f.key) {
                                // Filled from the tower's findings: shown, never typed over.
                                let shown = if value.is_empty() { "—" } else { value.as_str() };
                                ui.add_sized([w - 8.0, 20.0], egui::Label::new(RichText::new(shown).color(t.text_dim)).truncate())
                                    .on_hover_text(tl!("Filled from this tower's findings. Change a finding to change it."));
                            } else if field_widget(ui, f, value, w) {
                                changed = true;
                            }
                        }
                        if let Some(k) = issue {
                            ui.label(RichText::new(issue_text(k)).small().color(t.red));
                        }
                    });
                }
            });
        }
    }
    if changed {
        *issues = form::validate(fields, values);
    }
    ui.add_space(8.0);
    let dirty = st.dirty();
    let mut save = false;
    ui.horizontal(|ui| {
        if ui.add_enabled(dirty, egui::Button::new(tl!("Save"))).clicked() {
            save = true;
        }
        if ui.add_enabled(dirty, egui::Button::new(tl!("Revert"))).clicked() {
            st.values = st.original.clone();
            st.issues = form::validate(&st.fields, &st.values);
        }
        if !st.issues.is_empty() {
            let n = st.issues.len().to_string();
            ui.label(RichText::new(crate::i18n::fmt("{n} fields to check", &[("n", &n)])).small().color(t.amber));
        }
    });
    save
}

/// One field's input. Returns whether the value changed.
fn field_widget(ui: &mut Ui, f: &FieldDef, value: &mut String, w: f32) -> bool {
    let id = egui::Id::new(("geoflux_field", f.key.as_str()));
    match f.field_type {
        FieldType::Dropdown => {
            let mut changed = false;
            egui::ComboBox::from_id_salt(id).selected_text(value.as_str()).width(w - 8.0).show_ui(ui, |ui| {
                if ui.selectable_label(value.is_empty(), "—").clicked() && !value.is_empty() {
                    value.clear();
                    changed = true;
                }
                for o in &f.options {
                    if ui.selectable_label(value == o, o.as_str()).clicked() && value != o {
                        *value = o.clone();
                        changed = true;
                    }
                }
            });
            changed
        }
        FieldType::MultiSelect => {
            let mut chosen: Vec<String> = value.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            let mut changed = false;
            egui::ComboBox::from_id_salt(id).selected_text(value.as_str()).width(w - 8.0).show_ui(ui, |ui| {
                for o in &f.options {
                    let mut on = chosen.contains(o);
                    if ui.checkbox(&mut on, o.as_str()).changed() {
                        if on {
                            chosen.push(o.clone());
                        } else {
                            chosen.retain(|c| c != o);
                        }
                        changed = true;
                    }
                }
            });
            if changed {
                *value = chosen.join(", ");
            }
            changed
        }
        FieldType::Boolean => {
            let shown = match form::parse_bool(value) {
                Some(true) => tl!("Yes"),
                Some(false) => tl!("No"),
                None => "—",
            };
            let mut changed = false;
            egui::ComboBox::from_id_salt(id).selected_text(shown).width(w - 8.0).show_ui(ui, |ui| {
                for (text, v) in [("—", ""), (tl!("Yes"), "true"), (tl!("No"), "false")] {
                    if ui.selectable_label(value == v, text).clicked() && value != v {
                        *value = v.to_string();
                        changed = true;
                    }
                }
            });
            changed
        }
        _ => {
            let hint = match f.field_type {
                FieldType::Date => "YYYY-MM-DD",
                _ => "",
            };
            ui.add(egui::TextEdit::singleline(value).id(id).hint_text(hint).desired_width(w - 8.0)).changed()
        }
    }
}

/// The photo folder and the asset's slots, two per row.
fn photos_ui(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens, ctx: &egui::Context, layer: u32, row: usize, rev: u64) {
    ui.label(RichText::new(tl!("Photos")).font(semibold(12.5)));
    let linked = app.session.project.geoflux.photos_of(layer).map(|l| l.folder.clone());
    let st = &mut app.inspection;
    if st.folder.is_empty()
        && let Some(f) = &linked
    {
        st.folder = f.clone();
    }
    let mut link = false;
    // The path gets the whole width (paths are long); the buttons go under it.
    ui.add(egui::TextEdit::singleline(&mut st.folder).hint_text(tl!("Photo folder")).desired_width(ui.available_width()));
    ui.horizontal(|ui| {
        #[cfg(not(target_arch = "wasm32"))]
        if ui.small_button(tl!("Browse…")).clicked()
            && let Some(dir) = rfd::FileDialog::new().set_title(tl!("Folder of tower photos")).pick_folder()
        {
            st.folder = dir.display().to_string();
            link = true;
        }
        let label = if linked.is_some() { tl!("Relink") } else { tl!("Link") };
        if ui
            .small_button(label)
            .on_hover_text(tl!("One sub-folder per tower (76_0), or files named after the tower (76_0_thermal.jpg)."))
            .clicked()
        {
            link = true;
        }
    });
    if link {
        let folder = app.inspection.folder.trim().to_string();
        match app.run("geoflux.linkPhotos", json!({"layer": layer, "folder": folder})) {
            Ok(r) => {
                let none = r["linked"].as_u64() == Some(0);
                let text = if none {
                    tl!("No photo in that folder matched a tower. Pick the folder that holds one sub-folder per tower (76_0), a tower's own folder, or name the files after the tower (76_0_thermal.jpg). Or press + in a slot to add a photo.").to_string()
                } else {
                    crate::i18n::fmt(
                        "{linked} photos linked to {towers} towers; {unplaced} files not linked",
                        &[
                            ("linked", &r["linked"].to_string()),
                            ("towers", &r["towersWithPhotos"].to_string()),
                            ("unplaced", &r["unplaced"].to_string()),
                        ],
                    )
                };
                app.inspection.photo_note = Some((text, none));
                app.inspection.photos = None;
                app.inspection.cache.clear();
            }
            Err(e) => app.inspection.photo_note = Some((e, true)),
        }
        return;
    }
    if let Some((m, err)) = &app.inspection.photo_note {
        ui.label(RichText::new(m.as_str()).small().color(if *err { t.amber } else { t.text_dim }));
    }
    let picked: Vec<String> = app.session.project.geoflux.picked.iter().filter(|p| p.layer == layer).map(|p| p.path.clone()).collect();
    let st = &mut app.inspection;
    if st.photos.as_ref().is_none_or(|(k, _)| *k != (layer, row, rev)) {
        let slots = gf::photos(&app.session, layer, row).unwrap_or_default();
        let marks = crate::markupview::marks(app, layer, row, &slots);
        let st = &mut app.inspection;
        st.marks = marks;
        st.photos = Some(((layer, row, rev), slots));
    }
    let st = &mut app.inspection;
    let Some((_, slots)) = st.photos.take() else { return };
    let cell = ((ui.available_width() - ui.spacing().item_spacing.x) / 2.0).max(80.0);
    let mut action = None;
    for pair in slots.chunks(2) {
        ui.horizontal_top(|ui| {
            for slot in pair {
                ui.vertical(|ui| {
                    ui.set_width(cell);
                    if let Some(a) = slot_ui(st, ui, t, slot, cell, &picked) {
                        action = Some(a);
                    }
                });
            }
        });
    }
    st.photos = Some(((layer, row, rev), slots));
    viewer(st, ctx, t);
    let texture = st.markup.as_ref().and_then(|e| st.cache.get(e.photo(), VIEW_SIDE).cloned());
    crate::markupview::show(app, ctx, t, texture);
    crate::findingsview::show(app, ctx, t, layer, row);
    if let Some(a) = action {
        slot_action(app, layer, row, a);
    }
    if app.inspection.thermal.on {
        crate::thermalview::show(app, ui, t, layer, row);
    } else {
        app.inspection.thermal.release_idle(&app.session.thermal);
    }
}

/// What a slot asked for: photos to add to it, or a picked photo to take out.
enum SlotAction {
    // The browser has no file picker for this yet.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    Add(String, Vec<String>),
    Remove(String, String),
    /// Open the markup editor on a photo of the slot.
    Markup(String, String),
    /// Make a photo the one the slot's report shows.
    Choose(String, String),
}

/// Asks for photo files to add to a slot.
#[cfg(not(target_arch = "wasm32"))]
fn pick_photos(slot: &str, label: &str) -> Option<SlotAction> {
    let title = crate::i18n::fmt("Photos for {slot}", &[("slot", label)]);
    let files =
        rfd::FileDialog::new().set_title(title).add_filter(tl!("Photos"), &["jpg", "jpeg", "png", "JPG", "JPEG", "PNG"]).pick_files()?;
    (!files.is_empty()).then(|| SlotAction::Add(slot.to_string(), files.iter().map(|f| f.display().to_string()).collect()))
}

#[cfg(target_arch = "wasm32")]
fn pick_photos(_: &str, _: &str) -> Option<SlotAction> {
    None
}

/// Runs a slot's request and shows what it did.
fn slot_action(app: &mut RustGisApp, layer: u32, row: usize, action: SlotAction) {
    let result = match action {
        SlotAction::Add(slot, paths) => app
            .run("geoflux.addPhotos", json!({"layer": layer, "row": row, "slot": slot, "paths": paths}))
            .map(|r| crate::i18n::fmt("{n} photos added.", &[("n", &r["added"].to_string())])),
        SlotAction::Remove(slot, path) => app
            .run("geoflux.removePhoto", json!({"layer": layer, "row": row, "slot": slot, "path": path}))
            .map(|_| tl!("Photo taken out of the slot (the file is kept).").to_string()),
        SlotAction::Markup(slot, path) => {
            crate::markupview::open(app, layer, row, &slot, &path);
            return;
        }
        SlotAction::Choose(slot, path) => {
            let r = app
                .run("geoflux.reportPhoto", json!({"layer": layer, "row": row, "slot": slot, "path": path}))
                .map(|_| tl!("This photo is now the one in the report.").to_string());
            // The chosen photo moves to the front of its slot.
            app.inspection.shown.insert(slot, 0);
            r
        }
    };
    let st = &mut app.inspection;
    st.photo_note = Some(match result {
        Ok(m) => (m, false),
        Err(e) => (e, true),
    });
    st.photos = None;
}

fn slot_ui(st: &mut InspectionState, ui: &mut Ui, t: &Tokens, slot: &SlotPhotos, w: f32, picked: &[String]) -> Option<SlotAction> {
    let n = slot.photos.len();
    let i = st.shown.get(&slot.slot.id).copied().unwrap_or(0).min(n.saturating_sub(1));
    let mut action = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new(slot.slot.label.as_str()).small().color(t.text_dim));
        if n > 0 && !cfg!(target_arch = "wasm32") && ui.small_button("+").on_hover_text(tl!("Add photos to this slot")).clicked() {
            action = pick_photos(&slot.slot.id, &slot.slot.label);
        }
        if let Some(p) = slot.photos.get(i)
            && !cfg!(target_arch = "wasm32")
            && ui.small_button("✎").on_hover_text(tl!("Mark up this photo for the report")).clicked()
        {
            action = Some(SlotAction::Markup(slot.slot.id.clone(), p.clone()));
        }
        if let Some(p) = slot.photos.get(i).filter(|_| i > 0)
            && ui.small_button("★").on_hover_text(tl!("Use this photo in the report")).clicked()
        {
            action = Some(SlotAction::Choose(slot.slot.id.clone(), p.clone()));
        }
        if n > 1 && i == 0 {
            ui.label(RichText::new("★").small().color(t.accent)).on_hover_text(tl!("This photo is the one in the report"));
        }
        if let Some(p) = slot.photos.get(i).filter(|p| picked.contains(p))
            && ui.small_button("×").on_hover_text(tl!("Take this photo out of the slot (the file is kept)")).clicked()
        {
            action = Some(SlotAction::Remove(slot.slot.id.clone(), p.clone()));
        }
        if n > 1 {
            if ui.small_button("‹").clicked() {
                st.shown.insert(slot.slot.id.clone(), i.checked_sub(1).unwrap_or(n - 1));
            }
            ui.label(RichText::new(format!("{}/{n}", i + 1)).small());
            if ui.small_button("›").clicked() {
                st.shown.insert(slot.slot.id.clone(), (i + 1) % n);
            }
        }
    });
    let size = vec2(w, w * 0.75);
    let Some(path) = slot.photos.get(i) else {
        // An empty slot is one big + that picks its photos.
        let web = cfg!(target_arch = "wasm32");
        let (r, resp) = ui.allocate_exact_size(size, if web { egui::Sense::hover() } else { egui::Sense::click() });
        let hot = resp.hovered() && !web;
        let line = if hot { t.accent } else { t.border };
        ui.painter().rect_stroke(r, 4.0, egui::Stroke::new(1.0, line), egui::StrokeKind::Inside);
        if web {
            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, tl!("No photo"), egui::FontId::proportional(11.0), t.text_disabled);
        } else {
            let c = r.center();
            let arm = (r.height() * 0.12).clamp(8.0, 18.0);
            let ink = egui::Stroke::new(2.0, if hot { t.accent } else { t.text_dim });
            ui.painter().line_segment([c - vec2(arm, 0.0), c + vec2(arm, 0.0)], ink);
            ui.painter().line_segment([c - vec2(0.0, arm), c + vec2(0.0, arm)], ink);
            ui.painter().text(
                c + vec2(0.0, arm + 12.0),
                egui::Align2::CENTER_CENTER,
                tl!("Add photo"),
                egui::FontId::proportional(11.0),
                t.text_disabled,
            );
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tl!("Pick photos for this slot")).clicked() {
                action = pick_photos(&slot.slot.id, &slot.slot.label);
            }
        }
        return action;
    };
    match st.cache.get(path, THUMB_SIDE) {
        Some(Ok(tex)) => {
            let resp = ui.add(egui::Image::from_texture(tex).max_size(size).sense(egui::Sense::click())).on_hover_text(path.as_str());
            if let Some((_, _, set)) = st.marks.iter().find(|(s, p, _)| *s == slot.slot.id && p == path) {
                crate::markupview::paint(ui.painter(), resp.rect, set.shapes());
            }
            if resp.clicked() {
                st.viewer = Some(path.clone());
            }
        }
        Some(Err(e)) => {
            ui.label(RichText::new(e.as_str()).small().color(t.red)).on_hover_text(path.as_str());
        }
        None => {
            let (r, _) = ui.allocate_exact_size(size, egui::Sense::hover());
            ui.put(r, egui::Spinner::new());
        }
    }
    if n > 1 {
        strip(st, ui, t, slot, i, w);
    }
    action
}

/// Photos shown at once in a slot's strip: the shown one and its neighbours, so a tower with
/// dozens of sorted photos decodes only these (they share the slot's thumbnail cache).
const STRIP: usize = 7;

/// A row of small thumbnails under a slot's photo; clicking one shows it. The first photo, the
/// one in the report, is marked with a star.
fn strip(st: &mut InspectionState, ui: &mut Ui, t: &Tokens, slot: &SlotPhotos, shown: usize, w: f32) {
    let n = slot.photos.len();
    let start = shown.saturating_sub(STRIP / 2).min(n.saturating_sub(STRIP));
    let side = ((w - 4.0 * (STRIP as f32 - 1.0)) / STRIP as f32).max(16.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (j, path) in slot.photos.iter().enumerate().skip(start).take(STRIP) {
            let (r, resp) = ui.allocate_exact_size(vec2(side, side), egui::Sense::click());
            match st.cache.get(path, THUMB_SIDE) {
                Some(Ok(tex)) => {
                    egui::Image::from_texture(tex).maintain_aspect_ratio(false).paint_at(ui, r);
                }
                Some(Err(_)) => {
                    ui.painter().rect_filled(r, 2.0, t.red.gamma_multiply(0.3));
                }
                None => {
                    ui.painter().rect_filled(r, 2.0, t.border);
                }
            }
            let line = if j == shown { egui::Stroke::new(2.0, t.accent) } else { egui::Stroke::new(1.0, t.border) };
            ui.painter().rect_stroke(r, 2.0, line, egui::StrokeKind::Inside);
            if j == 0 {
                ui.painter().text(r.left_top() + vec2(2.0, 1.0), egui::Align2::LEFT_TOP, "★", egui::FontId::proportional(10.0), t.accent);
            }
            let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
            if resp.on_hover_text(name).clicked() {
                st.shown.insert(slot.slot.id.clone(), j);
            }
        }
    });
}

/// The full view of one photo, in its own window.
fn viewer(st: &mut InspectionState, ctx: &egui::Context, t: &Tokens) {
    let Some(path) = st.viewer.clone() else { return };
    let name = path.rsplit(['/', '\\']).next().unwrap_or(&path).to_string();
    let mut open = true;
    egui::Window::new(name).id(egui::Id::new("geoflux_viewer")).open(&mut open).default_size(vec2(900.0, 700.0)).resizable(true).show(
        ctx,
        |ui| {
            let room = ui.available_size() - vec2(0.0, 28.0);
            match st.cache.get(&path, VIEW_SIDE) {
                Some(Ok(tex)) => {
                    ui.add(egui::Image::from_texture(tex).max_size(room));
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e.as_str()).color(t.red));
                }
                None => {
                    ui.add(egui::Spinner::new());
                }
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new(path.as_str()).small().color(t.text_dim));
                #[cfg(not(target_arch = "wasm32"))]
                if ui.small_button(tl!("Open")).clicked()
                    && let Err(e) = crate::open_with_system(std::path::Path::new(&path))
                {
                    st.message = Some((e, true));
                }
            });
        },
    );
    if !open {
        st.viewer = None;
    }
}

/// GeoFlux › Import Survey CSV: picks a CSV, imports it with the default schema, and opens the
/// Inspection pane on the new layer.
pub fn import(app: &mut RustGisApp) -> Result<(), String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(path) = crate::dialogs::pick_open(&["csv", "txt"], tl!("Survey CSV")) else { return Ok(()) };
        let r = app.run("geoflux.importCsv", json!({"path": path}))?;
        let st = &mut app.inspection;
        st.layer = r["layer"].as_u64().map(|v| v as u32);
        st.row = None;
        st.loaded = None;
        app.ui.right = Some(crate::RightPane::Inspection);
        Ok(())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = app;
        Err(tl!("Importing survey CSVs needs the desktop app for now.").into())
    }
}

/// GeoFlux › Import Line KML: picks a line's KML or KMZ, adds its towers as an inspection layer
/// (and its spans beneath), and opens the Inspection pane on the towers.
pub fn import_line_kml(app: &mut RustGisApp) -> Result<(), String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(path) = crate::dialogs::pick_open(&["kml", "kmz"], tl!("Line KML")) else { return Ok(()) };
        let r = app.run("geoflux.importLineKml", json!({"path": path}))?;
        let st = &mut app.inspection;
        st.layer = r["layer"].as_u64().map(|v| v as u32);
        st.row = None;
        st.loaded = None;
        app.ui.right = Some(crate::RightPane::Inspection);
        Ok(())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = app;
        Err(tl!("Importing a line KML needs the desktop app for now.").into())
    }
}

/// GeoFlux › Import GeoFlux Project: reads a GeoFlux 1.x project (`.geox`, picked when `path` is
/// None) on a worker thread, with progress and cancel in the Inspection pane; [`poll_geox`] adds
/// its tower layer when it lands.
pub fn import_geox(app: &mut RustGisApp, path: Option<String>) -> Result<(), String> {
    if app.inspection.geox.is_some() {
        return Err(tl!("A GeoFlux project is already being imported.").into());
    }
    let Some(path) = path.or_else(|| crate::dialogs::pick_open(&["geox"], tl!("GeoFlux project"))) else { return Ok(()) };
    let name = path.rsplit(['/', '\\']).next().unwrap_or(&path).to_string();
    let job = GeoxJob { name, progress: rustgis_engine::geoflux_report::Progress::new(), result: Slot::default() };
    start_geox(path, &job)?;
    app.inspection.geox = Some(job);
    app.inspection.message = None;
    app.ui.right = Some(crate::RightPane::Inspection);
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn start_geox(path: String, job: &GeoxJob) -> Result<(), String> {
    let (progress, land) = (job.progress.clone(), job.result.clone());
    std::thread::Builder::new()
        .name("geoflux-geox".into())
        .spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rustgis_engine::cmd::geox::read(&path, None, &progress)))
                .unwrap_or_else(|_| Err(tl!("Reading the GeoFlux project failed unexpectedly.").to_string()));
            *land.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(target_arch = "wasm32")]
fn start_geox(_: String, _: &GeoxJob) -> Result<(), String> {
    Err(tl!("Importing GeoFlux projects needs the desktop app.").into())
}

/// The import's progress bar and Cancel, at the top of the Inspection pane.
fn geox_progress(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    let Some(job) = &app.inspection.geox else { return };
    let (done, total) = job.progress.get();
    ui.label(RichText::new(crate::i18n::fmt("Importing {name}", &[("name", &job.name)])).font(semibold(12.5)));
    let frac = if total == 0 { 0.0 } else { done as f32 / total as f32 };
    ui.add(
        egui::ProgressBar::new(frac)
            .text(crate::i18n::fmt("{done} of {total} towers", &[("done", &done.to_string()), ("total", &total.to_string())])),
    );
    if job.progress.is_cancelled() {
        ui.label(RichText::new(tl!("Stopping…")).small().color(t.text_dim));
    } else if ui.small_button(tl!("Cancel")).clicked() {
        job.progress.cancel();
    }
    ui.separator();
}

/// Adds a read GeoFlux project's layer when it lands (called every frame, so it lands with the
/// Inspection pane closed too).
pub fn poll_geox(app: &mut RustGisApp, ctx: &egui::Context) {
    let Some(job) = &app.inspection.geox else { return };
    let done = job.result.lock().unwrap_or_else(|e| e.into_inner()).take();
    let Some(result) = done else {
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
        return;
    };
    app.inspection.geox = None;
    let note = match result {
        Ok(project) => match rustgis_engine::cmd::geox::apply(&mut app.session, project, None) {
            Ok(r) => {
                let st = &mut app.inspection;
                st.layer = r["layer"].as_u64().map(|v| v as u32);
                st.row = None;
                st.loaded = None;
                st.cache.clear();
                let mut text = crate::i18n::fmt("Imported {n} towers", &[("n", &r["towers"].to_string())]);
                if let Some(n) = r["photosInSurveyFolder"].as_u64().filter(|n| *n > 0) {
                    text.push_str(&crate::i18n::fmt(
                        "; {n} photos were in GeoFlux's photo folder: use Link Photos to link it",
                        &[("n", &n.to_string())],
                    ));
                }
                (text, false)
            }
            Err(e) => (e, true),
        },
        Err(e) if e == "cancelled" => (tl!("Import cancelled; nothing was added.").to_string(), false),
        Err(e) => {
            app.session.notify(e.clone(), true);
            (e, true)
        }
    };
    app.inspection.message = Some(note);
    app.ui.right = Some(crate::RightPane::Inspection);
}

/// GeoFlux › Scan Raw Photos: picks a survey day's folder of drone photos and reads where each
/// was taken on a worker thread, with progress and cancel in the Inspection pane; [`poll_scan`]
/// adds the photo layer when it lands.
pub fn scan_photos(app: &mut RustGisApp, folder: Option<String>) -> Result<(), String> {
    if app.inspection.scan.is_some() {
        return Err(tl!("A photo folder is already being scanned.").into());
    }
    #[cfg(not(target_arch = "wasm32"))]
    let folder = folder
        .or_else(|| rfd::FileDialog::new().set_title(tl!("Folder of raw drone photos")).pick_folder().map(|d| d.display().to_string()));
    let Some(folder) = folder else { return Ok(()) };
    let name = folder.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(&folder).to_string();
    let job = ScanJob { name, progress: rustgis_engine::geoflux_report::Progress::new(), result: Slot::default() };
    start_scan(folder, &job)?;
    app.inspection.scan = Some(job);
    app.inspection.message = None;
    app.ui.right = Some(crate::RightPane::Inspection);
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn start_scan(folder: String, job: &ScanJob) -> Result<(), String> {
    let (progress, land) = (job.progress.clone(), job.result.clone());
    std::thread::Builder::new()
        .name("geoflux-photo-scan".into())
        .spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rustgis_engine::cmd::photo_scan::scan(&folder, &progress)))
                .unwrap_or_else(|_| Err(tl!("Scanning the photos failed unexpectedly.").to_string()));
            *land.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(target_arch = "wasm32")]
fn start_scan(_: String, _: &ScanJob) -> Result<(), String> {
    Err(tl!("Scanning photo folders needs the desktop app.").into())
}

/// The scan's progress bar and Cancel, at the top of the Inspection pane.
fn scan_progress(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    let Some(job) = &app.inspection.scan else { return };
    let (done, total) = job.progress.get();
    ui.label(RichText::new(crate::i18n::fmt("Scanning {name}", &[("name", &job.name)])).font(semibold(12.5)));
    let frac = if total == 0 { 0.0 } else { done as f32 / total as f32 };
    ui.add(
        egui::ProgressBar::new(frac)
            .text(crate::i18n::fmt("{done} of {total} photos", &[("done", &done.to_string()), ("total", &total.to_string())])),
    );
    if job.progress.is_cancelled() {
        ui.label(RichText::new(tl!("Stopping…")).small().color(t.text_dim));
    } else if ui.small_button(tl!("Cancel")).clicked() {
        job.progress.cancel();
    }
    ui.separator();
}

/// Adds a scanned folder's photo layer when it lands (called every frame, so it lands with the
/// Inspection pane closed too). The inspected tower layer stays as it is.
pub fn poll_scan(app: &mut RustGisApp, ctx: &egui::Context) {
    let Some(job) = &app.inspection.scan else { return };
    let done = job.result.lock().unwrap_or_else(|e| e.into_inner()).take();
    let Some(result) = done else {
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
        return;
    };
    app.inspection.scan = None;
    let note = match result {
        Ok(scan) => match rustgis_engine::cmd::photo_scan::apply(&mut app.session, scan, None) {
            Ok(r) => {
                // The photos' table opens under the map, so their details are one look away.
                app.session.table_layer = r["layer"].as_u64().map(|v| v as u32);
                let count = |k: &str| r["kinds"][k].as_u64().unwrap_or(0).to_string();
                (
                    crate::i18n::fmt(
                        "Found {n} photos with a position ({thermal} thermal, {visual} visual)",
                        &[("n", &r["photos"].to_string()), ("thermal", &count("thermal")), ("visual", &count("visual"))],
                    ),
                    false,
                )
            }
            Err(e) => (e, true),
        },
        Err(e) if e == "cancelled" => (tl!("Scan cancelled; nothing was added.").to_string(), false),
        Err(e) => {
            app.session.notify(e.clone(), true);
            (e, true)
        }
    };
    app.inspection.message = Some(note);
    app.ui.right = Some(crate::RightPane::Inspection);
}

/// GeoFlux › Sort Photos to Towers: gives each photo of the newest scanned day to the nearest
/// tower of the inspected (or active) layer, in that tower's slots.
pub fn sort_photos(app: &mut RustGisApp) -> Result<(), String> {
    let layer = app.inspection.layer.or(app.session.active_layer);
    let r = app.run("geoflux.sortPhotos", json!({"layer": layer}))?;
    // The photo table shows the new tower and metres columns.
    if let Some(photos) = r["photoLayer"].as_u64().map(|v| v as u32) {
        app.session.table_layer = Some(photos);
    }
    let st = &mut app.inspection;
    st.layer = r["towerLayer"].as_u64().map(|v| v as u32).or(st.layer);
    st.cache.clear();
    st.photos = None;
    st.message = Some((
        crate::i18n::fmt(
            "{n} photos sorted onto {towers} towers; the farthest was {m} m from its tower",
            &[("n", &r["sorted"].to_string()), ("towers", &r["towers"].to_string()), ("m", &r["farthestMetres"].to_string())],
        ),
        false,
    ));
    app.ui.right = Some(crate::RightPane::Inspection);
    Ok(())
}

/// GeoFlux › Make Tower Folders: picks where the folders go, then copies each sorted photo into
/// `<that folder>/<tower>/C1/` on a worker thread, with progress and cancel in the Inspection pane.
pub fn tower_folders(app: &mut RustGisApp, folder: Option<String>) -> Result<(), String> {
    if app.inspection.copy.is_some() {
        return Err(tl!("Tower folders are already being made.").into());
    }
    #[cfg(not(target_arch = "wasm32"))]
    let folder = folder
        .or_else(|| rfd::FileDialog::new().set_title(tl!("Where the tower folders go")).pick_folder().map(|d| d.display().to_string()));
    let Some(folder) = folder else { return Ok(()) };
    let plan = rustgis_engine::cmd::photo_sort::plan_folders(&app.session, &Value::Null, &folder)?;
    let job = CopyJob {
        folder,
        plan: std::sync::Arc::new(plan),
        progress: rustgis_engine::geoflux_report::Progress::new(),
        result: Slot::default(),
    };
    start_copy(&job)?;
    app.inspection.copy = Some(job);
    app.inspection.message = None;
    app.ui.right = Some(crate::RightPane::Inspection);
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn start_copy(job: &CopyJob) -> Result<(), String> {
    let (plan, progress, land) = (job.plan.clone(), job.progress.clone(), job.result.clone());
    std::thread::Builder::new()
        .name("geoflux-tower-folders".into())
        .spawn(move || {
            let r =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rustgis_engine::cmd::photo_sort::copy_folders(&plan, &progress)))
                    .unwrap_or_else(|_| Err(tl!("Copying the photos failed unexpectedly.").to_string()));
            *land.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(target_arch = "wasm32")]
fn start_copy(_: &CopyJob) -> Result<(), String> {
    Err(tl!("Making tower folders needs the desktop app.").into())
}

/// The copy's progress bar and Cancel, at the top of the Inspection pane.
fn copy_progress(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    let Some(job) = &app.inspection.copy else { return };
    let (done, total) = job.progress.get();
    ui.label(RichText::new(crate::i18n::fmt("Copying into {name}", &[("name", &job.folder)])).font(semibold(12.5)));
    let frac = if total == 0 { 0.0 } else { done as f32 / total as f32 };
    ui.add(
        egui::ProgressBar::new(frac)
            .text(crate::i18n::fmt("{done} of {total} photos", &[("done", &done.to_string()), ("total", &total.to_string())])),
    );
    if job.progress.is_cancelled() {
        ui.label(RichText::new(tl!("Stopping…")).small().color(t.text_dim));
    } else if ui.small_button(tl!("Cancel")).clicked() {
        job.progress.cancel();
    }
    ui.separator();
}

/// Says what the copy did when it lands (called every frame).
pub fn poll_copy(app: &mut RustGisApp, ctx: &egui::Context) {
    let Some(job) = &app.inspection.copy else { return };
    let done = job.result.lock().unwrap_or_else(|e| e.into_inner()).take();
    let Some(result) = done else {
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
        return;
    };
    let plan = job.plan.clone();
    app.inspection.copy = None;
    let note = match result {
        Ok(r) => {
            let v = rustgis_engine::cmd::photo_sort::folder_result(&plan, &r);
            let mut text = crate::i18n::fmt(
                "{copied} photos copied into {towers} tower folders",
                &[("copied", &v["copied"].to_string()), ("towers", &v["towers"].to_string())],
            );
            if r.already > 0 {
                text.push_str(&crate::i18n::fmt("; {n} were already there", &[("n", &r.already.to_string())]));
            }
            if r.clashes + r.failed > 0 {
                text.push_str(&crate::i18n::fmt("; {n} not copied: ", &[("n", &(r.clashes + r.failed).to_string())]));
                text.push_str(&r.details.join("; "));
                app.session.notify(text.clone(), true);
            }
            (text, r.clashes + r.failed > 0)
        }
        Err(e) if e == "cancelled" => {
            (tl!("Copy stopped; the photos already copied stay, and copying again carries on.").to_string(), false)
        }
        Err(e) => {
            app.session.notify(e.clone(), true);
            (e, true)
        }
    };
    app.inspection.message = Some(note);
    app.ui.right = Some(crate::RightPane::Inspection);
}

/// GeoFlux › Link Photos: picks a folder and links it to the inspected (or active) layer.
pub fn link_photos(app: &mut RustGisApp) -> Result<(), String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(dir) = rfd::FileDialog::new().set_title(tl!("Folder of tower photos")).pick_folder() else { return Ok(()) };
        let layer = app.inspection.layer.or(app.session.active_layer);
        let r = app.run("geoflux.linkPhotos", json!({"layer": layer, "folder": dir.display().to_string()}))?;
        let st = &mut app.inspection;
        st.folder = dir.display().to_string();
        st.cache.clear();
        st.message = Some((
            crate::i18n::fmt(
                "{linked} photos linked to {towers} towers",
                &[("linked", &r["linked"].to_string()), ("towers", &r["towersWithPhotos"].to_string())],
            ),
            false,
        ));
        app.ui.right = Some(crate::RightPane::Inspection);
        Ok(())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = app;
        Err(tl!("Linking photo folders needs the desktop app.").into())
    }
}
