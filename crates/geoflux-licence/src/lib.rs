//! GeoFlux L1: which paid GeoGIS features a signed-in account unlocks.
//!
//! GeoGIS is free to download. Signing in to a GeoFlux account (Pro or Organisation plan) unlocks
//! the paid inspection features. The account server signs a pass for this computer that keeps
//! working offline for 30 days and is renewed whenever the app is online; sites with no internet
//! use an offline pass file from the account page instead.
//!
//! - [`pass`]: the signed pass (Ed25519) and what it unlocks.
//! - [`account`]: the sign-in, renewal and sign-out contract with the account server.
//! - [`machine`]: the computer ID a pass and a seat are tied to.
//! - [`gate`]: the decision at start-up and after sign-in, over pluggable store, clock and machine ID.
//!
//! The app always opens; without a usable pass it runs in the free tier. Nothing here runs a
//! program, touches the disk or the network: the desktop app supplies those parts.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod account;
pub mod date;
mod error;
pub mod gate;
pub mod machine;
pub mod pass;

pub use account::{AccountError, AccountService, DeviceCode, Grant, SignInPoll};
pub use date::CivilDate;
pub use error::LicenceError;
pub use gate::{Access, Clock, FreeReason, Gate, Licensing, PassStore, StoredPass};
pub use machine::{MachineId, MachineIdSource};
#[cfg(not(target_arch = "wasm32"))]
pub use pass::Ed25519Keys;
pub use pass::{Feature, ONLINE_PASS_DAYS, Pass, PassKind, PassStatus, PassVerifier, Plan};
