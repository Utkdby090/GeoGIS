//! Why a thermal photo could not be turned into temperatures.

use crate::model::SceneSetting;

/// Every way a decode can fail. The `Display` text is written for the inspector, never a code
/// path or a debug dump; the variant tells the caller what it can do about it.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ThermalError {
    /// A normal photo, or a thermal picture saved without its temperature data (a screenshot,
    /// an edited copy). Show it as a plain picture.
    #[error("This photo has no temperature data.")]
    NotRadiometric,

    /// The file claims a radiometric format but its data is broken (truncated, wrong sizes).
    #[error("This thermal photo is damaged: {0}.")]
    Damaged(String),

    /// A real radiometric file in a variant this version can't read (a new raw encoding, a
    /// super-resolution picture).
    #[error("This thermal photo's format is not supported: {0}.")]
    Unsupported(String),

    /// Bigger than [`crate::MAX_PIXELS`]; refused before anything is allocated.
    #[error("This thermal photo is too large ({width}×{height}).")]
    TooLarge { width: u32, height: u32 },

    /// DJI photos need the `geoflux-dirp` helper and DJI's libraries next to GeoGIS, and they
    /// aren't there (or this is the browser).
    #[error("DJI temperatures need the desktop app's thermal helper: {0}")]
    HelperUnavailable(String),

    /// The helper stopped while reading this photo, most likely a crash inside DJI's SDK on a
    /// damaged file. Only this photo is lost; the next one starts a fresh helper.
    #[error("This DJI photo could not be read safely (the thermal helper stopped).")]
    HelperCrashed,

    /// The helper took longer than its limit on this photo and was stopped.
    #[error("Reading this DJI photo took too long and was stopped.")]
    HelperTimeout,

    /// The helper answered with something that breaks the protocol (a different version, a
    /// wrong size). Treated like a crash: the helper is restarted.
    #[error("The thermal helper gave an unexpected answer: {0}")]
    Protocol(String),

    /// DJI's SDK refused the file with one of its error codes.
    #[error("DJI's thermal library could not read this photo: {meaning}.")]
    Sdk { code: i32, meaning: String },

    /// A scene setting from the form is out of the camera's range and none of the fallbacks
    /// worked. Not raised when the decoder could fall back to the camera's own value; that is
    /// reported in [`crate::model::ThermalFrame::refused`] instead.
    #[error("The camera can't measure with this {0}.")]
    SettingRejected(SceneSetting),
}

impl ThermalError {
    /// Whether showing the photo as a plain picture (without temperatures) is the right fallback.
    pub fn picture_still_usable(&self) -> bool {
        !matches!(self, Self::Damaged(_) | Self::TooLarge { .. })
    }

    /// Whether the helper process should be replaced before the next photo.
    pub fn restarts_helper(&self) -> bool {
        matches!(self, Self::HelperCrashed | Self::HelperTimeout | Self::Protocol(_))
    }

    /// The meaning of a DJI SDK return code (`dirp_ret_code_e` in `dirp_api.h`), for [`Self::Sdk`].
    pub fn dji_code_meaning(code: i32) -> &'static str {
        match code {
            -1 => "out of memory",
            -2 | -3 | -8 | -9 => "it was called incorrectly",
            -7..=-4 => "it is not a valid DJI radiometric JPEG",
            -10 | -11 => "the image format is wrong",
            -12 => "this camera or function is not supported",
            -13 => "the library is not ready (a file may be missing)",
            -14 => "the library could not be activated",
            -15 | -16 => "the library set is incomplete",
            -64 => "super-resolution pictures are not supported",
            -63..=-32 => "it reported an internal error",
            _ => "it reported an unknown error",
        }
    }
}
