//! Routes and their JSON bodies (camelCase). Errors are always an [`crate::ApiError`].
//!
//! Who may call each route:
//! - **open**: anyone, rate-limited;
//! - **session**: the web app, with its session cookie and the `X-GeoFlux-CSRF: 1` header (a
//!   SameSite=Strict cookie plus a custom header a cross-site form can't send);
//! - **device**: GeoGIS, with `Authorization: Bearer <access token>`.

use geoflux_licence::Plan;
use serde::{Deserialize, Serialize};

use crate::model::{Device, Membership, Organisation, Role, Subscription, User};
use crate::{DeviceId, UserId};

/// Every route starts with this, so a v2 can run beside it.
pub const PREFIX: &str = "/api/v1";

/// `(method, path)`: the whole API of Phases 7a and 7b. Paths use `{name}` for parameters; a tower
/// id (`{asset}`) is the client's own text, so clients percent-encode it.
pub mod routes {
    // Web sign-in (open, then session).
    pub const SIGN_UP: (&str, &str) = ("POST", "/api/v1/auth/signup");
    pub const LOG_IN: (&str, &str) = ("POST", "/api/v1/auth/login");
    pub const LOG_OUT: (&str, &str) = ("POST", "/api/v1/auth/logout");
    pub const FORGOT_PASSWORD: (&str, &str) = ("POST", "/api/v1/auth/password/forgot");
    pub const RESET_PASSWORD: (&str, &str) = ("POST", "/api/v1/auth/password/reset");
    pub const VERIFY_EMAIL: (&str, &str) = ("POST", "/api/v1/auth/email/verify");
    pub const ME: (&str, &str) = ("GET", "/api/v1/me");

    // Desktop sign-in, RFC 8628 (open, then the refresh token).
    pub const DEVICE_CODE: (&str, &str) = ("POST", "/api/v1/device/code");
    pub const DEVICE_TOKEN: (&str, &str) = ("POST", "/api/v1/device/token");
    pub const DEVICE_RENEW: (&str, &str) = ("POST", "/api/v1/device/renew");
    pub const DEVICE_SIGN_OUT: (&str, &str) = ("POST", "/api/v1/device/signout");
    /// The account page's "approve this computer" (session).
    pub const DEVICE_APPROVE: (&str, &str) = ("POST", "/api/v1/device/approve");
    /// What a code would sign in, shown before approving (session; `?code=WDJB-MJHT`).
    pub const DEVICE_PENDING: (&str, &str) = ("GET", "/api/v1/device/pending");

    // Computers and people (session).
    pub const DEVICES: (&str, &str) = ("GET", "/api/v1/devices");
    pub const REVOKE_DEVICE: (&str, &str) = ("DELETE", "/api/v1/devices/{id}");
    pub const OFFLINE_PASS: (&str, &str) = ("POST", "/api/v1/devices/offline-pass");
    pub const MEMBERS: (&str, &str) = ("GET", "/api/v1/org/members");
    pub const INVITE: (&str, &str) = ("POST", "/api/v1/org/invites");
    pub const ACCEPT_INVITE: (&str, &str) = ("POST", "/api/v1/invites/accept");
    pub const SET_ROLE: (&str, &str) = ("PATCH", "/api/v1/org/members/{id}");
    pub const REMOVE_MEMBER: (&str, &str) = ("DELETE", "/api/v1/org/members/{id}");

    // Surveys (device for uploads, session for reading).
    pub const CREATE_SURVEY: (&str, &str) = ("POST", "/api/v1/surveys");
    pub const PUT_PHOTO: (&str, &str) = ("PUT", "/api/v1/surveys/{id}/photos/{sha256}");
    pub const COMMIT_SURVEY: (&str, &str) = ("POST", "/api/v1/surveys/{id}/commit");
    pub const SURVEYS: (&str, &str) = ("GET", "/api/v1/surveys");

    // The web app's reading (session; committed surveys only). Phase 7b.
    pub const SURVEY: (&str, &str) = ("GET", "/api/v1/surveys/{id}");
    /// GeoJSON, see [`crate::view::TowerCollection`].
    pub const SURVEY_TOWERS: (&str, &str) = ("GET", "/api/v1/surveys/{id}/towers");
    pub const SURVEY_ASSET: (&str, &str) = ("GET", "/api/v1/surveys/{id}/assets/{asset}");
    /// The photo's bytes (or a redirect to signed storage), only if the survey lists it.
    pub const PHOTO: (&str, &str) = ("GET", "/api/v1/surveys/{id}/photos/{sha256}");
    pub const TOWER_REPORT: (&str, &str) = ("GET", "/api/v1/surveys/{id}/assets/{asset}/report.pdf");
    pub const LINE_REPORT: (&str, &str) = ("GET", "/api/v1/surveys/{id}/report.zip");
    /// A built-in schema's fields and photo slots, for labelling a tower's form (open).
    pub const SCHEMA: (&str, &str) = ("GET", "/api/v1/schemas/{id}");

    // Liveness for the host's health check (open).
    pub const HEALTH: (&str, &str) = ("GET", "/api/v1/health");

    pub const ALL: [(&str, &str); 33] = [
        SIGN_UP,
        LOG_IN,
        LOG_OUT,
        FORGOT_PASSWORD,
        RESET_PASSWORD,
        VERIFY_EMAIL,
        ME,
        DEVICE_CODE,
        DEVICE_TOKEN,
        DEVICE_RENEW,
        DEVICE_SIGN_OUT,
        DEVICE_APPROVE,
        DEVICE_PENDING,
        DEVICES,
        REVOKE_DEVICE,
        OFFLINE_PASS,
        MEMBERS,
        INVITE,
        ACCEPT_INVITE,
        SET_ROLE,
        REMOVE_MEMBER,
        CREATE_SURVEY,
        PUT_PHOTO,
        COMMIT_SURVEY,
        SURVEYS,
        SURVEY,
        SURVEY_TOWERS,
        SURVEY_ASSET,
        PHOTO,
        TOWER_REPORT,
        LINE_REPORT,
        SCHEMA,
        HEALTH,
    ];
}

/// Creates an organisation (on the 15-day Pro trial) and its first admin, and signs them in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignUp {
    pub email: String,
    pub password: String,
    pub name: String,
    pub organisation: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogIn {
    pub email: String,
    pub password: String,
}

/// Always answered with 202, whether or not the address has an account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgotPassword {
    pub email: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetPassword {
    pub token: String,
    pub password: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyEmail {
    pub token: String,
}

/// Who is signed in, for the web app's header and menus.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Me {
    pub user: User,
    pub organisation: Organisation,
    pub membership: Membership,
    pub subscription: Subscription,
    /// The plan in effect now (Free once lapsed).
    pub effective_plan: Plan,
}

/// GeoGIS starts a sign-in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCodeRequest {
    /// `MachineId::as_str`.
    pub machine: String,
    /// The computer's name, shown on the approval page and the account's computer list.
    pub device_name: String,
}

/// RFC 8628 §3.2, in camelCase.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceTokenRequest {
    pub device_code: String,
}

/// The account page approves (or declines) a code typed by the person.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceApprove {
    pub user_code: String,
    pub approve: bool,
}

/// A sign-in or renewal: the pass, the next refresh token, and a short-lived access token for
/// uploads. Errors use the RFC 8628 codes while a sign-in is pending.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceGrant {
    pub pass: String,
    pub refresh_token: String,
    pub access_token: String,
    pub access_expires_in: u64,
    pub device: DeviceId,
}

impl From<DeviceGrant> for geoflux_licence::Grant {
    fn from(g: DeviceGrant) -> Self {
        geoflux_licence::Grant { pass: g.pass, refresh_token: g.refresh_token }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRenew {
    pub refresh_token: String,
    pub machine: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSignOut {
    pub refresh_token: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceList {
    pub devices: Vec<Device>,
}

/// An offline pass file for a computer with no internet (admins, Organisation plan).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfflinePassRequest {
    pub user: UserId,
    pub machine: String,
    pub device_name: String,
    /// Capped at the subscription's end and the pass format's 400 days.
    pub days: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfflinePass {
    pub pass: String,
    pub expires: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberList {
    pub members: Vec<Member>,
    pub seats: u32,
    pub viewer_seats: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub user: User,
    pub role: Role,
    pub joined: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Invite {
    pub email: String,
    pub role: Role,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptInvite {
    pub token: String,
    pub name: String,
    pub password: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetRole {
    pub role: Role,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SurveyList {
    pub surveys: Vec<crate::model::SurveySummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub ok: bool,
    pub version: String,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn routes_are_versioned_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for (method, path) in routes::ALL {
            assert!(path.starts_with(PREFIX), "{path}");
            assert!(["GET", "POST", "PUT", "PATCH", "DELETE"].contains(&method));
            seen.insert((method, path));
        }
        assert_eq!(seen.len(), routes::ALL.len());
    }

    #[test]
    fn bodies_are_camel_case() {
        let json = serde_json::to_string(&DeviceCodeRequest { machine: "M".into(), device_name: "Asha's laptop".into() }).unwrap();
        assert_eq!(json, r#"{"machine":"M","deviceName":"Asha's laptop"}"#);
        let g: DeviceTokenRequest = serde_json::from_str(r#"{"deviceCode":"gfd_x"}"#).unwrap();
        assert_eq!(g.device_code, "gfd_x");
    }
}
