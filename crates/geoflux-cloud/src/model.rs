//! What the server keeps: organisations, people, subscriptions, computers and surveys.
//!
//! Every organisation-owned record carries its [`OrgId`], and the stores only read them through an
//! [`crate::auth::OrgScope`], so one customer's data can't reach another's.
//!
//! Times are seconds since 1970-01-01 UTC, as in the desktop pass.

use geoflux_domain::Severity;
use geoflux_licence::Plan;
use serde::{Deserialize, Serialize};

use crate::{DeviceId, OrgId, SurveyId, UserId};

/// What a person may do in their organisation ([`crate::plans::allows`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    /// Members, computers, the subscription, plus everything an inspector does.
    Admin,
    /// Uses GeoGIS's paid features (a seat) and uploads surveys.
    Inspector,
    /// A client or manager: the web map, reports and dashboards, read-only. Not a desktop seat.
    Viewer,
}

impl Role {
    /// Whether the role signs in to GeoGIS and so takes a seat.
    pub fn is_desktop(self) -> bool {
        matches!(self, Role::Admin | Role::Inspector)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Organisation {
    pub id: OrgId,
    pub name: String,
    pub created: i64,
}

/// A person as the API shows them. The password hash never leaves the store
/// ([`crate::ports::UserRecord`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: UserId,
    /// Normalised ([`crate::auth::normalize_email`]).
    pub email: String,
    pub name: String,
    pub email_verified: bool,
    pub created: i64,
}

/// One person in one organisation. A person belongs to one organisation in this version; joining
/// several comes later if consultants need it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Membership {
    pub user: UserId,
    pub org: OrgId,
    pub role: Role,
    pub joined: i64,
}

/// Where a subscription stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum SubscriptionState {
    /// The 15-day Pro trial every new organisation gets, kept here so reinstalling can't restart it.
    Trial { ends: i64 },
    /// Paid up to `until` (online payment or an invoice activated by GeoFlux staff).
    Active { until: i64 },
    /// Ended. The data stays; paid features and uploads stop.
    Lapsed { since: i64 },
}

/// How the subscription is paid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Billing {
    Trial,
    /// Tenders: activated by hand after an invoice is paid.
    Invoice,
    /// A payment provider, once one is chosen.
    Online,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    pub org: OrgId,
    pub plan: Plan,
    pub state: SubscriptionState,
    pub billing: Billing,
    /// Admins and inspectors (desktop users) allowed.
    pub seats: u32,
    /// Viewers allowed (Organisation plan).
    pub viewer_seats: u32,
}

/// One computer signed in to GeoGIS, as the account page lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: DeviceId,
    pub org: OrgId,
    pub user: UserId,
    /// The computer's ID (`MachineId::as_str`); a renewal from another computer is refused.
    pub machine: String,
    /// As GeoGIS reports it (the computer's name), for people to tell their computers apart.
    pub name: String,
    pub signed_in: i64,
    pub last_seen: i64,
    /// An offline pass file was issued for it, which can't be revoked before it runs out.
    pub offline_until: Option<i64>,
}

/// Where an upload stands. Only committed surveys appear on the map and the dashboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SurveyState {
    /// The manifest is in; photos are arriving.
    Uploading,
    Committed,
}

/// One uploaded survey revision. Uploads never overwrite: the next revision of the same survey is a
/// new record, so trends can compare surveys over time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SurveySummary {
    pub id: SurveyId,
    pub org: OrgId,
    pub client_survey_id: String,
    pub revision: u32,
    pub project: String,
    pub line: Option<String>,
    pub schema_id: String,
    pub state: SurveyState,
    pub uploaded_by: UserId,
    pub uploaded: i64,
    pub committed: Option<i64>,
    pub assets: u32,
    pub photos: u32,
    pub photo_bytes: u64,
    /// Assets per severity, from [`Severity::ALL`] order.
    pub severity_counts: [u32; 5],
}

impl SurveySummary {
    pub fn count(&self, severity: Severity) -> u32 {
        Severity::ALL.iter().position(|s| *s == severity).and_then(|i| self.severity_counts.get(i).copied()).unwrap_or(0)
    }
}
