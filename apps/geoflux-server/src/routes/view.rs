//! What the web app reads (Phase 7b): one survey, its towers on the map, one tower, its photos,
//! the tower PDF and the line ZIP, the line's and a tower's health for the dashboard, a schema's
//! fields, and a sign-in code before it is approved.
//!
//! Only committed surveys of the caller's organisation are found; anything else is the same 404,
//! so ids of other organisations can't be probed.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use geoflux_cloud::auth::UserCode;
use geoflux_cloud::health::{HealthQuery, LineHealth, TowerHealth};
use geoflux_cloud::model::SurveySummary;
use geoflux_cloud::plans::Permission;
use geoflux_cloud::ports::{BlobSource, BlobStore, DeviceStore, ReportFailure, SurveyReader, limits};
use geoflux_cloud::sync::SurveyAsset;
use geoflux_cloud::view::{PendingDevice, ReportRequest, SurveyDetail, TowerCollection, TowerDetail};
use geoflux_cloud::{ApiError, ErrorCode, FieldError, Sha256, SurveyId};

use crate::crypto::random_bytes;
use crate::http::{self, Ctx, Fail, Result, bad, internal, limit};
use crate::{Backend, Server};

/// Same limit as the report crate's: a stored photo larger than this is shown as missing.
const MAX_PHOTO_BYTES: u64 = 40 * 1024 * 1024;

fn not_found() -> Fail {
    Fail(ApiError::not_found())
}

fn survey_id(id: &str) -> Result<SurveyId> {
    SurveyId::parse(id).map_err(|_| not_found())
}

/// The schema's label field, for the map's tower labels; blank when the schema is unknown.
fn label_field(schema_id: &str) -> String {
    match geoflux_domain::builtin_schema(schema_id) {
        Some(Ok(s)) => s.label_key().to_string(),
        _ => String::new(),
    }
}

pub async fn survey<B: Backend>(State(s): State<Arc<Server<B>>>, ctx: Ctx, Path(id): Path<String>) -> Result<Json<SurveyDetail>> {
    let p = http::any(&s, &ctx.method, &ctx.headers).await?;
    let scope = p.require(Permission::ViewSurveys, s.now())?;
    let id = survey_id(&id)?;
    Ok(Json(s.backend.surveys().survey(&scope, &id).await?.ok_or_else(not_found)?))
}

pub async fn towers<B: Backend>(State(s): State<Arc<Server<B>>>, ctx: Ctx, Path(id): Path<String>) -> Result<Json<TowerCollection>> {
    let p = http::any(&s, &ctx.method, &ctx.headers).await?;
    let scope = p.require(Permission::ViewSurveys, s.now())?;
    let id = survey_id(&id)?;
    let surveys = s.backend.surveys();
    let detail = surveys.survey(&scope, &id).await?.ok_or_else(not_found)?;
    let assets = surveys.survey_assets(&scope, &id).await?.ok_or_else(not_found)?;
    Ok(Json(TowerCollection::from_assets(&assets, &label_field(&detail.summary.schema_id))))
}

pub async fn asset<B: Backend>(
    State(s): State<Arc<Server<B>>>,
    ctx: Ctx,
    Path((id, asset)): Path<(String, String)>,
) -> Result<Json<TowerDetail>> {
    let p = http::any(&s, &ctx.method, &ctx.headers).await?;
    let scope = p.require(Permission::ViewSurveys, s.now())?;
    let id = survey_id(&id)?;
    let uploaded = s.backend.surveys().survey_asset(&scope, &id, &asset).await?.ok_or_else(not_found)?;
    Ok(Json(TowerDetail { survey: id, uploaded }))
}

/// The line at a glance for the dashboard, with optional filters. A filter that can't be an id,
/// an unknown key or a repeated one is refused, so a typo never shows as an empty dashboard.
pub async fn line_health<B: Backend>(
    State(s): State<Arc<Server<B>>>,
    ctx: Ctx,
    Path(id): Path<String>,
    query: std::result::Result<Query<HealthQuery>, QueryRejection>,
) -> Result<Json<LineHealth>> {
    let p = http::any(&s, &ctx.method, &ctx.headers).await?;
    let scope = p.require(Permission::ViewSurveys, s.now())?;
    let id = survey_id(&id)?;
    // Checked after the survey's access, like every other route here, but before any reading.
    let Query(query) = query.map_err(|_| Fail(ApiError::fields(vec![FieldError::new("query", "invalid")])))?;
    query.check().map_err(|f| Fail(ApiError::fields(f)))?;
    let surveys = s.backend.surveys();
    let detail = surveys.survey(&scope, &id).await?.ok_or_else(not_found)?;
    let assets = surveys.survey_assets(&scope, &id).await?.ok_or_else(not_found)?;
    let label = label_field(&detail.summary.schema_id);
    // Up to 100,000 findings to tally and sort: off the async workers.
    Ok(Json(http::blocking(move || LineHealth::build(id, &assets, &label, query)).await?))
}

/// One tower for the 3D view: its parts' health and its findings, unfiltered.
pub async fn tower_health<B: Backend>(
    State(s): State<Arc<Server<B>>>,
    ctx: Ctx,
    Path((id, asset)): Path<(String, String)>,
) -> Result<Json<TowerHealth>> {
    let p = http::any(&s, &ctx.method, &ctx.headers).await?;
    let scope = p.require(Permission::ViewSurveys, s.now())?;
    let id = survey_id(&id)?;
    let surveys = s.backend.surveys();
    let detail = surveys.survey(&scope, &id).await?.ok_or_else(not_found)?;
    let tower = surveys.survey_asset(&scope, &id, &asset).await?.ok_or_else(not_found)?;
    Ok(Json(TowerHealth::of(id, &tower, &label_field(&detail.summary.schema_id))))
}

/// A photo's bytes. Photos are named by their hash, so the browser may keep them for good.
pub async fn photo<B: Backend>(State(s): State<Arc<Server<B>>>, ctx: Ctx, Path((id, sha)): Path<(String, String)>) -> Result<Response> {
    let p = http::any(&s, &ctx.method, &ctx.headers).await?;
    let scope = p.require(Permission::ViewSurveys, s.now())?;
    let id = survey_id(&id)?;
    let sha = Sha256::parse(&sha).map_err(|_| not_found())?;
    let kind = s.backend.surveys().survey_photo(&scope, &id, &sha).await?.ok_or_else(not_found)?;
    let source = s.backend.blobs().source(scope.org(), &sha, s.now()).await?;
    let body = match source {
        BlobSource::Url(url) => {
            let to = HeaderValue::from_str(&url).map_err(|e| internal("photo url", e))?;
            return Ok((StatusCode::FOUND, [(header::LOCATION, to), (header::CACHE_CONTROL, HeaderValue::from_static("no-store"))])
                .into_response());
        }
        BlobSource::Bytes(b) => Body::from(b),
        BlobSource::File(path) => {
            let file = tokio::fs::File::open(&path).await.map_err(|e| internal("photo file", e))?;
            Body::from_stream(tokio_util::io::ReaderStream::new(file))
        }
    };
    Ok(file_response(body, kind.content_type(), None, "private, max-age=31536000, immutable"))
}

fn file_response(body: Body, content_type: &'static str, download: Option<&str>, cache: &'static str) -> Response {
    let mut r = Response::new(body);
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    if let Some(name) = download {
        // The name is letters, digits, `-`, `_` and `.` only (view::safe_file_part), so it can go
        // in the header as it is.
        if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
            h.insert(header::CONTENT_DISPOSITION, v);
        }
    }
    r
}

/// Where each photo of `towers` can be read from, looked up before the report starts so the
/// blocking report code never waits on storage. Photos in signed-URL storage are not read yet.
async fn photo_sources<B: Backend>(
    s: &Server<B>,
    org: &geoflux_cloud::OrgId,
    towers: &[&SurveyAsset],
) -> Result<HashMap<Sha256, BlobSource>> {
    let wanted: HashSet<&Sha256> = towers.iter().flat_map(|t| t.photos.iter().map(|p| &p.sha256)).collect();
    let mut out = HashMap::with_capacity(wanted.len());
    let now = s.now();
    for sha in wanted {
        match s.backend.blobs().source(org, sha, now).await {
            Ok(src) => {
                out.insert(sha.clone(), src);
            }
            // Missing or unreadable: the page shows "No image" for it.
            Err(e) => log::warn!("report photo {sha}: {e}"),
        }
    }
    Ok(out)
}

/// Reads a photo for the report code (on the blocking pool).
fn read_photo(sources: &HashMap<Sha256, BlobSource>, sha: &Sha256) -> Option<Vec<u8>> {
    match sources.get(sha)? {
        BlobSource::Bytes(b) => Some(b.clone()),
        BlobSource::File(path) => {
            let len = std::fs::metadata(path).ok()?.len();
            if len > MAX_PHOTO_BYTES {
                return None;
            }
            std::fs::read(path).ok()
        }
        BlobSource::Url(_) => None,
    }
}

fn report_failed(e: ReportFailure) -> Fail {
    match e {
        ReportFailure::TooManyTowers(_) => bad(ErrorCode::TooLarge, "This line has too many towers for one ZIP."),
        e => internal("report", e),
    }
}

/// Waits for a free report slot, so only a few reports are made at once.
async fn report_slot<B: Backend>(s: &Server<B>) -> Result<tokio::sync::OwnedSemaphorePermit> {
    s.report_slots.clone().acquire_owned().await.map_err(|e| internal("report slots", e))
}

pub async fn tower_report<B: Backend>(
    State(s): State<Arc<Server<B>>>,
    ctx: Ctx,
    Path((id, asset)): Path<(String, String)>,
) -> Result<Response> {
    let p = http::any(&s, &ctx.method, &ctx.headers).await?;
    let scope = p.require(Permission::DownloadReports, s.now())?;
    let id = survey_id(&id)?;
    let surveys = s.backend.surveys();
    let detail = surveys.survey(&scope, &id).await?.ok_or_else(not_found)?;
    let tower = surveys.survey_asset(&scope, &id, &asset).await?.ok_or_else(not_found)?;
    let (previous, next) = surveys.neighbours(&scope, &id, &asset).await?.ok_or_else(not_found)?;
    let sources = photo_sources(&s, scope.org(), &[&tower]).await?;
    let name = ReportRequest::Tower { asset: tower.asset.id.clone() }.file_name(&detail.summary);
    let schema_id = detail.summary.schema_id;
    let permit = report_slot(&s).await?;
    let renderer = s.reports.clone();
    let pdf = http::blocking(move || {
        let _permit = permit;
        renderer.tower_pdf(&schema_id, &tower, previous.as_ref(), next.as_ref(), &mut |h| read_photo(&sources, h))
    })
    .await?
    .map_err(report_failed)?;
    Ok(file_response(Body::from(pdf), "application/pdf", Some(&name), "private, no-store"))
}

/// Deletes the ZIP's temporary file when the download ends, finished or not.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub async fn line_report<B: Backend>(State(s): State<Arc<Server<B>>>, ctx: Ctx, Path(id): Path<String>) -> Result<Response> {
    let p = http::any(&s, &ctx.method, &ctx.headers).await?;
    let scope = p.require(Permission::DownloadReports, s.now())?;
    limit(&s, &format!("line-zip:{}", scope.org()), limits::LINE_REPORTS_PER_ORG)?;
    let id = survey_id(&id)?;
    let surveys = s.backend.surveys();
    let detail = surveys.survey(&scope, &id).await?.ok_or_else(not_found)?;
    let towers = surveys.survey_assets(&scope, &id).await?.ok_or_else(not_found)?;
    if towers.len() > crate::reports::MAX_LINE_TOWERS {
        return Err(report_failed(ReportFailure::TooManyTowers(crate::reports::MAX_LINE_TOWERS)));
    }
    let refs: Vec<&SurveyAsset> = towers.iter().collect();
    let sources = photo_sources(&s, scope.org(), &refs).await?;
    drop(refs);
    let summary: SurveySummary = detail.summary;
    let name = ReportRequest::Line.file_name(&summary);

    // Written to a temporary file (the ZIP needs to seek back to finish), then streamed.
    tokio::fs::create_dir_all(&s.settings.upload_dir).await.map_err(|e| internal("report dir", e))?;
    let tag: [u8; 16] = random_bytes(s.random.as_ref()).map_err(|e| internal("random", e))?;
    let path = s.settings.upload_dir.join(format!("{}.zip.part", Sha256::of(&tag)));
    let temp = TempFile(path.clone());
    let permit = report_slot(&s).await?;
    let renderer = s.reports.clone();
    let schema_id = summary.schema_id.clone();
    let write_path = path.clone();
    http::blocking(move || -> std::result::Result<u32, ReportFailure> {
        let _permit = permit;
        let file = std::fs::File::create_new(&write_path).map_err(|e| ReportFailure::Render(format!("temporary file: {e}")))?;
        let mut out = std::io::BufWriter::new(file);
        let n = renderer.line_zip(&schema_id, &towers, &mut |h| read_photo(&sources, h), &mut out)?;
        let file = out.into_inner().map_err(|e| ReportFailure::Render(format!("temporary file: {e}")))?;
        file.sync_all().map_err(|e| ReportFailure::Render(format!("temporary file: {e}")))?;
        Ok(n)
    })
    .await?
    .map_err(report_failed)?;
    let file = tokio::fs::File::open(&path).await.map_err(|e| internal("report file", e))?;
    let len = file.metadata().await.map_err(|e| internal("report file", e))?.len();
    let stream = tokio_util::io::ReaderStream::new(file);
    // The temporary file goes when the stream (and with it the guard) is dropped.
    let guarded = guarded(stream, temp);
    let mut r = file_response(Body::from_stream(guarded), "application/zip", Some(&name), "private, no-store");
    r.headers_mut().insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    Ok(r)
}

/// Keeps `guard` alive exactly as long as `stream`.
fn guarded<S, G>(stream: S, guard: G) -> Guarded<S, G> {
    Guarded { stream, _guard: guard }
}

/// A stream that owns a value dropped with it. Field order matters: the stream (and its open file)
/// is dropped before the guard deletes the file, which Windows needs.
struct Guarded<S, G> {
    stream: S,
    _guard: G,
}

impl<S: futures_core::Stream + Unpin, G: Unpin> futures_core::Stream for Guarded<S, G> {
    type Item = S::Item;
    fn poll_next(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Option<Self::Item>> {
        std::pin::Pin::new(&mut self.stream).poll_next(cx)
    }
}

pub async fn schema(Path(id): Path<String>) -> Result<Response> {
    match geoflux_domain::builtin_schema(&id) {
        Some(Ok(schema)) => {
            let mut r = Json(schema).into_response();
            r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=3600"));
            Ok(r)
        }
        Some(Err(e)) => Err(internal("built-in schema", e)),
        None => Err(not_found()),
    }
}

#[derive(serde::Deserialize)]
pub struct CodeQuery {
    #[serde(default)]
    code: String,
}

pub async fn pending_device<B: Backend>(
    State(s): State<Arc<Server<B>>>,
    ctx: Ctx,
    Query(q): Query<CodeQuery>,
) -> Result<Json<PendingDevice>> {
    let _ = http::web(&s, &ctx.method, &ctx.headers).await?;
    // The same limit as approving: a code can't be guessed by asking about it either.
    limit(&s, &format!("user-code:{}", ctx.ip), limits::USER_CODE_PER_IP)?;
    let not_found = || bad(ErrorCode::NotFound, "That code is not valid or has run out.");
    let code = UserCode::parse(&q.code).ok_or_else(not_found)?;
    let now = s.now();
    let rec = s.backend.devices().device_code_by_user_code(&code, now).await?.ok_or_else(not_found)?;
    Ok(Json(PendingDevice::of(&rec, now).ok_or_else(not_found)?))
}
