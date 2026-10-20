//! A throwaway PostgreSQL database per test, next to the one `GEOFLUX_TEST_DATABASE_URL` names.

use std::path::PathBuf;
use std::str::FromStr;

use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use sqlx::{Connection, Executor, PgConnection};

pub struct TestDb {
    admin_url: String,
    name: String,
    photos: PathBuf,
}

impl TestDb {
    /// `None` without `GEOFLUX_TEST_DATABASE_URL`.
    pub async fn create() -> Option<TestDb> {
        let admin_url = std::env::var("GEOFLUX_TEST_DATABASE_URL").ok().filter(|u| !u.is_empty())?;
        let mut n = [0u8; 8];
        getrandom::fill(&mut n).unwrap();
        let name = format!("geoflux_t_{:016x}", u64::from_le_bytes(n));
        let mut c = PgConnection::connect(&admin_url).await.unwrap();
        c.execute(format!("CREATE DATABASE {name}").as_str()).await.unwrap();
        c.close().await.unwrap();
        let photos = std::env::temp_dir().join(&name);
        Some(TestDb { admin_url, name, photos })
    }

    pub async fn pool(&self) -> PgPool {
        let opts = PgConnectOptions::from_str(&self.admin_url).unwrap().database(&self.name);
        PgPoolOptions::new().max_connections(8).connect_with(opts).await.unwrap()
    }

    #[allow(dead_code)] // Used by the flow tests only.
    pub fn photos_dir(&self) -> PathBuf {
        self.photos.clone()
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.photos);
        let (url, name) = (self.admin_url.clone(), self.name.clone());
        // Drop can't await; a small runtime on its own thread removes the database.
        let _ = std::thread::spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
            rt.block_on(async {
                if let Ok(mut c) = PgConnection::connect(&url).await {
                    let _ = c.execute(format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)").as_str()).await;
                    let _ = c.close().await;
                }
            });
        })
        .join();
    }
}
