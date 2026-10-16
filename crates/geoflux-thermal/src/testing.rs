//! Radiometric FLIR files built in memory, for tests here and in the engine (images aren't
//! committed to this repository). Not part of the API.
#![allow(clippy::expect_used, clippy::cast_possible_truncation)]

use crate::calibration::KELVIN;

// Typical constants of a FLIR T-series camera (the same as GeoFlux's fixtures).
const R1: f64 = 21106.77;
const R2: f64 = 0.012545258;
const B: f64 = 1501.0;
const F: f64 = 1.0;
const O: i32 = -7340;

/// Sensor counts of a perfect black body at `t` °C.
pub fn raw_for(t: f64) -> u16 {
    (R1 / (R2 * ((B / (t + KELVIN)).exp() - F)) - f64::from(O)).round() as u16
}

pub struct Scene {
    pub emissivity: f32,
    pub distance: f32,
}

/// A radiometric FLIR JPEG in memory, following the published layout (APP1 "FLIR\0" chunks →
/// FFF stream → record index → raw PNG + camera parameters).
pub fn jpeg(w: u32, h: u32, raw: &[u16], scene: Scene, big_endian: bool, chunks: usize, reverse: bool) -> Vec<u8> {
    let put16 =
        |b: &mut Vec<u8>, at: usize, v: u16| b[at..at + 2].copy_from_slice(&if big_endian { v.to_be_bytes() } else { v.to_le_bytes() });
    let put32 =
        |b: &mut Vec<u8>, at: usize, v: u32| b[at..at + 4].copy_from_slice(&if big_endian { v.to_be_bytes() } else { v.to_le_bytes() });
    let putf = |b: &mut Vec<u8>, at: usize, v: f32| put32(b, at, v.to_bits());

    let mut png_bytes = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png_bytes, w, h);
        enc.set_color(png::ColorType::Grayscale);
        enc.set_depth(png::BitDepth::Sixteen);
        let mut wr = enc.write_header().expect("in-memory PNG");
        // FLIR stores the samples byte-swapped.
        let data: Vec<u8> = raw.iter().flat_map(|v| v.swap_bytes().to_be_bytes()).collect();
        wr.write_image_data(&data).expect("in-memory PNG");
    }
    let mut raw_block = vec![0u8; 32];
    put16(&mut raw_block, 0, 2);
    raw_block.extend_from_slice(&png_bytes);

    let mut params = vec![0u8; 0x400];
    put16(&mut params, 0, 2);
    putf(&mut params, 0x20, scene.emissivity);
    putf(&mut params, 0x24, scene.distance);
    putf(&mut params, 0x28, 20.0 + KELVIN as f32);
    putf(&mut params, 0x2C, 20.0 + KELVIN as f32);
    putf(&mut params, 0x30, 20.0 + KELVIN as f32);
    putf(&mut params, 0x34, 1.0);
    putf(&mut params, 0x3C, 0.5);
    putf(&mut params, 0x58, R1 as f32);
    putf(&mut params, 0x5C, B as f32);
    putf(&mut params, 0x60, F as f32);
    put32(&mut params, 0x308, O as u32);
    putf(&mut params, 0x30C, R2 as f32);

    let raw_off = 0x40;
    let par_off = raw_off + raw_block.len();
    let idx_off = par_off + params.len();
    let mut fff = vec![0u8; idx_off + 2 * 32];
    fff[..4].copy_from_slice(b"FFF\0");
    put32(&mut fff, 0x14, 100);
    put32(&mut fff, 0x18, idx_off as u32);
    put32(&mut fff, 0x1C, 2);
    fff[raw_off..par_off].copy_from_slice(&raw_block);
    fff[par_off..idx_off].copy_from_slice(&params);
    for (i, (kind, off, len)) in [(1u16, raw_off, raw_block.len()), (0x20, par_off, params.len())].into_iter().enumerate() {
        let at = idx_off + i * 32;
        put16(&mut fff, at, kind);
        put32(&mut fff, at + 12, off as u32);
        put32(&mut fff, at + 16, len as u32);
    }

    let size = fff.len().div_ceil(chunks);
    let mut parts: Vec<(usize, &[u8])> = fff.chunks(size).enumerate().collect();
    let last = (parts.len() - 1) as u8;
    if reverse {
        parts.reverse();
    }
    let mut out = vec![0xFF, 0xD8];
    for (i, part) in parts {
        let mut payload = b"FLIR\0\x01".to_vec();
        payload.extend_from_slice(&[i as u8, last]);
        payload.extend_from_slice(part);
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&payload);
    }
    out.extend_from_slice(&[0xFF, 0xD9]);
    out
}
