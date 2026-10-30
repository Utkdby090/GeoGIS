//! GeoFlux › photo markup and inspection reports in the app.
//!
//! - The markup editor: a window over one photo with pen, arrow, box, ellipse, text and numbered
//!   marker tools, undo and redo. Shapes are kept as fractions of the photo (see
//!   `geoflux_domain::annotation`), saved with `geoflux.annotate`, and drawn the same way on the
//!   slot thumbnails and in the PDF.
//! - Reports: one tower's PDF is written on the spot (`geoflux.report`); a ZIP of the whole line is
//!   planned on the session and written on a worker thread with a progress bar and cancel.

use std::sync::Arc;

use egui::{Align2, Color32, FontId, Pos2, Rect, RichText, Stroke, Ui, pos2, vec2};
use serde_json::json;

use crate::theme::Tokens;
use crate::{RustGisApp, Slot};
use rustgis_engine::cmd::geoflux as gf;
use rustgis_engine::cmd::inspection_report as ir;
use rustgis_engine::geoflux_domain::annotation::{Annotation, AnnotationEditor, AnnotationSet, Color, NormPoint, Shape};
use rustgis_engine::geoflux_report::{Progress, Summary};

/// Pen points closer than this (in screen pixels) to the last one are skipped.
const PEN_STEP: f32 = 2.0;
/// The colours offered, GeoFlux red first.
const SWATCHES: [Color; 6] = [
    Color::RED,
    Color([0xFF, 0xD4, 0x00, 0xFF]),
    Color([0x16, 0xA3, 0x4A, 0xFF]),
    Color([0x25, 0x63, 0xEB, 0xFF]),
    Color([0xFF, 0xFF, 0xFF, 0xFF]),
    Color([0x11, 0x11, 0x11, 0xFF]),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Pen,
    Arrow,
    Rect,
    Ellipse,
    Text,
    Marker,
    Erase,
}

impl Tool {
    const ALL: [Tool; 7] = [Tool::Pen, Tool::Arrow, Tool::Rect, Tool::Ellipse, Tool::Text, Tool::Marker, Tool::Erase];

    fn label(self) -> &'static str {
        match self {
            Tool::Pen => tl!("Pen"),
            Tool::Arrow => tl!("Arrow"),
            Tool::Rect => tl!("Box"),
            Tool::Ellipse => tl!("Ellipse"),
            Tool::Text => tl!("Text"),
            Tool::Marker => tl!("Marker"),
            Tool::Erase => tl!("Erase"),
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Tool::Pen => tl!("Draw freehand."),
            Tool::Arrow => tl!("Drag from the note to the fault; the head goes where you let go."),
            Tool::Rect => tl!("Drag a box around the component."),
            Tool::Ellipse => tl!("Drag an ellipse around the component."),
            Tool::Text => tl!("Type the text, then click where it goes."),
            Tool::Marker => tl!("Click to place the next numbered marker."),
            Tool::Erase => tl!("Click a shape to remove it."),
        }
    }
}

/// The markup editor on one photo.
pub struct Editor {
    layer: u32,
    row: usize,
    slot: String,
    photo: String,
    editor: AnnotationEditor,
    tool: Tool,
    color: Color,
    width: f32,
    text: String,
    text_size: f32,
    /// The shape being dragged, in photo fractions.
    drawing: Option<Shape>,
    message: Option<(String, bool)>,
    confirm_close: bool,
}

impl Editor {
    pub fn photo(&self) -> &str {
        &self.photo
    }
}

/// Opens the editor on a photo of a slot, with the markup it already has.
pub fn open(app: &mut RustGisApp, layer: u32, row: usize, slot: &str, photo: &str) {
    let label = gf::asset(&app.session, layer, row).map(|(schema, a)| a.property(schema.label_key()).to_string()).unwrap_or_default();
    let set = ir::markup(&app.session, layer, &label, slot, photo);
    app.inspection.markup = Some(Editor {
        layer,
        row,
        slot: slot.to_string(),
        photo: photo.to_string(),
        editor: AnnotationEditor::new(set),
        tool: Tool::Ellipse,
        color: Color::RED,
        width: 6.0,
        text: String::new(),
        text_size: 40.0,
        drawing: None,
        message: None,
        confirm_close: false,
    });
}

/// Draws markup over a photo shown in `rect` (the photo's own box on screen), clipped to it.
pub fn paint(painter: &egui::Painter, rect: Rect, shapes: &[Annotation]) {
    if shapes.is_empty() || rect.width() < 1.0 || rect.height() < 1.0 {
        return;
    }
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    for a in shapes {
        paint_one(&painter, rect, &a.shape, a.color, a.width);
    }
}

fn to_screen(rect: Rect, p: NormPoint) -> Pos2 {
    pos2(rect.min.x + p.x * rect.width(), rect.min.y + p.y * rect.height())
}

fn to_photo(rect: Rect, p: Pos2) -> NormPoint {
    NormPoint::new(((p.x - rect.min.x) / rect.width()).clamp(0.0, 1.0), ((p.y - rect.min.y) / rect.height()).clamp(0.0, 1.0))
}

fn color32(c: Color) -> Color32 {
    let [r, g, b, a] = c.0;
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// One shape, sized the way the PDF writer sizes it (widths are thousandths of the shorter side).
fn paint_one(painter: &egui::Painter, rect: Rect, shape: &Shape, color: Color, width: f32) {
    let short = rect.width().min(rect.height());
    let lw = (width / 1000.0 * short).max(1.0);
    let col = color32(color);
    let stroke = Stroke::new(lw, col);
    let at = |p: NormPoint| to_screen(rect, p);
    match shape {
        Shape::Pen { points } => {
            if points.len() > 1 {
                painter.add(egui::Shape::line(points.iter().map(|p| at(*p)).collect(), stroke));
            }
        }
        Shape::Arrow { from, to } => {
            let (a, b) = (at(*from), at(*to));
            let d = b - a;
            let len = d.length().max(0.001);
            let u = d / len;
            let head = (lw * 4.5).min(len * 0.6);
            let base = b - u * head;
            painter.line_segment([a, base], stroke);
            let n = vec2(-u.y, u.x) * head * 0.5;
            painter.add(egui::Shape::convex_polygon(vec![b, base + n, base - n], col, Stroke::NONE));
        }
        Shape::Rect { a, b } => {
            painter.rect_stroke(Rect::from_two_pos(at(*a), at(*b)), 0.0, stroke, egui::StrokeKind::Middle);
        }
        Shape::Ellipse { a, b } => {
            let r = Rect::from_two_pos(at(*a), at(*b));
            let (c, rx, ry) = (r.center(), r.width() / 2.0, r.height() / 2.0);
            let pts: Vec<Pos2> =
                (0..64).map(|i| i as f32 / 64.0 * std::f32::consts::TAU).map(|t| c + vec2(rx * t.cos(), ry * t.sin())).collect();
            painter.add(egui::Shape::closed_line(pts, stroke));
        }
        Shape::Text { at: p, text, size } => {
            let cap = size / 1000.0 * short;
            painter.text(at(*p), Align2::LEFT_TOP, text, FontId::proportional((cap / 0.718).max(4.0)), col);
        }
        Shape::Marker { at: p, number } => {
            let r = (lw * 2.2).max(short * 0.045);
            let c = at(*p);
            painter.circle_filled(c, r, col);
            painter.text(c, Align2::CENTER_CENTER, number.to_string(), FontId::proportional(r * 1.15), Color32::WHITE);
        }
    }
}

/// The topmost shape under `p` (photo fractions), for the eraser: a point within a few pixels of
/// its outline's bounding box.
fn hit(shapes: &[Annotation], rect: Rect, p: Pos2) -> Option<usize> {
    let slack: f32 = 6.0;
    shapes.iter().enumerate().rev().find_map(|(i, a)| {
        let pts: Vec<Pos2> = match &a.shape {
            Shape::Pen { points } => points.iter().map(|q| to_screen(rect, *q)).collect(),
            Shape::Arrow { from, to } => vec![to_screen(rect, *from), to_screen(rect, *to)],
            Shape::Rect { a, b } | Shape::Ellipse { a, b } => vec![to_screen(rect, *a), to_screen(rect, *b)],
            Shape::Text { at, text, size } => {
                let s = to_screen(rect, *at);
                let h = size / 1000.0 * rect.width().min(rect.height()) / 0.718;
                vec![s, s + vec2(h * 0.6 * text.chars().count() as f32, h)]
            }
            Shape::Marker { at, .. } => vec![to_screen(rect, *at)],
        };
        let b = Rect::from_points(&pts).expand(slack.max(if matches!(a.shape, Shape::Marker { .. }) { 14.0 } else { 0.0 }));
        b.contains(p).then_some(i)
    })
}

/// The editor window. Needs the photo's full view from the Inspection pane's cache.
pub fn show(app: &mut RustGisApp, ctx: &egui::Context, t: &Tokens, texture: Option<Result<egui::TextureHandle, String>>) {
    let Some(ed) = app.inspection.markup.as_mut() else { return };
    let name = ed.photo.rsplit(['/', '\\']).next().unwrap_or(&ed.photo).to_string();
    let mut open = true;
    let mut save = false;
    let mut close = false;
    egui::Window::new(crate::i18n::fmt("Mark up {photo}", &[("photo", &name)]))
        .id(egui::Id::new("geoflux_markup"))
        .open(&mut open)
        .default_size(vec2(960.0, 720.0))
        .resizable(true)
        .show(ctx, |ui| {
            toolbar(ed, ui, t);
            ui.separator();
            let room = ui.available_size() - vec2(0.0, 34.0);
            match &texture {
                Some(Ok(tex)) => canvas(ed, ui, tex, room),
                Some(Err(e)) => {
                    ui.label(RichText::new(e.as_str()).color(t.red));
                }
                None => {
                    ui.add_sized(room.max(vec2(40.0, 40.0)), egui::Spinner::new());
                }
            }
            ui.horizontal(|ui| {
                let dirty = ed.editor.is_dirty();
                if ui
                    .add_enabled(dirty, egui::Button::new(tl!("Save")))
                    .on_hover_text(tl!("Keep this markup with the photo (the photo file is not changed)."))
                    .clicked()
                {
                    save = true;
                }
                if ed.confirm_close {
                    ui.label(RichText::new(tl!("Discard the changes?")).color(t.amber));
                    if ui.button(tl!("Discard")).clicked() {
                        close = true;
                    }
                    if ui.button(tl!("Keep editing")).clicked() {
                        ed.confirm_close = false;
                    }
                } else if ui.button(if dirty { tl!("Cancel") } else { tl!("Close") }).clicked() {
                    if dirty {
                        ed.confirm_close = true;
                    } else {
                        close = true;
                    }
                }
                if let Some((m, err)) = &ed.message {
                    ui.label(RichText::new(m.as_str()).small().color(if *err { t.red } else { t.text_dim }));
                }
            });
        });
    if !open {
        if ed.editor.is_dirty() {
            ed.confirm_close = true;
        } else {
            close = true;
        }
    }
    if save {
        let (layer, row, slot, photo) = (ed.layer, ed.row, ed.slot.clone(), ed.photo.clone());
        let shapes = serde_json::to_value(ed.editor.set()).unwrap_or_default();
        match app.run("geoflux.annotate", json!({"layer": layer, "row": row, "slot": slot, "photo": photo, "shapes": shapes})) {
            Ok(_) => {
                app.inspection.markup = None;
                app.inspection.forget_marks();
            }
            Err(e) => {
                if let Some(ed) = app.inspection.markup.as_mut() {
                    ed.message = Some((e, true));
                }
            }
        }
    } else if close {
        app.inspection.markup = None;
    }
}

fn toolbar(ed: &mut Editor, ui: &mut Ui, t: &Tokens) {
    ui.horizontal_wrapped(|ui| {
        for tool in Tool::ALL {
            if ui.selectable_label(ed.tool == tool, tool.label()).on_hover_text(tool.hint()).clicked() {
                ed.tool = tool;
            }
        }
        ui.separator();
        for c in SWATCHES {
            let (r, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), egui::Sense::click());
            ui.painter().rect_filled(r.shrink(2.0), 3.0, color32(c));
            let ring = if ed.color == c { Stroke::new(2.0, t.accent) } else { Stroke::new(1.0, t.border) };
            ui.painter().rect_stroke(r, 4.0, ring, egui::StrokeKind::Inside);
            if resp.on_hover_text(c.to_hex()).clicked() {
                ed.color = c;
            }
        }
        ui.separator();
        ui.label(RichText::new(tl!("Width")).small());
        ui.add(egui::Slider::new(&mut ed.width, 1.0..=50.0).integer().show_value(false));
        if ed.tool == Tool::Text {
            ui.add(egui::TextEdit::singleline(&mut ed.text).hint_text(tl!("Text")).char_limit(200).desired_width(160.0));
            ui.label(RichText::new(tl!("Size")).small());
            ui.add(egui::Slider::new(&mut ed.text_size, 10.0..=200.0).integer().show_value(false));
        }
        ui.separator();
        if ui.add_enabled(ed.editor.can_undo(), egui::Button::new(tl!("Undo"))).clicked() {
            ed.editor.undo();
        }
        if ui.add_enabled(ed.editor.can_redo(), egui::Button::new(tl!("Redo"))).clicked() {
            ed.editor.redo();
        }
        if ui.add_enabled(!ed.editor.set().is_empty(), egui::Button::new(tl!("Clear"))).clicked() {
            ed.editor.clear();
        }
    });
}

/// The photo, its markup and the shape being drawn; turns pointer input into shapes.
fn canvas(ed: &mut Editor, ui: &mut Ui, tex: &egui::TextureHandle, room: egui::Vec2) {
    let size = tex.size_vec2();
    let scale = (room.x / size.x).min(room.y / size.y).max(0.01);
    let (rect, resp) = ui.allocate_exact_size(size * scale, egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    paint(&painter, rect, ed.editor.set().shapes());
    let pointer = resp.interact_pointer_pos();
    let cursor = if ed.tool == Tool::Erase { egui::CursorIcon::PointingHand } else { egui::CursorIcon::Crosshair };
    let resp = resp.on_hover_cursor(cursor);

    if resp.drag_started()
        && let Some(p) = pointer.map(|p| to_photo(rect, p))
    {
        ed.drawing = match ed.tool {
            Tool::Pen => Some(Shape::Pen { points: vec![p] }),
            Tool::Arrow => Some(Shape::Arrow { from: p, to: p }),
            Tool::Rect => Some(Shape::Rect { a: p, b: p }),
            Tool::Ellipse => Some(Shape::Ellipse { a: p, b: p }),
            Tool::Text | Tool::Marker | Tool::Erase => None,
        };
    }
    if resp.dragged()
        && let (Some(pos), Some(shape)) = (pointer, ed.drawing.as_mut())
    {
        let p = to_photo(rect, pos);
        match shape {
            Shape::Pen { points } => {
                let far = points.last().is_none_or(|last| to_screen(rect, *last).distance(pos) >= PEN_STEP);
                if far && points.len() < rustgis_engine::geoflux_domain::annotation::MAX_PEN_POINTS {
                    points.push(p);
                }
            }
            Shape::Arrow { to, .. } => *to = p,
            Shape::Rect { b, .. } | Shape::Ellipse { b, .. } => *b = p,
            Shape::Text { .. } | Shape::Marker { .. } => {}
        }
    }
    if let Some(shape) = &ed.drawing {
        paint_one(&painter, rect, shape, ed.color, ed.width);
    }
    if resp.drag_stopped()
        && let Some(shape) = ed.drawing.take()
    {
        add(ed, shape);
    }
    if resp.clicked()
        && let Some(pos) = pointer
    {
        let p = to_photo(rect, pos);
        match ed.tool {
            Tool::Text if ed.text.trim().is_empty() => ed.message = Some((tl!("Type the text first.").into(), true)),
            Tool::Text => add(ed, Shape::Text { at: p, text: ed.text.trim().to_string(), size: ed.text_size }),
            Tool::Marker => add(ed, Shape::Marker { at: p, number: ed.editor.set().next_marker() }),
            Tool::Erase => {
                if let Some(i) = hit(ed.editor.set().shapes(), rect, pos) {
                    let _ = ed.editor.remove(i);
                }
            }
            _ => {}
        }
    }
}

/// Adds a finished shape; one the rules refuse (a click with the box tool, too many shapes) is
/// dropped with a note.
fn add(ed: &mut Editor, shape: Shape) {
    let mut a = Annotation::new(shape);
    a.color = ed.color;
    a.width = ed.width;
    match ed.editor.add(a) {
        Ok(()) => ed.message = None,
        Err(e) => ed.message = Some((e.to_string(), true)),
    }
}

/// A line's reports being written into a ZIP on a worker thread.
pub struct ReportJob {
    path: String,
    reports: usize,
    progress: Arc<Progress>,
    result: Slot<Result<Summary, String>>,
}

/// The Reports row of the Inspection pane: this tower's PDF, the whole line's ZIP, and the
/// progress of a ZIP being written.
pub fn reports_ui(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens, layer: u32, row: usize) {
    ui.label(RichText::new(tl!("Reports")).font(crate::theme::semibold(12.5)));
    if let Some(job) = &app.inspection.reports {
        let (done, total) = job.progress.get();
        let frac = if total == 0 { 0.0 } else { done as f32 / total as f32 };
        ui.add(
            egui::ProgressBar::new(frac)
                .text(crate::i18n::fmt("{done} of {total} reports", &[("done", &done.to_string()), ("total", &total.to_string())])),
        );
        if job.progress.is_cancelled() {
            ui.label(RichText::new(tl!("Stopping…")).small().color(t.text_dim));
        } else if ui.small_button(tl!("Cancel")).clicked() {
            job.progress.cancel();
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
    } else {
        ui.horizontal(|ui| {
            if ui.button(tl!("Tower PDF…")).on_hover_text(tl!("This tower's inspection report, with its photos and markup.")).clicked() {
                match report_one(app, layer, row) {
                    Ok(m) if m.is_empty() => {}
                    Ok(m) => app.inspection.report_note = Some((m, false)),
                    Err(e) => app.inspection.report_note = Some((e, true)),
                }
            }
            if ui.button(tl!("All Towers (ZIP)…")).on_hover_text(tl!("One PDF per tower whose form is complete, in a ZIP.")).clicked()
                && let Err(e) = report_all(app, layer)
            {
                app.inspection.report_note = Some((e, true));
            }
        });
    }
    if let Some((m, err)) = &app.inspection.report_note {
        ui.label(RichText::new(m.as_str()).small().color(if *err { t.red } else { t.text_dim }));
    }
}

/// Writes one tower's PDF where the person picks.
fn report_one(app: &mut RustGisApp, layer: u32, row: usize) -> Result<String, String> {
    let plan = ir::plan(&app.session, layer, Some(&[row]), &serde_json::Value::Null, true)?;
    let name = plan.pages.first().map(|p| p.path.rsplit('/').next().unwrap_or(&p.path).to_string()).unwrap_or_else(|| "Report.pdf".into());
    let Some(path) = crate::dialogs::pick_save(&name, &["pdf"], tl!("PDF report")) else { return Ok(String::new()) };
    let r = app.run("geoflux.report", json!({"layer": layer, "row": row, "path": path}))?;
    Ok(summary_text(&r, &path))
}

/// Plans every ready tower's report and starts writing the ZIP on a worker thread.
pub fn report_all(app: &mut RustGisApp, layer: u32) -> Result<(), String> {
    if app.inspection.reports.is_some() {
        return Err(tl!("Reports are already being written.").into());
    }
    let plan = ir::plan(&app.session, layer, None, &serde_json::Value::Null, false)?;
    if plan.pages.is_empty() {
        return Err(crate::i18n::fmt(
            "No tower is ready for a report: {n} have form issues. Fix them in the form, or use geoflux.reportBulk with \"all\": true.",
            &[("n", &plan.not_ready.len().to_string())],
        ));
    }
    let name = app.session.map().layer(layer).map_or_else(|| "Reports".to_string(), |l| format!("{} Reports", l.name));
    let Some(path) = crate::dialogs::pick_save(&format!("{name}.zip"), &["zip"], tl!("ZIP of PDF reports")) else { return Ok(()) };
    let job = ReportJob { path: path.clone(), reports: plan.pages.len(), progress: Progress::new(), result: Slot::default() };
    job.progress.start(plan.pages.len());
    if !plan.not_ready.is_empty() {
        app.inspection.report_note =
            Some((crate::i18n::fmt("{n} towers with form issues are left out.", &[("n", &plan.not_ready.len().to_string())]), false));
    }
    start(path, plan.pages, &job)?;
    app.inspection.reports = Some(job);
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn start(path: String, pages: Vec<rustgis_engine::geoflux_report::PlannedPage>, job: &ReportJob) -> Result<(), String> {
    let (progress, land) = (job.progress.clone(), job.result.clone());
    std::thread::Builder::new()
        .name("geoflux-reports".into())
        .spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ir::write_bulk_file(&path, pages, &progress)))
                .unwrap_or_else(|_| Err(tl!("Writing the reports failed unexpectedly.").to_string()));
            *land.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(target_arch = "wasm32")]
fn start(_: String, _: Vec<rustgis_engine::geoflux_report::PlannedPage>, _: &ReportJob) -> Result<(), String> {
    Err(tl!("Report ZIPs need the desktop app for now.").into())
}

/// Picks up a finished ZIP (called every frame, so it lands with the Inspection pane closed too).
pub fn poll(app: &mut RustGisApp) {
    let done = app.inspection.reports.as_ref().and_then(|j| j.result.lock().unwrap_or_else(|e| e.into_inner()).take());
    let Some(result) = done else { return };
    let Some(job) = app.inspection.reports.take() else { return };
    let note = match result {
        Ok(summary) => {
            let mut text = crate::i18n::fmt("Wrote {n} reports to {path}", &[("n", &job.reports.to_string()), ("path", &job.path)]);
            if !summary.photos_missing.is_empty() {
                text.push_str(&crate::i18n::fmt("; {n} photos could not be read", &[("n", &summary.photos_missing.len().to_string())]));
            }
            (text, false)
        }
        Err(e) if e == "cancelled" => (tl!("Cancelled; no ZIP was written.").to_string(), false),
        Err(e) => (e, true),
    };
    app.session.notify(note.0.clone(), note.1);
    app.inspection.report_note = Some(note);
}

/// "Wrote x.pdf", with what was left out.
fn summary_text(r: &serde_json::Value, path: &str) -> String {
    let mut text = crate::i18n::fmt("Wrote {path}", &[("path", path)]);
    if let Some(n) = r["photosMissing"].as_u64().filter(|n| *n > 0) {
        text.push_str(&crate::i18n::fmt("; {n} photos could not be read", &[("n", &n.to_string())]));
    }
    if let Some(c) = r["replacedCharacters"].as_str().filter(|c| !c.is_empty()) {
        text.push_str(&crate::i18n::fmt("; shown as ? (the PDF font is Latin only): {chars}", &[("chars", c)]));
    }
    text
}

/// The markup of each of the asset's photos by (slot, photo), with its confirmed findings, for
/// drawing over the thumbnails.
pub fn marks(app: &RustGisApp, layer: u32, row: usize, slots: &[gf::SlotPhotos]) -> Vec<(String, String, AnnotationSet)> {
    let mut out = Vec::new();
    let Ok((schema, asset)) = gf::asset(&app.session, layer, row) else { return out };
    let label = asset.property(schema.label_key()).to_string();
    let g = &app.session.project.geoflux;
    if g.markup.is_empty() && g.findings_of(layer).is_none() {
        return out;
    }
    // Confirmed findings are drawn too, as the report draws them.
    let found = rustgis_engine::cmd::findings::ReportFindings::new(g, layer);
    for sp in slots {
        for photo in &sp.photos {
            let set = found.add_to(ir::markup(&app.session, layer, &label, &sp.slot.id, photo), &label, photo, &schema);
            if !set.is_empty() {
                out.push((sp.slot.id.clone(), photo.clone(), set));
            }
        }
    }
    out
}

/// The ribbon's report commands: this tower's PDF, or the line's ZIP, for the inspected layer.
pub fn command(app: &mut RustGisApp, id: &str) -> Result<(), String> {
    let layer = app
        .inspection
        .layer
        .or(app.session.active_layer)
        .ok_or_else(|| tl!("Open the Inspection pane on a tower layer first.").to_string())?;
    app.ui.right = Some(crate::RightPane::Inspection);
    if id == "ui.geofluxReportBulk" {
        return report_all(app, layer);
    }
    let row = app
        .inspection
        .row
        .filter(|_| app.inspection.layer == Some(layer))
        .ok_or_else(|| tl!("Pick a tower in the Inspection pane first.").to_string())?;
    let note = report_one(app, layer, row)?;
    if !note.is_empty() {
        app.inspection.report_note = Some((note, false));
    }
    Ok(())
}
