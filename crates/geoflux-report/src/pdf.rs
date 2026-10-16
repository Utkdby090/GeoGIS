//! Writes [`Page`]s as a vector PDF: text and lines as PDF operators (selectable, searchable,
//! sharp at any zoom), JPEG photos embedded as their original bytes, and photo markup drawn as
//! vector shapes over the photo.
//!
//! Text uses the PDF's built-in Helvetica in Windows-1252 ("WinAnsi") encoding: Latin text shows
//! as is, and any other character becomes `?` and is listed in the [`Summary`].
//!
//! Layout is in points from the top left of the page; [`Canvas`] flips to PDF's bottom-left origin.

use std::collections::BTreeSet;
use std::io::Write;

use geoflux_domain::Severity;
use geoflux_domain::annotation::{Annotation, AnnotationSet, Color, Shape};
use miniz_oxide::deflate::compress_to_vec_zlib;
use pdf_writer::types::{LineCapStyle, LineJoinStyle};
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref, Str, TextStr};

use crate::image::decode_png;
use crate::{Image, ImageKind, Page, PageRow, PhotoIssue, ReportError, Summary};

const MARGIN: f32 = 28.0;
const TITLE_SIZE: f32 = 16.0;
const BAR_H: f32 = 22.0;
const NAV_W: f32 = 64.0;
const GAP: f32 = 8.0;
const CAPTION_H: f32 = 14.0;
const ROW_H: f32 = 16.0;
const TEXT: f32 = 8.5;
const LINE: f32 = 10.5;
/// Most lines a full-width value wraps to before it is cut with "…".
const MAX_WIDE_LINES: usize = 6;
const FOOTER_H: f32 = 22.0;
const LOGO_BOX: (f32, f32) = (110.0, 40.0);
const MAP_H: f32 = 180.0;

const DARK: [f32; 3] = [0.13, 0.15, 0.18];
const GREY: [f32; 3] = [0.45, 0.47, 0.5];
const RULE: [f32; 3] = [0.8, 0.82, 0.85];
const LABEL_BG: [f32; 3] = [0.945, 0.953, 0.961];
const WHITE: [f32; 3] = [1.0, 1.0, 1.0];

const F_REG: Name = Name(b"F1");
const F_BOLD: Name = Name(b"F2");

/// Writes one report (a page that may continue onto more pages when its table is long) as a PDF.
pub fn write_pdf(page: &Page, out: &mut dyn Write) -> Result<Summary, ReportError> {
    let mut summary = Summary::default();
    let bytes = render(page, &mut summary);
    out.write_all(&bytes)?;
    summary.bytes = bytes.len() as u64;
    Ok(summary)
}

/// The PDF's bytes. Never fails: a photo that can't be embedded is drawn as "No image" and listed.
pub(crate) fn render(page: &Page, summary: &mut Summary) -> Vec<u8> {
    let mut ids = Ids(0);
    let mut pdf = Pdf::new();
    let (catalog, tree, info) = (ids.next(), ids.next(), ids.next());
    let (font_reg, font_bold) = (ids.next(), ids.next());

    // Images first: each gets a name, and the ones that fail to embed become "No image".
    let mut xobjects: Vec<(String, Ref)> = Vec::new();
    let mut photos: Vec<PlacedPhoto> = Vec::new();
    for (i, p) in page.photos.iter().enumerate() {
        let image = match &p.image {
            Ok(Some(img)) => match embed(&mut pdf, &mut ids, img) {
                Ok(r) => {
                    let name = format!("Im{i}");
                    xobjects.push((name.clone(), r));
                    Ok(Some((name, img.info.shown_size(), img.info.orientation)))
                }
                Err(issue) => Err(issue),
            },
            Ok(None) => Ok(None),
            Err(issue) => Err(issue.clone()),
        };
        if let Err(issue) = &image {
            summary.photos_missing.push((page.label.clone(), p.caption.clone(), issue.clone()));
        }
        photos.push(PlacedPhoto { caption: p.caption.clone(), image, markup: &p.annotations });
    }
    let mut small = |img: &Option<Image>, name: &str| -> Option<(String, (u32, u32), u8)> {
        let img = img.as_ref()?;
        let r = embed(&mut pdf, &mut ids, img).ok()?;
        xobjects.push((name.to_string(), r));
        Some((name.to_string(), img.info.shown_size(), img.info.orientation))
    };
    let logo = small(&page.logo, "Logo");
    let map = small(&page.map, "Map");

    // Transparency used by markup, one graphics state per alpha value.
    let alphas: BTreeSet<u8> =
        page.photos.iter().flat_map(|p| p.annotations.shapes().iter().map(|a| a.color.0[3])).filter(|&a| a < 255).collect();
    let mut states: Vec<(String, Ref)> = Vec::new();
    for a in &alphas {
        let r = ids.next();
        let v = f32::from(*a) / 255.0;
        pdf.ext_graphics(r).stroking_alpha(v).non_stroking_alpha(v);
        states.push((format!("GS{a}"), r));
    }

    let (w, h) = page.size.points();
    let mut layout = Layout { w, h, pages: vec![Canvas::new(h)], y: MARGIN, text: Text::default() };
    layout.draw(page, &photos, logo.as_ref(), map.as_ref());
    summary.replaced_chars = layout.text.replaced.into_iter().collect();

    // Footers, now that the page count is known.
    let total = layout.pages.len();
    let footer = page.footer.trim();
    let mut text = Text::default();
    for (i, c) in layout.pages.iter_mut().enumerate() {
        let y = h - MARGIN + 4.0;
        c.rule(MARGIN, y - 12.0, w - MARGIN, y - 12.0, 0.5, RULE);
        c.text_centered(&mut text, footer, w / 2.0, y, TEXT, false, GREY);
        if total > 1 {
            let n = format!("Page {} of {total}", i + 1);
            c.text_right(&mut text, &n, w - MARGIN, y, TEXT - 1.0, false, GREY);
        }
    }

    let page_ids: Vec<Ref> = (0..total).map(|_| ids.next()).collect();
    pdf.catalog(catalog).pages(tree);
    pdf.pages(tree).kids(page_ids.iter().copied()).count(total as i32);
    for (canvas, &pid) in layout.pages.into_iter().zip(&page_ids) {
        let cid = ids.next();
        let mut pg = pdf.page(pid);
        pg.media_box(Rect::new(0.0, 0.0, w, h)).parent(tree).contents(cid);
        let mut res = pg.resources();
        res.fonts().pair(F_REG, font_reg).pair(F_BOLD, font_bold);
        if !xobjects.is_empty() {
            let mut xo = res.x_objects();
            for (name, r) in &xobjects {
                xo.pair(Name(name.as_bytes()), *r);
            }
        }
        if !states.is_empty() {
            let mut gs = res.ext_g_states();
            for (name, r) in &states {
                gs.pair(Name(name.as_bytes()), *r);
            }
        }
        res.finish();
        pg.finish();
        let data = compress_to_vec_zlib(&canvas.content.finish().into_vec(), 6);
        pdf.stream(cid, &data).filter(Filter::FlateDecode);
    }
    for (r, base) in [(font_reg, b"Helvetica".as_slice()), (font_bold, b"Helvetica-Bold".as_slice())] {
        pdf.type1_font(r).base_font(Name(base)).encoding_predefined(Name(b"WinAnsiEncoding"));
    }
    let title = if page.label.is_empty() { page.title.clone() } else { format!("{} – {}", page.title, page.label) };
    pdf.document_info(info).title(TextStr(&title)).producer(TextStr("GeoGIS"));
    summary.pages += total;
    pdf.finish()
}

/// Sequential object ids.
struct Ids(i32);

impl Ids {
    fn next(&mut self) -> Ref {
        self.0 += 1;
        Ref::new(self.0)
    }
}

/// Embeds an image (and its alpha mask) and returns its object id.
fn embed(pdf: &mut Pdf, ids: &mut Ids, img: &Image) -> Result<Ref, PhotoIssue> {
    let id = ids.next();
    match img.info.kind {
        ImageKind::Jpeg { components, inverted } => {
            let mut x = pdf.image_xobject(id, &img.bytes);
            x.filter(Filter::DctDecode);
            x.width(img.info.width as i32).height(img.info.height as i32).bits_per_component(8);
            match components {
                1 => x.color_space().device_gray(),
                4 => x.color_space().device_cmyk(),
                _ => x.color_space().device_rgb(),
            }
            if inverted {
                x.decode([1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0]);
            }
        }
        ImageKind::Png => {
            let png = decode_png(&img.bytes)?;
            let mask = png.alpha.as_ref().map(|_| ids.next());
            let mut x = pdf.image_xobject(id, &png.rgb);
            x.filter(Filter::FlateDecode);
            x.width(png.width as i32).height(png.height as i32).bits_per_component(8);
            x.color_space().device_rgb();
            if let Some(m) = mask {
                x.s_mask(m);
            }
            x.finish();
            if let (Some(m), Some(alpha)) = (mask, &png.alpha) {
                let mut s = pdf.image_xobject(m, alpha);
                s.filter(Filter::FlateDecode);
                s.width(png.width as i32).height(png.height as i32).bits_per_component(8);
                s.color_space().device_gray();
            }
        }
    }
    Ok(id)
}

/// A photo cell as laid out: its embedded image's name, shown size and orientation, or why not.
struct PlacedPhoto<'a> {
    caption: String,
    image: Result<Option<(String, (u32, u32), u8)>, PhotoIssue>,
    markup: &'a AnnotationSet,
}

/// One PDF page's content, drawn in top-left coordinates.
struct Canvas {
    content: Content,
    h: f32,
}

impl Canvas {
    fn new(h: f32) -> Self {
        Self { content: Content::new(), h }
    }

    fn fill(&mut self, c: [f32; 3]) -> &mut Content {
        self.content.set_fill_rgb(c[0], c[1], c[2])
    }

    fn stroke_rgb(&mut self, c: [f32; 3]) -> &mut Content {
        self.content.set_stroke_rgb(c[0], c[1], c[2])
    }

    fn rect_fill(&mut self, x: f32, y: f32, w: f32, h: f32, c: [f32; 3]) {
        self.fill(c);
        let top = self.h - y - h;
        self.content.rect(x, top, w, h).fill_nonzero();
    }

    fn rect_stroke(&mut self, x: f32, y: f32, w: f32, h: f32, width: f32, c: [f32; 3]) {
        self.stroke_rgb(c);
        let top = self.h - y - h;
        self.content.set_line_width(width).rect(x, top, w, h).stroke();
    }

    fn rule(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, width: f32, c: [f32; 3]) {
        self.stroke_rgb(c);
        let (a, b) = (self.h - y1, self.h - y2);
        self.content.set_line_width(width).move_to(x1, a).line_to(x2, b).stroke();
    }

    /// Text whose baseline starts at (x, y).
    #[allow(clippy::too_many_arguments)]
    fn text_at(&mut self, t: &mut Text, s: &str, x: f32, y: f32, size: f32, bold: bool, c: [f32; 3]) {
        let bytes = t.encode(s);
        if bytes.is_empty() {
            return;
        }
        self.fill(c);
        let base = self.h - y;
        self.content.begin_text().set_font(if bold { F_BOLD } else { F_REG }, size).next_line(x, base).show(Str(&bytes)).end_text();
    }

    #[allow(clippy::too_many_arguments)]
    fn text_centered(&mut self, t: &mut Text, s: &str, cx: f32, y: f32, size: f32, bold: bool, c: [f32; 3]) {
        let w = t.width(s, bold, size);
        self.text_at(t, s, cx - w / 2.0, y, size, bold, c);
    }

    #[allow(clippy::too_many_arguments)]
    fn text_right(&mut self, t: &mut Text, s: &str, right: f32, y: f32, size: f32, bold: bool, c: [f32; 3]) {
        let w = t.width(s, bold, size);
        self.text_at(t, s, right - w, y, size, bold, c);
    }

    /// An image named `name` fitted (whole, centred) into the box; returns the box it fills.
    fn image_fit(&mut self, name: &str, shown: (u32, u32), orientation: u8, bx: Bounds) -> Bounds {
        let r = fit(shown, bx);
        let (x, y, w, h) = (r.x, self.h - r.y - r.h, r.w, r.h);
        // The unit square to the page, turned as the EXIF orientation says (see `orient_matrix`).
        let m = orient_matrix(orientation, x, y, w, h);
        self.content.save_state();
        self.content.transform(m);
        self.content.x_object(Name(name.as_bytes()));
        self.content.restore_state();
        r
    }
}

/// A box in top-left page coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Bounds {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

/// The largest box with the image's proportions inside `b`, centred.
fn fit(shown: (u32, u32), b: Bounds) -> Bounds {
    let (iw, ih) = (shown.0.max(1) as f32, shown.1.max(1) as f32);
    let s = (b.w / iw).min(b.h / ih);
    let (w, h) = (iw * s, ih * s);
    Bounds { x: b.x + (b.w - w) / 2.0, y: b.y + (b.h - h) / 2.0, w, h }
}

/// The matrix that maps an image's unit square (PDF image space: u across the stored columns,
/// v up from the stored bottom row) onto the page box (x, y, w, h, bottom-left origin) so the
/// picture appears as EXIF `orientation` says it should be shown.
fn orient_matrix(orientation: u8, x: f32, y: f32, w: f32, h: f32) -> [f32; 6] {
    match orientation {
        2 => [-w, 0.0, 0.0, h, x + w, y],
        3 => [-w, 0.0, 0.0, -h, x + w, y + h],
        4 => [w, 0.0, 0.0, -h, x, y + h],
        5 => [0.0, -h, -w, 0.0, x + w, y + h],
        6 => [0.0, -h, w, 0.0, x, y + h],
        7 => [0.0, h, w, 0.0, x, y],
        8 => [0.0, h, -w, 0.0, x + w, y],
        _ => [w, 0.0, 0.0, h, x, y],
    }
}

/// Text encoding and measuring for the built-in Helvetica, remembering what it couldn't show.
#[derive(Default)]
struct Text {
    replaced: BTreeSet<char>,
}

impl Text {
    /// `s` in WinAnsi; characters it lacks become `?` and are remembered.
    fn encode(&mut self, s: &str) -> Vec<u8> {
        s.chars()
            .map(|c| {
                win_ansi(c).unwrap_or_else(|| {
                    if !c.is_control() {
                        self.replaced.insert(c);
                    }
                    b'?'
                })
            })
            .collect()
    }

    fn width(&self, s: &str, bold: bool, size: f32) -> f32 {
        s.chars().map(|c| glyph_width(win_ansi(c).unwrap_or(b'?'), bold)).sum::<f32>() * size / 1000.0
    }

    /// `s` cut to fit `max` points, ending in "…" when cut.
    fn clip(&self, s: &str, bold: bool, size: f32, max: f32) -> String {
        if self.width(s, bold, size) <= max {
            return s.to_string();
        }
        let budget = max - self.width("…", bold, size);
        let mut used = 0.0;
        let mut out = String::new();
        for c in s.chars() {
            let cw = glyph_width(win_ansi(c).unwrap_or(b'?'), bold) * size / 1000.0;
            if used + cw > budget {
                break;
            }
            used += cw;
            out.push(c);
        }
        out.push('…');
        out
    }

    /// `s` broken into lines of at most `max` points, at spaces where it can, at most `limit`
    /// lines (the last cut with "…").
    fn wrap(&self, s: &str, bold: bool, size: f32, max: f32, limit: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        let mut cur = String::new();
        for word in s.split_whitespace() {
            let next = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
            if self.width(&next, bold, size) <= max {
                cur = next;
                continue;
            }
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            // A word longer than a line is split where it overflows.
            let mut w = word.to_string();
            while self.width(&w, bold, size) > max && lines.len() <= limit {
                let head = self.clip(&w, bold, size, max);
                let head = head.trim_end_matches('…').to_string();
                if head.is_empty() {
                    break;
                }
                w = w.chars().skip(head.chars().count()).collect();
                lines.push(head);
            }
            cur = w;
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
        if lines.len() > limit {
            lines.truncate(limit);
            if let Some(last) = lines.last_mut() {
                *last = self.clip(&format!("{last} …"), bold, size, max);
            }
        }
        lines
    }
}

/// The Windows-1252 byte for `c`, which the PDF's built-in fonts can show.
fn win_ansi(c: char) -> Option<u8> {
    let u = c as u32;
    match u {
        0x20..=0x7E | 0xA0..=0xFF => u8::try_from(u).ok(),
        _ => {
            let b = match c {
                '€' => 0x80,
                '‚' => 0x82,
                'ƒ' => 0x83,
                '„' => 0x84,
                '…' => 0x85,
                '†' => 0x86,
                '‡' => 0x87,
                'ˆ' => 0x88,
                '‰' => 0x89,
                'Š' => 0x8A,
                '‹' => 0x8B,
                'Œ' => 0x8C,
                'Ž' => 0x8E,
                '‘' => 0x91,
                '’' => 0x92,
                '“' => 0x93,
                '”' => 0x94,
                '•' => 0x95,
                '–' => 0x96,
                '—' => 0x97,
                '˜' => 0x98,
                '™' => 0x99,
                'š' => 0x9A,
                '›' => 0x9B,
                'œ' => 0x9C,
                'ž' => 0x9E,
                'Ÿ' => 0x9F,
                '\t' => b' ',
                _ => return None,
            };
            Some(b)
        }
    }
}

/// Helvetica and Helvetica-Bold advance widths (per 1000 em) for printable ASCII, from the
/// standard AFM files; other characters use 556 (a digit's width), close for Latin letters.
fn glyph_width(b: u8, bold: bool) -> f32 {
    const REG: [u16; 95] = [
        278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556, 556, 556, 556, 556, 556,
        278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667,
        611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833,
        556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
    ];
    const BOLD: [u16; 95] = [
        278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556, 556, 556, 556, 556, 556,
        333, 333, 584, 584, 584, 611, 975, 722, 722, 722, 722, 667, 611, 778, 722, 278, 556, 722, 611, 833, 722, 778, 667, 778, 722, 667,
        611, 722, 667, 944, 667, 667, 611, 333, 278, 333, 584, 556, 333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556, 278, 889,
        611, 611, 611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584,
    ];
    let table = if bold { &BOLD } else { &REG };
    let w = match b {
        0x20..=0x7E => table.get(usize::from(b - 0x20)).copied().unwrap_or(556),
        0x85 => 1000,
        _ => 556,
    };
    f32::from(w)
}

fn rgb(c: Color) -> [f32; 3] {
    [f32::from(c.0[0]) / 255.0, f32::from(c.0[1]) / 255.0, f32::from(c.0[2]) / 255.0]
}

fn severity_colour(s: Severity) -> [f32; 3] {
    match s {
        Severity::None => [0.42, 0.45, 0.5],
        Severity::Low => [0.09, 0.64, 0.29],
        Severity::Medium => [0.85, 0.47, 0.02],
        Severity::High => [0.92, 0.35, 0.05],
        Severity::Critical => [0.86, 0.15, 0.15],
    }
}

/// The page being laid out: canvases so far and the cursor on the last one.
struct Layout {
    w: f32,
    h: f32,
    pages: Vec<Canvas>,
    y: f32,
    text: Text,
}

impl Layout {
    fn canvas(&mut self) -> &mut Canvas {
        if self.pages.is_empty() {
            self.pages.push(Canvas::new(self.h));
        }
        let last = self.pages.len() - 1;
        &mut self.pages[last]
    }

    /// The bottom of the space content may use (above the footer).
    fn floor(&self) -> f32 {
        self.h - MARGIN - FOOTER_H
    }

    fn inner_w(&self) -> f32 {
        self.w - 2.0 * MARGIN
    }

    /// Starts a continuation page, repeating the header bar.
    fn new_page(&mut self, page: &Page) {
        self.pages.push(Canvas::new(self.h));
        self.y = MARGIN;
        self.header_bar(page);
    }

    fn draw(
        &mut self,
        page: &Page,
        photos: &[PlacedPhoto],
        logo: Option<&(String, (u32, u32), u8)>,
        map: Option<&(String, (u32, u32), u8)>,
    ) {
        // Title, with the logo at the top right.
        let logo_w = if logo.is_some() { LOGO_BOX.0 + GAP } else { 0.0 };
        if let Some((name, shown, o)) = logo {
            let bx = Bounds { x: self.w - MARGIN - LOGO_BOX.0, y: MARGIN, w: LOGO_BOX.0, h: LOGO_BOX.1 };
            self.canvas().image_fit(name, *shown, *o, bx);
        }
        let title_w = self.inner_w() - 2.0 * logo_w;
        let title = self.text.clip(&page.title, true, TITLE_SIZE, title_w);
        let (cx, y) = (self.w / 2.0, MARGIN + 26.0);
        let mut t = std::mem::take(&mut self.text);
        self.canvas().text_centered(&mut t, &title, cx, y, TITLE_SIZE, true, DARK);
        self.text = t;
        self.y = MARGIN + LOGO_BOX.1 + GAP;
        self.header_bar(page);
        self.photo_grid(page, photos);
        for s in &page.sections {
            self.section(page, &s.title, &s.rows);
        }
        if let Some((name, shown, o)) = map {
            if self.y + MAP_H + CAPTION_H > self.floor() {
                self.new_page(page);
            }
            let bx = Bounds { x: MARGIN, y: self.y, w: self.inner_w(), h: MAP_H };
            let c = self.canvas();
            c.rect_fill(bx.x, bx.y, bx.w, bx.h, LABEL_BG);
            c.image_fit(name, *shown, *o, bx);
            c.rect_stroke(bx.x, bx.y, bx.w, bx.h, 0.5, RULE);
            let mut t = std::mem::take(&mut self.text);
            let cx = self.w / 2.0;
            self.canvas().text_centered(&mut t, "Location map", cx, bx.y + MAP_H + 10.0, TEXT, true, DARK);
            self.text = t;
            self.y += MAP_H + CAPTION_H + GAP;
        }
    }

    /// The dark "LINE | TOWER | CKT" bar, with the severity badge at its right.
    fn header_bar(&mut self, page: &Page) {
        let (x, y, w) = (MARGIN, self.y, self.inner_w());
        let accent = rgb(page.accent);
        let mut t = std::mem::take(&mut self.text);
        let c = self.canvas();
        c.rect_fill(x, y, w, BAR_H, accent);
        let mut badge_w = 0.0;
        if let Some(s) = page.severity {
            let label = s.as_str().to_uppercase();
            let bw = t.width(&label, true, 8.0) + 12.0;
            let (bx, by) = (x + w - bw - 4.0, y + 4.0);
            c.rect_fill(bx, by, bw, BAR_H - 8.0, severity_colour(s));
            c.text_centered(&mut t, &label, bx + bw / 2.0, by + 10.5, 8.0, true, WHITE);
            badge_w = bw + 8.0;
        }
        let header = t.clip(&page.header, true, 11.0, w - 2.0 * badge_w - 8.0);
        c.text_centered(&mut t, &header, x + w / 2.0, y + 15.0, 11.0, true, WHITE);
        self.text = t;
        self.y += BAR_H + GAP;
    }

    /// The 2×2 photo grid (one row when two or fewer), with the neighbours column at the right.
    fn photo_grid(&mut self, page: &Page, photos: &[PlacedPhoto]) {
        if photos.is_empty() {
            return;
        }
        let nav = page.previous.is_some() || page.next.is_some();
        let grid_w = self.inner_w() - if nav { NAV_W + GAP } else { 0.0 };
        let cols = if photos.len() == 1 { 1 } else { 2 };
        let rows = photos.len().div_ceil(2);
        let cell_w = (grid_w - GAP * (cols as f32 - 1.0)) / cols as f32;
        // Two rows of photos fill about half of the space below the header.
        let avail = self.floor() - self.y;
        let img_h = ((avail * 0.55) / rows as f32 - CAPTION_H - GAP).clamp(90.0, 260.0);
        let top = self.y;
        for (i, p) in photos.iter().enumerate() {
            let (col, row) = (i % 2, i / 2);
            let bx =
                Bounds { x: MARGIN + col as f32 * (cell_w + GAP), y: top + row as f32 * (img_h + CAPTION_H + GAP), w: cell_w, h: img_h };
            self.photo_cell(p, bx);
        }
        let grid_h = rows as f32 * (img_h + CAPTION_H + GAP) - GAP;
        if nav {
            self.neighbours(page, Bounds { x: MARGIN + grid_w + GAP, y: top, w: NAV_W, h: grid_h });
        }
        self.y = top + grid_h + GAP * 1.5;
    }

    fn photo_cell(&mut self, p: &PlacedPhoto, bx: Bounds) {
        let mut t = std::mem::take(&mut self.text);
        let c = self.canvas();
        c.rect_fill(bx.x, bx.y, bx.w, bx.h, LABEL_BG);
        match &p.image {
            Ok(Some((name, shown, o))) => {
                let shown_box = c.image_fit(name, *shown, *o, bx);
                draw_markup(c, &mut t, p.markup, shown_box);
            }
            Ok(None) => {
                c.text_centered(&mut t, "No image", bx.x + bx.w / 2.0, bx.y + bx.h / 2.0, 10.0, true, GREY);
            }
            Err(issue) => {
                c.text_centered(&mut t, "No image", bx.x + bx.w / 2.0, bx.y + bx.h / 2.0 - 4.0, 10.0, true, GREY);
                let why = t.clip(&issue.to_string(), false, 7.0, bx.w - 8.0);
                c.text_centered(&mut t, &why, bx.x + bx.w / 2.0, bx.y + bx.h / 2.0 + 8.0, 7.0, false, GREY);
            }
        }
        c.rect_stroke(bx.x, bx.y, bx.w, bx.h, 0.5, RULE);
        let cap = t.clip(&p.caption, true, 9.0, bx.w);
        c.text_centered(&mut t, &cap, bx.x + bx.w / 2.0, bx.y + bx.h + 10.5, 9.0, true, DARK);
        self.text = t;
    }

    /// Previous asset, an up arrow, this asset, a down arrow, the next asset.
    fn neighbours(&mut self, page: &Page, b: Bounds) {
        let mut t = std::mem::take(&mut self.text);
        let c = self.canvas();
        c.rect_stroke(b.x, b.y, b.w, b.h, 0.5, RULE);
        let cx = b.x + b.w / 2.0;
        let mid = b.y + b.h / 2.0;
        let label = |t: &Text, s: &str, bold: bool, size: f32| t.clip(s, bold, size, b.w - 6.0);
        if let Some(prev) = &page.previous {
            let s = label(&t, prev, false, 8.5);
            c.text_centered(&mut t, &s, cx, mid - 34.0, 8.5, false, GREY);
            triangle(c, cx, mid - 22.0, 5.0, true, GREY);
        }
        let cur = label(&t, &page.label, true, 11.0);
        c.text_centered(&mut t, &cur, cx, mid + 4.0, 11.0, true, DARK);
        if let Some(next) = &page.next {
            triangle(c, cx, mid + 16.0, 5.0, false, GREY);
            let s = label(&t, next, false, 8.5);
            c.text_centered(&mut t, &s, cx, mid + 34.0, 8.5, false, GREY);
        }
        self.text = t;
    }

    /// A titled section: label/value pairs two to a row, full-width values on their own row.
    fn section(&mut self, page: &Page, title: &str, rows: &[PageRow]) {
        if rows.is_empty() {
            return;
        }
        let w = self.inner_w();
        let (lw, vw) = (w * 0.18, w * 0.32);
        // A section heading never sits alone at the bottom of a page.
        if self.y + 2.0 * ROW_H > self.floor() {
            self.new_page(page);
        }
        let accent = rgb(page.accent);
        let mut t = std::mem::take(&mut self.text);
        let y = self.y;
        let c = self.canvas();
        c.rect_fill(MARGIN, y, w, ROW_H, accent);
        let heading = t.clip(title, true, 9.0, w - 12.0);
        c.text_at(&mut t, &heading, MARGIN + 6.0, y + 11.5, 9.0, true, WHITE);
        self.text = t;
        self.y += ROW_H;

        let mut i = 0;
        while i < rows.len() {
            let row = &rows[i];
            if row.wide {
                let lines = self.text.wrap(&row.value, false, TEXT, w - lw - 8.0, MAX_WIDE_LINES);
                let h = ROW_H.max(lines.len() as f32 * LINE + 5.5);
                self.ensure(page, h);
                self.cell_pair(&row.label, &lines, MARGIN, lw, w - lw, h);
                self.y += h;
                i += 1;
                continue;
            }
            self.ensure(page, ROW_H);
            let v = vec![self.text.clip(&row.value, false, TEXT, vw - 8.0)];
            self.cell_pair(&row.label, &v, MARGIN, lw, vw, ROW_H);
            // The second pair on this row, unless the next row is wide (then the cells stay empty).
            match rows.get(i + 1).filter(|r| !r.wide) {
                Some(r2) => {
                    let v2 = vec![self.text.clip(&r2.value, false, TEXT, vw - 8.0)];
                    self.cell_pair(&r2.label, &v2, MARGIN + lw + vw, lw, vw, ROW_H);
                    i += 2;
                }
                None => {
                    let y = self.y;
                    let c = self.canvas();
                    c.rect_stroke(MARGIN + lw + vw, y, lw + vw, ROW_H, 0.5, RULE);
                    i += 1;
                }
            }
            self.y += ROW_H;
        }
        self.y += GAP;
    }

    /// Room for `h` more points on this page, or a new page.
    fn ensure(&mut self, page: &Page, h: f32) {
        if self.y + h > self.floor() {
            self.new_page(page);
        }
    }

    fn cell_pair(&mut self, label: &str, value: &[String], x: f32, lw: f32, vw: f32, h: f32) {
        let mut t = std::mem::take(&mut self.text);
        let y = self.y;
        let c = self.canvas();
        c.rect_fill(x, y, lw, h, LABEL_BG);
        c.rect_stroke(x, y, lw, h, 0.5, RULE);
        c.rect_stroke(x + lw, y, vw, h, 0.5, RULE);
        let l = t.clip(label, true, TEXT, lw - 8.0);
        c.text_at(&mut t, &l, x + 4.0, y + 11.0, TEXT, true, DARK);
        for (k, line) in value.iter().enumerate() {
            c.text_at(&mut t, line, x + lw + 4.0, y + 11.0 + k as f32 * LINE, TEXT, false, DARK);
        }
        self.text = t;
    }
}

/// A small filled triangle pointing up or down, centred on (cx, cy).
fn triangle(c: &mut Canvas, cx: f32, cy: f32, r: f32, up: bool, col: [f32; 3]) {
    c.fill(col);
    let y = c.h - cy;
    let (tip, base) = if up { (y + r, y - r) } else { (y - r, y + r) };
    c.content.move_to(cx, tip).line_to(cx - r, base).line_to(cx + r, base).close_path().fill_nonzero();
}

/// Draws photo markup over the photo shown in `b` (top-left coordinates), clipped to it.
fn draw_markup(c: &mut Canvas, t: &mut Text, set: &AnnotationSet, b: Bounds) {
    if set.is_empty() {
        return;
    }
    let short = b.w.min(b.h);
    let px = |x: f32| b.x + x * b.w;
    let h = c.h;
    let py = |y: f32| h - (b.y + y * b.h);
    c.content.save_state();
    c.content.rect(b.x, h - b.y - b.h, b.w, b.h).clip_nonzero().end_path();
    c.content.set_line_cap(LineCapStyle::RoundCap).set_line_join(LineJoinStyle::RoundJoin);
    for a in set.shapes() {
        draw_shape(c, t, a, short, &px, &py);
    }
    c.content.restore_state();
}

fn draw_shape(c: &mut Canvas, t: &mut Text, a: &Annotation, short: f32, px: &dyn Fn(f32) -> f32, py: &dyn Fn(f32) -> f32) {
    let col = rgb(a.color);
    let lw = (a.width / 1000.0 * short).max(0.3);
    c.content.save_state();
    if a.color.0[3] < 255 {
        let gs = format!("GS{}", a.color.0[3]);
        c.content.set_parameters(Name(gs.as_bytes()));
    }
    c.stroke_rgb(col);
    c.fill(col);
    c.content.set_line_width(lw);
    match &a.shape {
        Shape::Pen { points } => {
            let mut it = points.iter();
            if let Some(p0) = it.next() {
                c.content.move_to(px(p0.x), py(p0.y));
                for p in it {
                    c.content.line_to(px(p.x), py(p.y));
                }
                c.content.stroke();
            }
        }
        Shape::Arrow { from, to } => {
            let (x1, y1, x2, y2) = (px(from.x), py(from.y), px(to.x), py(to.y));
            let (dx, dy) = (x2 - x1, y2 - y1);
            let len = (dx * dx + dy * dy).sqrt().max(0.001);
            let (ux, uy) = (dx / len, dy / len);
            let head = (lw * 4.5).min(len * 0.6);
            let (bx, by) = (x2 - ux * head, y2 - uy * head);
            c.content.move_to(x1, y1).line_to(bx, by).stroke();
            let (nx, ny) = (-uy * head * 0.5, ux * head * 0.5);
            c.content.move_to(x2, y2).line_to(bx + nx, by + ny).line_to(bx - nx, by - ny).close_path().fill_nonzero();
        }
        Shape::Rect { a: p, b: q } => {
            let (x, y) = (px(p.x.min(q.x)), py(p.y.max(q.y)));
            let (w, hh) = ((px(q.x) - px(p.x)).abs(), (py(q.y) - py(p.y)).abs());
            c.content.rect(x, y, w, hh).stroke();
        }
        Shape::Ellipse { a: p, b: q } => {
            let (cx, cy) = ((px(p.x) + px(q.x)) / 2.0, (py(p.y) + py(q.y)) / 2.0);
            let (rx, ry) = ((px(q.x) - px(p.x)).abs() / 2.0, (py(q.y) - py(p.y)).abs() / 2.0);
            ellipse(&mut c.content, cx, cy, rx, ry);
            c.content.stroke();
        }
        Shape::Text { at, text, size } => {
            let cap = size / 1000.0 * short;
            let fs = cap / 0.718;
            let bytes = t.encode(text);
            c.content.begin_text().set_font(F_BOLD, fs).next_line(px(at.x), py(at.y) - cap).show(Str(&bytes)).end_text();
        }
        Shape::Marker { at, number } => {
            let r = (lw * 2.2).max(short * 0.045);
            let (cx, cy) = (px(at.x), py(at.y));
            ellipse(&mut c.content, cx, cy, r, r);
            c.content.fill_nonzero();
            let s = number.to_string();
            let fs = r * 1.15;
            let w = t.width(&s, true, fs);
            c.content.set_fill_rgb(1.0, 1.0, 1.0);
            c.content.begin_text().set_font(F_BOLD, fs).next_line(cx - w / 2.0, cy - fs * 0.36).show(Str(s.as_bytes())).end_text();
        }
    }
    c.content.restore_state();
}

/// An ellipse path from four Bézier curves.
fn ellipse(c: &mut Content, cx: f32, cy: f32, rx: f32, ry: f32) {
    const K: f32 = 0.552_284_8;
    let (ox, oy) = (rx * K, ry * K);
    c.move_to(cx + rx, cy);
    c.cubic_to(cx + rx, cy + oy, cx + ox, cy + ry, cx, cy + ry);
    c.cubic_to(cx - ox, cy + ry, cx - rx, cy + oy, cx - rx, cy);
    c.cubic_to(cx - rx, cy - oy, cx - ox, cy - ry, cx, cy - ry);
    c.cubic_to(cx + ox, cy - ry, cx + rx, cy - oy, cx + rx, cy);
    c.close_path();
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn orientation_matrices_put_the_stored_top_left_where_exif_says() {
        // Stored top-left pixel is unit-square (0, 1). Where does it land in a 10×20 box at (0,0)?
        let at = |o: u8| {
            let m = orient_matrix(o, 0.0, 0.0, 10.0, 20.0);
            (m[2] + m[4], m[3] + m[5]) // u = 0, v = 1
        };
        assert_eq!(at(1), (0.0, 20.0)); // top left
        assert_eq!(at(3), (10.0, 0.0)); // rotated 180°: bottom right
        assert_eq!(at(6), (10.0, 20.0)); // rotated 90° clockwise: top right
        assert_eq!(at(8), (0.0, 0.0)); // rotated 90° anticlockwise: bottom left
    }

    #[test]
    fn text_encodes_latin_and_reports_the_rest() {
        let mut t = Text::default();
        assert_eq!(t.encode("Tower 76/0 – 35 °C"), b"Tower 76/0 \x96 35 \xb0C".to_vec());
        assert_eq!(t.encode("टावर A"), b"???? A".to_vec());
        assert_eq!(t.replaced.len(), 4);
    }

    #[test]
    fn long_text_is_clipped_and_wrapped() {
        let t = Text::default();
        let s = t.clip("A very long description of the fault", false, 10.0, 60.0);
        assert!(s.ends_with('…') && t.width(&s, false, 10.0) <= 60.0 + 0.01);
        let lines = t.wrap("one two three four five six seven eight nine ten", false, 10.0, 50.0, 3);
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().all(|l| t.width(l, false, 10.0) <= 50.0 + 0.01));
        assert!(t.wrap(&"x".repeat(500), false, 10.0, 50.0, 4).len() <= 4, "an unbreakable word still stops");
    }

    #[test]
    fn fit_keeps_proportions() {
        let b = fit((4000, 3000), Bounds { x: 0.0, y: 0.0, w: 200.0, h: 200.0 });
        assert_eq!((b.w, b.h, b.y), (200.0, 150.0, 25.0));
    }
}
