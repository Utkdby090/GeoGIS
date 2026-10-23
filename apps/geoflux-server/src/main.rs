//! `geoflux-server`: runs the account server.
//!
//! Settings come from the environment:
//! - `GEOFLUX_BIND` (default `127.0.0.1:8080`)
//! - `GEOFLUX_PUBLIC_URL` (default `http://localhost:8080`): where people reach the web app
//! - `GEOFLUX_DATA_DIR` (default `./geoflux-data`): photos and temporary uploads
//! - `GEOFLUX_SIGNING_KEY`: a file holding the Ed25519 PKCS#8 (DER) key that signs passes, and
//!   `GEOFLUX_SIGNING_KID` its id. Without them a throwaway key is made and its public half
//!   printed, which only suits a local trial (GeoGIS builds only trust the keys they ship with).
//! - `GEOFLUX_TRUST_PROXY=1` behind a proxy that sets `X-Forwarded-For`.
//! - `GEOFLUX_DATABASE_URL` (`postgres://user:pass@host/db`, add `?sslmode=require` for Neon or
//!   Supabase): records in PostgreSQL, photos under `GEOFLUX_DATA_DIR/photos`. The schema is
//!   created or updated at start-up. `GEOFLUX_DB_CONNECTIONS` caps the pool (default 10). Use the
//!   database's direct (session) address, not a transaction-mode pooler such as Supabase's port
//!   6543: the server keeps prepared statements per connection.
//!   Without a database URL everything is kept in memory and lost when the server stops.
//! - `GEOFLUX_MAIL_LOG=1`: no mail service yet, so write each email's link (address check,
//!   password reset, invite) to the log instead. Only on your own computer: the links are secrets.
//!   In-memory storage always does this.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use geoflux_cloud::ports::AccountStore;
use geoflux_licence::Ed25519Signer;
use geoflux_server::blobs::DiskBlobs;
use geoflux_server::crypto::{Argon2Hasher, MemoryRateLimiter, OsRandom, SystemClock};
use geoflux_server::mail::{LogMail, NoMail};
use geoflux_server::memory::MemoryBackend;
use geoflux_server::postgres::{PgBackend, PgStore};
use geoflux_server::{Backend, Server, Settings, router};

struct StderrLog;

impl log::Log for StderrLog {
    fn enabled(&self, m: &log::Metadata<'_>) -> bool {
        // The database driver's own notices ("already exists, skipping") only at warning level.
        let max = if m.target().starts_with("sqlx") { log::Level::Warn } else { log::Level::Info };
        m.level() <= max
    }
    fn log(&self, r: &log::Record<'_>) {
        if self.enabled(r.metadata()) {
            eprintln!("{} {}: {}", r.level(), r.target(), r.args());
        }
    }
    fn flush(&self) {}
}

static LOGGER: StderrLog = StderrLog;

fn env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn signer() -> Result<Ed25519Signer, String> {
    match (std::env::var("GEOFLUX_SIGNING_KEY"), std::env::var("GEOFLUX_SIGNING_KID")) {
        (Ok(path), Ok(kid)) => {
            let der = std::fs::read(&path).map_err(|e| format!("reading {path}: {e}"))?;
            Ed25519Signer::from_pkcs8(&kid, &der).map_err(|e| e.to_string())
        }
        _ => {
            let key = Ed25519Signer::from_pkcs8("local-trial", &Ed25519Signer::generate_pkcs8().map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            log::warn!("no GEOFLUX_SIGNING_KEY: using a throwaway key, public half {}", key.public_key_base64());
            Ok(key)
        }
    }
}

#[tokio::main]
async fn main() {
    let _ = log::set_logger(&LOGGER).map(|()| log::set_max_level(log::LevelFilter::Info));
    if let Err(e) = run().await {
        log::error!("{e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let bind: SocketAddr = env("GEOFLUX_BIND", "127.0.0.1:8080").parse().map_err(|e| format!("GEOFLUX_BIND: {e}"))?;
    let public_url = env("GEOFLUX_PUBLIC_URL", "http://localhost:8080").trim_end_matches('/').to_string();
    let data = PathBuf::from(env("GEOFLUX_DATA_DIR", "./geoflux-data"));
    let settings = Settings {
        secure_cookies: public_url.starts_with("https://"),
        public_url,
        upload_dir: data.join("uploads"),
        trust_proxy: env("GEOFLUX_TRUST_PROXY", "0") == "1",
    };
    match std::env::var("GEOFLUX_DATABASE_URL") {
        Ok(url) if !url.is_empty() => {
            let connections = env("GEOFLUX_DB_CONNECTIONS", "10").parse().map_err(|e| format!("GEOFLUX_DB_CONNECTIONS: {e}"))?;
            let store = PgStore::connect(&url, connections).await.map_err(|e| format!("connecting to the database: {e}"))?;
            store.migrate().await.map_err(|e| format!("updating the database: {e}"))?;
            log::info!("records in PostgreSQL, photos in {}", data.join("photos").display());
            let blobs = DiskBlobs::new(data.join("photos"));
            if env("GEOFLUX_MAIL_LOG", "0") == "1" {
                log::warn!("GEOFLUX_MAIL_LOG=1: email links are written to this log, not sent; never use this on a shared server");
                serve(PgBackend { store, blobs, mail: LogMail }, settings, bind).await
            } else {
                serve(PgBackend { store, blobs, mail: NoMail }, settings, bind).await
            }
        }
        _ => {
            log::warn!("no GEOFLUX_DATABASE_URL: storage is in memory and everything is lost when the server stops");
            serve(MemoryBackend::default(), settings, bind).await
        }
    }
}

async fn serve<B: Backend>(backend: B, settings: Settings, bind: SocketAddr) -> Result<(), String> {
    let server = Arc::new(Server {
        backend,
        hasher: Arc::new(Argon2Hasher::owasp()?),
        random: Arc::new(OsRandom),
        limiter: Arc::new(MemoryRateLimiter::default()),
        clock: Arc::new(SystemClock),
        signer: Arc::new(signer()?),
        reports: Arc::new(geoflux_server::reports::Reports),
        report_slots: Arc::new(tokio::sync::Semaphore::new(geoflux_server::reports::REPORTS_AT_ONCE)),
        settings,
    });
    // Expired tokens and sign-in codes are deleted hourly.
    let purge = server.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(3_600));
        loop {
            tick.tick().await;
            if let Err(e) = purge.backend.accounts().purge_expired(purge.now()).await {
                log::warn!("purging expired tokens: {e}");
            }
        }
    });
    let app = router(server).into_make_service_with_connect_info::<SocketAddr>();
    let listener = tokio::net::TcpListener::bind(bind).await.map_err(|e| format!("binding {bind}: {e}"))?;
    log::info!("listening on {bind}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(|e| e.to_string())
}
