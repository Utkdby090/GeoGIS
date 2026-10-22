//! Tower PDFs and line ZIPs made with GeoGIS's own report code (`geoflux-report`), from the
//! towers exactly as they were uploaded. The default template for the survey's schema is used.

use std::sync::Arc;

use geoflux_cloud::Sha256;
use geoflux_cloud::ports::{ReportFailure, ReportRenderer, ZipOut};
use geoflux_cloud::sync::SurveyAsset;
use geoflux_domain::DomainSchema;
use geoflux_report::{BulkEntry, CellPhoto, Image, PageInput, PhotoIssue, PlannedPage, Progress, ReportTemplate, plan_page};

/// Most towers in one line ZIP (the report crate's own limit for a bulk export).
pub const MAX_LINE_TOWERS: usize = geoflux_report::MAX_BULK_PAGES;

/// Reports made at once; the rest wait their turn. Each holds about one tower's photos and its PDF.
pub const REPORTS_AT_ONCE: usize = 2;

/// The production renderer.
#[derive(Clone, Copy, Debug, Default)]
pub struct Reports;

fn schema(id: &str) -> Result<(DomainSchema, ReportTemplate), ReportFailure> {
    let schema = geoflux_domain::builtin_schema(id)
        .ok_or_else(|| ReportFailure::UnknownSchema(id.to_string()))?
        .map_err(|e| ReportFailure::Render(format!("schema {id}: {e}")))?;
    let template = ReportTemplate::for_schema(&schema);
    Ok((schema, template))
}

fn label(schema: &DomainSchema, tower: Option<&SurveyAsset>) -> Option<String> {
    tower.map(|t| t.asset.property(schema.label_key()).trim().to_string()).filter(|l| !l.is_empty())
}

/// One tower's page with its photos read through `photo`. The first photo listed for a slot is the
/// one shown, as on the desktop.
fn page(
    schema: &DomainSchema,
    template: &ReportTemplate,
    tower: &SurveyAsset,
    previous: Option<&SurveyAsset>,
    next: Option<&SurveyAsset>,
    photo: &mut dyn FnMut(&Sha256) -> Option<Vec<u8>>,
) -> BulkEntry {
    let input = PageInput {
        schema,
        template,
        asset: &tower.asset,
        severity: tower.asset.severity,
        previous: label(schema, previous),
        next: label(schema, next),
        logo: None,
    };
    let PlannedPage { path, mut page, photos: keys } = plan_page(input, &mut |slot| {
        let p = tower.photos.iter().find(|p| p.slot == slot)?;
        Some(CellPhoto { key: p.sha256.as_str().to_string(), markup: p.markup.clone().unwrap_or_default() })
    });
    for (cell, key) in page.photos.iter_mut().zip(keys) {
        let Some(key) = key else { continue };
        cell.image = match Sha256::parse(&key).ok().and_then(|h| photo(&h)) {
            Some(bytes) => Image::from_bytes(bytes).map(Some),
            None => Err(PhotoIssue::Unreadable("not in storage".into())),
        };
    }
    BulkEntry { path, page }
}

fn render(e: geoflux_report::ReportError) -> ReportFailure {
    ReportFailure::Render(e.to_string())
}

impl ReportRenderer for Reports {
    fn tower_pdf(
        &self,
        schema_id: &str,
        tower: &SurveyAsset,
        previous: Option<&SurveyAsset>,
        next: Option<&SurveyAsset>,
        photo: &mut dyn FnMut(&Sha256) -> Option<Vec<u8>>,
    ) -> Result<Vec<u8>, ReportFailure> {
        let (schema, template) = schema(schema_id)?;
        let entry = page(&schema, &template, tower, previous, next, photo);
        let mut out = Vec::new();
        geoflux_report::write_pdf(&entry.page, &mut out).map_err(render)?;
        Ok(out)
    }

    fn line_zip(
        &self,
        schema_id: &str,
        towers: &[SurveyAsset],
        photo: &mut dyn FnMut(&Sha256) -> Option<Vec<u8>>,
        out: &mut dyn ZipOut,
    ) -> Result<u32, ReportFailure> {
        if towers.len() > MAX_LINE_TOWERS {
            return Err(ReportFailure::TooManyTowers(MAX_LINE_TOWERS));
        }
        let (schema, template) = schema(schema_id)?;
        // Pages are made one at a time as the ZIP asks for them, so only one tower's photos are
        // in memory at once.
        let entries = towers.iter().enumerate().map(|(i, t)| {
            let previous = i.checked_sub(1).and_then(|j| towers.get(j));
            page(&schema, &template, t, previous, towers.get(i + 1), &mut *photo)
        });
        let progress: Arc<Progress> = Progress::new();
        let summary = geoflux_report::write_bulk(out, towers.len(), entries, &progress).map_err(render)?;
        Ok(u32::try_from(summary.pages).unwrap_or(u32::MAX))
    }
}
