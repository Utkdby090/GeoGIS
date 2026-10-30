//! GeoFlux › the Thermal view: the tower's thermal photo as temperatures, inside the Inspection
//! pane, with a palette, a spot and a box to measure, a healthy component as the reference, and
//! the finding written to the form. Scan Line measures every tower of the layer.
//!
//! Decoding happens off the frame, one photo at a time: a DJI photo goes to the `geoflux-dirp`
//! helper, which takes a moment and may fail without harming the app. Only the photo on screen
//! is kept (its temperatures, about 1.3 MB for 640×512, and one texture), and the helper is
//! stopped when the pane closes.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use serde_json::{Value, json};

use crate::theme::{Tokens, semibold};
use crate::{RustGisApp, Slot};
use rustgis_engine::cmd::geoflux as gf;
use rustgis_engine::cmd::thermal as th;
use rustgis_engine::geoflux_thermal::{
    Decoders, FormBinding, Palette, Pixel, Region, SceneOverrides, TempRange, ThermalFinding, ThermalFrame, ThermalStats,
};

/// Steps in the colour bar under the photo.
const BAR_STEPS: usize = 48;

/// What a click or drag on the photo does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    /// Read one pixel.
    #[default]
    Spot,
    /// Box the component to measure (its hottest pixel is the finding).
    Box,
    /// Box a healthy, similar component to compare with (instead of the ambient temperature).
    Reference,
}

impl Tool {
    pub fn code(self) -> &'static str {
        match self {
            Tool::Spot => "spot",
            Tool::Box => "box",
            Tool::Reference => "reference",
        }
    }

    fn from_code(s: &str) -> Option<Self> {
        [Tool::Spot, Tool::Box, Tool::Reference].into_iter().find(|t| t.code() == s)
    }
}

pub fn palette_code(p: Palette) -> &'static str {
    match p {
        Palette::Iron => "iron",
        Palette::Rainbow => "rainbow",
        Palette::WhiteHot => "whiteHot",
        Palette::BlackHot => "blackHot",
    }
}

pub fn palette_label(p: Palette) -> &'static str {
    match p {
        Palette::Iron => tl!("Iron"),
        Palette::Rainbow => tl!("Rainbow"),
        Palette::WhiteHot => tl!("White Hot"),
        Palette::BlackHot => tl!("Black Hot"),
    }
}

/// Which photo a frame is of, and the settings it was measured with. A change to either (another
/// tower, another photo, an emissivity typed in the form) measures again.
#[derive(Clone, Debug, PartialEq)]
struct FrameKey {
    layer: u32,
    row: usize,
    path: String,
    overrides: SceneOverrides,
}

impl FrameKey {
    fn same_photo(&self, other: &FrameKey) -> bool {
        self.layer == other.layer && self.row == other.row && self.path == other.path
    }
}

type Decoded = Result<ThermalFrame, String>;

/// The frame on screen, the texture it was drawn into (with the palette and range used) and its
/// readings for the current marks.
struct Shown {
    key: FrameKey,
    frame: Decoded,
    texture: Option<(egui::TextureHandle, Palette, TempRange)>,
    readings: Option<Readings>,
}

/// What the readout shows, worked out once per change of marks or range rather than on every
/// repaint (each is a pass over the whole frame, and the pane repaints as the mouse moves).
struct Readings {
    marks: (Option<Region>, Option<Region>, TempRange),
    hot: Option<ThermalStats>,
    finding: Result<ThermalFinding, String>,
    bounds: (f32, f32),
}

impl Readings {
    fn of(frame: &ThermalFrame, region: Option<Region>, reference: Option<Region>, range: TempRange) -> Self {
        Self {
            marks: (region, reference, range),
            hot: frame.stats(region),
            finding: th::finding(frame, region, reference),
            bounds: range.bounds(frame),
        }
    }
}

/// A Scan Line in progress: photos measured so far, a cancel flag the worker checks between
/// photos, and the results once it ends.
struct ScanJob {
    layer: u32,
    total: usize,
    done: Arc<AtomicUsize>,
    cancel: Arc<AtomicBool>,
    result: Slot<Vec<Value>>,
}

/// State of the Thermal view (part of the Inspection pane's state).
#[derive(Default)]
pub struct ThermalState {
    pub on: bool,
    pub palette: Palette,
    pub range: TempRange,
    pub tool: Tool,
    shown: Option<Shown>,
    decoding: Option<(FrameKey, Slot<Decoded>)>,
    /// Decode threads still running, including ones a release forgot: a new one starts only when
    /// none is, so closing and reopening the pane can't pile threads up behind the DJI helper.
    in_flight: Arc<AtomicUsize>,
    /// The newest photo asked for while another was decoding; it starts when that one lands.
    wanted: Option<FrameKey>,
    spot: Option<Pixel>,
    region: Option<Region>,
    reference: Option<Region>,
    drag_from: Option<Pixel>,
    /// The RGBA pixels of the last drawing, reused for the next.
    rgba: Vec<u8>,
    scan: Option<ScanJob>,
    message: Option<(String, bool)>,
    /// Whether anything was decoded since the pane opened (so closing it stops the helper).
    used: bool,
}

impl ThermalState {
    /// Frees the frame and the helper when the view is switched off, unless a scan is running.
    pub fn release_idle(&mut self, thermal: &th::Thermal) {
        if self.scan.is_none() {
            self.release(thermal);
        }
    }

    /// Frees the frame and texture, cancels a scan and stops the DJI helper (the pane closed).
    /// Called on every frame the pane is closed, so it does nothing when there's nothing to free.
    pub fn release(&mut self, thermal: &th::Thermal) {
        if let Some(job) = self.scan.take() {
            job.cancel.store(true, Ordering::Release);
            self.message = Some((tl!("Scan Line cancelled: the pane was closed.").into(), true));
        }
        if self.shown.is_some() || self.decoding.is_some() || self.wanted.is_some() || !self.rgba.is_empty() {
            self.shown = None;
            self.decoding = None;
            self.wanted = None;
            self.rgba = Vec::new();
        }
        if self.used {
            self.used = false;
            thermal.stop_helper();
        }
    }

    fn clear_marks(&mut self) {
        self.spot = None;
        self.region = None;
        self.reference = None;
        self.drag_from = None;
    }

    /// The frame on screen, if it's of exactly `key` (photo and settings) and decoded.
    fn frame_for(&self, key: &FrameKey) -> Option<&ThermalFrame> {
        self.shown.as_ref().filter(|s| s.key == *key).and_then(|s| s.frame.as_ref().ok())
    }

    /// Asks for the frame of `key`: nothing when it's on screen or decoding, otherwise it starts
    /// now or (while another decodes) as soon as that one lands.
    fn want(&mut self, key: FrameKey, decoders: &Arc<Decoders>, ctx: &egui::Context) {
        if self.shown.as_ref().is_some_and(|s| s.key == key) || self.decoding.as_ref().is_some_and(|(k, _)| *k == key) {
            self.wanted = None;
            return;
        }
        if self.decoding.is_some() || self.in_flight.load(Ordering::Acquire) > 0 {
            self.wanted = Some(key);
            return;
        }
        self.wanted = None;
        self.used = true;
        let slot = start_decode(decoders.clone(), &key, ctx, &self.in_flight);
        self.decoding = Some((key, slot));
    }

    /// Takes a finished decode, if any, and starts the one waiting.
    fn poll(&mut self, decoders: &Arc<Decoders>, ctx: &egui::Context) {
        let done = self.decoding.as_ref().and_then(|(_, slot)| slot.lock().unwrap_or_else(|e| e.into_inner()).take());
        if let Some(frame) = done
            && let Some((key, _)) = self.decoding.take()
        {
            if !self.shown.as_ref().is_some_and(|s| s.key.same_photo(&key)) {
                self.clear_marks();
            }
            self.shown = Some(Shown { key, frame, texture: None, readings: None });
        }
        if self.decoding.is_none()
            && let Some(next) = self.wanted.take()
        {
            self.want(next, decoders, ctx);
        }
    }
}

/// Decodes one photo on its own thread; a panic in a decoder, or a failure to start the thread,
/// becomes an error for that photo.
#[cfg(not(target_arch = "wasm32"))]
fn start_decode(decoders: Arc<Decoders>, key: &FrameKey, ctx: &egui::Context, in_flight: &Arc<AtomicUsize>) -> Slot<Decoded> {
    let slot: Slot<Decoded> = Slot::default();
    let land = slot.clone();
    let (path, overrides, ctx, count) = (key.path.clone(), key.overrides, ctx.clone(), in_flight.clone());
    in_flight.fetch_add(1, Ordering::AcqRel);
    let spawned = std::thread::Builder::new().name("geoflux-thermal".into()).spawn(move || {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            th::decode_file(&decoders, &path, &overrides).map_err(|e| e.to_string())
        }))
        .unwrap_or_else(|_| Err(tl!("This photo could not be read.").to_string()));
        *land.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
        // After the result, so the repaint that follows sees the thread gone and can start the next.
        count.fetch_sub(1, Ordering::AcqRel);
        ctx.request_repaint();
    });
    if let Err(e) = spawned {
        in_flight.fetch_sub(1, Ordering::AcqRel);
        *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(Err(e.to_string()));
    }
    slot
}

#[cfg(target_arch = "wasm32")]
fn start_decode(_: Arc<Decoders>, _: &FrameKey, _: &egui::Context, _: &Arc<AtomicUsize>) -> Slot<Decoded> {
    Arc::new(std::sync::Mutex::new(Some(Err(tl!("Thermal photos show in the desktop app.").to_string()))))
}

/// The thermal photo shown for a tower: the one picked in its thermal slot.
fn thermal_path(app: &RustGisApp, layer: u32, row: usize) -> Option<String> {
    app.inspection.thermal_slot_photo(layer, row).or_else(|| th::thermal_photos(&app.session, layer, row).ok()?.into_iter().next())
}

/// The Thermal section of the Inspection pane, under the photos.
pub fn show(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens, layer: u32, row: usize) {
    let ctx = ui.ctx().clone();
    let decoders = app.session.thermal.decoders.clone();
    app.inspection.thermal.poll(&decoders, &ctx);

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(tl!("Thermal")).font(semibold(12.5)));
        let st = &mut app.inspection.thermal;
        egui::ComboBox::from_id_salt("geoflux_palette").selected_text(palette_label(st.palette)).width(96.0).show_ui(ui, |ui| {
            for p in Palette::ALL {
                ui.selectable_value(&mut st.palette, p, palette_label(p));
            }
        });
    });
    scan_ui(app, ui, t);
    if let Some((m, err)) = &app.inspection.thermal.message {
        ui.label(RichText::new(m.as_str()).small().color(if *err { t.red } else { t.text_dim }));
    }

    let Some(path) = thermal_path(app, layer, row) else {
        ui.label(RichText::new(tl!("This tower has no thermal photo linked.")).small().weak());
        return;
    };
    // The form's values as typed (saved or not) set the scene, so the reading follows them.
    let overrides = app.inspection.form_overrides();
    let key = FrameKey { layer, row, path, overrides };
    app.inspection.thermal.want(key, &decoders, &ctx);

    let st = &mut app.inspection.thermal;
    let busy = st.decoding.is_some();
    let Some(shown) = st.shown.as_mut().filter(|s| s.key.layer == layer && s.key.row == row) else {
        ui.add(egui::Spinner::new());
        return;
    };
    let frame = match &shown.frame {
        Ok(f) => f,
        Err(e) => {
            ui.label(RichText::new(e.as_str()).small().color(t.red)).on_hover_text(shown.key.path.as_str());
            return;
        }
    };
    // Redraw only when the frame, palette or range changed.
    if shown.texture.as_ref().is_none_or(|(_, p, r)| *p != st.palette || *r != st.range) {
        st.palette.render_into(frame, st.range, &mut st.rgba);
        let size = [frame.width() as usize, frame.height() as usize];
        let img = egui::ColorImage::from_rgba_unmultiplied(size, &st.rgba);
        let name = format!("geoflux-thermal:{}", shown.key.path);
        match &mut shown.texture {
            Some((tex, p, r)) => {
                tex.set(img, egui::TextureOptions::NEAREST);
                (*p, *r) = (st.palette, st.range);
            }
            None => shown.texture = Some((ctx.load_texture(name, img, egui::TextureOptions::NEAREST), st.palette, st.range)),
        }
    }
    let Some((tex, _, _)) = &shown.texture else { return };
    let (w, h) = (frame.width() as f32, frame.height() as f32);
    let room = ui.available_width();
    let size = vec2(room, (room * h / w.max(1.0)).min(room * 1.5));
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    if busy {
        painter.text(rect.right_top() + vec2(-6.0, 6.0), egui::Align2::RIGHT_TOP, "…", egui::FontId::proportional(14.0), Color32::WHITE);
    }

    let to_pixel = |p: Pos2| -> Pixel {
        let fx = ((p.x - rect.min.x) / rect.width() * w).floor().clamp(0.0, (w - 1.0).max(0.0));
        let fy = ((p.y - rect.min.y) / rect.height() * h).floor().clamp(0.0, (h - 1.0).max(0.0));
        Pixel { x: fx as u32, y: fy as u32 }
    };
    let to_screen = |px: Pixel, centre: bool| -> Pos2 {
        let o = if centre { 0.5 } else { 0.0 };
        pos2(rect.min.x + (px.x as f32 + o) / w * rect.width(), rect.min.y + (px.y as f32 + o) / h * rect.height())
    };
    let region_rect = |r: Region| -> Rect {
        let (x0, x1) = (r.a.x.min(r.b.x), r.a.x.max(r.b.x));
        let (y0, y1) = (r.a.y.min(r.b.y), r.a.y.max(r.b.y));
        Rect::from_min_max(to_screen(Pixel { x: x0, y: y0 }, false), to_screen(Pixel { x: x1 + 1, y: y1 + 1 }, false))
    };

    // Clicks and drags, by tool.
    let hover = resp.hover_pos().map(to_pixel);
    match st.tool {
        Tool::Spot => {
            if resp.clicked()
                && let Some(p) = resp.interact_pointer_pos()
            {
                st.spot = Some(to_pixel(p));
            }
        }
        Tool::Box | Tool::Reference => {
            if resp.drag_started()
                && let Some(p) = resp.interact_pointer_pos()
            {
                st.drag_from = Some(to_pixel(p));
            }
            let current = st.drag_from.zip(resp.interact_pointer_pos().map(to_pixel)).map(|(a, b)| Region { a, b });
            if resp.drag_stopped() {
                st.drag_from = None;
                if let Some(r) = current {
                    if st.tool == Tool::Box {
                        st.region = Some(r);
                    } else {
                        st.reference = Some(r);
                    }
                }
            } else if let Some(r) = current {
                painter.rect_stroke(region_rect(r), 0.0, Stroke::new(1.5, Color32::from_white_alpha(200)), egui::StrokeKind::Inside);
            }
        }
    }

    let outline = |r: Rect, c: Color32| {
        painter.rect_stroke(r, 0.0, Stroke::new(3.0, Color32::from_black_alpha(160)), egui::StrokeKind::Inside);
        painter.rect_stroke(r, 0.0, Stroke::new(1.5, c), egui::StrokeKind::Inside);
    };
    let cyan = Color32::from_rgb(0x3C, 0xD2, 0xF0);
    if let Some(r) = st.region {
        outline(region_rect(r), Color32::WHITE);
    }
    if let Some(r) = st.reference {
        outline(region_rect(r), cyan);
    }
    let mark = |p: Pos2, c: Color32| {
        for (wd, col) in [(3.0, Color32::from_black_alpha(180)), (1.5, c)] {
            painter.line_segment([p - vec2(6.0, 0.0), p + vec2(6.0, 0.0)], Stroke::new(wd, col));
            painter.line_segment([p - vec2(0.0, 6.0), p + vec2(0.0, 6.0)], Stroke::new(wd, col));
        }
    };
    let marks = (st.region, st.reference, st.range);
    if shown.readings.as_ref().is_none_or(|r| r.marks != marks) {
        shown.readings = Some(Readings::of(frame, st.region, st.reference, st.range));
    }
    let Some(readings) = &shown.readings else { return };
    let hot = readings.hot;
    if let Some(s) = hot {
        mark(to_screen(s.max_at, true), Color32::from_rgb(0xFF, 0x40, 0x40));
    }
    if let Some(p) = st.spot {
        mark(to_screen(p, true), Color32::WHITE);
    }

    // The colour bar, with the range's ends.
    let (lo, hi) = readings.bounds;
    let (bar, _) = ui.allocate_exact_size(vec2(room, 10.0), Sense::hover());
    let step = bar.width() / BAR_STEPS as f32;
    for i in 0..BAR_STEPS {
        let [r, g, b] = st.palette.rgb((i as f32 + 0.5) / BAR_STEPS as f32);
        let cell = Rect::from_min_size(pos2(bar.min.x + i as f32 * step, bar.min.y), vec2(step + 0.5, bar.height()));
        ui.painter().rect_filled(cell, 0.0, Color32::from_rgb(r, g, b));
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{lo:.1} °C")).small().color(t.text_dim));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(format!("{hi:.1} °C")).small().color(t.text_dim));
        });
    });

    // Readings.
    let reading = |p: Pixel| frame.temperature_at(p).filter(|v| v.is_finite()).map_or_else(|| "—".to_string(), |v| format!("{v:.1} °C"));
    if let Some(p) = hover {
        ui.label(RichText::new(format!("{}, {}: {}", p.x, p.y, reading(p))).small().color(t.text_dim));
    }
    if let Some(p) = st.spot {
        ui.label(RichText::new(crate::i18n::fmt("Spot {t}", &[("t", &reading(p))])).small());
    }
    if let Some(s) = hot {
        let what = if st.region.is_some() { tl!("Box") } else { tl!("Whole photo") };
        let text = crate::i18n::fmt(
            "{what}: max {max} °C, mean {mean} °C, min {min} °C",
            &[
                ("what", what),
                ("max", &format!("{:.1}", s.max_c)),
                ("mean", &format!("{:.1}", s.mean_c)),
                ("min", &format!("{:.1}", s.min_c)),
            ],
        );
        ui.label(RichText::new(text).small());
    }
    match &readings.finding {
        Ok(f) => {
            let reference = match f.reference {
                Some(r) if st.reference.is_some() => crate::i18n::fmt("{t} °C (reference box)", &[("t", &format!("{:.1}", r.temp_c()))]),
                Some(r) => crate::i18n::fmt("{t} °C (ambient)", &[("t", &format!("{:.1}", r.temp_c()))]),
                None => "—".into(),
            };
            let rise = f.rise_c().map_or_else(|| "—".into(), |r| format!("{r:.1} °C"));
            let text = crate::i18n::fmt("Reference {reference} · Rise {rise}", &[("reference", &reference), ("rise", &rise)]);
            ui.label(RichText::new(text).small().font(semibold(11.5)));
        }
        Err(e) => {
            ui.label(RichText::new(e.as_str()).small().color(t.amber));
        }
    }
    if !frame.refused().is_empty() {
        let names: Vec<String> = frame.refused().iter().map(ToString::to_string).collect();
        let text =
            crate::i18n::fmt("The camera's library refused the form's {names}; its own values are used.", &[("names", &names.join(", "))]);
        ui.label(RichText::new(text).small().color(t.amber));
    }

    // Tools and actions.
    ui.horizontal_wrapped(|ui| {
        for tool in [Tool::Spot, Tool::Box, Tool::Reference] {
            let label = match tool {
                Tool::Spot => tl!("Spot"),
                Tool::Box => tl!("Box"),
                Tool::Reference => tl!("Reference"),
            };
            ui.selectable_value(&mut st.tool, tool, label);
        }
        if ui.small_button(tl!("Clear Marks")).clicked() {
            st.clear_marks();
        }
        let mut fixed = matches!(st.range, TempRange::Fixed { .. });
        if ui.checkbox(&mut fixed, tl!("Fixed range")).on_hover_text(tl!("Keep one scale while walking the line.")).changed() {
            st.range = if fixed { TempRange::Fixed { lo, hi } } else { TempRange::Auto };
        }
        if let TempRange::Fixed { lo, hi } = &mut st.range {
            ui.add(egui::DragValue::new(lo).speed(0.5).suffix(" °C"));
            ui.add(egui::DragValue::new(hi).speed(0.5).suffix(" °C"));
        }
    });
    ui.horizontal(|ui| {
        if ui.button(tl!("Record Finding")).on_hover_text(tl!("Write the fault and rise temperatures into this tower's form.")).clicked()
            && let Err(e) = record(app)
        {
            app.inspection.thermal.message = Some((e, true));
        }
    });
}

/// Scan Line's progress and Cancel, or nothing.
fn scan_ui(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    let Some(job) = &app.inspection.thermal.scan else { return };
    let done = job.done.load(Ordering::Acquire);
    let total = job.total.max(1);
    ui.horizontal(|ui| {
        let text = crate::i18n::fmt("Scanning {n} of {total}", &[("n", &done.to_string()), ("total", &job.total.to_string())]);
        ui.add(
            egui::ProgressBar::new(done as f32 / total as f32).desired_width(ui.available_width() - 70.0).text(RichText::new(text).small()),
        );
        if ui.small_button(tl!("Cancel")).clicked() {
            job.cancel.store(true, Ordering::Release);
        }
    });
    if job.cancel.load(Ordering::Acquire) {
        ui.label(RichText::new(tl!("Stopping after this photo…")).small().color(t.text_dim));
    }
}

/// Records a finished scan's results (unless it was cancelled). Called on every frame the
/// Inspection pane is open, whatever it shows.
pub fn poll_scan(app: &mut RustGisApp) {
    let finished = app.inspection.thermal.scan.as_ref().and_then(|j| j.result.lock().unwrap_or_else(|e| e.into_inner()).take());
    let Some(results) = finished else { return };
    let Some(job) = app.inspection.thermal.scan.take() else { return };
    if job.cancel.load(Ordering::Acquire) {
        app.inspection.thermal.message = Some((tl!("Scan Line cancelled; nothing was recorded.").into(), false));
        return;
    }
    let message = match app.run("geoflux.thermal.scan", json!({"layer": job.layer, "results": results})) {
        // With findings, the scan suggests hot spots for review instead of writing the forms.
        Ok(r) if r.get("proposals").is_some() => {
            let p = &r["proposals"];
            let n = |k: &str| p[k].as_u64().unwrap_or(0).to_string();
            app.inspection.forget_marks();
            (
                crate::i18n::fmt(
                    "{proposed} hot spots suggested for review; {already} photos already looked at; {failed} could not be measured",
                    &[("proposed", &n("proposed")), ("already", &n("already")), ("failed", &n("failed"))],
                ),
                p["failed"].as_u64().is_some_and(|n| n > 0),
            )
        }
        Ok(r) => (
            crate::i18n::fmt(
                "Recorded {recorded} towers; {failed} photos could not be measured",
                &[("recorded", &r["recorded"].to_string()), ("failed", &r["failed"].to_string())],
            ),
            r["failed"].as_u64().is_some_and(|n| n > 0),
        ),
        Err(e) => (e, true),
    };
    app.inspection.thermal.message = Some(message);
}

/// Thermal › Record Finding: the hottest pixel in the box (or the photo) and its rise over the
/// reference go into the form. A clean form is saved as one undoable edit; one with unsaved
/// changes gets the values typed in, to be saved with the rest.
pub fn record(app: &mut RustGisApp) -> Result<(), String> {
    let (Some(layer), Some(row)) = (app.inspection.layer, app.inspection.row) else {
        return Err(tl!("Open a tower in the Inspection pane first.").into());
    };
    let wait = tl!("Wait for this tower's thermal photo to be measured.");
    let path = thermal_path(app, layer, row).ok_or(tl!("This tower has no thermal photo linked."))?;
    let key = FrameKey { layer, row, path, overrides: app.inspection.form_overrides() };
    let st = &app.inspection.thermal;
    let frame = st.frame_for(&key).ok_or(wait)?;
    let f = th::finding(frame, st.region, st.reference)?;
    let findings_mode = app.session.project.geoflux.findings_of(layer).is_some();
    if findings_mode && app.inspection.dirty() {
        // The finding fills the form's fault fields, which unsaved typing would then overwrite.
        return Err(tl!("Save or revert this tower's changes first.").into());
    }
    if app.inspection.dirty() {
        app.inspection.enter_values(f.form_values(&FormBinding::default()));
        app.inspection.thermal.message = Some((tl!("Fault and rise temperatures entered. Save to keep them.").into(), false));
        return Ok(());
    }
    let found = json!({"maxC": f.max_c, "referenceC": f.reference.map(|r| r.temp_c())});
    let by = if findings_mode { crate::findingsview::who(app) } else { String::new() };
    app.run("geoflux.thermal.record", json!({"layer": layer, "row": row, "found": found, "by": by}))?;
    app.inspection.forget_marks();
    app.inspection.thermal.message = Some((tl!("Finding recorded.").into(), false));
    Ok(())
}

/// Thermal › Scan Line: measures the first thermal photo of every tower on the layer, off the
/// frame, then records them all as one undoable edit.
pub fn scan(app: &mut RustGisApp) -> Result<(), String> {
    if app.inspection.thermal.scan.is_some() {
        return Err(tl!("A scan is already running.").into());
    }
    let layer = app
        .inspection
        .layer
        .or(app.session.active_layer.filter(|l| app.session.project.geoflux.links.iter().any(|k| k.layer == *l)))
        .ok_or(tl!("Import a survey CSV and link its photos first."))?;
    if app.inspection.dirty() {
        return Err(tl!("Save or revert this tower's changes first.").into());
    }
    let mut jobs = Vec::new();
    for stop in gf::sequence(&app.session, layer, false)? {
        let Some(path) = th::thermal_photos(&app.session, layer, stop.row)?.into_iter().next() else { continue };
        jobs.push((stop.row, path, th::form_overrides(&app.session, layer, stop.row)?));
    }
    if jobs.is_empty() {
        return Err(tl!("No tower on this layer has a thermal photo linked.").into());
    }
    app.ui.right = Some(crate::RightPane::Inspection);
    let st = &mut app.inspection;
    st.layer = Some(layer);
    st.thermal.on = true;
    st.thermal.used = true;
    st.thermal.message = None;
    let job = ScanJob { layer, total: jobs.len(), done: Arc::default(), cancel: Arc::default(), result: Slot::default() };
    start_scan(app.session.thermal.decoders.clone(), jobs, &job)?;
    app.inspection.thermal.scan = Some(job);
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn start_scan(decoders: Arc<Decoders>, jobs: Vec<(usize, String, SceneOverrides)>, job: &ScanJob) -> Result<(), String> {
    let (done, cancel, land) = (job.done.clone(), job.cancel.clone(), job.result.clone());
    std::thread::Builder::new()
        .name("geoflux-thermal-scan".into())
        .spawn(move || {
            let mut out = Vec::with_capacity(jobs.len());
            for (row, path, overrides) in jobs {
                if cancel.load(Ordering::Acquire) {
                    break;
                }
                let found = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    th::decode_file(&decoders, &path, &overrides).map_err(|e| e.to_string()).and_then(|f| th::finding(&f, None, None))
                }))
                .unwrap_or_else(|_| Err(tl!("This photo could not be read.").to_string()));
                out.push(match found {
                    Ok(f) => json!({"row": row, "maxC": f.max_c, "referenceC": f.reference.map(|r| r.temp_c())}),
                    Err(e) => json!({"row": row, "error": e}),
                });
                done.fetch_add(1, Ordering::AcqRel);
            }
            *land.lock().unwrap_or_else(|e| e.into_inner()) = Some(out);
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(target_arch = "wasm32")]
fn start_scan(_: Arc<Decoders>, _: Vec<(usize, String, SceneOverrides)>, _: &ScanJob) -> Result<(), String> {
    Err(tl!("Thermal photos show in the desktop app.").into())
}

/// The `ui.thermal.*` commands from the ribbon. Each opens the Inspection pane with the view on.
pub fn command(app: &mut RustGisApp, id: &str, p: &Value) -> Result<(), String> {
    let open = |app: &mut RustGisApp| {
        app.ui.right = Some(crate::RightPane::Inspection);
        app.inspection.thermal.on = true;
    };
    match id {
        "ui.thermal.view" => {
            if app.inspection.thermal.on && app.ui.right == Some(crate::RightPane::Inspection) {
                app.inspection.thermal.on = false;
            } else {
                open(app);
            }
        }
        "ui.thermal.palette" => {
            let code = p.get("palette").and_then(Value::as_str).ok_or("palette is required")?;
            app.inspection.thermal.palette = Palette::ALL.into_iter().find(|x| palette_code(*x) == code).ok_or("no such palette")?;
            open(app);
        }
        "ui.thermal.tool" => {
            let code = p.get("tool").and_then(Value::as_str).ok_or("tool is required")?;
            app.inspection.thermal.tool = Tool::from_code(code).ok_or("no such tool")?;
            open(app);
        }
        "ui.thermal.record" => {
            open(app);
            record(app)?;
        }
        "ui.thermal.scan" => scan(app)?,
        _ => return Err(format!("unknown command {id}")),
    }
    Ok(())
}
