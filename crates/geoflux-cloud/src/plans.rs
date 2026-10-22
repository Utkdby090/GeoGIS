//! What a subscription and a role allow, decided in one place so the API, the desktop pass and
//! the web app's buttons can't disagree.
//!
//! - Free: nothing paid; the desktop app runs in its free tier.
//! - Pro: every paid desktop feature, including Sync to the server, and the person's own uploads
//!   on the web.
//! - Organisation: Pro, plus viewer logins, the web map for the whole organisation and dashboards.
//!
//! A lapsed subscription stops what writes (uploads, the paid desktop features); reading what was
//! uploaded, and managing people and computers, carry on, so a customer never loses their data.

use geoflux_licence::{Feature, ONLINE_PASS_DAYS, PassClaims, PassKind, Plan};

use crate::error::{ApiError, ErrorCode};
use crate::model::{Role, Subscription, SubscriptionState};
use crate::{OrgId, UserId};

/// How long the Pro trial of a new organisation lasts.
pub const TRIAL_DAYS: i64 = 15;
/// Inspector and admin seats in the trial: enough for a small crew to try GeoFlux together.
pub const TRIAL_SEATS: u32 = 3;
/// Computers one person may have signed in at once.
pub const DEVICES_PER_SEAT: u32 = 2;
const DAY: i64 = 86_400;

/// Every paid desktop feature, in the order passes list them.
pub const DESKTOP_FEATURES: [Feature; 7] = [
    Feature::EditForms,
    Feature::Thermal,
    Feature::Markup,
    Feature::Reports,
    Feature::BulkReports,
    Feature::GeoxImport,
    Feature::CloudSync,
];

/// Something a request wants to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Permission {
    /// Collect a desktop pass with paid features (sign in, renew).
    UseDesktopFeatures,
    UploadSurveys,
    /// Surveys, towers and photos on the web.
    ViewSurveys,
    DownloadReports,
    ViewDashboard,
    /// Invite, change roles, remove people.
    ManageMembers,
    /// See and sign out everyone's computers; issue offline pass files.
    ManageDevices,
    ManageSubscription,
}

impl Permission {
    /// Changes something, so it stops when the subscription lapses.
    fn writes(self) -> bool {
        matches!(self, Permission::UseDesktopFeatures | Permission::UploadSurveys)
    }

    fn needs_organisation_plan(self) -> bool {
        matches!(self, Permission::ViewDashboard)
    }
}

/// Whether `role` may do it at all, whatever the plan.
pub fn role_allows(role: Role, p: Permission) -> bool {
    use Permission::*;
    match role {
        Role::Admin => true,
        Role::Inspector => matches!(p, UseDesktopFeatures | UploadSurveys | ViewSurveys | DownloadReports | ViewDashboard),
        Role::Viewer => matches!(p, ViewSurveys | DownloadReports | ViewDashboard),
    }
}

/// Whether the subscription is paid up (or in trial) at `now`.
pub fn is_current(sub: &Subscription, now: i64) -> bool {
    match sub.state {
        SubscriptionState::Trial { ends } => now < ends,
        SubscriptionState::Active { until } => now < until,
        SubscriptionState::Lapsed { .. } => false,
    }
}

/// The plan in effect at `now`: Free once the subscription has lapsed or run out.
pub fn effective_plan(sub: &Subscription, now: i64) -> Plan {
    if is_current(sub, now) { sub.plan } else { Plan::Free }
}

/// The one check every route makes after signing in.
pub fn check(sub: &Subscription, role: Role, p: Permission, now: i64) -> Result<(), ApiError> {
    if !role_allows(role, p) {
        return Err(ApiError::forbidden());
    }
    let plan = effective_plan(sub, now);
    let plan_ok = match plan {
        Plan::Organisation => true,
        Plan::Pro => !p.needs_organisation_plan(),
        // Free (or lapsed) keeps reads and management, never writes.
        Plan::Free | Plan::Unknown => !p.writes() && !p.needs_organisation_plan(),
    };
    if plan_ok { Ok(()) } else { Err(ApiError::new(ErrorCode::PlanRequired, "Your subscription doesn't include this, or it has ended.")) }
}

/// The paid desktop features a plan unlocks.
pub fn features(plan: Plan) -> &'static [Feature] {
    match plan {
        Plan::Pro | Plan::Organisation => &DESKTOP_FEATURES,
        Plan::Free | Plan::Unknown => &[],
    }
}

/// A new organisation's trial.
pub fn trial(org: OrgId, now: i64) -> Subscription {
    Subscription {
        org,
        plan: Plan::Pro,
        state: SubscriptionState::Trial { ends: now.saturating_add(TRIAL_DAYS * DAY) },
        billing: crate::model::Billing::Trial,
        seats: TRIAL_SEATS,
        viewer_seats: 0,
    }
}

/// Who a pass is for.
pub struct PassSubject<'a> {
    pub user: &'a UserId,
    pub display_name: &'a str,
    pub machine: &'a str,
}

/// The pass to sign for a sign-in or renewal: 30 days, or until the subscription runs out if that
/// is sooner, so a lapsed customer's computers drop to the free tier on time. A Free or lapsed
/// organisation still gets a pass (with no features) so the app shows who is signed in.
pub fn online_pass(sub: &Subscription, role: Role, who: PassSubject<'_>, now: i64) -> Result<PassClaims, ApiError> {
    if !role.is_desktop() {
        return Err(ApiError::forbidden());
    }
    let plan = effective_plan(sub, now);
    let paid_until = match sub.state {
        SubscriptionState::Trial { ends } => Some(ends),
        SubscriptionState::Active { until } => Some(until),
        SubscriptionState::Lapsed { .. } => None,
    };
    let mut expires = now.saturating_add(ONLINE_PASS_DAYS * DAY);
    if let Some(until) = paid_until.filter(|u| *u > now) {
        expires = expires.min(until);
    }
    Ok(PassClaims {
        account: who.user.to_string(),
        org: Some(sub.org.to_string()),
        name: who.display_name.to_string(),
        plan,
        features: features(plan).to_vec(),
        machine: who.machine.to_string(),
        kind: PassKind::Online,
        issued: now,
        expires,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::model::Billing;

    const NOW: i64 = 1_792_108_800;

    fn sub(plan: Plan, state: SubscriptionState) -> Subscription {
        Subscription { org: OrgId::from_random([1; 16]), plan, state, billing: Billing::Invoice, seats: 5, viewer_seats: 20 }
    }

    #[test]
    fn roles_and_plans() {
        let org = sub(Plan::Organisation, SubscriptionState::Active { until: NOW + 365 * DAY });
        let pro = sub(Plan::Pro, SubscriptionState::Active { until: NOW + 365 * DAY });
        let lapsed = sub(Plan::Organisation, SubscriptionState::Lapsed { since: NOW - DAY });
        assert!(check(&org, Role::Viewer, Permission::ViewDashboard, NOW).is_ok());
        assert_eq!(check(&org, Role::Viewer, Permission::UploadSurveys, NOW).unwrap_err().error, ErrorCode::Forbidden);
        assert_eq!(check(&org, Role::Inspector, Permission::ManageMembers, NOW).unwrap_err().error, ErrorCode::Forbidden);
        assert_eq!(check(&pro, Role::Admin, Permission::ViewDashboard, NOW).unwrap_err().error, ErrorCode::PlanRequired);
        assert!(check(&pro, Role::Inspector, Permission::UploadSurveys, NOW).is_ok());
        // Lapsed: uploads stop, the data stays readable and people manageable.
        assert_eq!(check(&lapsed, Role::Admin, Permission::UploadSurveys, NOW).unwrap_err().error, ErrorCode::PlanRequired);
        assert!(check(&lapsed, Role::Admin, Permission::ViewSurveys, NOW).is_ok());
        assert!(check(&lapsed, Role::Admin, Permission::ManageDevices, NOW).is_ok());
        // An Active date in the past counts as lapsed.
        let ran_out = sub(Plan::Pro, SubscriptionState::Active { until: NOW });
        assert_eq!(effective_plan(&ran_out, NOW), Plan::Free);
    }

    #[test]
    fn passes_never_outlive_the_subscription() {
        let user = UserId::from_random([2; 16]);
        let who = || PassSubject { user: &user, display_name: "Asha", machine: "M1" };
        let t = trial(OrgId::from_random([3; 16]), NOW);
        let p = online_pass(&t, Role::Admin, who(), NOW).unwrap();
        assert_eq!((p.plan, p.expires - p.issued), (Plan::Pro, TRIAL_DAYS * DAY));
        assert_eq!(p.features, DESKTOP_FEATURES.to_vec());
        let year = sub(Plan::Organisation, SubscriptionState::Active { until: NOW + 365 * DAY });
        assert_eq!(online_pass(&year, Role::Inspector, who(), NOW).unwrap().expires, NOW + ONLINE_PASS_DAYS * DAY);
        let lapsed = sub(Plan::Pro, SubscriptionState::Lapsed { since: NOW });
        let free = online_pass(&lapsed, Role::Inspector, who(), NOW).unwrap();
        assert_eq!((free.plan, free.features.len()), (Plan::Free, 0));
        assert!(online_pass(&year, Role::Viewer, who(), NOW).is_err());
    }
}
