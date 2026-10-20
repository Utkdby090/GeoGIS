//! Photos on the server's own disk (`<root>/<org>/<aa>/<sha256>`), for a single VM such as Oracle's
//! free tier. Paths are built from a typed organisation id and a checked hash only, never from
//! client text.

use std::path::{Path, PathBuf};

use geoflux_cloud::ports::{BlobSource, BlobStore, StoreError, StoreResult};
use geoflux_cloud::sync::PhotoType;
use geoflux_cloud::{OrgId, Sha256};

pub struct DiskBlobs {
    root: PathBuf,
}

impl DiskBlobs {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        DiskBlobs { root: root.into() }
    }

    fn path(&self, org: &OrgId, sha: &Sha256) -> PathBuf {
        let h = sha.as_str();
        self.root.join(org.as_str()).join(&h[..2]).join(h)
    }
}

fn io(e: std::io::Error) -> StoreError {
    StoreError::Unavailable(e.to_string())
}

impl BlobStore for DiskBlobs {
    async fn exists(&self, org: &OrgId, sha: &Sha256) -> StoreResult<bool> {
        tokio::fs::try_exists(self.path(org, sha)).await.map_err(io)
    }

    async fn put_file(&self, org: &OrgId, sha: &Sha256, file: &Path, _kind: PhotoType) -> StoreResult<()> {
        let dest = self.path(org, sha);
        if let Some(dir) = dest.parent() {
            tokio::fs::create_dir_all(dir).await.map_err(io)?;
        }
        // Same content under the same name: a concurrent upload of the same photo can't corrupt it.
        match tokio::fs::rename(file, &dest).await {
            Ok(()) => Ok(()),
            // Upload folder on another disk: copy to a temporary name, then rename into place.
            Err(_) => {
                let tmp = dest.with_extension("part");
                tokio::fs::copy(file, &tmp).await.map_err(io)?;
                tokio::fs::rename(&tmp, &dest).await.map_err(io)?;
                let _ = tokio::fs::remove_file(file).await;
                Ok(())
            }
        }
    }

    async fn source(&self, org: &OrgId, sha: &Sha256, _now: i64) -> StoreResult<BlobSource> {
        let p = self.path(org, sha);
        if tokio::fs::try_exists(&p).await.map_err(io)? { Ok(BlobSource::File(p)) } else { Err(StoreError::NotFound) }
    }
}
