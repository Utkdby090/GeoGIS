//! GeoFlux › Findings: what is wrong with the tower, reviewed on its photos.
//!
//! - Under the form, the tower's findings in a few lines and how many wait for review.
//! - The Findings window: one of the tower's photos with its findings drawn on it (numbered, in
//!   their severity's colour; suggestions dashed). Drag a box (or click a point) and pick the
//!   defect to add one; confirm, edit or reject a rule's suggestion; Next pending walks to the next
//!   tower with suggestions waiting.
//!
//! Every change is a `geoflux.*Finding` command (one undoable edit, gated like form edits); the
//! window only draws the session's findings. It follows the Inspection pane's tower.

use std::collections::{HashMap, HashSet};

use egui::{Align2, Color32, FontId, Pos2, Rect, RichText, Stroke, Ui, pos2, vec2};
use serde_json::json;

use crate::RustGisApp;
use crate::geofluxpane::{severity_color, severity_label};
use crate::theme::{Tokens, semibold};
use rustgis_engine::cmd::findings as fx;
use rustgis_engine::cmd::geoflux::{self as gf, SlotPhotos};
use rustgis_engine::geoflux_domain::annotation::NormPoint;
use rustgis_engine::geoflux_domain::finding::{Finding, FindingSource, FindingStatus, Region, measure};
use rustgis_engine::geoflux_domain::{DomainSchema, PhotoId, Severity};

/// Photo content ids kept (each read once from the first and last 64 KiB of the file).
const MAX_IDS: usize = 256;
/// The side panel's width, in points.
const PANEL: f32 = 320.0;
/// A drag smaller than this (a fraction of the photo) is a click.
const MIN_BOX: f32 = 0.01;
/// Findings listed under the form before "and N more".
const SUMMARY_ROWS: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    Box,
    Point,
}

/// A finding being made (`id` None) or changed.
#[derive(Clone, Debug, Default)]
struct Draft {
    id: Option<String>,
    region: Region,
    defect: String,
    /// None: the defect type's own severity.
    severity: Option<Severity>,
    note: String,
}

/// The Findings window on one tower.
pub struct Review {
    layer: u32,
    row: usize,
    /// The photo shown (one of the tower's), or empty when it has none.
    path: String,
    tool: Tool,
    /// A box being dragged, in photo fractions.
    drawing: Option<(NormPoint, NormPoint)>,
    draft: Option<Draft>,
    selected: Option<String>,
    show_rejected: bool,
    message: Option<(String, bool)>,
    /// Photo content ids by path.
    ids: HashMap<String, Result<PhotoId, String>>,
}

impl Review {
    fn new(layer: u32, row: usize) -> Self {
        Self {
            layer,
            row,
            path: String::new(),
            tool: Tool::Box,
            drawing: None,
            draft: None,
            selected: None,
            show_rejected: false,
            message: None,
            ids: HashMap::new(),
        }
    }

    /// The shown photo's content id, read once.
    fn photo_id(&mut self) -> Option<&Result<PhotoId, String>> {
        if self.path.is_empty() {
            return None;
        }
        if !self.ids.contains_key(&self.path) {
            if self.ids.len() >= MAX_IDS {
                self.ids.clear();
            }
            self.ids.insert(self.path.clone(), fx::photo_id_of(&self.path));
        }
        self.ids.get(&self.path)
    }
}

/// What the window asked for, done after drawing.
enum Act {
    Add(Draft),
    Save(Draft),
    Decide(String, &'static str),
    Remove(String),
    Photo(String),
    NextPending,
}

/// Opens the Findings window on the Inspection pane's tower (`ui.geofluxFindings`).
pub fn open(app: &mut RustGisApp) -> Result<(), String> {
    let (Some(layer), Some(row)) = (app.inspection.layer, app.inspection.row) else {
        app.ui.right = Some(crate::RightPane::Inspection);
        return Err(tl!("Open a tower in the Inspection pane first.").into());
    };
    app.ui.right = Some(crate::RightPane::Inspection);
    let mut r = Review::new(layer, row);
    r.path = first_photo(app, layer, row);
    app.inspection.review = Some(r);
    Ok(())
}

/// Who makes a change: the signed-in account's name, else the computer's user.
pub(crate) fn who(app: &RustGisApp) -> String {
    if let Some(geoflux_licence::Access::Subscribed { pass, .. }) = app.cloud.access()
        && !pass.name.trim().is_empty()
    {
        return pass.name.trim().to_string();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_default()
    }
    #[cfg(target_arch = "wasm32")]
    {
        String::new()
    }
}

/// The schema and label of the tower at `row`.
fn tower(app: &RustGisApp, layer: u32, row: usize) -> Option<(DomainSchema, String)> {
    let (schema, asset) = gf::asset(&app.session, layer, row).ok()?;
    let label = asset.property(schema.label_key()).trim().to_string();
    Some((schema, label))
}

/// One tower's schema, label and where its findings are in the layer's list, read once per
/// project revision: the summary and the window draw on every frame, and a line can hold
/// thousands of findings.
pub struct TowerCache {
    key: (u32, usize, u64),
    schema: DomainSchema,
    label: String,
    mine: Vec<usize>,
}

impl TowerCache {
    /// The tower's findings, in the layer's order.
    fn findings<'a>(&'a self, app: &'a RustGisApp) -> impl Iterator<Item = &'a Finding> + 'a {
        let all = app.session.project.geoflux.findings_of(self.key.0).map_or(&[][..], |set| set.findings());
        self.mine.iter().filter_map(move |&i| all.get(i))
    }
}

/// The tower's cache, taken out of the pane's state (put it back in `app.inspection.tower`), read
/// again when the project has changed.
fn tower_cache(app: &mut RustGisApp, layer: u32, row: usize) -> Option<TowerCache> {
    let key = (layer, row, app.session.revision);
    if let Some(c) = app.inspection.tower.take().filter(|c| c.key == key) {
        return Some(c);
    }
    let (schema, label) = tower(app, layer, row)?;
    let mine = app
        .session
        .project
        .geoflux
        .findings_of(layer)
        .map_or_else(Vec::new, |set| set.findings().iter().enumerate().filter(|(_, f)| f.asset == label).map(|(i, _)| i).collect());
    Some(TowerCache { key, schema, label, mine })
}

/// Worst first, then oldest, as the form lists them.
fn worst_first(a: &&Finding, b: &&Finding) -> std::cmp::Ordering {
    b.severity.cmp(&a.severity).then(a.created.cmp(&b.created))
}

/// The photo to open a tower on: one with a suggestion waiting, else the thermal slot's shown
/// photo, else the first.
fn first_photo(app: &RustGisApp, layer: u32, row: usize) -> String {
    let read;
    let slots: &[SlotPhotos] = match app.inspection.slots(layer, row) {
        Some(s) => s,
        None => {
            read = gf::photos(&app.session, layer, row).unwrap_or_default();
            &read
        }
    };
    let has = |p: &str| slots.iter().any(|s| s.photos.iter().any(|x| x == p));
    let g = &app.session.project.geoflux;
    if let (Some(set), Some((_, label))) = (g.findings_of(layer), tower(app, layer, row)) {
        let waiting = set.findings().iter().filter(|f| f.asset == label && f.status == FindingStatus::Proposed);
        if let Some(p) = waiting.filter_map(|f| f.photo.as_ref().and_then(|id| g.photo_paths.get(id))).find(|p| has(p)) {
            return p.clone();
        }
    }
    let thermal = slots.iter().find(|s| s.slot.thermal && !s.photos.is_empty());
    thermal.or_else(|| slots.iter().find(|s| !s.photos.is_empty())).and_then(|s| app.inspection.shown_in(s)).cloned().unwrap_or_default()
}

/// The tower's findings in a few lines under the form, and the button to review them.
pub fn summary_ui(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens, layer: u32, row: usize) {
    let Some(tc) = tower_cache(app, layer, row) else { return };
    let mut open_it = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(tl!("Findings")).font(semibold(12.5)));
        if ui.small_button(tl!("Review…")).on_hover_text(tl!("Draw and check findings on this tower's photos")).clicked() {
            open_it = true;
        }
    });
    if app.session.project.geoflux.findings_of(layer).is_none() {
        ui.label(
            RichText::new(tl!(
                "None yet. Press Review to mark a fault on a photo; the fault values typed so far become this line's first findings."
            ))
            .small()
            .color(t.text_dim),
        );
    } else {
        let mut counted: Vec<&Finding> = tc.findings(app).filter(|f| f.status.counts()).collect();
        counted.sort_by(worst_first);
        let pending = tc.findings(app).filter(|f| f.status == FindingStatus::Proposed).count();
        if counted.is_empty() {
            ui.label(RichText::new(tl!("No confirmed finding.")).small().color(t.text_dim));
        }
        for f in counted.iter().take(SUMMARY_ROWS) {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(r.center(), 4.0, severity_color(f.severity, t));
                ui.label(RichText::new(tc.schema.defect_label(&f.defect)).small());
                if let Some(m) = measured(f) {
                    ui.label(RichText::new(m).small().color(t.text_dim));
                }
            });
        }
        if counted.len() > SUMMARY_ROWS {
            let more = (counted.len() - SUMMARY_ROWS).to_string();
            ui.label(RichText::new(crate::i18n::fmt("and {n} more", &[("n", &more)])).small().color(t.text_dim));
        }
        if pending > 0 {
            let n = pending.to_string();
            ui.label(RichText::new(crate::i18n::fmt("Suggestions to review: {n}", &[("n", &n)])).small().color(t.amber));
        }
    }
    app.inspection.tower = Some(tc);
    if open_it {
        let _ = app.run("ui.geofluxFindings", serde_json::Value::Null);
    }
}

/// "max 61.0 °C · rise 25.0 °C", or None without temperatures.
fn measured(f: &Finding) -> Option<String> {
    let max = f.measurement(measure::MAX_C).map(|v| format!("{} {v:.1} °C", tl!("max")));
    let rise = f.measurement(measure::RISE_C).map(|v| format!("{} {v:.1} °C", tl!("rise")));
    match (max, rise) {
        (None, None) => None,
        (a, b) => Some(a.into_iter().chain(b).collect::<Vec<_>>().join(" · ")),
    }
}

fn source_text(f: &Finding) -> String {
    match &f.source {
        FindingSource::Engineer { user } if user.is_empty() => tl!("Engineer").to_string(),
        FindingSource::Engineer { user } => user.clone(),
        FindingSource::Rule { inspector, version } => {
            let rule = fx::thermal_rule();
            let name = if *inspector == rule.id { rule.label } else { inspector.clone() };
            format!("{name} v{version}")
        }
        FindingSource::Model { inspector, version } => match f.confidence {
            Some(c) => format!("{inspector} v{version} ({:.0}%)", c * 100.0),
            None => format!("{inspector} v{version}"),
        },
    }
}

fn status_text(s: FindingStatus) -> &'static str {
    match s {
        FindingStatus::Proposed => tl!("Suggested"),
        FindingStatus::Confirmed => tl!("Confirmed"),
        FindingStatus::Edited => tl!("Confirmed, edited"),
        FindingStatus::Rejected => tl!("Rejected"),
    }
}

fn to_screen(rect: Rect, p: NormPoint) -> Pos2 {
    pos2(rect.min.x + p.x * rect.width(), rect.min.y + p.y * rect.height())
}

fn to_photo(rect: Rect, p: Pos2) -> NormPoint {
    NormPoint::new(((p.x - rect.min.x) / rect.width()).clamp(0.0, 1.0), ((p.y - rect.min.y) / rect.height()).clamp(0.0, 1.0))
}

/// A region's outline on screen: its box (a point gets a small square around it).
fn screen_box(rect: Rect, region: &Region) -> Option<Rect> {
    let [x, y, w, h] = region.bbox()?;
    let r = Rect::from_min_size(to_screen(rect, NormPoint::new(x, y)), vec2(w * rect.width(), h * rect.height()));
    Some(if matches!(region, Region::Point { .. }) { Rect::from_center_size(r.center(), vec2(18.0, 18.0)) } else { r })
}

/// Draws a region: solid when it counts, dashed when suggested or rejected.
fn paint_region(painter: &egui::Painter, rect: Rect, region: &Region, color: Color32, width: f32, dashed: bool, fill: bool) {
    let Some(b) = screen_box(rect, region) else { return };
    let stroke = Stroke::new(width, color);
    if fill {
        painter.rect_filled(b, 0.0, color.gamma_multiply(0.18));
    }
    match region {
        Region::Polygon { points } if points.len() > 2 => {
            let mut pts: Vec<Pos2> = points.iter().map(|p| to_screen(rect, *p)).collect();
            if dashed {
                if let Some(first) = pts.first().copied() {
                    pts.push(first);
                }
                painter.extend(egui::Shape::dashed_line(&pts, stroke, 6.0, 4.0));
            } else {
                painter.add(egui::Shape::closed_line(pts, stroke));
            }
        }
        Region::Point { at } => {
            let c = to_screen(rect, *at);
            painter.circle_stroke(c, 9.0, stroke);
            painter.line_segment([c - vec2(4.0, 0.0), c + vec2(4.0, 0.0)], stroke);
            painter.line_segment([c - vec2(0.0, 4.0), c + vec2(0.0, 4.0)], stroke);
        }
        _ if dashed => {
            let pts = [b.left_top(), b.right_top(), b.right_bottom(), b.left_bottom(), b.left_top()];
            painter.extend(egui::Shape::dashed_line(&pts, stroke, 6.0, 4.0));
        }
        _ => {
            painter.rect_stroke(b, 0.0, stroke, egui::StrokeKind::Middle);
        }
    }
}

/// The finding's number tag at its region's top-left corner.
fn paint_tag(painter: &egui::Painter, rect: Rect, region: &Region, n: usize, color: Color32) {
    let Some(b) = screen_box(rect, region) else { return };
    let text = n.to_string();
    let at = pos2(b.min.x, (b.min.y - 16.0).max(rect.min.y));
    let tag = Rect::from_min_size(at, vec2(8.0 + 7.0 * text.len() as f32, 16.0));
    painter.rect_filled(tag, 3.0, color);
    let ink = if color.r() as u32 + color.g() as u32 + color.b() as u32 > 480 { Color32::BLACK } else { Color32::WHITE };
    painter.text(tag.center(), Align2::CENTER_CENTER, text, FontId::proportional(11.0), ink);
}

/// The window, while open; called by the Inspection pane on every frame it shows a tower.
pub fn show(app: &mut RustGisApp, ctx: &egui::Context, t: &Tokens, layer: u32, row: usize) {
    let Some(mut rv) = app.inspection.review.take() else { return };
    // It follows the pane's tower.
    if (rv.layer, rv.row) != (layer, row) {
        rv.layer = layer;
        rv.row = row;
        rv.path = first_photo(app, layer, row);
        rv.draft = None;
        rv.selected = None;
        rv.drawing = None;
        rv.message = None;
    }
    let Some(tc) = tower_cache(app, layer, row) else {
        app.inspection.review = Some(rv);
        return;
    };
    let photo_id = rv.photo_id().and_then(|r| r.as_ref().ok()).cloned();
    let id_error = rv.photo_id().and_then(|r| r.as_ref().err()).cloned();
    let texture = if rv.path.is_empty() { None } else { app.inspection.photo(&rv.path, true).cloned() };
    let dirty = app.inspection.dirty();
    // Every photo of the tower as (slot id, slot label, path), for the chooser.
    let slots: Vec<(&str, &str, &str)> = app.inspection.slots(layer, row).map_or_else(Vec::new, |s| {
        s.iter().flat_map(|sp| sp.photos.iter().map(move |p| (sp.slot.id.as_str(), sp.slot.label.as_str(), p.as_str()))).collect()
    });
    let slot = slots.iter().find(|(_, _, p)| *p == rv.path).map_or("", |(id, _, _)| *id);

    let paths = &app.session.project.geoflux.photo_paths;
    let mut here: Vec<&Finding> = Vec::new();
    // With the path of its photo when that is one of the tower's photos now.
    let mut elsewhere: Vec<(&Finding, Option<&str>)> = Vec::new();
    let mut pending_total = 0;
    for f in tc.findings(app) {
        if f.status == FindingStatus::Rejected && !rv.show_rejected {
            continue;
        }
        pending_total += usize::from(f.status == FindingStatus::Proposed);
        match (&f.photo, &photo_id) {
            (Some(p), Some(id)) if p == id => here.push(f),
            _ => {
                let path =
                    f.photo.as_ref().and_then(|id| paths.get(id)).map(String::as_str).filter(|p| slots.iter().any(|(_, _, x)| x == p));
                elsewhere.push((f, path));
            }
        }
    }
    here.sort_by(worst_first);
    elsewhere.sort_by(|a, b| worst_first(&a.0, &b.0));
    let (schema, label) = (&tc.schema, tc.label.as_str());

    let mut act: Option<Act> = None;
    let mut open = true;
    let title = crate::i18n::fmt("Findings · {tower}", &[("tower", label)]);
    egui::Window::new(title).id(egui::Id::new("geoflux_findings")).open(&mut open).default_size(vec2(1180.0, 760.0)).resizable(true).show(
        ctx,
        |ui| {
            toolbar(&mut rv, ui, &slots, &mut act, pending_total);
            ui.separator();
            let room = ui.available_size();
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    let canvas_room = vec2((room.x - PANEL - 12.0).max(200.0), (room.y - 8.0).max(160.0));
                    match (&texture, rv.path.is_empty()) {
                        (_, true) => {
                            ui.add_sized(
                                canvas_room,
                                egui::Label::new(
                                    RichText::new(tl!("This tower has no photos. Findings can still be added for the whole tower."))
                                        .color(t.text_dim),
                                ),
                            );
                        }
                        (Some(Ok(tex)), false) => canvas(&mut rv, ui, t, tex, canvas_room, &here),
                        (Some(Err(e)), false) => {
                            ui.add_sized(canvas_room, egui::Label::new(RichText::new(e.as_str()).color(t.red)));
                        }
                        (None, false) => {
                            ui.add_sized(canvas_room, egui::Spinner::new());
                        }
                    }
                });
                ui.vertical(|ui| {
                    ui.set_width(PANEL);
                    egui::ScrollArea::vertical().id_salt("geoflux_findings_list").max_height(room.y - 8.0).show(ui, |ui| {
                        panel(&mut rv, ui, t, schema, slot, &here, &elsewhere, dirty, id_error.as_deref(), &mut act);
                    });
                });
            });
        },
    );
    drop((here, elsewhere, slots));
    app.inspection.tower = Some(tc);
    if !open {
        app.inspection.review = None;
        return;
    }
    app.inspection.review = Some(rv);
    if let Some(a) = act {
        run(app, a, layer, row);
    }
}

fn toolbar(rv: &mut Review, ui: &mut Ui, slots: &[(&str, &str, &str)], act: &mut Option<Act>, pending: usize) {
    ui.horizontal_wrapped(|ui| {
        if ui.selectable_label(rv.tool == Tool::Box, tl!("Box")).on_hover_text(tl!("Drag a box around the fault.")).clicked() {
            rv.tool = Tool::Box;
        }
        if ui.selectable_label(rv.tool == Tool::Point, tl!("Point")).on_hover_text(tl!("Click the fault.")).clicked() {
            rv.tool = Tool::Point;
        }
        ui.separator();
        let at = slots.iter().position(|(_, _, p)| *p == rv.path);
        let n = slots.len();
        if ui.add_enabled(n > 1, egui::Button::new("‹").small()).on_hover_text(tl!("Previous photo")).clicked() {
            let i = at.map_or(0, |i| i.checked_sub(1).unwrap_or(n - 1));
            if let Some((_, _, p)) = slots.get(i) {
                *act = Some(Act::Photo(p.to_string()));
            }
        }
        let name = |(_, slot, p): &(&str, &str, &str)| format!("{slot} · {}", p.rsplit(['/', '\\']).next().unwrap_or(p));
        let shown = at.and_then(|i| slots.get(i)).map_or_else(|| "—".to_string(), name);
        egui::ComboBox::from_id_salt("geoflux_findings_photo").selected_text(shown).width(300.0).show_ui(ui, |ui| {
            for s in slots {
                if ui.selectable_label(s.2 == rv.path, name(s)).clicked() && s.2 != rv.path {
                    *act = Some(Act::Photo(s.2.to_string()));
                }
            }
        });
        if ui.add_enabled(n > 1, egui::Button::new("›").small()).on_hover_text(tl!("Next photo")).clicked() {
            let i = at.map_or(0, |i| (i + 1) % n.max(1));
            if let Some((_, _, p)) = slots.get(i) {
                *act = Some(Act::Photo(p.to_string()));
            }
        }
        ui.separator();
        ui.checkbox(&mut rv.show_rejected, tl!("Show rejected"));
        ui.separator();
        let next = egui::Button::new(tl!("Next pending ›"));
        if ui.add(next).on_hover_text(tl!("The next tower with suggestions to review")).clicked() {
            *act = Some(Act::NextPending);
        }
        if pending > 0 {
            let n = pending.to_string();
            ui.label(RichText::new(crate::i18n::fmt("Suggestions on this tower: {n}", &[("n", &n)])).small());
        }
    });
}

/// The photo with the findings on it; a drag draws a box, a click picks a finding (or, with the
/// point tool, places a point).
fn canvas(rv: &mut Review, ui: &mut Ui, t: &Tokens, tex: &egui::TextureHandle, room: egui::Vec2, here: &[&Finding]) {
    let size = tex.size_vec2();
    let scale = (room.x / size.x).min(room.y / size.y).max(0.01);
    let (rect, resp) = ui.allocate_exact_size(size * scale, egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    for (i, f) in here.iter().enumerate() {
        let selected = rv.selected.as_deref() == Some(f.id.as_str());
        let color = match f.status {
            FindingStatus::Rejected => t.text_disabled,
            _ => severity_color(f.severity, t),
        };
        let width = if selected { 3.5 } else { 2.0 };
        let dashed = !f.status.counts();
        paint_region(&painter, rect, &f.region, color, width, dashed, selected);
        paint_tag(&painter, rect, &f.region, i + 1, color);
    }
    let draft_region = rv.drawing.map(|(a, b)| Region::Box { a, b }).or_else(|| rv.draft.as_ref().map(|d| d.region.clone()));
    if let Some(r) = &draft_region {
        paint_region(&painter, rect, r, t.accent, 2.5, true, false);
    }
    let resp = resp.on_hover_cursor(egui::CursorIcon::Crosshair);
    let pointer = resp.interact_pointer_pos();
    if resp.drag_started()
        && rv.tool == Tool::Box
        && let Some(p) = pointer.map(|p| to_photo(rect, p))
    {
        rv.drawing = Some((p, p));
    }
    if resp.dragged()
        && let (Some(pos), Some((_, b))) = (pointer, rv.drawing.as_mut())
    {
        *b = to_photo(rect, pos);
    }
    if resp.drag_stopped()
        && let Some((a, b)) = rv.drawing.take()
    {
        let region =
            if (a.x - b.x).abs() < MIN_BOX && (a.y - b.y).abs() < MIN_BOX { Region::Point { at: a } } else { Region::Box { a, b } };
        place(rv, region);
    }
    if resp.clicked()
        && let Some(pos) = pointer
    {
        // The smallest finding under the click, so a box inside another can be picked.
        let hit = here
            .iter()
            .filter_map(|f| screen_box(rect, &f.region).filter(|b| b.expand(4.0).contains(pos)).map(|b| (b.area(), f.id.to_string())))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id);
        match (hit, rv.tool) {
            (Some(id), _) => {
                rv.selected = Some(id);
            }
            (None, Tool::Point) => place(rv, Region::Point { at: to_photo(rect, pos) }),
            (None, Tool::Box) => rv.selected = None,
        }
    }
}

/// A region drawn: it becomes the edited finding's new region, or a new finding's.
fn place(rv: &mut Review, region: Region) {
    match rv.draft.as_mut() {
        Some(d) => d.region = region,
        None => {
            rv.selected = None;
            rv.draft = Some(Draft { region, ..Draft::default() });
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn panel(
    rv: &mut Review,
    ui: &mut Ui,
    t: &Tokens,
    schema: &DomainSchema,
    slot: &str,
    here: &[&Finding],
    elsewhere: &[(&Finding, Option<&str>)],
    dirty: bool,
    id_error: Option<&str>,
    act: &mut Option<Act>,
) {
    if let Some((m, err)) = &rv.message {
        ui.label(RichText::new(m.as_str()).small().color(if *err { t.red } else { t.text_dim }));
    }
    if dirty {
        ui.label(RichText::new(tl!("Save or revert this tower's form first; findings change it.")).small().color(t.amber));
    }
    if let Some(e) = id_error {
        ui.label(RichText::new(e).small().color(t.red));
    }
    if rv.draft.is_some() {
        draft_ui(rv, ui, t, schema, slot, dirty, act);
        ui.separator();
    } else if ui.button(tl!("New finding for the whole photo")).on_hover_text(tl!("Or drag a box on the photo")).clicked() {
        rv.selected = None;
        rv.draft = Some(Draft::default());
    }
    ui.add_space(4.0);
    ui.label(RichText::new(tl!("On this photo")).font(semibold(12.5)));
    if here.is_empty() {
        ui.label(RichText::new(tl!("Nothing yet. Drag a box around a fault.")).small().color(t.text_dim));
    }
    for (i, f) in here.iter().enumerate() {
        item(rv, ui, t, schema, f, Some(i + 1), None, dirty, act);
    }
    if !elsewhere.is_empty() {
        ui.add_space(6.0);
        ui.label(RichText::new(tl!("Elsewhere on this tower")).font(semibold(12.5)));
        for (f, path) in elsewhere {
            item(rv, ui, t, schema, f, None, Some(*path), dirty, act);
        }
    }
}

fn draft_ui(rv: &mut Review, ui: &mut Ui, t: &Tokens, schema: &DomainSchema, slot: &str, dirty: bool, act: &mut Option<Act>) {
    let Some(d) = rv.draft.as_mut() else { return };
    let editing = d.id.is_some();
    ui.label(RichText::new(if editing { tl!("Edit finding") } else { tl!("New finding") }).font(semibold(12.5)));
    let place = match d.region {
        Region::Whole => tl!("Whole photo"),
        Region::Point { .. } => tl!("A point (click elsewhere to move it)"),
        Region::Box { .. } => tl!("A box (drag again to redraw it)"),
        Region::Polygon { .. } => tl!("An outline"),
    };
    ui.label(RichText::new(place).small().color(t.text_dim));
    let applies: Vec<_> = schema.defects.iter().filter(|x| x.applies_to(slot) || x.id.as_str() == d.defect).collect();
    let current =
        applies.iter().find(|x| x.id.as_str() == d.defect).map_or_else(|| tl!("Pick the defect…").to_string(), |x| x.label.clone());
    egui::ComboBox::from_id_salt("geoflux_finding_defect").selected_text(current).width(PANEL - 16.0).show_ui(ui, |ui| {
        for x in &applies {
            if ui.selectable_label(x.id.as_str() == d.defect, x.label.as_str()).clicked() {
                d.defect = x.id.as_str().to_string();
            }
        }
    });
    let default = applies.iter().find(|x| x.id.as_str() == d.defect).map(|x| x.severity);
    let shown = match (d.severity, default) {
        (Some(s), _) => severity_label(s).to_string(),
        (None, Some(s)) => crate::i18n::fmt("Default ({level})", &[("level", severity_label(s))]),
        (None, None) => tl!("Default").to_string(),
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(tl!("Severity")).small());
        egui::ComboBox::from_id_salt("geoflux_finding_severity").selected_text(shown).width(180.0).show_ui(ui, |ui| {
            if ui.selectable_label(d.severity.is_none(), tl!("The defect's default")).clicked() {
                d.severity = None;
            }
            for s in Severity::ALL {
                if ui.selectable_label(d.severity == Some(s), severity_label(s)).clicked() {
                    d.severity = Some(s);
                }
            }
        });
    });
    ui.add(
        egui::TextEdit::multiline(&mut d.note)
            .hint_text(tl!("Note (phase, component…)"))
            .desired_rows(2)
            .char_limit(2_000)
            .desired_width(PANEL - 16.0),
    );
    let mut cancel = false;
    ui.horizontal(|ui| {
        let ready = !d.defect.is_empty() && !dirty;
        let label = if editing { tl!("Save") } else { tl!("Add") };
        if ui.add_enabled(ready, egui::Button::new(label)).clicked() {
            *act = Some(if editing { Act::Save(d.clone()) } else { Act::Add(d.clone()) });
        }
        if ui.button(tl!("Cancel")).clicked() {
            cancel = true;
        }
    });
    if cancel {
        rv.draft = None;
    }
}

#[allow(clippy::too_many_arguments)]
fn item(
    rv: &mut Review,
    ui: &mut Ui,
    t: &Tokens,
    schema: &DomainSchema,
    f: &Finding,
    number: Option<usize>,
    // For a finding not on the shown photo: its photo's path, when that is one of the tower's.
    elsewhere: Option<Option<&str>>,
    dirty: bool,
    act: &mut Option<Act>,
) {
    let id = f.id.to_string();
    let selected = rv.selected.as_deref() == Some(id.as_str());
    let frame = egui::Frame::new()
        .inner_margin(6.0)
        .corner_radius(4.0)
        .stroke(Stroke::new(if selected { 1.5 } else { 1.0 }, if selected { t.accent } else { t.border }));
    frame.show(ui, |ui| {
        ui.set_width(PANEL - 24.0);
        ui.horizontal(|ui| {
            let (r, resp) = ui.allocate_exact_size(vec2(10.0, 10.0), egui::Sense::click());
            ui.painter().circle_filled(r.center(), 5.0, severity_color(f.severity, t));
            let title = match number {
                Some(n) => format!("{n}. {}", schema.defect_label(&f.defect)),
                None => schema.defect_label(&f.defect).to_string(),
            };
            if ui.selectable_label(selected, RichText::new(title).strong()).clicked() || resp.clicked() {
                rv.selected = if selected { None } else { Some(id.clone()) };
            }
        });
        let status = status_text(f.status);
        let status_color = match f.status {
            FindingStatus::Proposed => t.amber,
            FindingStatus::Rejected => t.text_disabled,
            _ => t.text_dim,
        };
        ui.label(RichText::new(format!("{status} · {} · {}", severity_label(f.severity), source_text(f))).small().color(status_color));
        if let Some(m) = measured(f) {
            ui.label(RichText::new(m).small());
        }
        if !f.note.is_empty() {
            ui.label(RichText::new(f.note.as_str()).small().italics());
        }
        if let Some(at) = elsewhere {
            match (at, &f.photo) {
                (Some(p), _) => {
                    if ui.small_button(tl!("Show its photo")).clicked() {
                        *act = Some(Act::Photo(p.to_string()));
                    }
                }
                (None, None) => {
                    ui.label(RichText::new(tl!("For the whole tower")).small().color(t.text_dim));
                }
                (None, Some(_)) => {
                    ui.label(RichText::new(tl!("On a photo not linked to this tower now")).small().color(t.text_dim));
                }
            }
        }
        ui.horizontal(|ui| {
            match f.status {
                FindingStatus::Proposed => {
                    if ui.add_enabled(!dirty, egui::Button::new(tl!("Confirm")).small()).clicked() {
                        *act = Some(Act::Decide(id.clone(), "confirm"));
                    }
                    if ui.add_enabled(!dirty, egui::Button::new(tl!("Reject")).small()).clicked() {
                        *act = Some(Act::Decide(id.clone(), "reject"));
                    }
                }
                FindingStatus::Rejected => {}
                _ => {
                    if matches!(f.source, FindingSource::Engineer { .. })
                        && ui.add_enabled(!dirty, egui::Button::new(tl!("Delete")).small()).clicked()
                    {
                        *act = Some(Act::Remove(id.clone()));
                    }
                }
            }
            if f.status != FindingStatus::Rejected && ui.add_enabled(!dirty, egui::Button::new(tl!("Edit")).small()).clicked() {
                rv.selected = Some(id.clone());
                rv.draft = Some(Draft {
                    id: Some(id.clone()),
                    region: f.region.clone(),
                    defect: f.defect.as_str().to_string(),
                    severity: Some(f.severity),
                    note: f.note.clone(),
                });
            }
        });
    });
}

/// Does what the window asked for.
fn run(app: &mut RustGisApp, a: Act, layer: u32, row: usize) {
    let by = who(app);
    let result = match a {
        Act::Photo(p) => {
            if let Some(rv) = app.inspection.review.as_mut() {
                rv.path = p;
                rv.draft = None;
                rv.selected = None;
                rv.message = None;
            }
            return;
        }
        Act::NextPending => {
            next_pending(app, layer, row);
            return;
        }
        Act::Add(d) => {
            let path = app.inspection.review.as_ref().map(|r| r.path.clone()).unwrap_or_default();
            let mut p = json!({"layer": layer, "row": row, "defect": d.defect, "region": d.region, "note": d.note, "by": by});
            if !path.is_empty() {
                p["path"] = json!(path);
            }
            if let Some(s) = d.severity {
                p["severity"] = json!(s);
            }
            app.run("geoflux.addFinding", p).map(|r| r["id"].as_str().map(str::to_string))
        }
        Act::Save(d) => {
            let mut p = json!({"layer": layer, "id": d.id, "defect": d.defect, "region": d.region, "note": d.note, "by": by});
            if let Some(s) = d.severity {
                p["severity"] = json!(s);
            }
            app.run("geoflux.editFinding", p).map(|_| d.id)
        }
        Act::Decide(id, decision) => {
            app.run("geoflux.decideFinding", json!({"layer": layer, "id": id, "decision": decision, "by": by})).map(|_| Some(id))
        }
        Act::Remove(id) => app.run("geoflux.removeFinding", json!({"layer": layer, "id": id})).map(|_| None),
    };
    let Some(rv) = app.inspection.review.as_mut() else { return };
    match result {
        Ok(selected) => {
            rv.draft = None;
            rv.selected = selected;
            rv.message = None;
            // The thumbnails redraw with the findings; the form reloads with the new revision.
            app.inspection.forget_marks();
        }
        Err(e) => rv.message = Some((e, true)),
    }
}

/// Moves the pane to the next tower (in walking order, wrapping) with suggestions waiting.
fn next_pending(app: &mut RustGisApp, layer: u32, row: usize) {
    let waiting: HashSet<&str> = app
        .session
        .project
        .geoflux
        .findings_of(layer)
        .map(|set| set.findings().iter().filter(|f| f.status == FindingStatus::Proposed).map(|f| f.asset.as_str()).collect())
        .unwrap_or_default();
    // The whole line: a tower with suggestions only isn't flagged (they don't count yet).
    let walk = match gf::sequence(&app.session, layer, false) {
        Ok(w) => w,
        Err(e) => {
            if let Some(rv) = app.inspection.review.as_mut() {
                rv.message = Some((e, true));
            }
            return;
        }
    };
    let at = walk.iter().position(|s| s.row == row).map_or(0, |i| i + 1);
    let next = (0..walk.len()).map(|k| (at + k) % walk.len().max(1)).filter_map(|i| walk.get(i)).find(|s| waiting.contains(s.label.trim()));
    let Some(target) = next.map(|s| s.row) else {
        if let Some(rv) = app.inspection.review.as_mut() {
            rv.message = Some((tl!("No suggestions left to review on this line.").into(), false));
        }
        return;
    };
    drop(waiting);
    if target != row {
        app.inspection.flagged_only = false;
        crate::geofluxpane::go(app, layer, target);
    }
    // The pane reads the new tower's photos on its next frame; the window follows then.
    if let Some(rv) = app.inspection.review.as_mut() {
        rv.row = usize::MAX;
    }
}
