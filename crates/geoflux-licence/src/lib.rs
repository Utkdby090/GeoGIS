//! GeoFlux L1: the desktop app's licence key and 15-day trial.
//!
//! A port of GeoFlux's `LicenseManager.kt`, `TrialManager.kt` and `HardwareUtil.kt`, so keys the
//! GeoFlux licence server already issued keep working and its signing service doesn't change:
//!
//! - [`key`]: `payload.signature` keys, RSA SHA-256 over `MachineID|YYYY-MM-DD`.
//! - [`machine`]: the Windows SMBIOS UUID a key is locked to.
//! - [`trial`]: 15 days from the first launch; a clock moved back a day or more locks.
//! - [`gate`]: the decision at start-up and on activation, over pluggable stores and clock.
//!
//! Two GeoFlux behaviours are deliberately not ported: the `SECRET_ADMIN_DEBUG` key that unlocked
//! any copy, and clearing a valid saved key when the machine ID couldn't be read.
//!
//! Only the desktop window is licensed; `rustgis-cli` and the browser build are not. Nothing here
//! runs a program or touches the disk: the desktop app supplies those parts.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod date;
mod error;
pub mod gate;
pub mod key;
pub mod machine;
pub mod trial;

pub use date::CivilDate;
pub use error::LicenceError;
pub use gate::{Access, Clock, Gate, LicenceStore, Licensing, LockReason};
#[cfg(not(target_arch = "wasm32"))]
pub use key::RsaSha256;
pub use key::{GEOFLUX_PUBLIC_KEY, KeyStatus, SignatureVerifier, SignedPayload, check_key};
pub use machine::{MachineId, MachineIdSource};
pub use trial::{TRIAL_DAYS, TrialStart, TrialStatus, TrialStore};
