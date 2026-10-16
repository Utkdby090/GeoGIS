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
