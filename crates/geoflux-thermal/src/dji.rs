//! DJI radiometric JPEGs (R-JPEG). DJI's calibration is not published, so temperatures come from
//! DJI's Thermal SDK through a [`DjiBackend`]. This crate never loads the SDK: on the desktop the
//! backend is the `geoflux-dirp` helper process (see [`crate::protocol`]); in tests it is a fake.

use serde::{Deserialize, Serialize};

use crate::decoder::{Sniff, ThermalDecoder};
use crate::error::ThermalError;
use crate::model::{SceneOverrides, SceneParams, SceneSetting, ThermalFrame, Vendor};

/// What the SDK measured for one photo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DjiMeasurement {
    pub width: u32,
    pub height: u32,
    /// °C, row by row, `width × height` values.
    pub temps: Vec<f32>,
    /// The settings the SDK actually measured with.
    pub applied: SceneParams,
    /// Overrides the SDK refused even on their own (their camera values were kept).
    pub refused: Vec<SceneSetting>,
    /// The camera model as the file names it ("M3T", "ZH20T"), for messages; empty when unknown.
    pub camera: String,
}

/// Something that runs DJI's SDK on one photo.
///
/// The contract, which the helper implements and [`DjiDecoder`] relies on:
/// - each override is clamped to the range the SDK reports for that photo;
/// - when the SDK refuses the overrides together, each is tried alone, and the ones it still
///   refuses keep the camera's value and are listed in [`DjiMeasurement::refused`];
/// - a crash or hang while reading the photo comes back as [`ThermalError::HelperCrashed`] or
///   [`ThermalError::HelperTimeout`], never as a crash of the caller.
pub trait DjiBackend: Send + Sync {
    fn measure(&self, jpeg: &[u8], overrides: &SceneOverrides) -> Result<DjiMeasurement, ThermalError>;
}

/// The [`ThermalDecoder`] for DJI files, over any backend.
pub struct DjiDecoder<B: DjiBackend> {
    backend: B,
}

impl<B: DjiBackend> DjiDecoder<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

impl<B: DjiBackend> ThermalDecoder for DjiDecoder<B> {
    fn name(&self) -> &'static str {
        "DJI radiometric JPEG"
    }

    /// `Yes` when the JPEG's metadata names DJI and a thermal camera; `Maybe` when it names DJI
    /// only (some thermal models don't say so, and only the SDK can tell); `No` otherwise.
    fn sniff(&self, bytes: &[u8]) -> Sniff {
        if !bytes.starts_with(&[0xFF, 0xD8]) {
            return Sniff::No;
        }
        // Metadata segments sit at the front; DJI writes its maker name in the EXIF there.
        let head = bytes.get(..bytes.len().min(64 * 1024)).unwrap_or_default();
        let has = |needle: &[u8]| head.windows(needle.len()).any(|w| w == needle);
        if !has(b"DJI") {
            return Sniff::No;
        }
        if has(b"Thermal") || has(b"thermal") || has(b"IR") { Sniff::Yes } else { Sniff::Maybe }
    }

    fn decode(&self, bytes: &[u8], overrides: &SceneOverrides) -> Result<ThermalFrame, ThermalError> {
        if !bytes.starts_with(&[0xFF, 0xD8]) {
            return Err(ThermalError::NotRadiometric);
        }
        let m = self.backend.measure(bytes, overrides)?;
        // The backend is another process: its answer is checked like any input.
        Ok(ThermalFrame::new(m.width, m.height, m.temps, Vendor::Dji, m.applied)?.with_refused(m.refused))
    }
}
