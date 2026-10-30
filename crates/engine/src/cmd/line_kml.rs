//! GeoFlux › Import Line KML: a transmission line's KML (or KMZ) as a tower layer and its spans.
//!
//! The towers are the KML's named points, chosen by [`geoflux_domain::line_kml`]; they become an
//! inspection layer like an imported survey sheet, so the Inspection pane, photos and reports
//! work on them at once. The KML's lines (the spans) and polygons are added under the towers as
//! plain map layers, in the KML's own styles. Points that are not towers (a store, an office) are
//! said in a note and left out of the tower layer.

use geoflux_domain::line_kml::{self, Placemark};
use rustgis_data::{Geometry, GeometryType};
use serde_json::{Value, json};

use super::geoflux::{self, DEFAULT_SCHEMA};
use super::ps;
use crate::{CmdResult, CommandSpec as C, Session};

pub fn commands() -> Vec<C> {
    vec![C::new("geoflux.importLineKml", "Import Line KML", "GeoFlux › Inspection", import_line_kml).params(
        r#"{"path"?: "a line's .kml or .kmz", "text"?: "the KML itself, as text", "schema"?: "a schema id (default com.geo.telecom)", "name"?: "the tower layer (default: the file name)", "line"?: "the line's name on every tower (default: the layer's name)"}"#,
    )]
}

fn import_line_kml(s: &mut Session, p: &Value) -> CmdResult {
    let schema_id = ps(p, "schema").map(str::trim).filter(|x| !x.is_empty()).unwrap_or(DEFAULT_SCHEMA);
    let schema = s.project.geoflux.schema(schema_id)?;

    let (bytes, file_name) = match (ps(p, "text"), ps(p, "path").map(str::trim).filter(|x| !x.is_empty())) {
        (Some(t), _) => (t.as_bytes().to_vec(), "line.kml".to_string()),
        (None, Some(path)) => (geoflux::read_file(path, rustgis_data::io::MAX_BYTES as usize)?, path.to_string()),
        (None, None) => return Err("give \"path\" (a line's .kml or .kmz) or \"text\"".into()),
    };
    let layers = rustgis_data::kml::read(&file_name, &bytes).map_err(|e| e.to_string())?;
    drop(bytes);

    let default_name = match ps(p, "text") {
        Some(_) => schema.name.clone(),
        None => std::path::Path::new(&file_name).file_stem().and_then(|n| n.to_str()).unwrap_or(&schema.name).to_string(),
    };
    let name = ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).map_or(default_name, str::to_string);
    let line = ps(p, "line").map(str::trim).filter(|n| !n.is_empty()).unwrap_or(&name).to_string();

    let mut placemarks = Vec::new();
    let mut others = Vec::new();
    for layer in layers {
        if layer.dataset.geometry_type == GeometryType::Point {
            placemarks.extend(points(&layer.dataset));
        } else {
            others.push(layer);
        }
    }
    let report = line_kml::towers(&placemarks, &schema, &line);
    drop(placemarks);
    if report.towers.is_empty() {
        let why = if report.left_out_count > 0 { format!(" ({})", report.left_out_summary()) } else { String::new() };
        return Err(format!("{file_name}: no tower points in it: a tower is a point named with its number{why}"));
    }

    // Spans first, so the towers draw over them.
    let mut lines = Vec::new();
    for layer in others {
        let id = super::layer::add_dataset(s, layer.dataset);
        super::layer::apply_styles(s, id, &layer.styles);
        lines.push(id);
    }
    let id = geoflux::add_inspection_layer(s, &schema, &name, &report.towers);
    if report.left_out_count > 0 {
        s.notify(format!("{} points were not taken as towers: {}", report.left_out_count, report.left_out_summary()), false);
    }
    Ok(json!({
        "layer": id,
        "schema": schema.id,
        "towers": report.towers.len(),
        "lines": lines,
        "leftOut": report.left_out_count,
        "leftOutDetails": report.left_out.iter().map(ToString::to_string).collect::<Vec<_>>(),
    }))
}

/// A point dataset's features as placemarks, by their `name` and `folder` fields. A multi-point
/// placemark stands at its first point; a feature without a point is passed with no position, so
/// it is left out with a reason rather than silently.
fn points(ds: &rustgis_data::Dataset) -> impl Iterator<Item = Placemark> + '_ {
    let text = |row: usize, field: Option<usize>| match field.map(|f| ds.value(row, f)) {
        Some(rustgis_data::Value::Text(t)) => t.clone(),
        _ => String::new(),
    };
    let name = ds.field_index("name");
    let folder = ds.field_index("folder");
    ds.features.iter().enumerate().map(move |(row, f)| {
        let at = match &f.geometry {
            Some(Geometry::Point(c)) => Some(*c),
            Some(Geometry::MultiPoint(cs)) => cs.first().copied(),
            _ => None,
        };
        let (lon, lat) = at.map_or((f64::NAN, f64::NAN), |c| (c.x, c.y));
        Placemark { name: text(row, name), folder: text(row, folder), lon, lat }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Project;

    const LINE: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<kml xmlns="http://www.opengis.net/kml/2.2"><Document><name>TW 01</name>
  <Style id="span"><LineStyle><color>ff0000ff</color><width>3</width></LineStyle></Style>
  <Folder><name>Towers</name>
    <Placemark><name>AP8</name><Point><coordinates>75.100,27.500,0</coordinates></Point></Placemark>
    <Placemark><name>34/1</name><Point><coordinates>75.102,27.503,0</coordinates></Point></Placemark>
    <Placemark><name>34/2</name><Point><coordinates>75.104,27.506,0</coordinates></Point></Placemark>
  </Folder>
  <Placemark><name>STORE</name><Point><coordinates>75.200,27.600,0</coordinates></Point></Placemark>
  <Placemark><name>AP8-34/1</name><styleUrl>#span</styleUrl>
    <LineString><coordinates>75.100,27.500,0 75.102,27.503,0</coordinates></LineString></Placemark>
</Document></kml>"##;

    #[test]
    fn towers_become_an_inspection_layer_and_spans_a_map_layer_beneath() {
        let mut s = Session::new(Project::default());
        let r = s.run("geoflux.importLineKml", &json!({"text": LINE, "name": "TW 01"})).expect("import");
        assert_eq!(r["towers"], 3);
        assert_eq!(r["leftOut"], 1);
        assert_eq!(r["leftOutDetails"][0], "\"STORE\": not a tower number");
        let id = r["layer"].as_u64().expect("layer") as u32;
        assert_eq!(s.project.geoflux.schema_of(id), Some(DEFAULT_SCHEMA));
        let ds = &s.map().layer(id).expect("tower layer").dataset;
        let tower = ds.field_index("towerNumber").expect("tower field");
        let line = ds.field_index("lineName").expect("line field");
        let names: Vec<String> = (0..ds.features.len()).map(|r| ds.value(r, tower).to_string()).collect();
        assert_eq!(names, ["AP8", "34/1", "34/2"]);
        assert_eq!(ds.value(0, line).to_string(), "TW 01");
        assert!(matches!(ds.features[1].geometry, Some(Geometry::Point(c)) if (c.x - 75.102).abs() < 1e-9 && (c.y - 27.503).abs() < 1e-9));

        let spans = r["lines"].as_array().expect("lines");
        assert_eq!(spans.len(), 1);
        let span = s.map().layer(spans[0].as_u64().expect("id") as u32).expect("span layer");
        assert_eq!(span.dataset.geometry_type, GeometryType::Polyline);
        assert_eq!(s.project.geoflux.schema_of(span.id), None);
        assert!(s.can_undo());
    }

    #[test]
    fn the_line_name_can_differ_from_the_layer_name() {
        let mut s = Session::new(Project::default());
        let r = s.run("geoflux.importLineKml", &json!({"text": LINE, "name": "Day 1", "line": "Bhadla-Sikar"})).expect("import");
        let ds = &s.map().layer(r["layer"].as_u64().expect("layer") as u32).expect("layer").dataset;
        assert_eq!(ds.value(0, ds.field_index("lineName").expect("line field")).to_string(), "Bhadla-Sikar");
    }

    #[test]
    fn a_kml_without_tower_points_is_refused_with_what_it_had() {
        let mut s = Session::new(Project::default());
        let kml =
            r#"<kml><Document><Placemark><name>STORE</name><Point><coordinates>75,27</coordinates></Point></Placemark></Document></kml>"#;
        let e = s.run("geoflux.importLineKml", &json!({"text": kml})).expect_err("no towers");
        assert!(e.contains("no tower points") && e.contains("STORE"), "{e}");
        assert!(s.run("geoflux.importLineKml", &json!({})).is_err());
        assert!(s.run("geoflux.importLineKml", &json!({"text": "not xml"})).is_err());
    }
}
