//! One asset's page from its template: the same function for GeoGIS's reports and the web
//! server's, so a tower's PDF reads the same wherever it was made.
//!
//! Text only: the photos are listed by key and read later through a [`crate::PhotoSource`], one
//! page at a time ([`PlannedPage::load`]).

use geoflux_domain::annotation::AnnotationSet;
use geoflux_domain::{Asset, DomainSchema, Severity, form};

use crate::template::{expand, safe_name};
use crate::{Image, Page, PagePhoto, PageRow, PageSection, PlannedPage, ReportTemplate};

/// Everything about one asset's page other than its photos.
pub struct PageInput<'a> {
    pub schema: &'a DomainSchema,
    pub template: &'a ReportTemplate,
    pub asset: &'a Asset,
    pub severity: Severity,
    /// The labels of the towers before and after it in walking order, for the page's arrows.
    pub previous: Option<String>,
    pub next: Option<String>,
    pub logo: Option<Image>,
}

/// One photo cell's photo: the key its bytes are read by, and the markup drawn over it.
pub struct CellPhoto {
    pub key: String,
    pub markup: AnnotationSet,
}

/// The page for `input.asset`. `photo` gives each template photo cell's photo by slot id, or `None`
/// for an empty cell ("No image").
pub fn plan_page(input: PageInput<'_>, photo: &mut dyn FnMut(&str) -> Option<CellPhoto>) -> PlannedPage {
    let PageInput { schema, template: t, asset, severity, previous, next, logo } = input;
    let fields = schema.fields_for(asset);
    let values = form::values(asset, &fields);
    let label = asset.property(schema.label_key()).trim().to_string();
    let value = |k: &str| -> Option<String> {
        match k {
            "label" => Some(label.clone()),
            "severity" => Some(severity.as_str().to_string()),
            "status" => Some(if severity > Severity::None { "FAULT" } else { "NORMAL" }.to_string()),
            _ => values.get(k).cloned().or_else(|| asset.properties.contains(k).then(|| asset.property(k).to_string())),
        }
    };
    let text = |k: &str| value(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let header: Vec<String> = t.header.iter().filter_map(|k| text(k)).collect();
    let (previous, next) = if t.neighbours { (previous, next) } else { (None, None) };

    let mut photos = Vec::with_capacity(t.photos.len());
    let mut keys = Vec::with_capacity(t.photos.len());
    for cell in &t.photos {
        let caption = if cell.caption.trim().is_empty() {
            schema.image_slots.iter().find(|s| s.id == cell.slot).map_or_else(|| cell.slot.clone(), |s| s.label.clone())
        } else {
            cell.caption.clone()
        };
        let found = photo(&cell.slot);
        let (key, annotations) = match found {
            Some(p) => (Some(p.key), p.markup),
            None => (None, AnnotationSet::default()),
        };
        photos.push(PagePhoto { caption, image: Ok(None), annotations });
        keys.push(key);
    }
    let sections = t
        .sections
        .iter()
        .map(|sec| PageSection {
            title: sec.title.clone(),
            rows: sec
                .fields
                .iter()
                .map(|k| PageRow {
                    label: schema.field(k).map_or_else(|| k.clone(), |f| f.label.clone()),
                    value: text(k).unwrap_or_else(|| "N/A".into()),
                    wide: sec.wide.contains(k),
                })
                .collect(),
        })
        .collect();
    let company = if t.branding.company.trim().is_empty() { text("companyName").unwrap_or_default() } else { t.branding.company.clone() };
    let footer = expand(&t.branding.footer, |k| (k == "company").then(|| company.clone()).or_else(|| value(k)));
    let file = safe_name(&expand(&t.file_name, value));
    let folder = safe_name(&expand(&t.folder, value));
    let path = if t.folder.trim().is_empty() { file } else { format!("{folder}/{file}") };
    let page = Page {
        size: t.page,
        title: expand(&t.title, value),
        header: header.join(" | "),
        severity: (t.severity && schema.severity.is_some()).then_some(severity),
        label,
        previous,
        next,
        photos,
        sections,
        map: None,
        logo,
        accent: t.branding.accent,
        footer: footer.trim().to_string(),
    };
    PlannedPage { path, page, photos: keys }
}
