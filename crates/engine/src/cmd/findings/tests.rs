#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use super::*;
use crate::Project;

const LINE: &str = r#"<kml><Document><name>L</name>
  <Placemark><name>34/1</name><Point><coordinates>75.100,27.500</coordinates></Point></Placemark>
  <Placemark><name>34/2</name><Point><coordinates>75.104,27.500</coordinates></Point></Placemark>
</Document></kml>"#;

/// A folder removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let p = std::env::temp_dir().join(format!("geoflux-findings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("temp dir");
        TempDir(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The line's two towers, with typed fault values on 34/1 and a thermal photo on 34/2.
fn session(dir: &TempDir) -> (Session, u32, String) {
    let mut s = Session::new(Project::default());
    let layer = s.run("geoflux.importLineKml", &json!({"text": LINE})).expect("line")["layer"].as_u64().expect("id") as u32;
    s.run("geoflux.setValues", &json!({"layer": layer, "row": 0, "values": {"faultDescription": "Hot jumper", "riseTemp": "20"}}))
        .expect("typed");
    let photo = dir.0.join("DJI_0002_T.JPG");
    std::fs::write(&photo, b"a thermal photo of 34/2").expect("photo");
    let path = photo.display().to_string();
    s.run("geoflux.addPhotos", &json!({"layer": layer, "row": 1, "slot": "THERMAL", "paths": [path]})).expect("added");
    (s, layer, path)
}

fn value(s: &Session, layer: u32, row: usize, key: &str) -> String {
    let ds = &s.map().layer(layer).expect("layer").dataset;
    let v = ds.field_index(key).map(|c| ds.value(row, c).to_string()).unwrap_or_default();
    if v == "<Null>" { String::new() } else { v }
}

fn hot_box() -> Value {
    json!({"kind": "box", "a": {"x": 0.2, "y": 0.2}, "b": {"x": 0.4, "y": 0.5}})
}

#[test]
fn the_first_finding_carries_typed_faults_over_and_drives_the_form() {
    let dir = TempDir::new("first");
    let (mut s, layer, path) = session(&dir);
    assert_eq!(value(&s, layer, 0, "severity"), "high", "rated by the rule before findings");

    let r = s
        .run(
            "geoflux.addFinding",
            &json!({"layer": layer, "row": 1, "path": path, "defect": "hotspot.joint", "region": hot_box(),
                    "measurements": {"maxC": 61.0, "riseC": 25.0}, "note": "R phase", "by": "eng@geoflux"}),
        )
        .expect("added");
    assert_eq!(r["severity"], "medium", "the defect type's default");

    // 34/1's typed values became one finding, and still rate it high.
    let set = s.project.geoflux.findings_of(layer).expect("findings mode");
    let carried = set.for_asset("34/1");
    assert_eq!(carried.counted.len(), 1);
    assert!(carried.counted[0].defect.is_unclassified());
    assert_eq!(value(&s, layer, 0, "severity"), "high");
    assert_eq!(value(&s, layer, 0, "faultDescription"), "Hot jumper");

    // 34/2's form shows its finding.
    assert_eq!(value(&s, layer, 1, "faultDescription"), "Hot joint or connector (R phase)");
    assert_eq!(value(&s, layer, 1, "faultTemp"), "61.0");
    assert_eq!(value(&s, layer, 1, "riseTemp"), "25.0");
    assert_eq!(value(&s, layer, 1, "severity"), "medium");
    let f = &s.project.geoflux.findings_of(layer).expect("set").for_asset("34/2").counted[0].clone();
    assert_eq!(f.slot, "THERMAL");
    assert_eq!(f.source, geoflux_domain::FindingSource::Engineer { user: "eng@geoflux".into() });

    // Undo takes the whole step back, findings mode included.
    s.run("edit.undo", &json!({})).expect("undo");
    assert!(s.project.geoflux.findings_of(layer).is_none());
    assert_eq!(value(&s, layer, 1, "faultDescription"), "");
}

#[test]
fn derived_fields_cant_be_typed_over_but_the_rest_can() {
    let dir = TempDir::new("typed");
    let (mut s, layer, path) = session(&dir);
    s.run("geoflux.addFinding", &json!({"layer": layer, "row": 1, "path": path, "defect": "hotspot.joint"})).expect("added");
    let e = s.run("geoflux.setValues", &json!({"layer": layer, "row": 1, "values": {"riseTemp": "3"}})).expect_err("derived");
    assert!(e.contains("Rise Temp (°C) is filled from the tower's findings"), "{e}");
    s.run("geoflux.setValues", &json!({"layer": layer, "row": 1, "values": {"phase": "R", "riseTemp": ""}})).expect("unchanged derived");
    assert_eq!(value(&s, layer, 1, "phase"), "R");
    assert_eq!(value(&s, layer, 1, "severity"), "medium", "still from the finding, not the rule");
}

#[test]
fn review_edit_and_delete_keep_the_form_in_step() {
    let dir = TempDir::new("review");
    let (mut s, layer, path) = session(&dir);
    let id = s
        .run("geoflux.addFinding", &json!({"layer": layer, "row": 1, "path": path, "defect": "bird_nest", "severity": "low"}))
        .expect("added")["id"]
        .as_str()
        .expect("id")
        .to_string();
    s.run("geoflux.editFinding", &json!({"layer": layer, "id": id, "severity": "critical", "note": "on the cross-arm"})).expect("edited");
    assert_eq!(value(&s, layer, 1, "severity"), "critical");
    assert_eq!(value(&s, layer, 1, "faultDescription"), "Bird nest (on the cross-arm)");
    let e = s.run("geoflux.editFinding", &json!({"layer": layer, "id": id, "defect": "made.up"})).expect_err("unknown type");
    assert!(e.contains("made.up"), "{e}");

    let view = s.run("geoflux.findings", &json!({"layer": layer, "row": 1, "path": path})).expect("view");
    assert_eq!(view["onPhoto"], json!([id]));
    assert_eq!(view["findings"][0]["path"], json!(path));
    assert_eq!(view["findings"][0]["defectLabel"], "Bird nest");
    assert!(view["defects"].as_array().is_some_and(|d| d.len() == 19));

    s.run("geoflux.removeFinding", &json!({"layer": layer, "id": id})).expect("removed");
    assert_eq!(value(&s, layer, 1, "severity"), "none");
    assert_eq!(value(&s, layer, 1, "faultDescription"), "");
    assert!(s.run("geoflux.decideFinding", &json!({"layer": layer, "id": id, "decision": "confirm"})).is_err());
}

#[test]
fn a_finding_follows_its_photo_when_the_folder_moves() {
    let dir = TempDir::new("moved");
    let (mut s, layer, path) = session(&dir);
    s.run("geoflux.addFinding", &json!({"layer": layer, "row": 1, "path": path, "defect": "hotspot.joint", "region": hot_box()}))
        .expect("added");
    let moved = dir.0.join("elsewhere");
    std::fs::create_dir_all(&moved).expect("dir");
    let new_path = moved.join("DJI_0002_T.JPG").display().to_string();
    std::fs::rename(&path, &new_path).expect("moved");
    s.run("geoflux.removePhoto", &json!({"layer": layer, "row": 1, "slot": "THERMAL", "path": path})).expect("old link");
    s.run("geoflux.addPhotos", &json!({"layer": layer, "row": 1, "slot": "THERMAL", "paths": [new_path]})).expect("relinked");
    let view = s.run("geoflux.findings", &json!({"layer": layer, "row": 1, "path": new_path})).expect("view");
    assert_eq!(view["onPhoto"].as_array().map(Vec::len), Some(1), "found by content, not by path");
    assert_eq!(view["findings"][0]["path"], json!(new_path), "and its last known place is updated");
}

#[test]
fn the_thermal_rule_proposes_and_a_person_decides() {
    let dir = TempDir::new("rule");
    let (mut s, layer, path) = session(&dir);
    // Before findings, Scan Line Thermal writes the form as it always did.
    s.run("geoflux.thermal.scan", &json!({"layer": layer, "results": [{"row": 1, "maxC": 34.0, "referenceC": 30.0}]})).expect("scan");
    assert_eq!(value(&s, layer, 1, "riseTemp"), "4.0");

    // With findings, it proposes.
    s.run("geoflux.addFinding", &json!({"layer": layer, "row": 1, "defect": "tower.missing_plate"})).expect("first finding");
    let results = json!({"layer": layer, "results": [{"row": 1, "maxC": 60.0, "referenceC": 30.0}, {"row": 0, "error": "no thermal"}]});
    let r = s.run("geoflux.thermal.scan", &results).expect("scan");
    assert_eq!(r["proposals"]["proposed"], 1, "{r}");
    assert_eq!(value(&s, layer, 1, "severity"), "medium", "a proposal doesn't count yet (the carried 4 °C rise is medium)");
    let again = s.run("geoflux.thermal.scan", &results).expect("again");
    assert_eq!(again["proposals"]["already"], 1, "the same rule version doesn't look twice");

    let view = s.run("geoflux.findings", &json!({"layer": layer, "row": 1, "path": path})).expect("view");
    assert_eq!(view["pending"], 1);
    let proposal = view["findings"].as_array().expect("list").iter().find(|f| f["status"] == "proposed").expect("proposal").clone();
    assert_eq!(proposal["source"]["kind"], "rule");
    assert_eq!(proposal["measurements"]["riseC"], 30.0);
    assert_eq!(proposal["severity"], "high", "a 30 °C rise on the schema's thresholds");
    s.run("geoflux.decideFinding", &json!({"layer": layer, "id": proposal["id"], "decision": "confirm"})).expect("confirmed");
    assert_eq!(value(&s, layer, 1, "severity"), "high");
    assert_eq!(value(&s, layer, 1, "riseTemp"), "30.0");
}

#[test]
fn recording_a_thermal_measurement_adds_a_finding_in_findings_mode() {
    let dir = TempDir::new("record");
    let (mut s, layer, _) = session(&dir);
    s.run("geoflux.addFinding", &json!({"layer": layer, "row": 0, "defect": "bird_nest"})).expect("first finding");
    let r = s
        .run("geoflux.thermal.record", &json!({"layer": layer, "row": 1, "found": {"maxC": 50.0, "referenceC": 30.0}, "by": "eng"}))
        .expect("recorded");
    assert!(r["finding"].is_string());
    let f = s.project.geoflux.findings_of(layer).expect("set").for_asset("34/2").counted[0].clone();
    assert_eq!(f.defect.as_str(), "hotspot.other");
    assert!(f.photo.is_some(), "on the tower's thermal photo");
    assert_eq!(f.measurement(measure::RISE_C), Some(20.0));
    assert_eq!(value(&s, layer, 1, "severity"), "high");
}

#[test]
fn photo_ids_read_both_ends_of_the_file() {
    let dir = TempDir::new("ids");
    let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let p = dir.0.join("big.jpg");
    std::fs::write(&p, &big).expect("write");
    assert_eq!(photo_id_of(&p.display().to_string()), Ok(PhotoId::of_bytes(&big)));
    assert!(photo_id_of(&dir.0.join("missing.jpg").display().to_string()).is_err());
}

#[test]
fn confirmed_findings_are_drawn_on_the_report_photo() {
    let dir = TempDir::new("report");
    let (mut s, layer, path) = session(&dir);
    s.run("geoflux.addFinding", &json!({"layer": layer, "row": 1, "path": path, "defect": "hotspot.joint", "region": hot_box()}))
        .expect("confirmed");
    let r =
        s.run("geoflux.thermal.scan", &json!({"layer": layer, "results": [{"row": 1, "maxC": 90.0, "referenceC": 30.0}]})).expect("scan");
    assert_eq!(r["proposals"]["proposed"], 1, "{r}");
    let plan = crate::cmd::inspection_report::plan(&s, layer, Some(&[1]), &json!({}), true).expect("planned");
    let page = &plan.pages[0];
    let cell = page.photos.iter().position(|p| p.as_deref() == Some(path.as_str())).expect("thermal photo printed");
    let shapes = page.page.photos[cell].annotations.shapes();
    assert_eq!(shapes.len(), 2, "the confirmed box and its label; the proposal isn't drawn");
    assert!(matches!(&shapes[1].shape, geoflux_domain::annotation::Shape::Text { text, .. } if text == "Hot joint or connector"));
}
