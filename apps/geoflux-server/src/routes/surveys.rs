//! Project › Sync: the manifest, photos by hash, the commit; and the survey list.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use geoflux_cloud::api::SurveyList;
use geoflux_cloud::plans::Permission;
use geoflux_cloud::ports::{BlobStore, SurveyStore};
use geoflux_cloud::sync::SurveyManifest;
use geoflux_cloud::{ApiError, ErrorCode, FieldError, Sha256, SurveyId};
use http_body_util::BodyExt;
use sha2::Digest;
use tokio::io::AsyncWriteExt;

use crate::crypto::random_bytes;
use crate::http::{self, Ctx, Fail, Result, bad, internal, parse};
use crate::{Backend, Server};

pub async fn create<B: Backend>(State(s): State<Arc<Server<B>>>, ctx: Ctx, body: Bytes) -> Result<Response> {
    let p = http::desktop(&s, &ctx.headers).await?;
    let scope = p.require(Permission::UploadSurveys, s.now())?;
    let m: SurveyManifest = parse(&body)?;
    drop(body);
    m.validate().map_err(|f| Fail(ApiError::fields(f)))?;
    if let Err(h) = m.photos() {
        return Err(ApiError::fields(vec![FieldError::new(format!("photos.{h}"), "conflicting_sizes")]).into());
    }
    let created = s.backend.surveys().create_survey(&scope, &m, s.now()).await?;
    let status = if created.existing { StatusCode::OK } else { StatusCode::CREATED };
    Ok((status, Json(created)).into_response())
}

/// Deletes a partly written upload unless it was handed to storage.
struct TempUpload(Option<PathBuf>);

impl Drop for TempUpload {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

pub async fn put_photo<B: Backend>(
    State(s): State<Arc<Server<B>>>,
    ctx: Ctx,
    Path((id, sha)): Path<(String, String)>,
    body: Body,
) -> Result<StatusCode> {
    let p = http::desktop(&s, &ctx.headers).await?;
    let scope = p.require(Permission::UploadSurveys, s.now())?;
    let id = SurveyId::parse(&id).map_err(|_| Fail(ApiError::not_found()))?;
    let sha = Sha256::parse(&sha).map_err(|_| Fail(ApiError::not_found()))?;
    let surveys = s.backend.surveys();
    let (size, kind) = surveys.expected_photo(&scope, &id, &sha).await?.ok_or_else(|| Fail(ApiError::not_found()))?;
    let blobs = s.backend.blobs();
    if blobs.exists(scope.org(), &sha).await? {
        // Already stored (another tower, an earlier revision): nothing to read.
        surveys.record_photo(&scope, &sha, size).await?;
        return Ok(StatusCode::NO_CONTENT);
    }

    tokio::fs::create_dir_all(&s.settings.upload_dir).await.map_err(|e| internal("upload dir", e))?;
    let name: [u8; 16] = random_bytes(s.random.as_ref()).map_err(|e| internal("random", e))?;
    let path = s.settings.upload_dir.join(format!("{}.part", Sha256::of(&name)));
    let mut guard = TempUpload(Some(path.clone()));
    let mut file = tokio::fs::File::create_new(&path).await.map_err(|e| internal("upload file", e))?;
    let mut hasher = sha2::Sha256::new();
    let mut written: u64 = 0;
    let mut head: Vec<u8> = Vec::with_capacity(8);
    let mut body = body;
    // Streamed: memory stays at one frame however large the photo.
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| bad(ErrorCode::BadRequest, "The upload was interrupted."))?;
        let Ok(chunk) = frame.into_data() else { continue };
        written = written.saturating_add(chunk.len() as u64);
        if written > size {
            return Err(bad(ErrorCode::TooLarge, "The photo is larger than the manifest said."));
        }
        if head.len() < 8 {
            let take = (8 - head.len()).min(chunk.len());
            head.extend_from_slice(&chunk[..take]);
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(|e| internal("write upload", e))?;
    }
    file.flush().await.map_err(|e| internal("flush upload", e))?;
    file.sync_all().await.map_err(|e| internal("sync upload", e))?;
    drop(file);
    if written != size {
        return Err(bad(ErrorCode::BadRequest, "The photo is smaller than the manifest said."));
    }
    if !kind.matches(&head) {
        return Err(bad(ErrorCode::UnsupportedMediaType, "The photo is not the type the manifest said."));
    }
    if Sha256::from_digest(hasher.finalize().into()) != sha {
        return Err(bad(ErrorCode::BadRequest, "The photo's contents don't match its hash."));
    }
    blobs.put_file(scope.org(), &sha, &path, kind).await?;
    guard.0 = None;
    surveys.record_photo(&scope, &sha, size).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn commit<B: Backend>(State(s): State<Arc<Server<B>>>, ctx: Ctx, Path(id): Path<String>) -> Result<Response> {
    let p = http::desktop(&s, &ctx.headers).await?;
    let scope = p.require(Permission::UploadSurveys, s.now())?;
    let id = SurveyId::parse(&id).map_err(|_| Fail(ApiError::not_found()))?;
    match s.backend.surveys().commit(&scope, &id, s.now()).await? {
        Ok(summary) => Ok(Json(summary).into_response()),
        Err(missing) => Err(ApiError::incomplete(missing).into()),
    }
}

pub async fn list<B: Backend>(State(s): State<Arc<Server<B>>>, ctx: Ctx) -> Result<Json<SurveyList>> {
    let p = http::any(&s, &ctx.method, &ctx.headers).await?;
    let scope = p.require(Permission::ViewSurveys, s.now())?;
    Ok(Json(SurveyList { surveys: s.backend.surveys().surveys(&scope).await? }))
}
