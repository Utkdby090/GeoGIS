use super::*;
use crate::defect::DefectId;

fn schema() -> DomainSchema {
    crate::builtin_schema("com.geo.telecom").expect("listed").expect("valid")
}

fn defect(id: &str) -> Option<DefectId> {
    DefectId::parse(id)
}

fn obs(d: &str) -> Observation {
    Observation { defect: defect(d), ..Default::default() }
}

fn rule() -> InspectorInfo {
    InspectorInfo { id: "geoflux.thermal.hotspot".into(), version: "1".into(), kind: InspectorKind::Rule, label: "Hot spots".into() }
}

fn photo(n: u8) -> PhotoId {
    PhotoId::of_bytes(&[n; 10])
}

/// Records one rule run proposing `found` on photo `p` of tower 1/0.
fn propose(set: &mut FindingSet, p: &PhotoId, found: Vec<Observation>) -> RunReport {
    set.record_run("1/0", p, "THERMAL", &rule(), Ok(found), 100, &schema()).expect("recorded")
}

#[test]
fn ids_are_stable_and_parts_cannot_run_together() {
    assert_eq!(FindingId::derived(&["a", "b"]), FindingId::derived(&["a", "b"]));
    assert_ne!(FindingId::derived(&["ab", "c"]), FindingId::derived(&["a", "bc"]));
    let id = FindingId::derived(&["x"]);
    assert_eq!(id.as_str().len(), 34);
    assert_eq!(FindingId::parse(id.as_str()), Some(id));
    assert!(FindingId::parse("has space").is_none());
}

#[test]
fn an_engineers_finding_is_checked_and_confirmed_as_made() {
    let s = schema();
    let mut set = FindingSet::new();
    let p = photo(1);
    let boxed = Observation {
        region: Region::Box { a: NormPoint::new(0.2, 0.2), b: NormPoint::new(1.5, 0.4) },
        note: "  R phase jumper  ".into(),
        ..obs("hotspot.joint")
    };
    let id = set.add("1/0", Some(p.clone()), "THERMAL", boxed, "eng@x", 5, &s).expect("added");
    let f = set.get(&id).expect("stored");
    assert_eq!(f.status, FindingStatus::Confirmed);
    assert_eq!(f.severity, Severity::Medium, "the defect type's default");
    assert_eq!(f.note, "R phase jumper");
    assert_eq!(f.region.bbox(), Some([0.2, 0.2, 0.8, 0.2]), "kept on the photo");
    assert_eq!(f.decided, Some(Decision { by: "eng@x".into(), at: 5 }));
    // Same moment, same photo: still a new id.
    let again = set.add("1/0", Some(p.clone()), "THERMAL", obs("hotspot.joint"), "eng@x", 5, &s).expect("added");
    assert_ne!(again, id);

    let refused = |o: Observation, photo: Option<PhotoId>, slot: &str| set.clone().add("1/0", photo, slot, o, "", 0, &s).unwrap_err();
    assert_eq!(refused(Observation::default(), Some(p.clone()), "THERMAL"), FindingError::NoDefect);
    assert!(matches!(refused(obs("rust.everywhere"), Some(p.clone()), "THERMAL"), FindingError::UnknownDefect(_)));
    assert!(matches!(refused(obs("hotspot.joint"), Some(p.clone()), "RGB_ZOOM"), FindingError::DefectNotForSlot { .. }));
    let point = Observation { region: Region::Point { at: NormPoint::new(0.5, 0.5) }, ..obs("bird_nest") };
    assert_eq!(refused(point, None, ""), FindingError::RegionWithoutPhoto);
    let line = Observation {
        region: Region::Polygon { points: vec![NormPoint::new(0.1, 0.1), NormPoint::new(0.5, 0.1), NormPoint::new(0.9, 0.1)] },
        ..obs("bird_nest")
    };
    assert!(matches!(refused(line, Some(p.clone()), "RGB_ZOOM"), FindingError::BadRegion(_)));
    let flat = Observation { region: Region::Box { a: NormPoint::new(0.2, 0.2), b: NormPoint::new(0.2, 0.9) }, ..obs("bird_nest") };
    assert!(matches!(refused(flat, Some(p.clone()), "RGB_ZOOM"), FindingError::BadRegion(_)));
    let nan = Observation { measurements: [("maxC".to_string(), f64::NAN)].into(), ..obs("bird_nest") };
    assert!(matches!(refused(nan, Some(p.clone()), "RGB_ZOOM"), FindingError::BadMeasurement(_)));
    let long = Observation { note: "x".repeat(MAX_NOTE_CHARS + 1), ..obs("bird_nest") };
    assert_eq!(refused(long, Some(p.clone()), "RGB_ZOOM"), FindingError::NoteTooLong);
    let sure = Observation { confidence: Some(1.5), ..obs("bird_nest") };
    assert_eq!(refused(sure, Some(p), "RGB_ZOOM"), FindingError::BadConfidence);
}

#[test]
fn proposals_count_only_after_a_person_decides() {
    let s = schema();
    let mut set = FindingSet::new();
    let p = photo(2);
    let hot = Observation { measurements: [(measure::MAX_C.to_string(), 81.0)].into(), confidence: Some(0.9), ..obs("hotspot.clamp") };
    let report = propose(&mut set, &p, vec![hot, obs("hotspot.other")]);
    assert_eq!((report.added, report.outcome), (2, Some(RunOutcome::Found { count: 2 })));
    let ids: Vec<FindingId> = set.findings().iter().map(|f| f.id.clone()).collect();
    assert_eq!(set.for_asset("1/0").pending, 2);
    assert_eq!(set.for_asset("1/0").severity(), Severity::None, "proposals never count");

    set.confirm(&ids[0], "eng", 200).expect("confirmed");
    let edit = FindingEdit { severity: Some(Severity::High), note: Some("bolted clamp".into()), ..Default::default() };
    set.edit(&ids[1], edit, "eng", 210, &s).expect("edited");
    let edited = set.get(&ids[1]).expect("kept");
    assert_eq!(edited.status, FindingStatus::Edited);
    assert_eq!(edited.proposed.as_ref().and_then(|o| o.severity), Some(Severity::Low), "what the rule said is kept");
    // A second edit keeps the rule's original, and confirming keeps it Edited.
    set.edit(&ids[1], FindingEdit { severity: Some(Severity::Critical), ..Default::default() }, "eng", 220, &s).expect("edited");
    set.confirm(&ids[1], "eng", 230).expect("confirmed");
    let edited = set.get(&ids[1]).expect("kept");
    assert_eq!((edited.status, edited.severity), (FindingStatus::Edited, Severity::Critical));
    assert_eq!(edited.proposed.as_ref().and_then(|o| o.severity), Some(Severity::Low));

    let summary = set.for_asset("1/0");
    assert_eq!(summary.severity(), Severity::Critical);
    assert_eq!(summary.counted.iter().map(|f| &f.id).collect::<Vec<_>>(), vec![&ids[1], &ids[0]], "worst first");
    assert_eq!(set.labelled_examples().count(), 2);

    set.reject(&ids[1], "lead", 300).expect("rejected");
    assert_eq!(set.for_asset("1/0").severity(), Severity::Medium);
    assert_eq!(set.remove(&ids[0]), Err(FindingError::InspectorFindingsAreKept));
    assert_eq!(set.findings().len(), 2);
}

#[test]
fn worst_counted_finding_wins_and_nothing_else_counts() {
    // Every mix of status and severity on three findings: the asset's severity is the highest
    // severity among confirmed and edited ones, and None without any.
    let s = schema();
    let statuses = [FindingStatus::Proposed, FindingStatus::Confirmed, FindingStatus::Edited, FindingStatus::Rejected];
    let p = photo(3);
    for mix in 0..(4 * 5usize).pow(3) {
        let mut set = FindingSet::new();
        let mut expected = Severity::None;
        let mut m = mix;
        for i in 0..3 {
            let (status, severity) = (statuses[m % 4], Severity::ALL[(m / 4) % 5]);
            m /= 20;
            let o = Observation { severity: Some(severity), ..obs("bird_nest") };
            let id = set.add("1/0", Some(p.clone()), "RGB_ZOOM", o, "", i, &s).expect("added");
            let f = set.findings.iter_mut().find(|f| f.id == id).expect("stored");
            f.status = status;
            if status.counts() {
                expected = expected.max(severity);
            }
        }
        assert_eq!(set.for_asset("1/0").severity(), expected, "mix {mix}");
    }
}

#[test]
fn form_fields_are_derived_from_counted_findings() {
    let s = schema();
    let mut set = FindingSet::new();
    let p = photo(4);
    let hot = |c: f64, r: f64, note: &str| Observation {
        measurements: [(measure::MAX_C.to_string(), c), (measure::RISE_C.to_string(), r)].into(),
        note: note.into(),
        ..obs("hotspot.joint")
    };
    set.add("1/0", Some(p.clone()), "THERMAL", hot(60.0, 25.0, "R phase"), "", 1, &s).expect("added");
    set.add("1/0", Some(p.clone()), "THERMAL", hot(72.25, 20.0, ""), "", 2, &s).expect("added");
    set.add("1/0", Some(p.clone()), "RGB_ZOOM", obs("bird_nest"), "", 3, &s).expect("added");
    set.add("1/0", Some(p.clone()), "RGB_ZOOM", obs("bird_nest"), "", 4, &s).expect("added");
    propose(&mut set, &p, vec![Observation { measurements: [(measure::MAX_C.to_string(), 99.0)].into(), ..obs("hotspot.joint") }]);
    let values = set.for_asset("1/0").form_values(&s);
    assert_eq!(
        values,
        vec![
            ("faultDescription".to_string(), "Hot joint or connector (R phase); Hot joint or connector; Bird nest ×2".to_string()),
            ("faultTemp".to_string(), "72.2".to_string()),
            ("riseTemp".to_string(), "25.0".to_string()),
        ],
        "the 99 °C proposal isn't counted"
    );
    let none = FindingSet::new();
    assert!(none.for_asset("1/0").form_values(&s).iter().all(|(_, v)| v.is_empty()), "stale text is cleared");
}

#[test]
fn form_values_typed_before_findings_carry_over_once() {
    let s = schema();
    let mut set = FindingSet::new();
    let props: Properties = [("faultDescription", "Hot spot on jumper"), ("faultTemp", "70,5"), ("riseTemp", "20"), ("ambientTemp", "30")]
        .into_iter()
        .collect();
    let other: Properties = [("ambientTemp", "30")].into_iter().collect();
    let ids = set.migrate_layer([("7/0", &props), ("8/0", &other)], &s, 9);
    assert_eq!(ids.len(), 1, "8/0 has no fault values");
    let id = ids[0].clone();
    let f = set.get(&id).expect("stored");
    assert!(f.defect.is_unclassified() && f.photo.is_none() && f.status == FindingStatus::Confirmed);
    assert_eq!(f.severity, Severity::High, "the schema's rule: a 20 °C rise");
    assert_eq!(f.measurement(measure::MAX_C), Some(70.5));
    assert!(!f.is_labelled_example(), "no photo, no class: not training data");
    assert!(set.is_migrated());
    let values = set.for_asset("7/0").form_values(&s);
    assert_eq!(values[0].1, "Hot spot on jumper", "an unclassified finding shows its note only");
    assert_eq!(values[1].1, "70.5");
    // Deleted by the engineer: not carried over again on the next load.
    set.remove(&id).expect("removed");
    assert!(set.migrate_layer([("7/0", &props)], &s, 11).is_empty());
    assert!(set.findings().is_empty());
}

#[test]
fn a_run_is_bounded_and_bad_observations_are_dropped() {
    let mut set = FindingSet::new();
    let p = photo(5);
    let mut many: Vec<Observation> = (0..MAX_PER_RUN + 50).map(|_| obs("hotspot.joint")).collect();
    many[0] = obs("insulator.nope");
    let report = propose(&mut set, &p, many);
    assert_eq!((report.added, report.invalid, report.over_limit), (MAX_PER_RUN, 1, 49));
    assert!(report.first_invalid.is_some_and(|r| r.contains("insulator.nope")));
}

#[test]
fn failures_are_retried_and_skips_remembered() {
    let s = schema();
    let mut set = FindingSet::new();
    let p = photo(6);
    let failed = set.record_run("1/0", &p, "THERMAL", &rule(), Err(InspectError::Unreadable("truncated".into())), 1, &s).expect("recorded");
    assert!(matches!(failed.outcome, Some(RunOutcome::Failed { .. })));
    assert!(set.needs_run("1/0", &p, &rule()));
    propose(&mut set, &p, vec![]);
    assert_eq!(set.runs().len(), 1, "the retry replaced the failure");
    assert_eq!(set.runs()[0].outcome, RunOutcome::Nothing);
    assert!(!set.needs_run("1/0", &p, &rule()));
    assert!(propose(&mut set, &p, vec![obs("hotspot.joint")]).already_done);
    assert!(set.findings().is_empty());
    let skipped = set.record_run("2/0", &p, "THERMAL", &rule(), Err(InspectError::NotApplicable("a PNG".into())), 1, &s).expect("recorded");
    assert!(matches!(skipped.outcome, Some(RunOutcome::Skipped { .. })));
    assert!(!set.needs_run("2/0", &p, &rule()));
}

#[test]
fn decided_proposals_survive_a_new_inspector_version() {
    let s = schema();
    let mut set = FindingSet::new();
    let p = photo(7);
    propose(&mut set, &p, vec![obs("hotspot.joint"), obs("hotspot.clamp")]);
    let first = set.findings()[0].id.clone();
    set.confirm(&first, "eng", 150).expect("confirmed");
    let v2 = InspectorInfo { version: "2".into(), ..rule() };
    let report = set.record_run("1/0", &p, "THERMAL", &v2, Ok(vec![obs("hotspot.conductor")]), 200, &s).expect("recorded");
    assert_eq!((report.added, report.replaced), (1, 1));
    assert!(set.get(&first).is_some_and(|f| f.status == FindingStatus::Confirmed));
}

#[test]
fn stored_sets_round_trip_and_are_sanitized_on_load() {
    let s = schema();
    let mut set = FindingSet::new();
    let p = photo(8);
    set.add("1/0", Some(p.clone()), "THERMAL", obs("hotspot.joint"), "eng", 1, &s).expect("added");
    propose(&mut set, &p, vec![Observation { confidence: Some(0.4), ..obs("hotspot.clamp") }]);
    let json = serde_json::to_string(&set).expect("serialises");
    let mut back: FindingSet = serde_json::from_str(&json).expect("reads");
    assert_eq!(back.sanitize(), Ok(0));
    assert_eq!(back, set);

    // A hand-edited file: a duplicate id, a region off the photo, and NaN confidence.
    let mut v: serde_json::Value = serde_json::from_str(&json).expect("json");
    let mut dup = v["findings"][0].clone();
    dup["note"] = "dup".into();
    let mut nan = v["findings"][1].clone();
    nan["id"] = "f-other".into();
    nan["confidence"] = serde_json::json!(null);
    nan["region"] = serde_json::json!({"kind":"box","a":{"x":0.1,"y":0.1},"b":{"x":0.1,"y":0.5}});
    v["findings"].as_array_mut().expect("array").extend([dup, nan]);
    let mut edited: FindingSet = serde_json::from_value(v.clone()).expect("reads");
    assert_eq!(edited.sanitize(), Ok(2));
    assert_eq!(edited.findings().len(), 2);

    v["version"] = (FINDINGS_VERSION + 1).into();
    let mut newer: FindingSet = serde_json::from_value(v).expect("reads");
    assert_eq!(newer.sanitize(), Err(FindingError::NewerFormat(FINDINGS_VERSION + 1)));
}

#[test]
fn engineer_findings_can_be_deleted() {
    let s = schema();
    let mut set = FindingSet::new();
    let id = set.add("1/0", None, "", obs("tower.missing_plate"), "", 1, &s).expect("added");
    assert_eq!(set.remove(&id).map(|f| f.asset), Ok("1/0".to_string()));
    assert_eq!(set.remove(&id), Err(FindingError::NotFound(id.to_string())));
}
