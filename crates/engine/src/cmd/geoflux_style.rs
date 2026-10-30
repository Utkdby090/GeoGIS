//! How GeoFlux's layers look on the map: towers and survey photos must read apart at a glance.
//!
//! - Towers are triangles coloured by severity (white when nothing is wrong, then yellow, orange,
//!   red and maroon), labelled with their tower number once zoomed in.
//! - Survey photos are small circles coloured by kind (thermal pink, visual blue, wide teal, zoom
//!   violet). Their label (the tower they were sorted to) is set up but off, since a day's 700
//!   photos would hide the map; Style › Label turns it on.
//!
//! Both are ordinary renderers and labels, so the Style tab changes them like any layer's.

use geoflux_domain::Severity;
use geoflux_domain::survey_photos::PhotoKind;
use rustgis_map::{Marker, Ramp, Renderer, Rgba, Symbol};

use crate::Session;

/// Labels show once zoomed in beyond 1:this, so a whole line isn't a wall of text.
const LABEL_SCALE: f64 = 150_000.0;

fn point(fill: Rgba, stroke: Rgba, size: f32, marker: Marker) -> Symbol {
    Symbol { fill, stroke, stroke_width: 1.2, size, marker }
}

/// A severity's colour on the map.
pub fn severity_color(level: Severity) -> Rgba {
    match level {
        Severity::None => Rgba::rgb(0xF8, 0xF9, 0xFA),
        Severity::Low => Rgba::rgb(0xFF, 0xD4, 0x3B),
        Severity::Medium => Rgba::rgb(0xFF, 0x92, 0x2B),
        Severity::High => Rgba::rgb(0xF0, 0x3E, 0x3E),
        Severity::Critical => Rgba::rgb(0x86, 0x0F, 0x0F),
    }
}

/// A photo kind's colour on the map.
pub fn kind_color(kind: PhotoKind) -> Rgba {
    match kind {
        PhotoKind::Thermal => Rgba::rgb(0xE6, 0x49, 0x80),
        PhotoKind::Visual => Rgba::rgb(0x33, 0x9A, 0xF0),
        PhotoKind::Wide => Rgba::rgb(0x20, 0xC9, 0x97),
        PhotoKind::Zoom => Rgba::rgb(0x84, 0x5E, 0xF7),
        PhotoKind::Other => Rgba::rgb(0xAD, 0xB5, 0xBD),
    }
}

/// Towers: triangles by severity, labelled with `label_field`.
pub fn style_towers(s: &mut Session, layer: u32, severity_field: &str, label_field: &str) {
    let outline = Rgba::rgb(0x21, 0x25, 0x29);
    let classes =
        Severity::ALL.iter().map(|&l| (l.as_str().to_string(), point(severity_color(l), outline, 13.0, Marker::Triangle))).collect();
    let other = point(severity_color(Severity::None), outline, 13.0, Marker::Triangle);
    let Some(l) = s.map_mut().layer_mut(layer) else { return };
    let has = |f: &str| l.dataset.field_index(f).is_some();
    l.renderer = if has(severity_field) {
        Renderer::Unique { field: severity_field.to_string(), classes, other, ramp: Ramp::default() }
    } else {
        Renderer::Single(other)
    };
    if has(label_field) {
        l.labels.field = label_field.to_string();
        l.labels.enabled = true;
        l.labels.bold = true;
        l.labels.halo = true;
        l.labels.size = 11.0;
        l.labels.color = Rgba::rgb(0xFF, 0xFF, 0xFF);
        l.labels.min_scale = Some(LABEL_SCALE);
    }
}

/// Survey photos: small circles by kind; labelled with `label_field` (off until turned on).
pub fn style_photos(s: &mut Session, layer: u32, kind_field: &str, label_field: &str) {
    let white = Rgba::rgb(0xFF, 0xFF, 0xFF);
    let classes = PhotoKind::ALL.iter().map(|&k| (k.as_str().to_string(), point(kind_color(k), white, 6.0, Marker::Circle))).collect();
    let other = point(kind_color(PhotoKind::Other), white, 6.0, Marker::Circle);
    let Some(l) = s.map_mut().layer_mut(layer) else { return };
    if l.dataset.field_index(kind_field).is_some() {
        l.renderer = Renderer::Unique { field: kind_field.to_string(), classes, other, ramp: Ramp::default() };
    }
    set_photo_label(l, label_field);
}

/// Points the photo layer's (off) label at `field`, keeping whether it is on.
pub fn set_photo_label(l: &mut rustgis_map::Layer, field: &str) {
    if l.dataset.field_index(field).is_some() {
        l.labels.field = field.to_string();
        l.labels.size = 9.0;
        l.labels.halo = true;
        l.labels.color = Rgba::rgb(0xFF, 0xFF, 0xFF);
        l.labels.min_scale = Some(LABEL_SCALE / 10.0);
    }
}
