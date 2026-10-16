use geoflux_domain::builtin_schema;

use super::template::{expand, safe_name};
use super::*;

fn telecom() -> geoflux_domain::DomainSchema {
    builtin_schema("com.geo.telecom").expect("listed").expect("valid")
}

#[test]
fn builtin_templates_check_against_their_schemas() {
    for (id, json) in template::BUILTIN_TEMPLATES {
        let schema = builtin_schema(id).expect("a schema for every template").expect("valid");
        let t = ReportTemplate::from_json(json).expect("parses");
        t.check(&schema).expect("checks");
        assert_eq!(ReportTemplate::for_schema(&schema), t);
    }
}

#[test]
fn template_made_from_a_schema_checks() {
    let mut schema = telecom();
    schema.id = "com.example.other".into();
    let t = ReportTemplate::for_schema(&schema);
    t.check(&schema).expect("checks");
    assert_eq!(t.photos.len(), 4);
    assert_eq!(t.header, ["towerNumber"]);
    assert_eq!(
        t.sections.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(),
        ["Location & Temporal Data", "Environmental & Load Data", "Fault Analysis"]
    );
}

#[test]
fn check_names_what_is_wrong() {
    let schema = telecom();
    let mut t = ReportTemplate::for_schema(&schema);
    t.photos[0].slot = "NOPE".into();
    assert!(matches!(t.check(&schema), Err(ReportError::Template(m)) if m.contains("NOPE")));
    let mut t = ReportTemplate::for_schema(&schema);
    t.sections[0].fields.push("ghost".into());
    assert!(matches!(t.check(&schema), Err(ReportError::Template(m)) if m.contains("ghost")));
    let mut t = ReportTemplate::for_schema(&schema);
    t.file_name = "{label}.docx".into();
    assert!(t.check(&schema).is_err());
    assert!(ReportTemplate::from_json(r#"{"schemaId":"x","title":"t","header":[],"photos":[],"sections":[],"extra":1}"#).is_err());
}

#[test]
fn tokens_expand_and_unknown_ones_stay() {
    let v = |k: &str| match k {
        "label" => Some("76/0".to_string()),
        "status" => Some("FAULT".to_string()),
        _ => None,
    };
    assert_eq!(expand("{status}_{reportType}_{label}_Report.pdf", v), "FAULT_{reportType}_76/0_Report.pdf");
    assert_eq!(expand("open {brace", v), "open {brace");
    assert_eq!(expand("", v), "");
}

#[test]
fn names_are_safe_everywhere() {
    assert_eq!(safe_name("FAULT_Tower_76/0_Report.pdf"), "FAULT_Tower_76_0_Report.pdf");
    assert_eq!(safe_name("../../etc"), "_.._etc");
    assert_eq!(safe_name("..."), "_");
    assert_eq!(safe_name("CON.pdf"), "_CON.pdf");
    assert_eq!(safe_name("COM1"), "_COM1");
    assert_eq!(safe_name("a\u{0}b "), "a_b");
}

#[test]
fn progress_counts_and_cancels() {
    let p = Progress::new();
    p.start(3);
    assert!(p.step().is_ok());
    p.cancel();
    assert!(matches!(p.step(), Err(ReportError::Cancelled)));
    assert_eq!(p.get(), (2, 3));
}

#[test]
fn rotated_photos_swap_their_size() {
    let info = ImageInfo { kind: ImageKind::Png, width: 4000, height: 3000, orientation: 6 };
    assert_eq!(info.shown_size(), (3000, 4000));
}

mod pdf_output {
    use std::io::{Cursor, Read};

    use ::image::{ExtendedColorType, ImageEncoder};
    use geoflux_domain::Severity;
    use geoflux_domain::annotation::{Annotation, AnnotationSet, Color, NormPoint, Shape};

    use crate::*;

    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        let rgb: Vec<u8> = (0..w * h).flat_map(|i| [(i % 251) as u8, 90, 200]).collect();
        let mut out = Vec::new();
        ::image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 70)
            .write_image(&rgb, w, h, ExtendedColorType::Rgb8)
            .expect("encodes");
        out
    }

    /// A JPEG with a large APP1 segment after SOI, the way FLIR and DJI store raw thermal data.
    fn thermal_jpeg(w: u32, h: u32, extra: usize) -> Vec<u8> {
        let base = jpeg(w, h);
        let mut out = base[..2].to_vec();
        let mut left = extra;
        while left > 0 {
            let n = left.min(60_000);
            out.extend_from_slice(&[0xFF, 0xE1]);
            out.extend_from_slice(&u16::try_from(n + 2).expect("fits").to_be_bytes());
            out.extend(std::iter::repeat_n(0xAB, n));
            left -= n;
        }
        out.extend_from_slice(&base[2..]);
        out
    }

    fn png_rgba(w: u32, h: u32) -> Vec<u8> {
        let rgba: Vec<u8> = (0..w * h).flat_map(|i| [200, 30, (i % 256) as u8, if i % 2 == 0 { 255 } else { 40 }]).collect();
        let mut out = Vec::new();
        ::image::codecs::png::PngEncoder::new(&mut out).write_image(&rgba, w, h, ExtendedColorType::Rgba8).expect("encodes");
        out
    }

    fn markup() -> AnnotationSet {
        let p = NormPoint::new;
        let mut text = Annotation::new(Shape::Text { at: p(0.1, 0.1), text: "Hot clamp 92 °C".into(), size: 40.0 });
        text.color = Color([255, 255, 0, 128]);
        AnnotationSet::from_shapes(vec![
            Annotation::new(Shape::Ellipse { a: p(0.2, 0.2), b: p(0.5, 0.6) }),
            Annotation::new(Shape::Arrow { from: p(0.9, 0.9), to: p(0.5, 0.5) }),
            Annotation::new(Shape::Rect { a: p(0.6, 0.1), b: p(0.8, 0.3) }),
            Annotation::new(Shape::Pen { points: vec![p(0.1, 0.9), p(0.2, 0.8), p(0.3, 0.9)] }),
            Annotation::new(Shape::Marker { at: p(0.7, 0.7), number: 1 }),
            text,
        ])
        .expect("valid markup")
    }

    fn page(label: &str, photos: Vec<PagePhoto>, rows: usize) -> Page {
        Page {
            size: PageSize::A4,
            title: "Thermal Inspection Report".into(),
            header: "LINE A | 76/0 | CKT 1".into(),
            severity: Some(Severity::High),
            label: label.into(),
            previous: Some("75/0".into()),
            next: Some("77/0".into()),
            photos,
            sections: vec![PageSection {
                title: "Fault Analysis".into(),
                rows: (0..rows)
                    .map(|i| PageRow { label: format!("Field {i}"), value: "value ".repeat(i % 40), wide: i % 7 == 0 })
                    .collect(),
            }],
            map: None,
            logo: None,
            accent: Color([0x1F, 0x2A, 0x44, 0xFF]),
            footer: "Generated by GeoGIS".into(),
        }
    }

    fn photo(bytes: Vec<u8>, annotations: AnnotationSet) -> PagePhoto {
        PagePhoto { caption: "Thermal".into(), image: Image::from_bytes(bytes).map(Some), annotations }
    }

    fn count(hay: &[u8], needle: &[u8]) -> usize {
        hay.windows(needle.len()).filter(|w| *w == needle).count()
    }

    #[test]
    fn a_page_with_photos_and_markup_is_a_pdf() {
        let photos = vec![
            photo(jpeg(64, 48), markup()),
            photo(png_rgba(20, 10), AnnotationSet::new()),
            PagePhoto { caption: "Visual".into(), image: Ok(None), annotations: AnnotationSet::new() },
            PagePhoto {
                caption: "Zoom".into(),
                image: Image::from_bytes(b"not an image".to_vec()).map(Some),
                annotations: AnnotationSet::new(),
            },
        ];
        let mut out = Vec::new();
        let summary = write_pdf(&page("76/0", photos, 6), &mut out).expect("writes");
        assert!(out.starts_with(b"%PDF-"));
        assert!(out.ends_with(b"%%EOF") || out.ends_with(b"%%EOF\n"));
        assert_eq!(count(&out, b"/DCTDecode"), 1);
        assert_eq!(count(&out, b"/SMask"), 1, "the PNG's transparency is a soft mask");
        assert_eq!(summary.pages, 1);
        assert_eq!(summary.bytes, out.len() as u64);
        assert_eq!(summary.photos_missing.len(), 1);
        assert_eq!(summary.photos_missing[0].2, PhotoIssue::NotAnImage);
        assert!(summary.replaced_chars.is_empty(), "°C is in WinAnsi");
    }

    #[test]
    fn thermal_data_is_left_out_of_the_pdf() {
        let original = thermal_jpeg(64, 48, 300_000);
        let mut out = Vec::new();
        write_pdf(&page("1", vec![photo(original.clone(), AnnotationSet::new())], 0), &mut out).expect("writes");
        assert!(out.len() < 100_000, "{} bytes for a {} byte photo", out.len(), original.len());
        let slim = slim_jpeg(&original).expect("APP1 removed");
        assert_eq!(ImageInfo::read(&slim), ImageInfo::read(&original));
    }

    #[test]
    fn a_long_table_continues_on_more_pages() {
        let mut out = Vec::new();
        let summary = write_pdf(&page("1", vec![photo(jpeg(32, 32), AnnotationSet::new())], 400), &mut out).expect("writes");
        assert!(summary.pages > 2, "{} pages", summary.pages);
        assert_eq!(count(&out, format!("/Count {}", summary.pages).as_bytes()), 1);
    }

    #[test]
    fn non_latin_text_is_reported() {
        let mut p = page("टावर 5", vec![], 0);
        p.title = "निरीक्षण".into();
        let mut out = Vec::new();
        let summary = write_pdf(&p, &mut out).expect("writes");
        assert!(summary.replaced_chars.contains(&'न'), "{:?}", summary.replaced_chars);
    }

    #[test]
    fn hostile_images_never_panic() {
        let mut cases: Vec<Vec<u8>> =
            vec![vec![], vec![0xFF, 0xD8], vec![0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x01], b"\x89PNG\r\n\x1a\n".to_vec()];
        let good = jpeg(16, 16);
        for cut in [3, 20, 100, good.len() / 2] {
            cases.push(good[..cut.min(good.len())].to_vec());
        }
        let png = png_rgba(8, 8);
        cases.push(png[..png.len() / 2].to_vec());
        // A PNG whose header claims 30000 × 30000 pixels.
        let mut huge = png.clone();
        huge[16..24].copy_from_slice(&[0, 0, 0x75, 0x30, 0, 0, 0x75, 0x30]);
        cases.push(huge);
        for bytes in cases {
            let photos =
                vec![PagePhoto { caption: "x".into(), image: Image::from_bytes(bytes).map(Some), annotations: AnnotationSet::new() }];
            let mut out = Vec::new();
            write_pdf(&page("1", photos, 1), &mut out).expect("a bad photo never stops the page");
        }
    }

    #[test]
    fn bulk_writes_one_pdf_per_entry_and_dedupes_names() {
        let entries: Vec<BulkEntry> = ["1", "2", "2"]
            .iter()
            .map(|l| BulkEntry { path: format!("FAULT/Tower_{l}_Report.pdf"), page: page(l, vec![photo(jpeg(16, 16), markup())], 3) })
            .collect();
        let progress = Progress::new();
        let mut zip = Cursor::new(Vec::new());
        let summary = write_bulk(&mut zip, entries.len(), entries, &progress).expect("writes");
        assert_eq!(summary.pages, 3);
        assert_eq!(progress.get(), (3, 3));
        let mut archive = zip::ZipArchive::new(Cursor::new(zip.into_inner())).expect("a zip");
        let names: Vec<String> = archive.file_names().map(str::to_string).collect();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"FAULT/Tower_2_Report (2).pdf".to_string()), "{names:?}");
        let mut first = Vec::new();
        archive.by_index(0).expect("entry").read_to_end(&mut first).expect("reads");
        assert!(first.starts_with(b"%PDF-"));
    }

    #[test]
    fn bulk_stops_when_cancelled() {
        let progress = Progress::new();
        let p = progress.clone();
        let entries = (0..10).map(move |i| {
            if i == 2 {
                p.cancel();
            }
            BulkEntry { path: format!("{i}.pdf"), page: page("x", vec![], 0) }
        });
        let result = write_bulk(Cursor::new(Vec::new()), 10, entries, &progress);
        assert!(matches!(result, Err(ReportError::Cancelled)));
        assert!(progress.get().0 <= 3);
    }
}
