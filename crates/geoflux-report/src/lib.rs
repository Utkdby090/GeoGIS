//! GeoFlux L1: inspection reports.
//!
//! - [`template`]: what a report shows, as data ([`ReportTemplate`]), one per schema.
//! - [`page`]: one page with every value resolved ([`Page`]), the photos as their original bytes.
//!
//! The PDF is written with vectors: text stays selectable and searchable, lines stay sharp, photo
//! markup is drawn over the photo as shapes, and JPEG photos are embedded as the original files
//! (never decoded or re-compressed), so a page costs about the size of its photos in memory and on
//! disk. A bulk export writes one PDF per asset into a ZIP, one page at a time.
//!
//! Nothing here touches the file system or a UI: photos come through [`PhotoSource`], so this
//! builds for the browser too.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod bulk;
mod error;
mod image;
pub mod page;
mod pdf;
pub mod template;

pub use bulk::{MAX_BULK_PAGES, write_bulk};
pub use error::{PhotoIssue, ReportError};
pub use image::slim_jpeg;
pub use page::{
    BulkEntry, Image, ImageInfo, ImageKind, Page, PagePhoto, PageRow, PageSection, PhotoSource, PlannedPage, Progress, Summary,
};
pub use pdf::write_pdf;
pub use template::{Branding, FieldSection, PageSize, PhotoCell, ReportTemplate};

#[cfg(test)]
mod tests;
