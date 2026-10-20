//! The HTTP plumbing every route shares: the error response, body parsing, cookies, the client's
//! address, and turning a session or access token into a [`Principal`].

use std::net::SocketAddr;

use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use geoflux_cloud::auth::{AuthVia, Principal, SecretToken, TokenKind};
use geoflux_cloud::ports::{AccountStore, StoreError};
use geoflux_cloud::{ApiError, ErrorCode, Sha256};
use serde::de::DeserializeOwned;

use crate::{Backend, Server};

pub const SESSION_COOKIE: &str = "gf_session";
pub const CSRF_HEADER: &str = "x-geoflux-csrf";

/// Any route's failure, as the shared JSON error.
#[derive(Debug)]
pub struct Fail(pub ApiError);

pub type Result<T> = std::result::Result<T, Fail>;

impl From<ApiError> for Fail {
    fn from(e: ApiError) -> Self {
        Fail(e)
    }
}

impl From<StoreError> for Fail {
    fn from(e: StoreError) -> Self {
        if !matches!(e, StoreError::NotFound | StoreError::Conflict(_)) {
            log::error!("store: {e}");
        }
        Fail(e.into())
    }
}

/// Logs the cause and answers with a bare internal error: details never reach the client.
pub fn internal(what: &str, cause: impl std::fmt::Display) -> Fail {
    log::error!("{what}: {cause}");
    Fail(ApiError::internal())
}

impl IntoResponse for Fail {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.0.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let retry = self.0.retry_after;
        let mut res = (status, axum::Json(self.0)).into_response();
        if let Some(secs) = retry
            && let Ok(v) = HeaderValue::from_str(&secs.to_string())
        {
            res.headers_mut().insert(header::RETRY_AFTER, v);
        }
        res
    }
}

pub fn bad(code: ErrorCode, message: &str) -> Fail {
    Fail(ApiError::new(code, message))
}

/// A JSON body, with the shared error shape when it isn't one.
pub fn parse<T: DeserializeOwned>(body: &[u8]) -> Result<T> {
    serde_json::from_slice(body).map_err(|_| bad(ErrorCode::BadRequest, "The request body is not valid JSON of the expected shape."))
}

/// The value of one cookie.
pub fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(';')).find_map(|kv| {
        let (k, v) = kv.trim().split_once('=')?;
        (k == name).then_some(v)
    })
}

/// `Set-Cookie` for the session: HttpOnly, SameSite=Strict, Secure unless a localhost trial.
pub fn session_cookie(value: &str, max_age: i64, secure: bool) -> HeaderValue {
    let secure = if secure { "; Secure" } else { "" };
    let text = format!("{SESSION_COOKIE}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{secure}");
    HeaderValue::from_str(&text).unwrap_or_else(|_| HeaderValue::from_static("gf_session=; Path=/; Max-Age=0"))
}

/// The caller's address for rate limits: the proxy's `X-Forwarded-For` only when configured to
/// trust it (otherwise anyone could pick their own address).
pub fn client_ip(headers: &HeaderMap, connect: Option<&ConnectInfo<SocketAddr>>, trust_proxy: bool) -> String {
    if trust_proxy && let Some(first) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()).and_then(|v| v.split(',').next()) {
        return first.trim().to_string();
    }
    connect.map_or_else(|| "unknown".to_string(), |c| c.0.ip().to_string())
}

/// Counts one attempt against a limit; a 429 when over.
pub fn limit<B: Backend>(server: &Server<B>, key: &str, l: geoflux_cloud::ports::Limit) -> Result<()> {
    server.limiter.hit(key, l, server.now()).map_err(|wait| Fail(ApiError::rate_limited(wait)))
}

fn unauthenticated() -> Fail {
    bad(ErrorCode::Unauthenticated, "Please sign in.")
}

async fn principal_for<B: Backend>(server: &Server<B>, user: &geoflux_cloud::UserId, via: AuthVia) -> Result<Principal> {
    let a = server.backend.accounts();
    let rec = a.user(user).await?.filter(|u| !u.disabled).ok_or_else(unauthenticated)?;
    let (m, sub) = a.membership(&rec.user.id).await?.ok_or_else(unauthenticated)?;
    Ok(Principal::new(rec.user.id, m.role, via, sub))
}

/// The web app's caller, from the session cookie. Requests that change something must also send
/// the CSRF header, which a form on another site can't add.
pub async fn web<B: Backend>(server: &Server<B>, method: &Method, headers: &HeaderMap) -> Result<(Principal, Sha256)> {
    if method != Method::GET && headers.get(CSRF_HEADER).is_none_or(|v| v.as_bytes() != b"1") {
        return Err(bad(ErrorCode::Forbidden, "Missing the X-GeoFlux-CSRF header."));
    }
    let token = cookie(headers, SESSION_COOKIE).and_then(|v| SecretToken::parse(TokenKind::Session, v)).ok_or_else(unauthenticated)?;
    let digest = token.digest();
    let now = server.now();
    let a = server.backend.accounts();
    let rec = a.token(TokenKind::Session, &digest, now).await?.ok_or_else(unauthenticated)?;
    // Slide the expiry on use, at most once a minute.
    let full = now.saturating_add(TokenKind::Session.lifetime_secs());
    if full - rec.expires >= 60 {
        a.extend_token(TokenKind::Session, &digest, full).await?;
    }
    Ok((principal_for(server, &rec.user, AuthVia::Session).await?, digest))
}

/// GeoGIS's caller, from `Authorization: Bearer <access token>`.
pub async fn desktop<B: Backend>(server: &Server<B>, headers: &HeaderMap) -> Result<Principal> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .and_then(|v| SecretToken::parse(TokenKind::Access, v.trim()))
        .ok_or_else(unauthenticated)?;
    let rec = server.backend.accounts().token(TokenKind::Access, &token.digest(), server.now()).await?.ok_or_else(unauthenticated)?;
    let device = rec.device.clone().ok_or_else(unauthenticated)?;
    principal_for(server, &rec.user, AuthVia::Device(device)).await
}

/// Either: the web app or GeoGIS, for routes both read.
pub async fn any<B: Backend>(server: &Server<B>, method: &Method, headers: &HeaderMap) -> Result<Principal> {
    if headers.contains_key(header::AUTHORIZATION) { desktop(server, headers).await } else { Ok(web(server, method, headers).await?.0) }
}

/// A new secret of `kind`, or a 500 if the random source failed.
pub fn new_token<B: Backend>(server: &Server<B>, kind: TokenKind) -> Result<SecretToken> {
    let bytes = crate::crypto::random_bytes::<32>(server.random.as_ref()).map_err(|e| internal("random", e))?;
    Ok(SecretToken::from_random(kind, bytes))
}

/// Runs password hashing on the blocking pool, off the request threads.
pub async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f).await.map_err(|e| internal("blocking task", e))
}

/// What every handler reads from the request line and headers.
pub struct Ctx {
    pub method: Method,
    pub headers: HeaderMap,
    /// For rate limits ([`client_ip`]).
    pub ip: String,
}

impl<B: Backend> axum::extract::FromRequestParts<std::sync::Arc<Server<B>>> for Ctx {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        server: &std::sync::Arc<Server<B>>,
    ) -> std::result::Result<Self, Self::Rejection> {
        let ip = client_ip(&parts.headers, parts.extensions.get::<ConnectInfo<SocketAddr>>(), server.settings.trust_proxy);
        Ok(Ctx { method: parts.method.clone(), headers: parts.headers.clone(), ip })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn cookies_and_addresses() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("a=1; gf_session=gfs_x; b=2"));
        assert_eq!(cookie(&h, SESSION_COOKIE), Some("gfs_x"));
        assert_eq!(cookie(&h, "c"), None);
        h.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.9, 10.0.0.1"));
        assert_eq!(client_ip(&h, None, true), "203.0.113.9");
        assert_eq!(client_ip(&h, None, false), "unknown");
        let c = session_cookie("gfs_x", 60, true);
        let c = c.to_str().unwrap();
        assert!(c.contains("HttpOnly") && c.contains("SameSite=Strict") && c.contains("Secure"));
    }
}
