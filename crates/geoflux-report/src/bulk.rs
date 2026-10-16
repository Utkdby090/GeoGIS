//! Bulk export: one PDF per asset, written into a ZIP one page at a time.
//!
//! Pages come from an iterator, so the caller builds (and reads the photos of) one page only when
//! the ZIP is ready for it: memory stays at about one page's photos plus its PDF, whatever the
//! number of assets. PDFs are stored, not deflated again (their photos are JPEG already).

use std::collections::HashSet;
use std::io::{Seek, Write};

use zip::CompressionMethod;
use zip::write::{SimpleFileOptions, ZipWriter};

use crate::pdf::render;
use crate::template::safe_name;
use crate::{BulkEntry, Progress, ReportError, Summary};

/// Most PDFs in one bulk export.
pub const MAX_BULK_PAGES: usize = 20_000;

/// Writes every entry as a PDF into a ZIP on `out`, stepping `progress` after each one. `total` is
/// the number of entries expected (for the progress bar). Two entries with the same path (ignoring
/// case, as Windows does) get " (2)", " (3)"… before `.pdf`.
///
/// Stops with [`ReportError::Cancelled`] when the person cancels; the caller then removes what was
/// written (the engine writes to a `.part` file and renames it only on success).
pub fn write_bulk<W, I>(out: W, total: usize, entries: I, progress: &Progress) -> Result<Summary, ReportError>
where
    W: Write + Seek,
    I: IntoIterator<Item = BulkEntry>,
{
    if total > MAX_BULK_PAGES {
        return Err(ReportError::TooLarge { what: "a bulk export", max: MAX_BULK_PAGES });
    }
    progress.start(total);
    let mut zip = ZipWriter::new(out);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored).large_file(false);
    let mut names = UniqueNames::default();
    let mut summary = Summary::default();
    for (n, entry) in entries.into_iter().enumerate() {
        if progress.is_cancelled() {
            return Err(ReportError::Cancelled);
        }
        if n >= MAX_BULK_PAGES {
            return Err(ReportError::TooLarge { what: "a bulk export", max: MAX_BULK_PAGES });
        }
        let path = names.claim(&entry.path);
        let mut page = Summary::default();
        let bytes = render(&entry.page, &mut page);
        zip.start_file(path, options).map_err(zip_error)?;
        zip.write_all(&bytes)?;
        summary.photos_missing.append(&mut page.photos_missing);
        summary.replaced_chars.extend(page.replaced_chars);
        summary.pages += page.pages;
        summary.bytes += bytes.len() as u64;
        progress.step()?;
    }
    summary.replaced_chars.sort_unstable();
    summary.replaced_chars.dedup();
    let mut out = zip.finish().map_err(zip_error)?;
    out.flush()?;
    Ok(summary)
}

fn zip_error(e: zip::result::ZipError) -> ReportError {
    match e {
        zip::result::ZipError::Io(e) => ReportError::Io(e),
        e => ReportError::Io(std::io::Error::other(e.to_string())),
    }
}

/// ZIP paths handed out so far, compared without case.
#[derive(Default)]
struct UniqueNames {
    taken: HashSet<String>,
}

impl UniqueNames {
    /// `path` made safe (each folder and the file name), or with " (n)" added when it is taken.
    fn claim(&mut self, path: &str) -> String {
        let mut parts: Vec<String> = path.split(['/', '\\']).filter(|p| !p.is_empty()).map(safe_name).collect();
        let file = parts.pop().unwrap_or_else(|| "report.pdf".to_string());
        let dir = parts.iter().map(|p| format!("{p}/")).collect::<String>();
        let (stem, ext) = match file.rsplit_once('.') {
            Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
            _ => (file.clone(), String::new()),
        };
        let mut candidate = format!("{dir}{file}");
        let mut k = 2;
        while !self.taken.insert(candidate.to_lowercase()) {
            candidate = format!("{dir}{stem} ({k}){ext}");
            k += 1;
        }
        candidate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_safe_and_unique() {
        let mut n = UniqueNames::default();
        assert_eq!(n.claim("FAULT/Tower_1_Report.pdf"), "FAULT/Tower_1_Report.pdf");
        assert_eq!(n.claim("fault/tower_1_report.pdf"), "fault/tower_1_report (2).pdf");
        assert_eq!(n.claim("FAULT/Tower_1_Report.pdf"), "FAULT/Tower_1_Report (3).pdf");
        let escaped = n.claim("../../etc/passwd");
        assert!(!escaped.contains(".."), "{escaped}");
    }
}
