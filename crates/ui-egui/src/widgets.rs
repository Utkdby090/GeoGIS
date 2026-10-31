//! Ribbon widgets: large and small buttons, menu buttons, groups and columns. Each control runs a
//! command by id and greys out when the command is disabled.

use egui::{Align2, CornerRadius, Rect, Response, Sense, Stroke, Ui, pos2, vec2};
use serde_json::Value;

use crate::theme::{Tokens, regular, semibold};
use crate::{RustGisApp, icons};

/// Height of the ribbon's control area (without group labels).
pub const CONTENT_H: f32 = 72.0;
pub const LABEL_H: f32 = 18.0;

pub fn text_w(ui: &Ui, s: &str, font: egui::FontId) -> f32 {
    ui.ctx().fonts_mut(|f| f.layout_no_wrap(s.to_string(), font, egui::Color32::WHITE).size().x)
}

/// Shortcut text (`⌘S` on macOS, `Ctrl+S` elsewhere).
pub fn shortcut_text(app: &RustGisApp, id: &str) -> String {
    let Some(spec) = app.session.registry.get(id) else { return String::new() };
    if spec.shortcut.is_empty() {
        return String::new();
    }
    if cfg!(target_os = "macos") {
        spec.shortcut.replace("Mod+", "⌘").replace("Shift+", "⇧")
    } else {
        spec.shortcut.replace("Mod+", "Ctrl+")
    }
}

/// Whether a command exists and is enabled now.
pub fn enabled(app: &RustGisApp, id: &str) -> bool {
    match app.session.registry.get(id) {
        Some(s) => (s.enabled)(&app.session).is_none(),
        None => id.starts_with("ui.") || rustgis_engine::plugins::command(id).is_some(),
    }
}

pub fn why_disabled(app: &RustGisApp, id: &str) -> Option<&'static str> {
    app.session.registry.get(id).and_then(|s| (s.enabled)(&app.session))
}

/// A rich tooltip: title (with shortcut), description and why it's disabled.
pub fn tooltip(app: &RustGisApp, resp: Response, title: &str, desc: &str, id: &str) -> Response {
    let sc = shortcut_text(app, id);
    let why = why_disabled(app, id);
    resp.on_hover_ui(|ui| {
        ui.set_max_width(280.0);
        // Callers pass text that is already translated.
        let head = if sc.is_empty() { title.to_string() } else { format!("{title}  ({sc})") };
        ui.label(egui::RichText::new(head).font(semibold(12.5)));
        if !desc.is_empty() {
            ui.label(egui::RichText::new(desc).size(11.5).weak());
        }
        if let Some(w) = why {
            ui.label(egui::RichText::new(crate::i18n::fmt("Unavailable: {reason}", &[("reason", tl!(w))])).size(11.0).italics().weak());
        }
    })
}

fn bg(ui: &Ui, r: Rect, resp: &Response, checked: bool, t: &Tokens) {
    let fill = if resp.is_pointer_button_down_on() {
        t.pressed
    } else if checked {
        t.checked
    } else if resp.hovered() {
        t.hover
    } else {
        return;
    };
    ui.painter().rect_filled(r, CornerRadius::same(6), fill);
    if checked {
        ui.painter().rect_stroke(r, CornerRadius::same(6), Stroke::new(1.0, t.accent.gamma_multiply(0.55)), egui::StrokeKind::Inside);
    }
}

/// A small downward caret marking a menu.
pub fn caret(p: &egui::Painter, c: egui::Pos2, col: egui::Color32) {
    p.add(egui::Shape::convex_polygon(vec![c + vec2(-3.5, -1.8), c + vec2(3.5, -1.8), c + vec2(0.0, 2.2)], col, Stroke::NONE));
}

/// A large button: 32 px icon over a one- or two-line label. Returns the response.
pub fn big(
    ui: &mut Ui,
    app: &mut RustGisApp,
    icon: &str,
    label: &str,
    desc: &str,
    id: &str,
    params: Value,
    checked: bool,
    menu: bool,
) -> Response {
    let (label, desc) = (tl!(label), tl!(desc));
    let t = Tokens::get(ui.ctx());
    let lw = label.split('\n').map(|l| text_w(ui, l, regular(11.5)) + if menu { 9.0 } else { 0.0 }).fold(0.0, f32::max);
    let w = (lw + 14.0).max(50.0);
    let (r, resp) = ui.allocate_exact_size(vec2(w, CONTENT_H - 2.0), Sense::click());
    let on = enabled(app, id) || menu;
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, on, label.replace('\n', " ")));
    bg(ui, r, &resp, checked, &t);
    let ic = Rect::from_center_size(pos2(r.center().x, r.min.y + 21.0), vec2(32.0, 32.0));
    let (c, a) = if on { (t.icon, t.accent) } else { (t.text_disabled, t.text_disabled) };
    icons::paint(ui.painter(), ic, icon, c, a);
    let lines: Vec<&str> = label.split('\n').collect();
    let mut y = r.min.y + 42.0;
    for (i, l) in lines.iter().enumerate() {
        let col = if on { t.text } else { t.text_disabled };
        if menu && i + 1 == lines.len() {
            let tw = text_w(ui, l, regular(11.5));
            let x0 = r.center().x - (tw + 9.0) / 2.0;
            ui.painter().text(pos2(x0, y), Align2::LEFT_TOP, *l, regular(11.5), col);
            caret(ui.painter(), pos2(x0 + tw + 5.0, y + 7.5), col);
        } else {
            ui.painter().text(pos2(r.center().x, y), Align2::CENTER_TOP, *l, regular(11.5), col);
        }
        y += 13.5;
    }
    let resp = tooltip(app, resp, &label.replace('\n', " "), desc, id);
    if resp.clicked() && on && !menu {
        let _ = app.run(id, params);
    }
    resp
}

/// A small button: 18 px icon with an optional label.
pub fn small(ui: &mut Ui, app: &mut RustGisApp, icon: &str, label: &str, desc: &str, id: &str, params: Value, checked: bool) -> Response {
    let (label, desc) = (tl!(label), tl!(desc));
    let t = Tokens::get(ui.ctx());
    let tw = if label.is_empty() { 0.0 } else { text_w(ui, label, regular(11.5)) + 8.0 };
    let (r, resp) = ui.allocate_exact_size(vec2(26.0 + tw, 22.0), Sense::click());
    let on = enabled(app, id);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, on, if label.is_empty() { desc } else { label }));
    bg(ui, r, &resp, checked, &t);
    let ic = Rect::from_center_size(pos2(r.min.x + 13.0, r.center().y), vec2(18.0, 18.0));
    let (c, a) = if on { (t.icon, t.accent) } else { (t.text_disabled, t.text_disabled) };
    icons::paint(ui.painter(), ic, icon, c, a);
    if !label.is_empty() {
        ui.painter().text(
            pos2(r.min.x + 26.0, r.center().y),
            Align2::LEFT_CENTER,
            label,
            regular(11.5),
            if on { t.text } else { t.text_disabled },
        );
    }
    let title = if label.is_empty() { desc } else { label };
    let resp = tooltip(app, resp, title, if label.is_empty() { "" } else { desc }, id);
    if resp.clicked() && on {
        let _ = app.run(id, params);
    }
    resp
}

/// A large button that opens a menu.
pub fn big_menu(ui: &mut Ui, app: &mut RustGisApp, icon: &str, label: &str, desc: &str, menu: impl FnOnce(&mut Ui, &mut RustGisApp)) {
    let resp = big(ui, app, icon, label, desc, "ui.menu", Value::Null, false, true);
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(200.0);
        menu(ui, app);
    });
}

/// A small button that opens a menu.
pub fn small_menu(ui: &mut Ui, app: &mut RustGisApp, icon: &str, label: &str, desc: &str, menu: impl FnOnce(&mut Ui, &mut RustGisApp)) {
    let (label, desc) = (tl!(label), tl!(desc));
    let t = Tokens::get(ui.ctx());
    let tw = text_w(ui, label, regular(11.5)) + 20.0;
    let (r, resp) = ui.allocate_exact_size(vec2(26.0 + tw, 22.0), Sense::click());
    bg(ui, r, &resp, false, &t);
    icons::paint(ui.painter(), Rect::from_center_size(pos2(r.min.x + 13.0, r.center().y), vec2(18.0, 18.0)), icon, t.icon, t.accent);
    ui.painter().text(pos2(r.min.x + 26.0, r.center().y), Align2::LEFT_CENTER, label, regular(11.5), t.text);
    caret(ui.painter(), pos2(r.max.x - 9.0, r.center().y), t.text);
    let resp = tooltip(app, resp, label, desc, "ui.menu");
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(200.0);
        menu(ui, app);
    });
}

/// A frameless button as a row of a menu or list. Such a button paints nothing behind its text,
/// so the row's hover (or pressed) fill goes under it, `expand` wider than the row on each side.
pub fn row_button(ui: &mut Ui, on: bool, button: egui::Button<'_>, expand: f32) -> Response {
    let under = ui.painter().add(egui::Shape::Noop);
    let resp = ui.add_enabled(on, button);
    if on && (resp.hovered() || resp.is_pointer_button_down_on()) {
        let t = Tokens::get(ui.ctx());
        let fill = if resp.is_pointer_button_down_on() { t.pressed } else { t.hover };
        ui.painter().set(under, egui::Shape::rect_filled(resp.rect.expand2(vec2(expand, 0.0)), CornerRadius::same(5), fill));
    }
    resp
}

/// Room for a menu row's icon before its label.
const MENU_ICON_W: f32 = 22.0;

/// A menu row that runs a command (disabled when the command is).
pub fn menu_item(ui: &mut Ui, app: &mut RustGisApp, icon: &str, label: &str, id: &str, params: Value) {
    let label = tl!(label);
    let on = enabled(app, id);
    // With an icon, the label starts after it, so the icon is drawn inside the row (and the menu).
    let text: egui::WidgetText = if icon.is_empty() {
        label.into()
    } else {
        let mut job = egui::text::LayoutJob::default();
        let font = egui::TextStyle::Button.resolve(ui.style());
        job.append(label, MENU_ICON_W, egui::TextFormat::simple(font, egui::Color32::PLACEHOLDER));
        job.into()
    };
    let mut button = egui::Button::new(text).frame(false).min_size(vec2(0.0, 24.0));
    // Only with a shortcut: an empty right text would end the button's accessible label in a space.
    let shortcut = shortcut_text(app, id);
    if !shortcut.is_empty() {
        button = button.right_text(shortcut);
    }
    let resp = row_button(ui, on, button, 4.0);
    if !icon.is_empty() {
        let t = Tokens::get(ui.ctx());
        icons::paint(
            ui.painter(),
            Rect::from_center_size(
                resp.rect.left_center() + vec2(ui.spacing().button_padding.x + MENU_ICON_W / 2.0 - 2.0, 0.0),
                vec2(16.0, 16.0),
            ),
            icon,
            t.icon,
            t.accent,
        );
    }
    if resp.clicked() {
        let _ = app.run(id, params);
        ui.close();
    }
}

/// A ribbon group: controls laid out left to right over a centred label, with a separator.
pub fn group(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
    let title = tl!(title);
    let t = Tokens::get(ui.ctx());
    let start = ui.cursor().min;
    let inner = ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = vec2(2.0, 1.0);
        let r = ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
            ui.set_min_height(CONTENT_H);
            ui.spacing_mut().item_spacing = vec2(2.0, 1.0);
            add(ui);
        });
        let lw = text_w(ui, title, regular(11.0));
        let w = r.response.rect.width().max(lw + 12.0);
        ui.allocate_exact_size(vec2(w, LABEL_H), Sense::hover());
        w
    });
    let w = inner.inner;
    ui.painter().text(
        pos2(start.x + w / 2.0, start.y + CONTENT_H + LABEL_H / 2.0 - 1.0),
        Align2::CENTER_CENTER,
        title,
        regular(11.0),
        t.group_label,
    );
    ui.add_space(6.0);
    let x = ui.cursor().min.x;
    ui.painter().line_segment([pos2(x, start.y + 6.0), pos2(x, start.y + CONTENT_H + LABEL_H - 4.0)], Stroke::new(1.0, t.border));
    ui.add_space(7.0);
}

/// A column of up to three small controls.
pub fn col(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = vec2(1.0, 1.0);
        ui.add_space(1.0);
        add(ui);
    });
}

/// A compact labelled combo for ribbons. Returns the chosen value when changed.
pub fn ribbon_combo(ui: &mut Ui, id: &str, label: &str, current: &str, options: &[String], width: f32) -> Option<String> {
    let mut out = None;
    ui.horizontal(|ui| {
        if !label.is_empty() {
            ui.label(egui::RichText::new(tl!(label)).size(11.5));
        }
        egui::ComboBox::from_id_salt(id).selected_text(current).width(width).show_ui(ui, |ui| {
            for o in options {
                if ui.selectable_label(o == current, o).clicked() {
                    out = Some(o.clone());
                }
            }
        });
    });
    out
}

/// A filled action button (Run, Install…). Painted here so the label sits in the middle of the
/// button: egui places button text by the font's line metrics, which leaves some fonts low.
pub fn primary_button(ui: &mut Ui, label: &str, enabled: bool, min_size: egui::Vec2) -> Response {
    let t = Tokens::get(ui.ctx());
    let font = semibold(13.0);
    let size = vec2(min_size.x.max(text_w(ui, label, font.clone()) + 28.0), min_size.y.max(28.0));
    let (r, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    // Screen readers (and UI tests) find it by its label.
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    let fill = if !enabled {
        t.green.gamma_multiply(0.45)
    } else if resp.is_pointer_button_down_on() {
        t.green.gamma_multiply(0.8)
    } else if resp.hovered() {
        t.green.gamma_multiply(0.9)
    } else {
        t.green
    };
    ui.painter().rect_filled(r, CornerRadius::same(6), fill);
    centred_label(ui, r, label, font, egui::Color32::WHITE);
    if enabled { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp }
}

/// A dialog button (OK, Cancel…): `fill` paints it as the default action, otherwise it takes the
/// theme's button look. Painted like [`primary_button`] so the label sits in the middle.
pub fn dialog_button(ui: &mut Ui, label: &str, fill: Option<egui::Color32>, min_size: egui::Vec2) -> Response {
    let font = if fill.is_some() { semibold(12.5) } else { regular(12.5) };
    let size = vec2(min_size.x.max(text_w(ui, label, font.clone()) + 24.0), min_size.y);
    let (r, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    let v = *ui.style().interact(&resp);
    let (bg, text) = match fill {
        Some(c) if resp.is_pointer_button_down_on() => (c.gamma_multiply(0.8), egui::Color32::WHITE),
        Some(c) if resp.hovered() => (c.gamma_multiply(0.9), egui::Color32::WHITE),
        Some(c) => (c, egui::Color32::WHITE),
        None => (v.weak_bg_fill, v.text_color()),
    };
    ui.painter().rect(r, v.corner_radius, bg, if fill.is_some() { Stroke::NONE } else { v.bg_stroke }, egui::StrokeKind::Inside);
    centred_label(ui, r, label, font, text);
    resp
}

/// Paints `label` centred in `r` on its capital letters (baseline to cap height) rather than on
/// the font's line box, whose ascent and descent are uneven in some faces.
fn centred_label(ui: &Ui, r: Rect, label: &str, font: egui::FontId, color: egui::Color32) {
    let painter = ui.painter();
    let galley = painter.layout_no_wrap(label.to_string(), font.clone(), color);
    let cap = painter.layout_no_wrap("H".to_string(), font, color);
    let mid = cap
        .rows
        .first()
        .and_then(|row| Some((row.pos.y + row.glyphs.first()?.pos.y, row.pos.y + row.visuals.mesh_bounds.min.y)))
        .filter(|(base, top)| base > top)
        .map(|(base, top)| (base + top) / 2.0)
        .unwrap_or(galley.size().y / 2.0);
    painter.galley(pos2(r.center().x - galley.size().x / 2.0, r.center().y - mid).round(), galley, color);
}

/// A small icon button (back, close…), drawn with the app's own icons rather than a glyph the
/// interface font may lack.
pub fn icon_button(ui: &mut Ui, icon: &str, hover: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(r, CornerRadius::same(5), t.hover);
    }
    ui.painter().rect_stroke(r, CornerRadius::same(5), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    icons::paint(ui.painter(), r.shrink(5.0), icon, t.text, t.accent);
    resp.on_hover_text(hover)
}

/// Zoom and pan of a photo shown fitted in a box: 1 is the whole photo, up to [`MAX_PHOTO_ZOOM`].
/// Kept in egui's memory per view, and reset when the view shows another photo.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PhotoZoom {
    zoom: f32,
    /// The photo point (0..1 each way) at the middle of the box.
    centre: egui::Pos2,
    /// Which photo it is for (a hash of its path).
    photo: u64,
}

pub const MAX_PHOTO_ZOOM: f32 = 8.0;

impl PhotoZoom {
    fn new(photo: u64) -> Self {
        Self { zoom: 1.0, centre: pos2(0.5, 0.5), photo }
    }

    /// The photo's rectangle on screen; larger than `fit` once zoomed in.
    fn rect(&self, fit: Rect) -> Rect {
        let size = fit.size() * self.zoom;
        Rect::from_min_size(fit.center() - vec2(self.centre.x * size.x, self.centre.y * size.y), size)
    }

    /// Keeps the zoom in range and the photo covering the box.
    fn clamp(&mut self) {
        self.zoom = self.zoom.clamp(1.0, MAX_PHOTO_ZOOM);
        let half = 0.5 / self.zoom;
        self.centre = pos2(self.centre.x.clamp(half, 1.0 - half), self.centre.y.clamp(half, 1.0 - half));
    }

    /// Zooms by `factor` keeping the photo point under `at` (a screen point in `fit`) still.
    fn zoom_about(&mut self, fit: Rect, at: egui::Pos2, factor: f32) {
        let before = self.rect(fit);
        let p = (at - before.min) / before.size();
        self.zoom = (self.zoom * factor).clamp(1.0, MAX_PHOTO_ZOOM);
        let size = fit.size() * self.zoom;
        let off = (at - fit.center()) / size;
        self.centre = pos2(p.x - off.x, p.y - off.y);
        self.clamp();
    }
}

fn photo_hash(photo: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    photo.hash(&mut h);
    h.finish()
}

/// How far the view `id` is zoomed into `photo` (1 when not, or when it shows another photo).
/// Read before drawing, to pick a sharper copy of the photo once zoomed in.
pub fn photo_zoom_level(ctx: &egui::Context, id: egui::Id, photo: &str) -> f32 {
    let key = photo_hash(photo);
    ctx.data(|d| d.get_temp::<PhotoZoom>(id)).filter(|z| z.photo == key).map_or(1.0, |z| z.zoom)
}

/// Zoom and pan for a photo fitted into `fit` (the area `resp` covers): the mouse wheel or a
/// pinch zooms about the pointer, a right- or middle-button drag pans (a left drag too when
/// `left_pans`, for views where it draws nothing), a double-click goes back to the whole photo,
/// and small − / + / Fit buttons sit in the corner. Returns the photo's rectangle on screen, to
/// draw it (clipped to `fit`) and to map between screen and photo points.
pub fn photo_zoom(ui: &mut Ui, id: egui::Id, photo: &str, fit: Rect, resp: &Response, left_pans: bool) -> Rect {
    let key = photo_hash(photo);
    let mut z = ui.ctx().data(|d| d.get_temp::<PhotoZoom>(id)).filter(|z| z.photo == key).unwrap_or_else(|| PhotoZoom::new(key));
    let before = z;
    if resp.hovered()
        && let Some(at) = resp.hover_pos()
    {
        let (wheel, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let factor = pinch * (wheel * 0.002).exp();
        if (factor - 1.0).abs() > 1e-4 {
            z.zoom_about(fit, at, factor);
            // The wheel zoomed the photo: it doesn't also scroll the window around it.
            ui.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
        }
    }
    let panning = resp.dragged_by(egui::PointerButton::Secondary)
        || resp.dragged_by(egui::PointerButton::Middle)
        || (left_pans && resp.dragged_by(egui::PointerButton::Primary));
    if panning && z.zoom > 1.0 {
        let d = resp.drag_delta() / (fit.size() * z.zoom);
        z.centre -= d;
        z.clamp();
    }
    if resp.double_clicked() {
        z = PhotoZoom::new(key);
    }
    // − / + / Fit, top right of the photo.
    let t = Tokens::get(ui.ctx());
    let bar = Rect::from_min_size(pos2(fit.max.x - 178.0, fit.min.y + 6.0), vec2(172.0, 24.0));
    if fit.width() > 220.0 && fit.height() > 60.0 {
        // Drawn on a layer kept just above this one, so the photo (painted after) can't cover it.
        let layer = egui::LayerId::new(ui.layer_id().order, id.with("zoom_bar"));
        ui.ctx().set_sublayer(ui.layer_id(), layer);
        let painter = ui.painter().clone().with_layer_id(layer);
        painter.rect_filled(bar, CornerRadius::same(5), t.pane.gamma_multiply(0.92));
        let pct = format!("{:.0}%", z.zoom * 100.0);
        let level = ui.interact(Rect::from_min_size(bar.min, vec2(50.0, 24.0)), id.with("zoom_level"), Sense::hover());
        level.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, format!("Zoom {pct}")));
        painter.text(pos2(bar.min.x + 25.0, bar.center().y), Align2::CENTER_CENTER, pct, regular(12.0), t.text_dim);
        let cells = [("−", 1.0 / 1.5), ("+", 1.5), ("Fit", 0.0)];
        for (k, (label, factor)) in cells.into_iter().enumerate() {
            let w = if k == 2 { 46.0 } else { 36.0 };
            let x = bar.min.x + 50.0 + (k as f32) * 40.0;
            let r = Rect::from_min_size(pos2(x, bar.min.y + 2.0), vec2(w, 20.0));
            let b = ui.interact(r, id.with(("zoom_button", k)), Sense::click());
            b.widget_info(|| {
                let name = match k {
                    0 => crate::tl!("Zoom out"),
                    1 => crate::tl!("Zoom in"),
                    _ => crate::tl!("Fit photo"),
                };
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name)
            });
            if b.hovered() {
                painter.rect_filled(r, CornerRadius::same(4), t.hover);
            }
            let label = if k == 2 { crate::tl!("Fit") } else { label };
            painter.text(r.center(), Align2::CENTER_CENTER, label, regular(13.0), t.text);
            if b.clicked() {
                if factor == 0.0 {
                    z = PhotoZoom::new(key);
                } else {
                    z.zoom_about(fit, fit.center(), factor);
                }
            }
        }
    }
    if z != before || ui.ctx().data(|d| d.get_temp::<PhotoZoom>(id)).is_none() {
        ui.ctx().data_mut(|d| d.insert_temp(id, z));
    }
    if panning && z.zoom > 1.0 {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    }
    z.rect(fit)
}

#[cfg(test)]
mod photo_zoom_tests {
    use super::*;

    fn fit() -> Rect {
        Rect::from_min_size(pos2(100.0, 50.0), vec2(400.0, 300.0))
    }

    #[test]
    fn whole_photo_fills_the_box() {
        assert_eq!(PhotoZoom::new(1).rect(fit()), fit());
    }

    #[test]
    fn zooming_keeps_the_point_under_the_pointer_still() {
        let mut z = PhotoZoom::new(1);
        let at = pos2(180.0, 120.0);
        let before = (at - z.rect(fit()).min) / z.rect(fit()).size();
        z.zoom_about(fit(), at, 2.5);
        let r = z.rect(fit());
        let after = (at - r.min) / r.size();
        assert!((z.zoom - 2.5).abs() < 1e-5);
        assert!((before - after).length() < 1e-4, "{before:?} vs {after:?}");
        assert!(r.contains_rect(fit()), "the zoomed photo still covers the box");
    }

    #[test]
    fn zoom_stays_in_range_and_the_photo_covers_the_box() {
        let mut z = PhotoZoom::new(1);
        z.zoom_about(fit(), fit().left_top(), 100.0);
        assert_eq!(z.zoom, MAX_PHOTO_ZOOM);
        assert!(z.rect(fit()).contains_rect(fit()));
        z.zoom_about(fit(), fit().right_bottom(), 0.001);
        assert_eq!(z.zoom, 1.0);
        assert_eq!(z.rect(fit()), fit());
        // Panning past the edge stops at the edge.
        z.zoom = 4.0;
        z.centre = pos2(-3.0, 9.0);
        z.clamp();
        assert_eq!(z.centre, pos2(0.125, 0.875));
        assert!(z.rect(fit()).contains_rect(fit()));
    }
}
