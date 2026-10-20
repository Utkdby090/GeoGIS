//! GeoGIS's side of GeoFlux Cloud: signing a computer in (the device-code flow), renewing its pass,
//! signing out, and uploading a survey with its photos.
//!
//! Every call blocks on the network, so the window runs them on a worker thread. Nothing here keeps
//! secrets: the caller stores the refresh token and pass, and hands an access token in through
//! [`AccessTokens`] when uploading.
//!
//! Uploads are resumable by design of the server: the manifest is sent first and the server answers
//! with the photos it doesn't have, so running [`CloudClient::upload`] again after a dropped
//! connection sends only what is still missing.

use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;
use std::time::Duration;

use geoflux_cloud::api::{DeviceCodeRequest, DeviceCodeResponse, DeviceGrant, DeviceRenew, DeviceSignOut, DeviceTokenRequest, routes};
use geoflux_cloud::model::SurveySummary;
use geoflux_cloud::sync::{SurveyCreated, SurveyManifest};
use geoflux_cloud::{ApiError, ErrorCode, Sha256};
use geoflux_licence::{AccountError, AccountService, DeviceCode, MachineId, SignInPoll};
use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

/// The largest answer read from the server. Answers are small JSON; a survey list is the largest.
const MAX_REPLY: u64 = 16 * 1024 * 1024;
const USER_AGENT: &str = concat!("GeoGIS/", env!("CARGO_PKG_VERSION"), " (+https://github.com/Utkdby090/GeoGIS)");

/// Why a call failed, before it is turned into what the caller handles.
#[derive(Debug)]
enum Failure {
    /// The server answered with its error shape.
    Api(ApiError),
    /// No answer, or a proxy or gateway in the way.
    Offline(String),
    /// An answer that isn't the API's (a captive portal page, a wrong address).
    Other(String),
    /// Getting an access token failed.
    Account(AccountError),
    /// The photo to send can't be read, or changed.
    Photo(String),
}

impl From<Failure> for AccountError {
    fn from(f: Failure) -> Self {
        match f {
            Failure::Api(e) => e.into(),
            Failure::Offline(m) => AccountError::Offline(m),
            Failure::Other(m) | Failure::Photo(m) => AccountError::Server(m),
            Failure::Account(e) => e,
        }
    }
}

/// What an upload ended with when it didn't finish.
#[derive(Debug, Error)]
pub enum SyncError {
    /// Offline, signed out, or the server refused; nothing is lost, run the upload again later.
    #[error(transparent)]
    Account(#[from] AccountError),
    /// A photo the manifest lists can't be read, or changed since the manifest was made.
    #[error("photo {sha} can't be sent: {reason}")]
    Photo { sha: Sha256, reason: String },
    /// The server refused the survey itself (a field it doesn't accept, or the plan's limits).
    #[error("the survey was refused: {0}")]
    Refused(String),
    /// [`Progress`] asked to stop.
    #[error("the upload was cancelled")]
    Cancelled,
}

/// How far an upload has got, for a progress bar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// Photos sent so far, of `photos` the server asked for.
    pub sent: usize,
    pub photos: usize,
    pub bytes_sent: u64,
    pub bytes: u64,
}

/// A finished upload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uploaded {
    pub survey: SurveySummary,
    /// Photos sent in this run; the rest were already on the server.
    pub sent: usize,
    /// This revision had been uploaded before (a retry after the commit got through).
    pub existing: bool,
}

/// Where uploads get their access token. Access tokens last an hour; when the server refuses one,
/// [`CloudClient::upload`] asks once for a `fresh` one (a renewal) and repeats the request.
pub trait AccessTokens {
    fn access_token(&mut self, fresh: bool) -> Result<String, AccountError>;
}

/// The result of asking whether a sign-in finished.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DevicePoll {
    Pending,
    /// Asked too often; wait this many seconds before the next poll.
    SlowDown(u64),
    Granted(DeviceGrant),
    Denied,
    Expired,
}

/// The GeoFlux Cloud API at one address.
#[derive(Clone)]
pub struct CloudClient {
    base: String,
    device_name: String,
    /// JSON calls: 30 seconds each.
    agent: ureq::Agent,
    /// Photo uploads (up to 64 MiB over a field connection): 15 minutes each.
    uploads: ureq::Agent,
}

impl std::fmt::Debug for CloudClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudClient").field("base", &self.base).field("device_name", &self.device_name).finish()
    }
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .timeout_connect(Some(Duration::from_secs(15)))
        .user_agent(USER_AGENT)
        // Error answers are read as the API's error shape, not as transport errors.
        .http_status_as_error(false)
        // A redirect could carry the bearer token elsewhere; the API never redirects.
        .max_redirects(0)
        .build()
        .into()
}

/// Whether `url` is an address tokens may be sent to: HTTPS, or plain HTTP to this computer only
/// (a local trial of the server).
fn check_base(url: &str) -> Result<String, AccountError> {
    let url = url.trim().trim_end_matches('/');
    let local = ["http://localhost", "http://127.0.0.1", "http://[::1]"]
        .iter()
        .any(|p| url.strip_prefix(p).is_some_and(|rest| rest.is_empty() || rest.starts_with([':', '/'])));
    let host = url.strip_prefix("https://").unwrap_or("");
    if local || (!host.is_empty() && !host.starts_with(['/', ':', '@']) && !host.contains('@')) {
        Ok(url.to_string())
    } else {
        Err(AccountError::Server(format!("{url} is not an https:// address")))
    }
}

/// Reads an answer: the body as `T` on success, else the API's error shape.
fn reply<T: DeserializeOwned>(mut res: ureq::http::Response<ureq::Body>) -> Result<T, Failure> {
    let status = res.status().as_u16();
    let text =
        res.body_mut().with_config().limit(MAX_REPLY).read_to_string().map_err(|e| Failure::Offline(format!("reading the answer: {e}")))?;
    if (200..300).contains(&status) {
        // `()` answers (204) have no body.
        let text = if text.is_empty() { "null" } else { text.as_str() };
        return serde_json::from_str(text).map_err(|e| Failure::Other(format!("unexpected answer: {e}")));
    }
    match serde_json::from_str::<ApiError>(&text) {
        Ok(e) => Err(Failure::Api(e)),
        // Gateways and proxies in front of a server that is down or restarting.
        Err(_) if matches!(status, 502..=504) => Err(Failure::Offline(format!("the server answered {status}"))),
        Err(_) => Err(Failure::Other(format!("the server answered {status}"))),
    }
}

fn transport(e: ureq::Error) -> Failure {
    Failure::Offline(e.to_string())
}

/// RFC 8628 answers to a poll, which arrive as errors.
fn poll_answer(f: Failure) -> Result<DevicePoll, AccountError> {
    match f {
        Failure::Api(e) => match e.error {
            ErrorCode::AuthorizationPending => Ok(DevicePoll::Pending),
            ErrorCode::SlowDown => Ok(DevicePoll::SlowDown(e.retry_after.unwrap_or(5))),
            ErrorCode::AccessDenied => Ok(DevicePoll::Denied),
            ErrorCode::ExpiredToken => Ok(DevicePoll::Expired),
            _ => Err(e.into()),
        },
        f => Err(f.into()),
    }
}

impl CloudClient {
    /// The server at `base_url` (`https://cloud.example`), reporting this computer as `device_name`
    /// on the account page.
    pub fn new(base_url: &str, device_name: &str) -> Result<Self, AccountError> {
        Ok(CloudClient {
            base: check_base(base_url)?,
            device_name: device_name.chars().filter(|c| !c.is_control()).take(80).collect(),
            agent: agent(Duration::from_secs(30)),
            uploads: agent(Duration::from_secs(900)),
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn url(&self, route: (&str, &str)) -> String {
        format!("{}{}", self.base, route.1)
    }

    fn post<B: Serialize, T: DeserializeOwned>(&self, route: (&str, &str), body: &B, bearer: Option<&str>) -> Result<T, Failure> {
        let json = serde_json::to_vec(body).map_err(|e| Failure::Other(e.to_string()))?;
        self.post_bytes(&self.url(route), &json, bearer)
    }

    fn post_bytes<T: DeserializeOwned>(&self, url: &str, json: &[u8], bearer: Option<&str>) -> Result<T, Failure> {
        let mut req = self.agent.post(url).header("content-type", "application/json");
        if let Some(t) = bearer {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        reply(req.send(json).map_err(transport)?)
    }

    /// Starts signing this computer in; show `user_code` and open `verification_uri_complete`.
    pub fn device_code(&self, machine: &MachineId) -> Result<DeviceCodeResponse, AccountError> {
        let req = DeviceCodeRequest { machine: machine.as_str().to_string(), device_name: self.device_name.clone() };
        Ok(self.post(routes::DEVICE_CODE, &req, None)?)
    }

    /// Asks whether the person approved the code. Poll no faster than the `interval` it came with.
    pub fn poll_device(&self, device_code: &str) -> Result<DevicePoll, AccountError> {
        let req = DeviceTokenRequest { device_code: device_code.to_string() };
        match self.post(routes::DEVICE_TOKEN, &req, None) {
            Ok(g) => Ok(DevicePoll::Granted(g)),
            Err(f) => poll_answer(f),
        }
    }

    /// A fresh pass, access token and the next refresh token. The old refresh token stops working,
    /// so store the new one before anything else.
    pub fn renew_device(&self, refresh_token: &str, machine: &MachineId) -> Result<DeviceGrant, AccountError> {
        let req = DeviceRenew { refresh_token: refresh_token.to_string(), machine: machine.as_str().to_string() };
        Ok(self.post(routes::DEVICE_RENEW, &req, None)?)
    }

    pub fn sign_out_device(&self, refresh_token: &str) -> Result<(), AccountError> {
        let req = DeviceSignOut { refresh_token: refresh_token.to_string() };
        Ok(self.post(routes::DEVICE_SIGN_OUT, &req, None)?)
    }

    /// Runs `call` with an access token; if the server refuses the token, once more with a fresh one.
    fn authed<T>(&self, tokens: &mut dyn AccessTokens, call: impl Fn(&str) -> Result<T, Failure>) -> Result<T, Failure> {
        let token = tokens.access_token(false).map_err(Failure::Account)?;
        match call(&token) {
            Err(Failure::Api(e)) if e.error == ErrorCode::Unauthenticated => {
                let token = tokens.access_token(true).map_err(Failure::Account)?;
                call(&token)
            }
            r => r,
        }
    }

    /// Uploads a survey: the manifest, then each photo the server doesn't have yet (read from the
    /// file `photo` returns for its hash), then the commit. `progress` is called after each photo;
    /// returning `false` stops the upload, which a later run resumes.
    pub fn upload(
        &self,
        tokens: &mut dyn AccessTokens,
        manifest: &SurveyManifest,
        photo: &dyn Fn(&Sha256) -> Option<PathBuf>,
        progress: &mut dyn FnMut(Progress) -> bool,
    ) -> Result<Uploaded, SyncError> {
        let sizes = manifest.photos().map_err(|sha| SyncError::Refused(format!("photo {sha} is listed with two sizes")))?;
        let json = serde_json::to_vec(manifest).map_err(|e| SyncError::Refused(e.to_string()))?;
        let url = self.url(routes::CREATE_SURVEY);
        let created: SurveyCreated = self.authed(tokens, |t| self.post_bytes(&url, &json, Some(t))).map_err(sync_failure)?;
        // Up to 100,000 photos: looked up by hash, not searched.
        let sizes: HashMap<Sha256, u64> = sizes.into_iter().map(|(h, b, _)| (h, b)).collect();
        let size_of = |sha: &Sha256| sizes.get(sha).copied().unwrap_or(0);
        let mut p = Progress {
            photos: created.missing.len(),
            bytes: created.missing.iter().map(size_of).fold(0u64, u64::saturating_add),
            ..Progress::default()
        };
        if !progress(p) {
            return Err(SyncError::Cancelled);
        }
        for sha in &created.missing {
            let expected = size_of(sha);
            let path = photo(sha).ok_or_else(|| SyncError::Photo { sha: sha.clone(), reason: "not found in the project".into() })?;
            let url = self.url(routes::PUT_PHOTO).replace("{id}", created.survey.as_str()).replace("{sha256}", sha.as_str());
            self.authed(tokens, |t| {
                // Opened for each attempt, so a retry starts from the first byte. Closed when the
                // request ends, whatever happens.
                let file = File::open(&path).map_err(|e| Failure::Photo(e.to_string()))?;
                let len = file.metadata().map_err(|e| Failure::Photo(e.to_string()))?.len();
                if len != expected {
                    return Err(Failure::Photo("it changed since the survey was prepared".into()));
                }
                let res = self
                    .uploads
                    .put(&url)
                    .header("content-type", "application/octet-stream")
                    .header("authorization", format!("Bearer {t}"))
                    .send(file)
                    .map_err(transport)?;
                reply::<()>(res)
            })
            .map_err(|f| match f {
                Failure::Photo(reason) => SyncError::Photo { sha: sha.clone(), reason },
                f => sync_failure(f),
            })?;
            p.sent += 1;
            p.bytes_sent = p.bytes_sent.saturating_add(expected);
            if !progress(p) {
                return Err(SyncError::Cancelled);
            }
        }
        let url = self.url(routes::COMMIT_SURVEY).replace("{id}", created.survey.as_str());
        let survey: SurveySummary = self.authed(tokens, |t| self.post_bytes(&url, &[], Some(t))).map_err(sync_failure)?;
        Ok(Uploaded { survey, sent: p.sent, existing: created.existing })
    }
}

/// Field, plan and size refusals are about the survey; the rest about the connection or account.
fn sync_failure(f: Failure) -> SyncError {
    match f {
        Failure::Api(e) => match e.error {
            ErrorCode::InvalidField
            | ErrorCode::BadRequest
            | ErrorCode::TooLarge
            | ErrorCode::UnsupportedMediaType
            | ErrorCode::Incomplete
            | ErrorCode::NotFound
            | ErrorCode::Conflict => SyncError::Refused(e.message),
            _ => SyncError::Account(e.into()),
        },
        f => SyncError::Account(f.into()),
    }
}

impl AccountService for CloudClient {
    fn start_sign_in(&self, machine: &MachineId) -> Result<DeviceCode, AccountError> {
        let c = self.device_code(machine)?;
        Ok(DeviceCode {
            device_code: c.device_code,
            user_code: c.user_code,
            verification_uri: c.verification_uri_complete,
            interval_secs: c.interval,
            expires_in_secs: c.expires_in,
        })
    }

    fn poll_sign_in(&self, code: &DeviceCode) -> Result<SignInPoll, AccountError> {
        Ok(match self.poll_device(&code.device_code)? {
            DevicePoll::Pending => SignInPoll::Pending,
            DevicePoll::SlowDown(_) => SignInPoll::SlowDown,
            DevicePoll::Granted(g) => SignInPoll::Granted(g.into()),
            DevicePoll::Denied => SignInPoll::Denied,
            DevicePoll::Expired => SignInPoll::Expired,
        })
    }

    fn renew(&self, refresh_token: &str, machine: &MachineId) -> Result<geoflux_licence::Grant, AccountError> {
        Ok(self.renew_device(refresh_token, machine)?.into())
    }

    fn sign_out(&self, refresh_token: &str) -> Result<(), AccountError> {
        self.sign_out_device(refresh_token)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn only_https_or_this_computer() {
        assert_eq!(check_base("https://cloud.geoflux.in/").unwrap(), "https://cloud.geoflux.in");
        assert!(check_base("http://localhost:8080").is_ok());
        assert!(check_base("http://127.0.0.1").is_ok());
        for bad in ["http://cloud.geoflux.in", "http://localhost.evil.com", "https://", "https://user@host", "ftp://x", ""] {
            assert!(check_base(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn rfc_8628_answers_are_not_errors() {
        let api = |c| Failure::Api(ApiError::new(c, "m"));
        assert_eq!(poll_answer(api(ErrorCode::AuthorizationPending)).unwrap(), DevicePoll::Pending);
        assert_eq!(poll_answer(api(ErrorCode::SlowDown)).unwrap(), DevicePoll::SlowDown(5));
        assert_eq!(poll_answer(api(ErrorCode::AccessDenied)).unwrap(), DevicePoll::Denied);
        assert_eq!(poll_answer(api(ErrorCode::ExpiredToken)).unwrap(), DevicePoll::Expired);
        assert_eq!(poll_answer(api(ErrorCode::Revoked)), Err(AccountError::Revoked));
        assert!(matches!(poll_answer(Failure::Offline("x".into())), Err(AccountError::Offline(_))));
    }
}
