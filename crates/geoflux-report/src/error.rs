//! Why a report could not be made.

use thiserror::Error;

/// Why a report (or one page of it) could not be written.
///
/// A missing or unreadable photo is not an error: the page shows "No image" in its place and the
/// [`crate::Summary`] lists it, so one bad file never stops a line's reports.
#[derive(Debug, Error)]
pub enum ReportError {
    /// The template doesn't check out (see [`crate::ReportTemplate::check`]).
    #[error("the report template is not usable: {0}")]
    Template(String),
    /// A value is over a limit that keeps a page or a report bounded.
    #[error("{what} is too large (at most {max})")]
    TooLarge { what: &'static str, max: usize },
    /// Writing the PDF or ZIP failed (disk full, folder gone, permission).
    #[error("could not write the report: {0}")]
    Io(#[from] std::io::Error),
    /// Two pages would be written to the same path in a bulk export.
    #[error("two reports would both be named {0}")]
    DuplicateName(String),
    /// The person cancelled a bulk export. What was written so far is removed.
    #[error("cancelled")]
    Cancelled,
}

/// Why one photo was left out of a page, reported but not fatal.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum PhotoIssue {
    #[error("the file could not be read: {0}")]
    Unreadable(String),
    #[error("not a JPEG or PNG image")]
    NotAnImage,
    #[error("the image is larger than {max_mb} MB")]
    TooLarge { max_mb: usize },
    #[error("the image is damaged: {0}")]
    Damaged(String),
}
