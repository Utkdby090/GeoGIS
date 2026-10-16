//! GeoFlux › Thermal: temperatures from a tower's thermal photo, and findings written to its form.
//!
//! Decoding lives in `geoflux-thermal`. FLIR files are read in this process. DJI files go to the
//! `geoflux-dirp` helper program (desktop only), which is started on the first DJI photo and can
//! be stopped with [`Thermal::stop_helper`]; a crash inside DJI's library ends the helper, never
//! GeoGIS.
//!
//! Decoding a DJI photo takes a moment, so the app measures off the frame and passes what it
//! found to `geoflux.thermal.record` and `geoflux.thermal.scan` (`found` / `results`); called
//! without them (the CLI, notebooks), they measure themselves.

use std::sync::Arc;

use geoflux_thermal::findings::Reference;
use geoflux_thermal::{Decoders, FlirDecoder, FormBinding, Pixel, Region, SceneOverrides, ThermalError, ThermalFinding, ThermalFrame};
use serde_json::{Value, json};

use super::geoflux::{self, SlotPhotos};
use super::need_feature_layer;
use crate::{CmdResult, CommandSpec as C, Session};

/// The largest thermal photo read (radiometric JPEGs are a few MB).
#[cfg(not(target_arch = "wasm32"))]
const MAX_PHOTO_BYTES: u64 = 64 << 20;
/// Most per-tower problems `geoflux.thermal.scan` lists one by one.
const MAX_LISTED: usize = 50;

/// The decoders a session uses, and the DJI helper behind them.
#[derive(Clone)]
pub struct Thermal {
    pub decoders: Arc<Decoders>,
    #[cfg(not(target_arch = "wasm32"))]
    helper: Arc<geoflux_thermal::helper::HelperBackend>,
}

impl Default for Thermal {
    fn default() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            use geoflux_thermal::dji::DjiDecoder;
            use geoflux_thermal::helper::{HelperBackend, HelperConfig};
            let helper = Arc::new(HelperBackend::new(HelperConfig::beside_app()));
            let decoders = Decoders::new().with(FlirDecoder::default()).with(DjiDecoder::new(helper.clone()));
            Self { decoders: Arc::new(decoders), helper }
        }
        #[cfg(target_arch = "wasm32")]
        {
            Self { decoders: Arc::new(Decoders::new().with(FlirDecoder::default())) }
        }
    }
}

impl Thermal {
    /// Stops the DJI helper, if running (when the Inspection pane closes). Desktop only.
    pub fn stop_helper(&self) {
        #[cfg(not(target_arch = "wasm32"))]
        self.helper.stop();
    }
}

pub fn commands() -> Vec<C> {
    vec![
        C::new("geoflux.thermal.measure", "Measure Thermal Photo", "GeoFlux › Thermal", measure_cmd)
            .view()
            .when(need_feature_layer)
            .params(
                r#"{"layer"?, "row": number, "photo"?: "index in the thermal slot (default 0)", "region"?: [x0, y0, x1, y1] (pixels; default the whole photo), "reference"?: [x0, y0, x1, y1] (a healthy component to compare with; default the ambient temperature)}"#,
            ),
        C::new("geoflux.thermal.record", "Record Thermal Finding", "GeoFlux › Thermal", record)
            .when(need_feature_layer)
            .params(
                r#"{"layer"?, "row": number, "photo"?, "region"?, "reference"?, "found"?: {"maxC": number, "referenceC"?: number} (measured already, by the app)}"#,
            ),
        C::new("geoflux.thermal.scan", "Scan Line Thermal", "GeoFlux › Thermal", scan)
            .when(need_feature_layer)
            .params(
                r#"{"layer"?, "results"?: [{"row": number, "maxC"?: number, "referenceC"?: number, "error"?: text}] (measured already, by the app)}"#,
            ),
    ]
}

/// The binding of a layer's form fields; the default one when the schema names none.
fn binding() -> FormBinding {
    FormBinding::default()
}

/// The paths in the asset's first thermal slot.
pub fn thermal_photos(s: &Session, layer: u32, row: usize) -> Result<Vec<String>, String> {
    let slots: Vec<SlotPhotos> = geoflux::photos(s, layer, row)?;
    let slot = slots.into_iter().find(|sp| sp.slot.thermal).ok_or("this layer's schema has no thermal image slot")?;
    Ok(slot.photos)
}

/// The scene overrides the asset's form gives (emissivity, ambient temperature, humidity).
pub fn form_overrides(s: &Session, layer: u32, row: usize) -> Result<SceneOverrides, String> {
    let (_, asset) = geoflux::asset(s, layer, row)?;
    Ok(binding().overrides(|k| Some(asset.property(k).to_string()).filter(|v| !v.trim().is_empty())))
}

/// Reads and decodes one photo. Safe to call from any thread with the session's decoders.
pub fn decode_file(decoders: &Decoders, path: &str, overrides: &SceneOverrides) -> Result<ThermalFrame, ThermalError> {
    let bytes = read_photo(path)?;
    decoders.decode(&bytes, overrides)
}

fn read_photo(path: &str) -> Result<Vec<u8>, ThermalError> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = path;
        Err(ThermalError::HelperUnavailable("the web app can't read photo folders".into()))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::io::Read;
        let file = std::fs::File::open(path).map_err(|e| ThermalError::Damaged(format!("it can't be opened ({e})")))?;
        let mut bytes = Vec::new();
        // One byte past the limit tells an oversized file from one exactly at it.
        file.take(MAX_PHOTO_BYTES + 1).read_to_end(&mut bytes).map_err(|e| ThermalError::Damaged(e.to_string()))?;
        if bytes.len() as u64 > MAX_PHOTO_BYTES {
            return Err(ThermalError::Damaged("the file is too large for a thermal photo".into()));
        }
        Ok(bytes)
    }
}

fn region_param(p: &Value, key: &str) -> Result<Option<Region>, String> {
    let Some(v) = p.get(key).filter(|v| !v.is_null()) else { return Ok(None) };
    let n: Vec<u32> = v
        .as_array()
        .filter(|a| a.len() == 4)
        .map(|a| {
            a.iter().filter_map(|x| x.as_f64()).filter(|x| x.is_finite() && *x >= 0.0).map(|x| x.min(f64::from(u32::MAX)) as u32).collect()
        })
        .unwrap_or_default();
    match n.as_slice() {
        [x0, y0, x1, y1] => Ok(Some(Region { a: Pixel { x: *x0, y: *y0 }, b: Pixel { x: *x1, y: *y1 } })),
        _ => Err(format!("{key} must be [x0, y0, x1, y1] in pixels")),
    }
}

fn row_param(p: &Value) -> Result<usize, String> {
    p.get("row").and_then(Value::as_u64).map(|r| r as usize).ok_or_else(|| "row is required".to_string())
}

/// The finding for a frame: the hottest pixel in `region`, over the hottest in `reference` or,
/// without one, over the ambient temperature (the form's, else the camera's).
pub fn finding(frame: &ThermalFrame, region: Option<Region>, reference: Option<Region>) -> Result<ThermalFinding, String> {
    let hot = frame.stats(region).ok_or("no pixel in the region has a reading")?;
    let reference = match reference {
        Some(r) => {
            let st = frame.stats(Some(r)).ok_or("no pixel in the reference has a reading")?;
            Some(Reference::Component { region: r, max_c: st.max_c })
        }
        None => Some(frame.scene().ambient_temp_c as f32).filter(|t| t.is_finite()).map(Reference::Ambient),
    };
    Ok(ThermalFinding { max_c: hot.max_c, at: hot.max_at, region, reference })
}

/// Opens the asset's thermal photo and decodes it with the form's settings.
fn frame_of(s: &Session, p: &Value) -> Result<(u32, usize, String, ThermalFrame), String> {
    let id = s.layer_id(p)?;
    let row = row_param(p)?;
    let index = p.get("photo").and_then(Value::as_u64).unwrap_or(0) as usize;
    let photos = thermal_photos(s, id, row)?;
    let path = photos.get(index).cloned().ok_or("this tower has no thermal photo linked (Link Photos first)")?;
    let overrides = form_overrides(s, id, row)?;
    let frame = decode_file(&s.thermal.decoders, &path, &overrides).map_err(|e| e.to_string())?;
    Ok((id, row, path, frame))
}

fn stats_json(frame: &ThermalFrame, region: Option<Region>) -> Value {
    match frame.stats(region) {
        Some(st) => json!({
            "minC": st.min_c, "maxC": st.max_c, "meanC": st.mean_c,
            "minAt": [st.min_at.x, st.min_at.y], "maxAt": [st.max_at.x, st.max_at.y], "pixels": st.valid_pixels,
        }),
        None => Value::Null,
    }
}

fn measure_cmd(s: &mut Session, p: &Value) -> CmdResult {
    let region = region_param(p, "region")?;
    let reference = region_param(p, "reference")?;
    let (id, row, path, frame) = frame_of(s, p)?;
    let f = finding(&frame, region, reference)?;
    Ok(json!({
        "layer": id,
        "row": row,
        "photo": path,
        "vendor": frame.vendor(),
        "width": frame.width(),
        "height": frame.height(),
        "scene": frame.scene(),
        "refused": frame.refused(),
        "stats": stats_json(&frame, None),
        "region": region.map(|_| stats_json(&frame, region)),
        "referenceC": f.reference.map(|r| r.temp_c()),
        "riseC": f.rise_c(),
    }))
}

/// Writes a finding into the asset's form (Fault Temp, Rise Temp) as one undoable edit.
fn record(s: &mut Session, p: &Value) -> CmdResult {
    let id = s.layer_id(p)?;
    let row = row_param(p)?;
    let finding = match p.get("found").filter(|v| v.is_object()) {
        Some(found) => {
            let max_c = found.get("maxC").and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or("found.maxC must be a number")? as f32;
            let reference = found.get("referenceC").and_then(Value::as_f64).filter(|v| v.is_finite()).map(|t| Reference::Ambient(t as f32));
            ThermalFinding { max_c, at: Pixel { x: 0, y: 0 }, region: None, reference }
        }
        None => {
            let region = region_param(p, "region")?;
            let reference = region_param(p, "reference")?;
            let (_, _, _, frame) = frame_of(s, p)?;
            finding(&frame, region, reference)?
        }
    };
    let values: serde_json::Map<String, Value> = finding.form_values(&binding()).into_iter().map(|(k, v)| (k, Value::String(v))).collect();
    if values.is_empty() {
        return Err("this layer's form has no fault or rise temperature field".into());
    }
    let out = geoflux::set_values(s, &json!({"layer": id, "row": row, "values": values}))?;
    Ok(json!({"layer": id, "row": row, "maxC": finding.max_c, "riseC": finding.rise_c(), "severity": out.get("severity")}))
}

/// Measures every tower's thermal photo and records each hottest pixel and its rise over ambient,
/// all as one undoable edit. Towers without a thermal photo are skipped; ones whose photo can't
/// be read are counted and listed.
fn scan(s: &mut Session, p: &Value) -> CmdResult {
    let id = s.layer_id(p)?;
    let rows: Vec<(usize, Result<ThermalFinding, String>)> = match p.get("results").and_then(Value::as_array) {
        Some(given) => given
            .iter()
            .filter_map(|r| {
                let row = r.get("row").and_then(Value::as_u64)? as usize;
                let found = match (r.get("maxC").and_then(Value::as_f64).filter(|v| v.is_finite()), r.get("error").and_then(Value::as_str))
                {
                    (Some(max), _) => Ok(ThermalFinding {
                        max_c: max as f32,
                        at: Pixel { x: 0, y: 0 },
                        region: None,
                        reference: r
                            .get("referenceC")
                            .and_then(Value::as_f64)
                            .filter(|v| v.is_finite())
                            .map(|t| Reference::Ambient(t as f32)),
                    }),
                    (None, Some(e)) => Err(e.chars().take(200).collect()),
                    (None, None) => return None,
                };
                Some((row, found))
            })
            .collect(),
        None => {
            let stops = geoflux::sequence(s, id, false)?;
            let mut out = Vec::new();
            for st in stops {
                let Some(path) = thermal_photos(s, id, st.row)?.into_iter().next() else { continue };
                let overrides = form_overrides(s, id, st.row)?;
                let found =
                    decode_file(&s.thermal.decoders, &path, &overrides).map_err(|e| e.to_string()).and_then(|f| finding(&f, None, None));
                out.push((st.row, found));
            }
            out
        }
    };
    let (mut recorded, mut failed) = (0usize, Vec::new());
    for (row, found) in rows {
        let label = geoflux::asset(s, id, row).map(|(schema, a)| a.property(schema.label_key()).to_string()).unwrap_or_default();
        let result = found.and_then(|f| {
            let values: serde_json::Map<String, Value> =
                f.form_values(&binding()).into_iter().map(|(k, v)| (k, Value::String(v))).collect();
            geoflux::set_values(s, &json!({"layer": id, "row": row, "values": values}))
        });
        match result {
            Ok(_) => recorded += 1,
            Err(e) => failed.push(json!({"row": row, "tower": label, "error": e})),
        }
    }
    let failures = failed.len();
    failed.truncate(MAX_LISTED);
    if failures > 0 {
        s.notify(format!("{failures} thermal photos could not be measured (geoflux.thermal.scan lists them)"), false);
    }
    Ok(json!({"layer": id, "recorded": recorded, "failed": failures, "failures": failed}))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use geoflux_thermal::testing::{Scene, jpeg, raw_for};

    use super::*;
    use crate::Project;

    const SHEET: &str = "Tower No.,Line Name,Lat.,Long.,Ambeint Temp.,Emissivity\n\
        76_0,Bhadla-Sikar,27.50,75.16,30,\n\
        77_0,Bhadla-Sikar,27.51,75.17,30,0.9\n\
        78_0,Bhadla-Sikar,27.52,75.18,30,\n";

    /// A line with FLIR photos for 76_0 (a 90 °C hot spot) and 77_0, a damaged one for 78_0.
    fn line(tag: &str) -> (Session, u32, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("geoflux-thermal-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let raw: Vec<u16> = [20.0, 25.0, 30.0, 40.0, 50.0, 60.0, 80.0, 90.0].iter().map(|t| raw_for(*t)).collect();
        let flir = jpeg(4, 2, &raw, Scene { emissivity: 1.0, distance: 0.0 }, false, 1, false);
        std::fs::write(dir.join("76_0_thermal.jpg"), &flir).expect("write");
        std::fs::write(dir.join("77_0_thermal.jpg"), &flir).expect("write");
        std::fs::write(dir.join("78_0_thermal.jpg"), &flir[..flir.len() / 2]).expect("write");
        let mut s = Session::new(Project::default());
        let r = s.run("geoflux.importCsv", &json!({"text": SHEET})).expect("import");
        let id = r["layer"].as_u64().expect("id") as u32;
        s.run("geoflux.linkPhotos", &json!({"layer": id, "folder": dir.to_string_lossy()})).expect("link");
        (s, id, dir)
    }

    fn row_of(s: &Session, id: u32, label: &str) -> usize {
        geoflux::sequence(s, id, false).expect("seq").into_iter().find(|st| st.label == label).expect("tower").row
    }

    #[test]
    fn a_tower_is_measured_with_its_form_settings_and_recorded_as_one_undo() {
        let (mut s, id, dir) = line("record");
        let row = row_of(&s, id, "76_0");
        let m = s.run("geoflux.thermal.measure", &json!({"layer": id, "row": row})).expect("measure");
        assert_eq!(m["vendor"], "Flir");
        let max = m["stats"]["maxC"].as_f64().expect("max");
        assert!((max - 90.0).abs() < 0.2, "max {max}");
        // The form's ambient (30 °C) is the reference: rise = 90 - 30.
        assert!((m["riseC"].as_f64().expect("rise") - 60.0).abs() < 0.3);
        let boxed = s.run("geoflux.thermal.measure", &json!({"layer": id, "row": row, "region": [0, 0, 1, 0]})).expect("box");
        assert!((boxed["region"]["maxC"].as_f64().expect("box max") - 25.0).abs() < 0.2);

        // The form's emissivity is used: 77_0 says 0.9, so the same counts read hotter.
        let row77 = row_of(&s, id, "77_0");
        let m77 = s.run("geoflux.thermal.measure", &json!({"layer": id, "row": row77})).expect("measure 77");
        assert!(m77["stats"]["maxC"].as_f64().expect("max") > max + 1.0);
        assert!((m77["scene"]["emissivity"].as_f64().expect("e") - 0.9).abs() < 1e-6);

        let depth = s.undo_depth();
        s.run("geoflux.thermal.record", &json!({"layer": id, "row": row})).expect("record");
        assert_eq!(s.undo_depth(), depth + 1, "one undo step");
        let (_, asset) = geoflux::asset(&s, id, row).expect("asset");
        assert_eq!(asset.property("faultTemp"), "90.0");
        assert_eq!(asset.property("riseTemp"), "60.0");
        assert!(s.undo());
        assert_eq!(geoflux::asset(&s, id, row).expect("asset").1.property("faultTemp"), "", "undo takes the finding back");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_scan_records_every_readable_tower_and_lists_the_rest() {
        let (mut s, id, dir) = line("scan");
        let depth = s.undo_depth();
        let r = s.run("geoflux.thermal.scan", &json!({"layer": id})).expect("scan");
        assert_eq!(s.undo_depth(), depth + 1, "the whole line is one undo step");
        assert_eq!(r["recorded"], 2);
        assert_eq!(r["failed"], 1);
        assert_eq!(r["failures"][0]["tower"], "78_0");
        let row = row_of(&s, id, "76_0");
        assert_eq!(geoflux::asset(&s, id, row).expect("asset").1.property("faultTemp"), "90.0");
        // The app's own results are taken as given.
        let r = s
            .run("geoflux.thermal.scan", &json!({"layer": id, "results": [{"row": row, "maxC": 101.26, "referenceC": 31.0}]}))
            .expect("given");
        assert_eq!(r["recorded"], 1);
        assert_eq!(geoflux::asset(&s, id, row).expect("asset").1.property("riseTemp"), "70.3");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bad_requests_fail_cleanly() {
        let (mut s, id, dir) = line("bad");
        let row = row_of(&s, id, "78_0");
        assert!(s.run("geoflux.thermal.measure", &json!({"layer": id, "row": row})).is_err(), "damaged photo");
        assert!(s.run("geoflux.thermal.measure", &json!({"layer": id, "row": 99})).is_err());
        assert!(s.run("geoflux.thermal.measure", &json!({"layer": id, "row": 0, "region": [1, 2]})).is_err());
        assert!(s.run("geoflux.thermal.measure", &json!({"layer": id, "row": 0, "photo": 5})).is_err());
        assert!(s.run("geoflux.thermal.record", &json!({"layer": id, "row": 0, "found": {"maxC": "hot"}})).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
