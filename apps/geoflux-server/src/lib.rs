//! The GeoFlux Cloud account server (Phase 7a): sign-in for GeoGIS and the web app,
//! organisations and their members, computers, and survey upload.
//!
//! The rules and the wire format live in `geoflux-cloud`; this crate is the HTTP layer over them
//! and the adapters for its ports. Storage is a [`Backend`]: [`memory::MemoryBackend`] for tests and
//! local trials now, PostgreSQL next, and the hosting choice (Oracle, Render + Neon + R2, Supabase)
//! is decided at the end as further adapters.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod blobs;
pub mod crypto;
mod http;
pub mod memory;
mod routes;

use std::path::PathBuf;
use std::sync::Arc;

use geoflux_cloud::ports::{
    AccountStore, BlobStore, Clock, DeviceStore, Mailer, PassSigner, PasswordHasher, Random, RateLimiter, SurveyStore,
};

/// The storage a server runs on. One type bundles the async stores (their traits use
/// `impl Future`, so they are generic parameters rather than trait objects).
pub trait Backend: Send + Sync + 'static {
    type Accounts: AccountStore + 'static;
    type Devices: DeviceStore + 'static;
    type Surveys: SurveyStore + 'static;
    type Blobs: BlobStore + 'static;
    type Mail: Mailer + 'static;

    fn accounts(&self) -> &Self::Accounts;
    fn devices(&self) -> &Self::Devices;
    fn surveys(&self) -> &Self::Surveys;
    fn blobs(&self) -> &Self::Blobs;
    fn mail(&self) -> &Self::Mail;
}

/// Settings that differ between a laptop trial and a hosted server.
#[derive(Clone, Debug)]
pub struct Settings {
    /// Where people reach the web app, without a trailing slash (`https://cloud.geoflux.in`); links
    /// in emails and the device-code page are built from it.
    pub public_url: String,
    /// Where uploaded photos are written while their hash is checked, before storage takes them.
    /// On the same disk as disk storage, so the final move is a rename.
    pub upload_dir: PathBuf,
    /// Mark cookies `Secure` (off only for `http://localhost` trials).
    pub secure_cookies: bool,
    /// Read the client address from `X-Forwarded-For` (only behind a proxy that sets it).
    pub trust_proxy: bool,
}

/// Everything a request needs.
pub struct Server<B: Backend> {
    pub backend: B,
    pub hasher: Arc<dyn PasswordHasher>,
    pub random: Arc<dyn Random>,
    pub limiter: Arc<dyn RateLimiter>,
    pub clock: Arc<dyn Clock>,
    pub signer: Arc<dyn PassSigner>,
    pub settings: Settings,
}

impl<B: Backend> Server<B> {
    pub fn now(&self) -> i64 {
        self.clock.now()
    }
}

/// The whole API as an Axum router.
pub fn router<B: Backend>(server: Arc<Server<B>>) -> axum::Router {
    routes::router(server)
}
