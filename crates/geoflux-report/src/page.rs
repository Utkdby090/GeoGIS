//! One report page with every value resolved: what the PDF writer draws, with no schema, project
//! or file system behind it. The engine builds pages from an asset and its template; tests build
//! them by hand.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use geoflux_domain::Severity;
use geoflux_domain::annotation::{AnnotationSet, Color};

use crate::template::PageSize;
use crate::{PhotoIssue, ReportError};

/// The largest photo file read into a report (a DJI R-JPEG is 1 to 15 MB).
pub const MAX_PHOTO_BYTES: usize = 40 * 1024 * 1024;
/// The largest logo or map image.
pub const MAX_SMALL_IMAGE_BYTES: usize = 8 * 1024 * 1024;
/// Most pixels in an image (width × height), so a hostile header can't ask for a huge buffer.
pub const MAX_PIXELS: u64 = 200_000_000;
/// Most pixels in a PNG, which (unlike a JPEG) is decoded: 40 MP is 160 MB of RGBA.
pub const MAX_PNG_PIXELS: u64 = 40_000_000;

/// How an image's bytes go into the PDF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageKind {
    /// Embedded as it is (DCT), never decoded: `components` is 1 (grey), 3 (colour) or 4 (CMYK);
    /// `inverted` marks Adobe CMYK, which PDF readers need told to invert.
    Jpeg { components: u8, inverted: bool },
    /// Decoded and re-compressed losslessly (PDF has no PNG filter), with alpha as a soft mask.
    Png,
}

/// What a header says about an image, read without decoding it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageInfo {
    pub kind: ImageKind,
    /// The stored size, before orientation.
    pub width: u32,
    pub height: u32,
    /// The EXIF orientation (1 to 8; 1 when absent), applied when the image is placed.
    pub orientation: u8,
}

impl ImageInfo {
    /// The size as shown, after orientation (5 to 8 swap width and height).
    pub fn shown_size(&self) -> (u32, u32) {
        if (5..=8).contains(&self.orientation) { (self.height, self.width) } else { (self.width, self.height) }
    }
}

/// An image ready to place: its bytes (shared, so a logo is read once for a whole export) and header.
#[derive(Clone, Debug)]
pub struct Image {
    pub bytes: Arc<[u8]>,
    pub info: ImageInfo,
}

/// One cell of the photo grid.
#[derive(Clone, Debug)]
pub struct PagePhoto {
    pub caption: String,
    /// The photo, or why it isn't there (no photo linked is `Ok(None)`: the cell says "No image").
    pub image: Result<Option<Image>, PhotoIssue>,
    /// Markup drawn over the photo as vectors.
    pub annotations: AnnotationSet,
}

/// One row of a section: a label and its value, or a value across the whole row.
#[derive(Clone, Debug, PartialEq)]
pub struct PageRow {
    pub label: String,
    pub value: String,
    pub wide: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageSection {
    pub title: String,
    pub rows: Vec<PageRow>,
}

/// A page with every value filled in.
#[derive(Clone, Debug)]
pub struct Page {
    pub size: PageSize,
    pub title: String,
    /// The header bar's text ("LINE A | 76/0 | CKT 1").
    pub header: String,
    pub severity: Option<Severity>,
    /// The asset's label, and the previous and next ones along the line.
    pub label: String,
    pub previous: Option<String>,
    pub next: Option<String>,
    pub photos: Vec<PagePhoto>,
    pub sections: Vec<PageSection>,
    pub map: Option<Image>,
    pub logo: Option<Image>,
    pub accent: Color,
    pub footer: String,
}

/// One PDF of a bulk export: where it goes in the ZIP, and its page.
#[derive(Clone, Debug)]
pub struct BulkEntry {
    /// `folder/name.pdf`, already made safe.
    pub path: String,
    pub page: Page,
}

/// A page whose photos are still paths: the engine plans every page of an export up front (cheap:
/// text only), and the export reads one page's photos at a time with [`PlannedPage::load`].
#[derive(Clone, Debug)]
pub struct PlannedPage {
    /// Where the PDF goes in a bulk export (`folder/name.pdf`).
    pub path: String,
    /// The page, its photo cells still empty.
    pub page: Page,
    /// Each photo cell's file, in the order of `page.photos`; None when the slot has no photo.
    pub photos: Vec<Option<String>>,
}

impl PlannedPage {
    /// Reads the page's photos through `source`; one that can't be read becomes "No image".
    pub fn load(mut self, source: &dyn PhotoSource) -> BulkEntry {
        for (cell, path) in self.page.photos.iter_mut().zip(&self.photos) {
            if let Some(path) = path {
                cell.image = source.read(path, MAX_PHOTO_BYTES).and_then(Image::from_bytes).map(Some);
            }
        }
        BulkEntry { path: self.path, page: self.page }
    }
}

/// What a written report left out or changed, for the person to check.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    /// `(asset label, caption, why)` for each photo left out.
    pub photos_missing: Vec<(String, String, PhotoIssue)>,
    /// Characters the PDF's built-in font can't show, replaced with `?` (Latin text shows as is).
    pub replaced_chars: Vec<char>,
    pub pages: usize,
    pub bytes: u64,
}

/// Reads photo, logo and map files for a report. The engine reads from disk; tests and the browser
/// pass bytes they already hold. Must not read more than `max_bytes`.
pub trait PhotoSource: Sync {
    fn read(&self, path: &str, max_bytes: usize) -> Result<Vec<u8>, PhotoIssue>;
}

/// Progress of a bulk export, shared between the worker writing it and the window showing it.
#[derive(Debug, Default)]
pub struct Progress {
    done: AtomicUsize,
    total: AtomicUsize,
    cancel: AtomicBool,
}

impl Progress {
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    /// `(done, total)` pages.
    pub fn get(&self) -> (usize, usize) {
        (self.done.load(Ordering::Acquire), self.total.load(Ordering::Acquire))
    }

    /// Asks the export to stop after the page it is on.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }

    pub fn start(&self, total: usize) {
        self.total.store(total, Ordering::Release);
        self.done.store(0, Ordering::Release);
    }

    /// One more page written (called by the export worker); an error when the person cancelled.
    pub fn step(&self) -> Result<(), ReportError> {
        self.done.fetch_add(1, Ordering::AcqRel);
        if self.is_cancelled() { Err(ReportError::Cancelled) } else { Ok(()) }
    }
}
