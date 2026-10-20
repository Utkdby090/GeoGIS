//! One error shape for every route.
//!
//! The body is `{"error": "<code>", "message": "…", …details}`. Clients branch on `error`, never on
//! the message, which is for people and may change. The device-code codes are the ones RFC 8628
//! names, so a standard client reads them too.

use geoflux_licence::AccountError;
use serde::{Deserialize, Serialize};

use crate::Sha256;

/// What went wrong, as clients branch on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The body is not JSON, or not the expected shape.
    BadRequest,
    /// One or more fields are wrong; see `fields`.
    InvalidField,
    /// No session or token, or it ran out.
    Unauthenticated,
    /// Wrong email or password. Deliberately the same for an unknown email.
    InvalidCredentials,
    /// The email address has to be confirmed first.
    EmailNotVerified,
    /// Signed in, but the role doesn't allow it.
    Forbidden,
    /// The subscription doesn't include it, or it has lapsed.
    PlanRequired,
    /// The plan's computers per person are all in use; see `devices`.
    SeatLimit,
    /// This computer was signed out remotely, or its refresh token was used twice.
    Revoked,
    NotFound,
    /// Already exists (an email address), or changed by someone else meanwhile.
    Conflict,
    /// The survey is missing photos; see `missing`.
    Incomplete,
    TooLarge,
    UnsupportedMediaType,
    /// Too many attempts; see `retry_after`.
    RateLimited,
    /// RFC 8628: the person hasn't approved the code yet.
    AuthorizationPending,
    /// RFC 8628: polling too fast.
    SlowDown,
    /// RFC 8628: the person declined.
    AccessDenied,
    /// RFC 8628: the device code ran out.
    ExpiredToken,
    /// A bug or a failed dependency; details are in the server log, never in the response.
    Internal,
    /// The database or photo storage is unreachable; try again later.
    Unavailable,
}

impl ErrorCode {
    pub fn http_status(self) -> u16 {
        use ErrorCode::*;
        match self {
            BadRequest | AuthorizationPending | SlowDown | AccessDenied | ExpiredToken => 400,
            Unauthenticated | InvalidCredentials | Revoked => 401,
            PlanRequired => 402,
            EmailNotVerified | Forbidden | SeatLimit => 403,
            NotFound => 404,
            Conflict | Incomplete => 409,
            TooLarge => 413,
            UnsupportedMediaType => 415,
            InvalidField => 422,
            RateLimited => 429,
            Internal => 500,
            Unavailable => 503,
        }
    }
}

/// One field that failed a check, by its JSON name (`password`, `assets[3].photos[0].bytes`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldError {
    pub field: String,
    /// A short stable reason (`too_short`, `not_an_email`), for clients to translate.
    pub reason: String,
}

impl FieldError {
    pub fn new(field: impl Into<String>, reason: &str) -> Self {
        FieldError { field: field.into(), reason: reason.to_string() }
    }
}

/// The error body of every route.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    pub error: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<FieldError>,
    /// For [`ErrorCode::SeatLimit`]: how many computers the person has signed in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub devices: Option<u32>,
    /// For [`ErrorCode::Incomplete`]: photos still to send.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<Sha256>,
    /// For [`ErrorCode::RateLimited`] and [`ErrorCode::SlowDown`]: seconds to wait.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<u64>,
}

impl ApiError {
    pub fn new(error: ErrorCode, message: impl Into<String>) -> Self {
        ApiError { error, message: message.into(), fields: Vec::new(), devices: None, missing: Vec::new(), retry_after: None }
    }

    pub fn fields(fields: Vec<FieldError>) -> Self {
        ApiError { fields, ..ApiError::new(ErrorCode::InvalidField, "Some fields need fixing.") }
    }

    pub fn seat_limit(devices: u32) -> Self {
        ApiError {
            devices: Some(devices),
            ..ApiError::new(ErrorCode::SeatLimit, "Your account is signed in on too many computers. Sign out on one first.")
        }
    }

    pub fn incomplete(missing: Vec<Sha256>) -> Self {
        ApiError { missing, ..ApiError::new(ErrorCode::Incomplete, "Some photos have not been sent yet.") }
    }

    pub fn rate_limited(retry_after: u64) -> Self {
        ApiError { retry_after: Some(retry_after), ..ApiError::new(ErrorCode::RateLimited, "Too many attempts. Try again shortly.") }
    }

    pub fn forbidden() -> Self {
        ApiError::new(ErrorCode::Forbidden, "Your role doesn't allow this.")
    }

    pub fn not_found() -> Self {
        ApiError::new(ErrorCode::NotFound, "Not found.")
    }

    pub fn internal() -> Self {
        ApiError::new(ErrorCode::Internal, "Something went wrong on our side.")
    }

    pub fn http_status(&self) -> u16 {
        self.error.http_status()
    }
}

/// What the desktop app's sign-in makes of a server error. None of these lock the app.
impl From<ApiError> for AccountError {
    fn from(e: ApiError) -> Self {
        match e.error {
            ErrorCode::Revoked | ErrorCode::Unauthenticated | ErrorCode::PlanRequired => AccountError::Revoked,
            ErrorCode::SeatLimit => AccountError::SeatLimit { devices: e.devices.unwrap_or(0) },
            ErrorCode::Unavailable | ErrorCode::RateLimited => AccountError::Offline(e.message),
            _ => AccountError::Server(e.message),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_shape_is_stable() {
        let e = ApiError::seat_limit(2);
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["error"], "seat_limit");
        assert_eq!(json["devices"], 2);
        assert!(json.get("missing").is_none() && json.get("retryAfter").is_none());
        assert_eq!(e.http_status(), 403);
        assert_eq!(serde_json::to_value(ErrorCode::AuthorizationPending).unwrap(), "authorization_pending");
        let back: ApiError = serde_json::from_value(json).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn the_desktop_never_locks_on_a_server_error() {
        assert_eq!(AccountError::from(ApiError::seat_limit(3)), AccountError::SeatLimit { devices: 3 });
        assert_eq!(AccountError::from(ApiError::new(ErrorCode::Revoked, "x")), AccountError::Revoked);
        assert!(matches!(AccountError::from(ApiError::rate_limited(5)), AccountError::Offline(_)));
        assert!(matches!(AccountError::from(ApiError::internal()), AccountError::Server(_)));
    }
}
