//! Radiometric FLIR JPEGs (FLIR cameras, and drone payloads that write FLIR's format), in pure Rust.
//! Ported from GeoFlux's `FlirDecoder`.
//!
//! The thermal data sits in JPEG APP1 segments that start with `FLIR\0`. Their payloads, joined in
//! index order, form an "FFF" stream: a record index pointing at the raw 16-bit image (an embedded
//! PNG or TIFF) and at the camera parameters (Planck constants, emissivity, distance, humidity…).
//! Every offset and length read from the file is checked before use.

use crate::calibration::{Calibration, KELVIN};
use crate::decoder::{Sniff, ThermalDecoder};
use crate::error::ThermalError;
use crate::model::{PlanckConstants, SceneOverrides, SceneParams, ThermalFrame, Vendor, check_size};

const SEGMENT_HEADER: usize = 8; // "FLIR\0", version, index, last index
const RECORD_SIZE: usize = 32;
const MAX_RECORDS: usize = 1024;
const TYPE_RAW: u16 = 0x01;
const TYPE_PARAMS: u16 = 0x20;
const RAW_HEADER: usize = 32;
const PARAMS_MIN: usize = 0x310;
/// A real scene is somewhere between deep freeze and a furnace; used to tell whether raw values
/// are stored byte-swapped.
const PLAUSIBLE_C: std::ops::RangeInclusive<f32> = -80.0..=700.0;
/// The most bytes the embedded raw image may decode to (4096² 16-bit pixels).
const MAX_RAW_BYTES: usize = 4096 * 4096 * 2;

/// FLIR's published defaults for the atmosphere model, for files that leave them at zero.
const DEFAULT_ALPHA1: f64 = 0.006569;
const DEFAULT_ALPHA2: f64 = 0.01262;
const DEFAULT_BETA1: f64 = -0.002276;
const DEFAULT_BETA2: f64 = -0.00667;
const DEFAULT_X: f64 = 1.9;

/// Whether raw values are stored byte-swapped. FLIR writes PNG samples swapped, so `Auto` tries
/// swapped first and falls back when that gives impossible temperatures.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ByteSwap {
    #[default]
    Auto,
    Swapped,
    AsStored,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct FlirDecoder {
    pub swap: ByteSwap,
}

impl ThermalDecoder for FlirDecoder {
    fn name(&self) -> &'static str {
        "FLIR radiometric JPEG"
    }

    fn sniff(&self, bytes: &[u8]) -> Sniff {
        if segments(bytes).next().is_some() { Sniff::Yes } else { Sniff::No }
    }

    fn decode(&self, bytes: &[u8], overrides: &SceneOverrides) -> Result<ThermalFrame, ThermalError> {
        let fff = assemble(bytes).ok_or(ThermalError::NotRadiometric)?;
        let records = records(&fff)?;
        let raw_rec = records.iter().find(|r| r.kind == TYPE_RAW).ok_or_else(|| damaged("it has no raw thermal image"))?;
        let par_rec = records.iter().find(|r| r.kind == TYPE_PARAMS).ok_or_else(|| damaged("it has no camera parameters"))?;
        let (scene, planck) = params(&fff, par_rec)?;
        let scene = overrides.apply_to(scene);
        let (w, h, raw) = raw_image(&fff, raw_rec)?;
        let cal = Calibration::new(planck, &scene);
        let swap = match self.swap {
            ByteSwap::Swapped => true,
            ByteSwap::AsStored => false,
            ByteSwap::Auto => looks_swapped(&raw, &cal),
        };
        let temps = raw.iter().map(|&v| cal.temp_c(if swap { v.swap_bytes() } else { v })).collect();
        Ok(ThermalFrame::new(w, h, temps, Vendor::Flir, scene)?.with_planck(planck))
    }
}

fn damaged(why: &str) -> ThermalError {
    ThermalError::Damaged(why.to_string())
}

/// Judged by the median non-zero raw value: swapped unless only the stored order is plausible.
fn looks_swapped(raw: &[u16], cal: &Calibration) -> bool {
    // A sample of at most 4096 values keeps this cheap on any size.
    let step = (raw.len() / 4096).max(1);
    let mut sample: Vec<u16> = raw.iter().step_by(step).copied().filter(|v| *v != 0).collect();
    sample.sort_unstable();
    let median = sample.get(sample.len() / 2).copied().unwrap_or(0);
    let plausible = |v: u16| {
        let t = cal.temp_c(v);
        !t.is_nan() && PLAUSIBLE_C.contains(&t)
    };
    plausible(median.swap_bytes()) || !plausible(median)
}

// --- JPEG to FFF stream --------------------------------------------------------------------------

/// The FLIR APP1 segments, as `(index, payload)`, in file order. Stops at the image data.
fn segments(jpeg: &[u8]) -> impl Iterator<Item = (u8, &[u8])> {
    let mut pos = if jpeg.starts_with(&[0xFF, 0xD8]) { 2 } else { usize::MAX };
    std::iter::from_fn(move || {
        loop {
            let head = jpeg.get(pos..pos.checked_add(4)?)?;
            if head[0] != 0xFF {
                return None;
            }
            let marker = head[1];
            match marker {
                0xDA | 0xD9 => return None, // image data / end: no more metadata
                0xFF => {
                    pos += 1;
                    continue;
                }
                0x01 | 0xD0..=0xD7 => {
                    pos += 2;
                    continue;
                }
                _ => {}
            }
            let len = usize::from(u16::from_be_bytes([head[2], head[3]]));
            let end = pos + 2 + len;
            if len < 2 || end > jpeg.len() {
                return None;
            }
            let data = jpeg.get(pos + 4..end)?;
            pos = end;
            if marker == 0xE1 && data.len() > SEGMENT_HEADER && data.starts_with(b"FLIR\0") {
                return Some((data[6], &data[SEGMENT_HEADER..]));
            }
        }
    })
}

fn assemble(jpeg: &[u8]) -> Option<Vec<u8>> {
    let mut parts: Vec<(u8, &[u8])> = segments(jpeg).collect();
    if parts.is_empty() {
        return None;
    }
    parts.sort_by_key(|(i, _)| *i);
    Some(parts.iter().flat_map(|(_, p)| p.iter().copied()).collect())
}

// --- FFF records ---------------------------------------------------------------------------------

struct Record {
    kind: u16,
    offset: usize,
    len: usize,
}

#[derive(Clone, Copy)]
struct Order(bool); // true = little-endian

impl Order {
    fn u16(self, b: &[u8], at: usize) -> Option<u16> {
        let a: [u8; 2] = b.get(at..at + 2)?.try_into().ok()?;
        Some(if self.0 { u16::from_le_bytes(a) } else { u16::from_be_bytes(a) })
    }
    fn u32(self, b: &[u8], at: usize) -> Option<u32> {
        let a: [u8; 4] = b.get(at..at + 4)?.try_into().ok()?;
        Some(if self.0 { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) })
    }
    fn f32(self, b: &[u8], at: usize) -> Option<f64> {
        self.u32(b, at).map(|v| f64::from(f32::from_bits(v)))
    }
}

fn records(fff: &[u8]) -> Result<Vec<Record>, ThermalError> {
    if fff.len() < 0x40 || !fff.starts_with(b"FFF") {
        return Err(damaged("its FLIR data has no FFF header"));
    }
    let le = Order(true);
    let order = Order(le.u32(fff, 0x14).is_some_and(|v| (100..=200).contains(&v)));
    let bad = || damaged("its FLIR record index is broken");
    let index = order.u32(fff, 0x18).ok_or_else(bad)? as usize;
    let count = order.u32(fff, 0x1C).ok_or_else(bad)? as usize;
    if count > MAX_RECORDS || index.checked_add(count * RECORD_SIZE).is_none_or(|end| end > fff.len()) {
        return Err(bad());
    }
    Ok((0..count)
        .filter_map(|i| {
            let at = index + i * RECORD_SIZE;
            let kind = order.u16(fff, at)?;
            let offset = order.u32(fff, at + 12)? as usize;
            let len = order.u32(fff, at + 16)? as usize;
            (kind != 0 && len > 0 && offset.checked_add(len).is_some_and(|e| e <= fff.len())).then_some(Record { kind, offset, len })
        })
        .collect())
}

/// A block's own byte order: its first 16-bit word is 2 in its order.
fn order_at(fff: &[u8], offset: usize) -> Order {
    Order(Order(true).u16(fff, offset) == Some(2))
}

fn params(fff: &[u8], r: &Record) -> Result<(SceneParams, PlanckConstants), ThermalError> {
    if r.len < PARAMS_MIN {
        return Err(damaged("its FLIR camera parameters are cut short"));
    }
    let o = order_at(fff, r.offset);
    let f = |at: usize| o.f32(fff, r.offset + at).unwrap_or(f64::NAN);
    let or_default = |v: f64, d: f64| if v == 0.0 || !v.is_finite() { d } else { v };
    let humidity = f(0x3C);
    let scene = SceneParams {
        emissivity: f(0x20),
        distance_m: f(0x24),
        reflected_temp_c: f(0x28) - KELVIN,
        ambient_temp_c: f(0x2C) - KELVIN,
        humidity_pct: if humidity <= 1.5 { humidity * 100.0 } else { humidity },
    };
    let planck = PlanckConstants {
        r1: f(0x58),
        b: f(0x5C),
        f: f(0x60),
        atm_alpha1: or_default(f(0x70), DEFAULT_ALPHA1),
        atm_alpha2: or_default(f(0x74), DEFAULT_ALPHA2),
        atm_beta1: or_default(f(0x78), DEFAULT_BETA1),
        atm_beta2: or_default(f(0x7C), DEFAULT_BETA2),
        atm_x: or_default(f(0x80), DEFAULT_X),
        o: o.u32(fff, r.offset + 0x308).map_or(f64::NAN, |v| f64::from(v as i32)),
        r2: f(0x30C),
        ir_window_temp_c: f(0x30) - KELVIN,
        ir_window_transmission: f(0x34),
    };
    let essentials = [
        scene.emissivity,
        scene.distance_m,
        scene.reflected_temp_c,
        scene.ambient_temp_c,
        scene.humidity_pct,
        planck.r1,
        planck.r2,
        planck.b,
        planck.f,
        planck.o,
    ];
    if essentials.iter().any(|v| !v.is_finite()) || planck.r1 == 0.0 || planck.r2 == 0.0 || planck.b == 0.0 {
        return Err(damaged("its FLIR calibration constants are missing or unreadable"));
    }
    Ok((scene, planck))
}

// --- raw image -----------------------------------------------------------------------------------

fn raw_image(fff: &[u8], r: &Record) -> Result<(u32, u32, Vec<u16>), ThermalError> {
    let data = fff.get(r.offset..r.offset + r.len).and_then(|b| b.get(RAW_HEADER..)).filter(|b| !b.is_empty());
    let data = data.ok_or_else(|| damaged("its FLIR raw image is empty"))?;
    if data.starts_with(b"\x89PNG") {
        raw_png(data)
    } else if matches!(data.get(..4), Some([b'I', b'I', 42, 0] | [b'M', b'M', 0, 42])) {
        raw_tiff(data)
    } else {
        Err(ThermalError::Unsupported("this camera stores its raw image in a format that isn't supported yet".into()))
    }
}

fn raw_png(data: &[u8]) -> Result<(u32, u32, Vec<u16>), ThermalError> {
    let bad = |e: png::DecodingError| damaged(&format!("its raw PNG can't be read ({e})"));
    let mut dec = png::Decoder::new_with_limits(std::io::Cursor::new(data), png::Limits { bytes: MAX_RAW_BYTES });
    dec.set_transformations(png::Transformations::IDENTITY);
    let mut reader = dec.read_info().map_err(bad)?;
    let info = reader.info();
    let (w, h) = (info.width, info.height);
    check_size(w, h)?;
    if info.color_type != png::ColorType::Grayscale || info.bit_depth != png::BitDepth::Sixteen {
        return Err(ThermalError::Unsupported("the raw image is not 16-bit greyscale".into()));
    }
    let size = reader.output_buffer_size().ok_or_else(|| damaged("its raw PNG is too large"))?;
    let mut buf = vec![0u8; size];
    let out = reader.next_frame(&mut buf).map_err(bad)?;
    // PNG stores 16-bit samples big-endian, rows padded to line_size.
    let mut raw = Vec::with_capacity(w as usize * h as usize);
    for row in buf.chunks(out.line_size).take(h as usize) {
        raw.extend(row.as_chunks::<2>().0.iter().take(w as usize).map(|c| u16::from_be_bytes(*c)));
    }
    if raw.len() != w as usize * h as usize {
        return Err(damaged("its raw PNG is cut short"));
    }
    Ok((w, h, raw))
}

fn raw_tiff(data: &[u8]) -> Result<(u32, u32, Vec<u16>), ThermalError> {
    use tiff::decoder::{Decoder, DecodingResult, Limits};
    let bad = |e: tiff::TiffError| damaged(&format!("its raw TIFF can't be read ({e})"));
    let mut limits = Limits::default();
    limits.decoding_buffer_size = MAX_RAW_BYTES;
    limits.intermediate_buffer_size = MAX_RAW_BYTES;
    limits.ifd_value_size = 1 << 20;
    let mut dec = Decoder::new(std::io::Cursor::new(data)).map_err(bad)?.with_limits(limits);
    let (w, h) = dec.dimensions().map_err(bad)?;
    check_size(w, h)?;
    match dec.read_image().map_err(bad)? {
        DecodingResult::U16(v) if v.len() == w as usize * h as usize => Ok((w, h, v)),
        _ => Err(ThermalError::Unsupported("the raw image is not 16-bit greyscale".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Pixel, SceneOverrides};
    use crate::testing::{Scene, jpeg, raw_for};

    const TEMPS: [f64; 8] = [20.0, 25.0, 30.0, 40.0, 50.0, 60.0, 80.0, 90.0];

    fn sample(big_endian: bool, chunks: usize, reverse: bool) -> Vec<u8> {
        let raw: Vec<u16> = TEMPS.iter().map(|t| raw_for(*t)).collect();
        jpeg(4, 2, &raw, Scene { emissivity: 1.0, distance: 0.0 }, big_endian, chunks, reverse)
    }

    fn assert_temps(frame: &ThermalFrame) {
        assert_eq!((frame.width(), frame.height()), (4, 2));
        for (i, (got, want)) in frame.temps().iter().zip(TEMPS).enumerate() {
            assert!((f64::from(*got) - want).abs() < 0.1, "pixel {i}: {got} vs {want}");
        }
    }

    #[test]
    fn a_black_body_reads_back_its_temperatures_in_any_byte_order_and_chunking() {
        let d = FlirDecoder::default();
        assert_eq!(d.sniff(&sample(false, 1, false)), Sniff::Yes);
        assert_temps(&d.decode(&sample(false, 1, false), &SceneOverrides::default()).unwrap());
        assert_temps(&d.decode(&sample(true, 1, false), &SceneOverrides::default()).unwrap());
        assert_temps(&d.decode(&sample(false, 3, true), &SceneOverrides::default()).unwrap());
    }

    #[test]
    fn overrides_move_the_reading_the_right_way_and_are_reported() {
        let file = sample(false, 1, false);
        let d = FlirDecoder::default();
        let stored = d.decode(&file, &SceneOverrides::default()).unwrap();
        let o = SceneOverrides { emissivity: Some(0.7), ..Default::default() };
        let lower_e = d.decode(&file, &o).unwrap();
        let p = Pixel { x: 3, y: 1 };
        // Less emissive surfaces radiate less, so the same counts mean a hotter object.
        assert!(lower_e.temperature_at(p).unwrap() > stored.temperature_at(p).unwrap());
        assert_eq!(lower_e.scene().emissivity, 0.7);
        assert!(lower_e.planck().is_some());
    }

    #[test]
    fn damaged_and_plain_files_fail_without_panicking() {
        let d = FlirDecoder::default();
        let none = SceneOverrides::default();
        assert_eq!(d.decode(b"\xFF\xD8\xFF\xD9", &none), Err(ThermalError::NotRadiometric));
        assert_eq!(d.sniff(b"not a jpeg"), Sniff::No);
        let good = sample(false, 1, false);
        // Every truncation, and a flipped byte at every position, must fail cleanly or decode.
        for cut in (0..good.len()).step_by(7) {
            let _ = d.decode(&good[..cut], &none);
        }
        for at in (0..good.len()).step_by(3) {
            let mut bad = good.clone();
            bad[at] ^= 0xA5;
            let _ = d.decode(&bad, &none);
        }
    }

    /// A real camera file: GeoFlux's FLIR AX8 sample, whose burned-in overlay reads max 25.5, min
    /// 24.3, average 25.1 °C. Images aren't committed here; set `GEOFLUX_THERMAL_SAMPLES` to a
    /// folder holding `ax8-radiometric.jpg` to run it.
    #[test]
    fn the_ax8_sample_reads_what_the_camera_printed() {
        let Some(dir) = std::env::var_os("GEOFLUX_THERMAL_SAMPLES") else { return };
        let bytes = std::fs::read(std::path::Path::new(&dir).join("ax8-radiometric.jpg")).unwrap();
        let f = FlirDecoder::default().decode(&bytes, &SceneOverrides::default()).unwrap();
        let s = f.stats(None).unwrap();
        assert_eq!((f.width(), f.height(), s.valid_pixels), (80, 60, 4800));
        assert!((s.max_c - 25.5).abs() < 0.1, "max {}", s.max_c);
        assert!((s.min_c - 24.3).abs() < 0.15, "min {}", s.min_c);
        assert!((s.mean_c - 25.1).abs() < 0.15, "mean {}", s.mean_c);
        assert!((f.scene().emissivity - 0.95).abs() < 1e-3);
        assert!((f.scene().humidity_pct - 50.0).abs() < 1e-3);
        assert_eq!(f.planck().unwrap().o, -7142.0);
        let shot = std::fs::read(std::path::Path::new(&dir).join("flir-screenshot-not-radiometric.jpg")).unwrap();
        assert_eq!(FlirDecoder::default().sniff(&shot), Sniff::No);
    }
}
