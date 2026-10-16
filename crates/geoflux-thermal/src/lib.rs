//! GeoFlux L1: thermal photos to temperatures, independent of any UI, process or camera SDK.
//!
//! - [`model`]: a frame of temperatures, the scene settings it was measured with, regions and
//!   statistics.
//! - [`decoder`]: the [`ThermalDecoder`] contract one vendor format implements, and [`Decoders`],
//!   which picks the right one for a file.
//! - [`dji`]: DJI R-JPEGs. Their calibration is DJI's secret, so the decoder hands the bytes to a
//!   [`DjiBackend`]; this crate never loads DJI's libraries itself.
//! - [`protocol`]: the pipe protocol between GeoGIS and the `geoflux-dirp` helper program, which is
//!   the one place DJI's SDK runs (in its own process, so a damaged photo can't close GeoGIS).
//! - [`palette`]: colour ramps for showing a frame.
//! - [`findings`]: a measurement turned into form values (fault temperature, rise over reference).
//! - [`error`]: [`ThermalError`], whose messages are safe to show to the user.
//!
//! Everything here works on bytes, so it builds for the browser too (FLIR there; DJI needs the
//! desktop helper).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod decoder;
pub mod dji;
pub mod error;
pub mod findings;
pub mod model;
pub mod palette;
pub mod protocol;

pub use decoder::{Decoders, Sniff, ThermalDecoder};
pub use dji::{DjiBackend, DjiMeasurement};
pub use error::ThermalError;
pub use findings::{FormBinding, ThermalFinding};
pub use model::{
    MAX_PIXELS, Pixel, PlanckConstants, Region, SceneOverrides, SceneParams, SceneSetting, ThermalFrame, ThermalStats, Vendor,
};
pub use palette::{Palette, TempRange};
