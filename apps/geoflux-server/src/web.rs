//! The web app's files, built into the binary (see build.rs), served at every address that isn't
//! the API. An address with no file is one of the app's own pages (`/surveys/…`, `/device`), so it
//! gets index.html and the app's router takes over.
//!
//! Every page carries a Content-Security-Policy: scripts only from this server, connections only
//! to this server and the map's basemap, and no framing (so no clickjacking of the approve page).

use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use geoflux_cloud::ApiError;

use crate::http::Fail;

include!(concat!(env!("OUT_DIR"), "/web_files.rs"));

/// Where the map's basemap comes from (apps/geoflux-web/src/survey/LineMap.tsx).
const BASEMAP: &str = "https://tiles.openfreemap.org";

fn csp() -> String {
    [
        "default-src 'self'".to_string(),
        "script-src 'self'".to_string(),
        // MapLibre and React set style attributes.
        "style-src 'self' 'unsafe-inline'".to_string(),
        format!("img-src 'self' data: blob: {BASEMAP}"),
        format!("connect-src 'self' {BASEMAP}"),
        "worker-src 'self' blob:".to_string(),
        "font-src 'self'".to_string(),
        "object-src 'none'".to_string(),
        "base-uri 'self'".to_string(),
        "form-action 'self'".to_string(),
        "frame-ancestors 'none'".to_string(),
    ]
    .join("; ")
}

fn file(path: &str) -> Option<(&'static str, &'static [u8])> {
    FILES.iter().find(|(p, _, _)| *p == path).map(|(_, kind, bytes)| (*kind, *bytes))
}

/// Whether the app was built into this binary.
pub fn built_in() -> bool {
    file("/index.html").is_some()
}

const NOT_BUILT: &str = "<!doctype html><meta charset=utf-8><title>GeoFlux</title>\
<p>The GeoFlux web app isn't built into this server. In <code>apps/geoflux-web</code> run \
<code>npm ci</code> and <code>npm run build</code>, then build the server again. \
(For development, <code>npm run dev</code> serves it on port 5173.)</p>";

/// The fallback for every request no API route took.
pub async fn serve(method: Method, uri: Uri) -> Response {
    let path = uri.path();
    if path == "/api" || path.starts_with("/api/") {
        return Fail(ApiError::not_found()).into_response();
    }
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let mut headers = HeaderMap::new();
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("same-origin"));
    let (kind, body, immutable) = match file(path) {
        // index.html is served as the app's pages below, never cached as a file.
        Some((kind, bytes)) if path != "/index.html" => (kind, bytes, path.starts_with("/assets/")),
        _ if path.rsplit('/').next().is_some_and(|last| last.contains('.')) => {
            // A missing file (an old asset name after an update, a typo): a real 404, not the app.
            return StatusCode::NOT_FOUND.into_response();
        }
        _ => match file("/index.html") {
            Some((kind, bytes)) => (kind, bytes, false),
            None => ("text/html; charset=utf-8", NOT_BUILT.as_bytes(), false),
        },
    };
    // Built files have their content's hash in their name, so they never change; everything
    // else (the pages, the service worker) is checked again on each visit.
    let cache = if immutable { "public, max-age=31536000, immutable" } else { "no-cache" };
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
    if kind.starts_with("text/html") {
        if let Ok(v) = HeaderValue::from_str(&csp()) {
            headers.insert(header::CONTENT_SECURITY_POLICY, v);
        }
        headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    }
    if path == "/sw.js" {
        // The service worker may look after the whole site.
        headers.insert("service-worker-allowed", HeaderValue::from_static("/"));
    }
    (StatusCode::OK, headers, body).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_policy_keeps_scripts_and_frames_to_this_server() {
        let p = csp();
        assert!(p.contains("script-src 'self'") && !p.contains("unsafe-eval"));
        assert!(p.contains("frame-ancestors 'none'"));
    }
}
