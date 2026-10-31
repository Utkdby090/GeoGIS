//! GeoFlux Cloud: the contract between the account server, the desktop app and the web app.
//!
//! Phase 7a. The server (an Axum binary, stage 3) implements this; the desktop app's sign-in and
//! Project › Sync call it; the React web app reads the same JSON.
//!
//! - [`ids`]: typed ids (`org_…`, `usr_…`, `dev_…`, `srv_…`) and photo hashes.
//! - [`model`]: organisations, people and roles, subscriptions, computers, surveys.
//! - [`plans`]: what a subscription and a role allow, decided in one place.
//! - [`auth`]: email and password rules, secret tokens, device codes, and the signed-in
//!   [`auth::Principal`] whose [`auth::OrgScope`] every organisation read needs.
//! - [`sync`]: the survey upload (a manifest, then photos by hash, then a commit).
//! - [`view`]: what the web app reads: a survey, its towers as GeoJSON, one tower, reports.
//! - [`health`]: the line health dashboard: towers rated by findings, tallies by defect and part,
//!   and one tower's parts for the 3D view.
//! - [`api`]: routes and request and response bodies.
//! - [`error`]: one error shape for every route, with its HTTP status.
//! - [`ports`]: storage, photos, mail, password hashing, randomness, rate limits and single
//!   sign-on as traits, so the hosting (Oracle, Render + Neon + R2, Supabase, AWS) is an adapter
//!   and the rules above never change with it.
//!
//! Nothing here touches the network, a database or the disk, so it builds for the browser too.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod api;
pub mod auth;
pub mod error;
pub mod health;
pub mod ids;
pub mod model;
pub mod plans;
pub mod ports;
pub mod sync;
pub mod view;

pub use error::{ApiError, ErrorCode, FieldError};
pub use ids::{DeviceId, IdError, OrgId, Sha256, SurveyId, UserId};
