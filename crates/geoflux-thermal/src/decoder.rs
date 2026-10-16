//! The contract one vendor format implements, and choosing a decoder for a file.

use crate::error::ThermalError;
use crate::model::{SceneOverrides, ThermalFrame};

/// How sure a decoder is that a file is its format, from a cheap look at the bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Sniff {
    No,
    /// The maker matches but only a full decode can tell (a DJI file whose metadata does not say
    /// thermal: some models, like the Mavic 2 Enterprise Advanced, don't).
    Maybe,
    Yes,
}

/// Turns the bytes of one image file into temperatures. One implementation per vendor format.
///
/// Implementations must not panic on any input, must check sizes before allocating, and must be
/// usable from several threads at once (the Inspection pane decodes off the screen thread).
pub trait ThermalDecoder: Send + Sync {
    /// A short name for messages and logs ("FLIR radiometric JPEG").
    fn name(&self) -> &'static str;

    /// A cheap check on the first bytes and metadata; no full decode.
    fn sniff(&self, bytes: &[u8]) -> Sniff;

    /// The temperatures, measured with `overrides` where the format allows.
    fn decode(&self, bytes: &[u8], overrides: &SceneOverrides) -> Result<ThermalFrame, ThermalError>;
}

/// The decoders GeoGIS knows, tried in order of how sure each is.
#[derive(Default)]
pub struct Decoders {
    list: Vec<Box<dyn ThermalDecoder>>,
}

impl Decoders {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, d: impl ThermalDecoder + 'static) -> Self {
        self.list.push(Box::new(d));
        self
    }

    /// Decodes with the most confident decoder; a `Maybe` one is tried only after every `Yes` one
    /// failed. [`ThermalError::NotRadiometric`] when none claims the file.
    pub fn decode(&self, bytes: &[u8], overrides: &SceneOverrides) -> Result<ThermalFrame, ThermalError> {
        let mut ranked: Vec<(Sniff, &dyn ThermalDecoder)> =
            self.list.iter().map(|d| (d.sniff(bytes), d.as_ref())).filter(|(s, _)| *s != Sniff::No).collect();
        ranked.sort_by_key(|r| std::cmp::Reverse(r.0));
        let mut first_err = None;
        for (_, d) in ranked {
            match d.decode(bytes, overrides) {
                Ok(frame) => return Ok(frame),
                Err(e) => {
                    first_err.get_or_insert(e);
                }
            }
        }
        Err(first_err.unwrap_or(ThermalError::NotRadiometric))
    }
}
