//! Colour ramps for showing a frame.

use serde::{Deserialize, Serialize};

use crate::model::ThermalFrame;

/// A colour ramp from cold to hot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Palette {
    #[default]
    Iron,
    Rainbow,
    WhiteHot,
    BlackHot,
}

impl Palette {
    pub const ALL: [Palette; 4] = [Palette::Iron, Palette::Rainbow, Palette::WhiteHot, Palette::BlackHot];

    /// Evenly spaced RGB stops, cold first (the same ramps as GeoFlux).
    fn stops(self) -> &'static [[u8; 3]] {
        match self {
            Palette::Iron => &[
                [0x00, 0x00, 0x0A],
                [0x20, 0x00, 0x6E],
                [0x7A, 0x0A, 0x8C],
                [0xC8, 0x32, 0x1E],
                [0xF0, 0x8C, 0x00],
                [0xFF, 0xD2, 0x3C],
                [0xFF, 0xFF, 0xE6],
            ],
            Palette::Rainbow => &[
                [0x00, 0x00, 0xA0],
                [0x00, 0x64, 0xFF],
                [0x00, 0xD2, 0xC8],
                [0x32, 0xDC, 0x32],
                [0xFF, 0xEB, 0x00],
                [0xFF, 0x78, 0x00],
                [0xDC, 0x00, 0x00],
            ],
            Palette::WhiteHot => &[[0, 0, 0], [255, 255, 255]],
            Palette::BlackHot => &[[255, 255, 255], [0, 0, 0]],
        }
    }

    /// The colour `fraction` of the way from cold (0) to hot (1); outside is clamped, NaN is cold.
    pub fn rgb(self, fraction: f32) -> [u8; 3] {
        let stops = self.stops();
        let f = if fraction.is_nan() { 0.0 } else { fraction.clamp(0.0, 1.0) };
        let scaled = f * (stops.len() - 1) as f32;
        let i = (scaled as usize).min(stops.len().saturating_sub(2));
        let t = scaled - i as f32;
        let (Some(a), Some(b)) = (stops.get(i), stops.get(i + 1)) else { return [0, 0, 0] };
        std::array::from_fn(|c| {
            let (a, b) = (f32::from(a[c]), f32::from(b[c]));
            (a + (b - a) * t).round().clamp(0.0, 255.0) as u8
        })
    }

    /// RGBA pixels for `frame` scaled over `range`, written into `out` (resized and reused, so
    /// redrawing on every slider move allocates nothing after the first time). Pixels without a
    /// reading are transparent.
    pub fn render_into(self, frame: &ThermalFrame, range: TempRange, out: &mut Vec<u8>) {
        let (lo, hi) = range.bounds(frame);
        let span = if hi - lo > 1e-6 { hi - lo } else { 1.0 };
        out.clear();
        out.reserve(frame.temps().len() * 4);
        for &t in frame.temps() {
            if t.is_nan() {
                out.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                let [r, g, b] = self.rgb((t - lo) / span);
                out.extend_from_slice(&[r, g, b, 255]);
            }
        }
    }
}

/// The temperatures a palette spans.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum TempRange {
    /// The frame's own coldest to hottest.
    #[default]
    Auto,
    /// Fixed bounds in °C, so photos of a line are compared on one scale.
    Fixed { lo: f32, hi: f32 },
}

impl TempRange {
    /// `(lo, hi)` in °C, lowest first; `(0, 1)` for a frame with no valid pixel.
    pub fn bounds(self, frame: &ThermalFrame) -> (f32, f32) {
        match self {
            TempRange::Fixed { lo, hi } if lo.is_finite() && hi.is_finite() => (lo.min(hi), lo.max(hi)),
            _ => frame.stats(None).map_or((0.0, 1.0), |s| (s.min_c, s.max_c)),
        }
    }
}
