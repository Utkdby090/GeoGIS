//! Markup drawn on an inspection photo: pen strokes, arrows, boxes, ellipses, text and numbered
//! markers, kept as editable shapes beside the photo. The photo file is never changed; the shapes
//! are drawn over it on screen and in reports.
//!
//! Positions are fractions of the photo's width and height (0 to 1, from the top left, after its
//! EXIF orientation is applied), so the same shapes fit a thumbnail, the full photo and a PDF page.
//! Stroke widths and text sizes are thousandths of the photo's shorter side, for the same reason.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

/// Most shapes on one photo.
pub const MAX_SHAPES: usize = 500;
/// Most points in one pen stroke (the editor thins a stroke before it gets near this).
pub const MAX_PEN_POINTS: usize = 4_096;
/// Most points in all pen strokes of one photo, so a photo's markup stays small in the project.
pub const MAX_POINTS: usize = 50_000;
/// Longest text label, in characters.
pub const MAX_TEXT_CHARS: usize = 200;
/// Stroke width range, in thousandths of the photo's shorter side.
pub const WIDTH_RANGE: (f32, f32) = (1.0, 50.0);
/// Text size range (the height of a capital letter), in thousandths of the photo's shorter side.
pub const TEXT_SIZE_RANGE: (f32, f32) = (10.0, 200.0);
/// Most steps the editor can undo.
pub const MAX_UNDO: usize = 200;

/// A point on the photo, as fractions of its width (`x`) and height (`y`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct NormPoint {
    pub x: f32,
    pub y: f32,
}

impl NormPoint {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }

    /// Kept on the photo: a shape dragged past its edge stops at the edge.
    fn clamped(self) -> Self {
        Self { x: self.x.clamp(0.0, 1.0), y: self.y.clamp(0.0, 1.0) }
    }
}

/// An sRGB colour with alpha, stored as `#rrggbb` or `#rrggbbaa`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Color(pub [u8; 4]);

impl Color {
    /// GeoFlux's markup red, the default for new shapes.
    pub const RED: Color = Color([0xE5, 0x1A, 0x1A, 0xFF]);

    pub fn to_hex(self) -> String {
        let [r, g, b, a] = self.0;
        if a == 0xFF { format!("#{r:02x}{g:02x}{b:02x}") } else { format!("#{r:02x}{g:02x}{b:02x}{a:02x}") }
    }

    pub fn from_hex(text: &str) -> Result<Self, AnnotationError> {
        let bad = || AnnotationError::BadColor(text.chars().take(16).collect());
        let hex = text.trim().strip_prefix('#').ok_or_else(bad)?;
        if !(hex.len() == 6 || hex.len() == 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(bad());
        }
        let byte = |i: usize| hex.get(i..i + 2).and_then(|s| u8::from_str_radix(s, 16).ok()).ok_or_else(bad);
        let alpha = if hex.len() == 8 { byte(6)? } else { 0xFF };
        Ok(Color([byte(0)?, byte(2)?, byte(4)?, alpha]))
    }
}

impl TryFrom<String> for Color {
    type Error = AnnotationError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Color::from_hex(&s)
    }
}

impl From<Color> for String {
    fn from(c: Color) -> String {
        c.to_hex()
    }
}

/// The geometry of one shape.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Shape {
    /// A freehand line through `points`.
    Pen { points: Vec<NormPoint> },
    /// A line from `from` with an arrow head at `to`.
    Arrow { from: NormPoint, to: NormPoint },
    /// A box with corners `a` and `b` (any two opposite corners).
    Rect { a: NormPoint, b: NormPoint },
    /// The ellipse inside the box with corners `a` and `b`.
    Ellipse { a: NormPoint, b: NormPoint },
    /// A text label whose top left is at `at`; `size` is the capital height.
    Text { at: NormPoint, text: String, size: f32 },
    /// A filled circle with a number in it, centred at `at`, for "see note 3" references.
    Marker { at: NormPoint, number: u16 },
}

impl Shape {
    fn points(&self) -> Vec<NormPoint> {
        match self {
            Shape::Pen { points } => points.clone(),
            Shape::Arrow { from: a, to: b } | Shape::Rect { a, b } | Shape::Ellipse { a, b } => vec![*a, *b],
            Shape::Text { at, .. } | Shape::Marker { at, .. } => vec![*at],
        }
    }

    fn for_each_point(&mut self, f: impl Fn(NormPoint) -> NormPoint) {
        match self {
            Shape::Pen { points } => points.iter_mut().for_each(|p| *p = f(*p)),
            Shape::Arrow { from: a, to: b } | Shape::Rect { a, b } | Shape::Ellipse { a, b } => {
                *a = f(*a);
                *b = f(*b);
            }
            Shape::Text { at, .. } | Shape::Marker { at, .. } => *at = f(*at),
        }
    }
}

/// One shape and how it is drawn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    #[serde(flatten)]
    pub shape: Shape,
    pub color: Color,
    /// Stroke width in thousandths of the photo's shorter side (unused by text).
    pub width: f32,
}

impl Annotation {
    pub fn new(shape: Shape) -> Self {
        Self { shape, color: Color::RED, width: 6.0 }
    }
}

/// Why a shape or a photo's markup was refused.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum AnnotationError {
    #[error("a photo can carry at most {MAX_SHAPES} shapes")]
    TooManyShapes,
    #[error("a pen stroke can have at most {MAX_PEN_POINTS} points, and a photo at most {MAX_POINTS}")]
    TooManyPoints,
    #[error("a text label can be at most {MAX_TEXT_CHARS} characters")]
    TextTooLong,
    #[error("a position, width or size isn't a number")]
    NotFinite,
    #[error("the shape has no size (a pen stroke needs two points, a box two different corners, text some text)")]
    Empty,
    #[error("'{0}' isn't a colour like #e51a1a")]
    BadColor(String),
    #[error("there is no shape {0}")]
    NoShape(usize),
}

/// The markup on one photo, in drawing order (later shapes are drawn on top).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AnnotationSet(Vec<Annotation>);

impl AnnotationSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Checks shapes read from a project file or a command, fixing what can be fixed: positions are
    /// kept on the photo and widths and sizes in range. Refuses what can't be (too many shapes or
    /// points, text too long, a value that isn't a number, a shape with no size).
    pub fn from_shapes(shapes: Vec<Annotation>) -> Result<Self, AnnotationError> {
        if shapes.len() > MAX_SHAPES {
            return Err(AnnotationError::TooManyShapes);
        }
        let mut total = 0usize;
        let mut out = Vec::with_capacity(shapes.len());
        for a in shapes {
            let a = checked(a)?;
            if let Shape::Pen { points } = &a.shape {
                total += points.len();
            }
            if total > MAX_POINTS {
                return Err(AnnotationError::TooManyPoints);
            }
            out.push(a);
        }
        Ok(Self(out))
    }

    pub fn shapes(&self) -> &[Annotation] {
        &self.0
    }

    pub fn into_shapes(self) -> Vec<Annotation> {
        self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The number a new marker gets: one more than the highest so far.
    pub fn next_marker(&self) -> u16 {
        let top = self.0.iter().filter_map(|a| if let Shape::Marker { number, .. } = a.shape { Some(number) } else { None }).max();
        top.map_or(1, |n| n.saturating_add(1))
    }

    fn pen_points(&self) -> usize {
        self.0.iter().map(|a| if let Shape::Pen { points } = &a.shape { points.len() } else { 0 }).sum()
    }
}

/// One shape checked and tidied (see [`AnnotationSet::from_shapes`]).
pub fn checked(mut a: Annotation) -> Result<Annotation, AnnotationError> {
    if !a.width.is_finite() || !a.shape.points().iter().all(|p| p.is_finite()) {
        return Err(AnnotationError::NotFinite);
    }
    a.width = a.width.clamp(WIDTH_RANGE.0, WIDTH_RANGE.1);
    a.shape.for_each_point(NormPoint::clamped);
    match &mut a.shape {
        Shape::Pen { points } => {
            if points.len() > MAX_PEN_POINTS {
                return Err(AnnotationError::TooManyPoints);
            }
            points.dedup();
            if points.len() < 2 {
                return Err(AnnotationError::Empty);
            }
        }
        Shape::Arrow { from: a, to: b } | Shape::Rect { a, b } | Shape::Ellipse { a, b } => {
            if a == b {
                return Err(AnnotationError::Empty);
            }
        }
        Shape::Text { text, size, .. } => {
            if !size.is_finite() {
                return Err(AnnotationError::NotFinite);
            }
            *size = size.clamp(TEXT_SIZE_RANGE.0, TEXT_SIZE_RANGE.1);
            // Line breaks and other control characters would break the label's layout.
            let clean: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
            let clean = clean.trim();
            if clean.is_empty() {
                return Err(AnnotationError::Empty);
            }
            if clean.chars().count() > MAX_TEXT_CHARS {
                return Err(AnnotationError::TextTooLong);
            }
            *text = clean.to_string();
        }
        Shape::Marker { .. } => {}
    }
    Ok(a)
}

/// One editing step, kept so it can be undone.
#[derive(Clone, Debug, PartialEq)]
enum Step {
    Add(Annotation),
    Remove(usize, Annotation),
    Replace(usize, Annotation, Annotation),
    Clear(Vec<Annotation>),
}

/// Edits one photo's markup with undo and redo. The editor holds only its own photo's shapes and
/// the steps that changed them (not copies of the whole set), at most [`MAX_UNDO`] of them.
#[derive(Clone, Debug, Default)]
pub struct AnnotationEditor {
    set: AnnotationSet,
    /// The markup as opened, to tell whether it changed.
    initial: AnnotationSet,
    done: VecDeque<Step>,
    undone: Vec<Step>,
}

impl AnnotationEditor {
    pub fn new(set: AnnotationSet) -> Self {
        Self { initial: set.clone(), set, done: VecDeque::new(), undone: Vec::new() }
    }

    pub fn set(&self) -> &AnnotationSet {
        &self.set
    }

    /// The markup, ending the edit.
    pub fn into_set(self) -> AnnotationSet {
        self.set
    }

    pub fn can_undo(&self) -> bool {
        !self.done.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }

    /// Whether the markup differs from what the editor opened with.
    pub fn is_dirty(&self) -> bool {
        self.set != self.initial
    }

    pub fn add(&mut self, a: Annotation) -> Result<(), AnnotationError> {
        let a = checked(a)?;
        if self.set.len() >= MAX_SHAPES {
            return Err(AnnotationError::TooManyShapes);
        }
        if self.set.pen_points() + pen_len(&a) > MAX_POINTS {
            return Err(AnnotationError::TooManyPoints);
        }
        self.set.0.push(a.clone());
        self.record(Step::Add(a));
        Ok(())
    }

    pub fn remove(&mut self, i: usize) -> Result<(), AnnotationError> {
        if i >= self.set.len() {
            return Err(AnnotationError::NoShape(i));
        }
        let old = self.set.0.remove(i);
        self.record(Step::Remove(i, old));
        Ok(())
    }

    /// Changes shape `i` (moved, recoloured, text edited).
    pub fn replace(&mut self, i: usize, a: Annotation) -> Result<(), AnnotationError> {
        let a = checked(a)?;
        let old_len = self.set.0.get(i).map(pen_len).ok_or(AnnotationError::NoShape(i))?;
        if self.set.pen_points() - old_len + pen_len(&a) > MAX_POINTS {
            return Err(AnnotationError::TooManyPoints);
        }
        let Some(slot) = self.set.0.get_mut(i) else { return Err(AnnotationError::NoShape(i)) };
        let old = std::mem::replace(slot, a.clone());
        self.record(Step::Replace(i, old, a));
        Ok(())
    }

    pub fn clear(&mut self) {
        if self.set.is_empty() {
            return;
        }
        let old = std::mem::take(&mut self.set.0);
        self.record(Step::Clear(old));
    }

    pub fn undo(&mut self) -> bool {
        let Some(step) = self.done.pop_back() else { return false };
        match &step {
            Step::Add(_) => {
                self.set.0.pop();
            }
            Step::Remove(i, a) => self.set.0.insert((*i).min(self.set.0.len()), a.clone()),
            Step::Replace(i, old, _) => {
                if let Some(slot) = self.set.0.get_mut(*i) {
                    *slot = old.clone();
                }
            }
            Step::Clear(old) => self.set.0 = old.clone(),
        }
        self.undone.push(step);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(step) = self.undone.pop() else { return false };
        match &step {
            Step::Add(a) => self.set.0.push(a.clone()),
            Step::Remove(i, _) => {
                if *i < self.set.0.len() {
                    self.set.0.remove(*i);
                }
            }
            Step::Replace(i, _, new) => {
                if let Some(slot) = self.set.0.get_mut(*i) {
                    *slot = new.clone();
                }
            }
            Step::Clear(_) => self.set.0.clear(),
        }
        self.done.push_back(step);
        true
    }

    fn record(&mut self, step: Step) {
        self.undone.clear();
        if self.done.len() == MAX_UNDO {
            self.done.pop_front();
        }
        self.done.push_back(step);
    }
}

fn pen_len(a: &Annotation) -> usize {
    if let Shape::Pen { points } = &a.shape { points.len() } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32) -> Annotation {
        Annotation::new(Shape::Rect { a: NormPoint::new(x, 0.1), b: NormPoint::new(x + 0.2, 0.3) })
    }

    #[test]
    fn colour_round_trips_and_rejects_junk() {
        assert_eq!(Color::from_hex("#E51A1A"), Ok(Color::RED));
        assert_eq!(Color::from_hex("#00ff0080").map(|c| c.to_hex()), Ok("#00ff0080".to_string()));
        assert!(Color::from_hex("red").is_err());
        assert!(Color::from_hex("#12345").is_err());
        assert!(Color::from_hex("#ééééé").is_err());
    }

    #[test]
    fn json_shape_is_flat_and_tagged() {
        let set = AnnotationSet::from_shapes(vec![Annotation::new(Shape::Marker { at: NormPoint::new(0.5, 0.5), number: 3 })]).unwrap();
        let json = serde_json::to_string(&set).unwrap();
        assert_eq!(json, r##"[{"kind":"marker","at":{"x":0.5,"y":0.5},"number":3,"color":"#e51a1a","width":6.0}]"##);
        let back: AnnotationSet = serde_json::from_str(&json).unwrap();
        assert_eq!(back, set);
    }

    #[test]
    fn checks_clamp_and_refuse() {
        let a = checked(Annotation { width: 999.0, ..rect(0.9) }).unwrap();
        assert_eq!(a.width, WIDTH_RANGE.1);
        assert_eq!(a.shape, Shape::Rect { a: NormPoint::new(0.9, 0.1), b: NormPoint::new(1.0, 0.3) });
        assert_eq!(checked(rect(f32::NAN)), Err(AnnotationError::NotFinite));
        let dot = Annotation::new(Shape::Pen { points: vec![NormPoint::new(0.2, 0.2); 3] });
        assert_eq!(checked(dot), Err(AnnotationError::Empty));
        let text = Annotation::new(Shape::Text { at: NormPoint::new(0.1, 0.1), text: " crack\nhere ".into(), size: 40.0 });
        assert!(matches!(checked(text).unwrap().shape, Shape::Text { ref text, .. } if text == "crack here"));
        assert_eq!(AnnotationSet::from_shapes(vec![rect(0.1); MAX_SHAPES + 1]), Err(AnnotationError::TooManyShapes));
    }

    #[test]
    fn undo_and_redo_walk_every_step() {
        let mut ed = AnnotationEditor::new(AnnotationSet::new());
        ed.add(rect(0.1)).unwrap();
        ed.add(rect(0.2)).unwrap();
        ed.replace(0, rect(0.5)).unwrap();
        ed.remove(1).unwrap();
        ed.clear();
        assert!(ed.set().is_empty());
        let mut seen = vec![ed.set().len()];
        while ed.undo() {
            seen.push(ed.set().len());
        }
        assert_eq!(seen, [0, 1, 2, 2, 1, 0]);
        assert!(!ed.is_dirty());
        while ed.redo() {}
        assert!(ed.set().is_empty());
        ed.undo();
        assert_eq!(ed.set().shapes(), &[rect(0.5)]);
        ed.add(rect(0.3)).unwrap();
        assert!(!ed.can_redo(), "a new edit drops the redo steps");
    }

    #[test]
    fn undo_history_is_capped() {
        let mut ed = AnnotationEditor::new(AnnotationSet::new());
        for i in 0..MAX_UNDO + 10 {
            ed.add(rect((i % 50) as f32 / 100.0)).unwrap();
        }
        let mut n = 0;
        while ed.undo() {
            n += 1;
        }
        assert_eq!(n, MAX_UNDO);
        assert_eq!(ed.set().len(), 10);
    }

    #[test]
    fn markers_count_up() {
        let mut ed = AnnotationEditor::new(AnnotationSet::new());
        assert_eq!(ed.set().next_marker(), 1);
        ed.add(Annotation::new(Shape::Marker { at: NormPoint::new(0.1, 0.1), number: 4 })).unwrap();
        assert_eq!(ed.set().next_marker(), 5);
    }
}
