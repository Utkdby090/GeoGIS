//! The decision at start-up, after a sign-in and after a renewal: which paid features are on.
//!
//! The app always opens. Without a usable pass it runs in the free tier, and [`FreeReason`] says
//! why, so the status bar can offer the right fix (sign in, reconnect, fix the clock).

use crate::date::SECS_PER_DAY;
use crate::{Feature, Grant, LicenceError, MachineId, MachineIdSource, Pass, PassKind, PassStatus, PassVerifier, Plan};

/// Days left at or under which the status bar asks the person to go online.
pub const RENEW_WARN_DAYS: i64 = 5;
/// An online pass older than this is renewed at the next chance.
pub const RENEW_AFTER_SECS: i64 = SECS_PER_DAY;

/// What is kept on disk between runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoredPass {
    pub pass: String,
    /// None for an offline pass file, which isn't renewed.
    pub refresh_token: Option<String>,
    /// The latest time the app ran with this pass (seconds since 1970-01-01 UTC), to catch the
    /// clock being moved back.
    pub last_seen: Option<i64>,
}

/// Where the pass is kept. Stage 3 writes it atomically to `%APPDATA%\GeoGIS`, readable only by
/// the user; the refresh token is the one secret in it.
pub trait PassStore: Send + Sync {
    fn load(&self) -> Result<Option<StoredPass>, LicenceError>;
    fn save(&self, stored: &StoredPass) -> Result<(), LicenceError>;
    fn clear(&self) -> Result<(), LicenceError>;
}

pub trait Clock: Send + Sync {
    /// Seconds since 1970-01-01 UTC.
    fn now(&self) -> i64;
}

/// What the app may do now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    Subscribed { pass: Pass, days_left: i64 },
    Free(FreeReason),
}

impl Access {
    pub fn allows(&self, feature: Feature) -> bool {
        matches!(self, Access::Subscribed { pass, .. } if pass.allows(feature))
    }

    pub fn plan(&self) -> Plan {
        match self {
            Access::Subscribed { pass, .. } => pass.plan,
            Access::Free(_) => Plan::Free,
        }
    }

    /// True when the status bar should ask for something: going online soon, or a problem with
    /// the pass. Plain free use (never signed in) asks nothing.
    pub fn needs_attention(&self) -> bool {
        match self {
            Access::Subscribed { days_left, .. } => *days_left <= RENEW_WARN_DAYS,
            Access::Free(reason) => *reason != FreeReason::NotSignedIn,
        }
    }
}

/// Why the app is in the free tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FreeReason {
    NotSignedIn,
    /// Offline for longer than the pass allows.
    Expired,
    WrongMachine,
    InvalidPass,
    ClockBack,
    /// The machine ID could not be read. The pass is kept for the next run.
    NoMachineId,
    /// The stored pass could not be read or saved.
    Storage,
}

impl FreeReason {
    pub fn message(self) -> &'static str {
        match self {
            FreeReason::NotSignedIn => "Free version. Sign in to use your subscription.",
            FreeReason::Expired => "GeoGIS hasn't reached your account for 30 days. Connect to the internet to keep using paid features.",
            FreeReason::WrongMachine => "This sign-in belongs to a different computer. Sign in again here.",
            FreeReason::InvalidPass => "Your sign-in could not be checked. Sign in again.",
            FreeReason::ClockBack => "The computer's date is earlier than your last use. Correct the date and time, then restart GeoGIS.",
            FreeReason::NoMachineId => {
                "GeoGIS could not read this computer's ID to check your subscription. Restart GeoGIS; if it happens again, contact support."
            }
            FreeReason::Storage => {
                "GeoGIS could not read or save your sign-in. Check that your user folder is writable, then restart GeoGIS."
            }
        }
    }
}

/// What the window asks of licensing. Calls may run a program (the machine ID) or touch the disk,
/// so the window runs them on a worker thread.
pub trait Licensing: Send + Sync {
    /// The decision at start-up, from the stored pass.
    fn startup(&self) -> Access;
    /// Keeps a grant from sign-in or renewal.
    fn accept_grant(&self, grant: &Grant) -> Result<Access, FreeReason>;
    /// Keeps an offline pass file opened by the person.
    fn accept_offline_pass(&self, text: &str) -> Result<Access, FreeReason>;
    /// The refresh token to renew with, when the stored pass is an online one older than a day.
    fn due_for_renewal(&self) -> Option<String>;
    /// Forgets the stored pass (after signing out, or when the server revoked it).
    fn forget(&self) -> Result<(), LicenceError>;
    fn machine_id(&self) -> Result<MachineId, LicenceError>;
}

/// The rules over pluggable parts.
pub struct Gate {
    pub machine: Box<dyn MachineIdSource>,
    pub verifier: Box<dyn PassVerifier>,
    pub store: Box<dyn PassStore>,
    pub clock: Box<dyn Clock>,
}

impl Gate {
    /// Checks a pass for this computer now. A pass that checks out is saved with `refresh_token`.
    fn check(&self, text: &str, refresh_token: Option<String>, last_seen: Option<i64>) -> Result<Access, FreeReason> {
        let pass = Pass::verify(text, self.verifier.as_ref()).map_err(|_| FreeReason::InvalidPass)?;
        let machine = self.machine.machine_id().map_err(|_| FreeReason::NoMachineId)?;
        let now = self.clock.now();
        match pass.status(&machine, now, last_seen) {
            PassStatus::Valid { days_left } => {
                let stored = StoredPass {
                    pass: text.trim().to_string(),
                    refresh_token,
                    last_seen: Some(last_seen.map_or(now, |seen| seen.max(now))),
                };
                self.store.save(&stored).map_err(|_| FreeReason::Storage)?;
                Ok(Access::Subscribed { pass, days_left })
            }
            PassStatus::Expired => Err(FreeReason::Expired),
            PassStatus::WrongMachine => Err(FreeReason::WrongMachine),
            PassStatus::ClockBack => Err(FreeReason::ClockBack),
        }
    }
}

impl Licensing for Gate {
    /// The stored pass decides. An expired pass is kept, so its refresh token can renew it the
    /// moment the computer is online; a broken or foreign one is cleared.
    fn startup(&self) -> Access {
        let stored = match self.store.load() {
            Ok(Some(stored)) => stored,
            Ok(None) => return Access::Free(FreeReason::NotSignedIn),
            Err(_) => return Access::Free(FreeReason::Storage),
        };
        match self.check(&stored.pass, stored.refresh_token, stored.last_seen) {
            Ok(access) => access,
            Err(reason @ (FreeReason::InvalidPass | FreeReason::WrongMachine)) => {
                let _ = self.store.clear();
                Access::Free(reason)
            }
            Err(reason) => Access::Free(reason),
        }
    }

    fn accept_grant(&self, grant: &Grant) -> Result<Access, FreeReason> {
        let last_seen = self.store.load().ok().flatten().and_then(|s| s.last_seen);
        self.check(&grant.pass, Some(grant.refresh_token.clone()), last_seen)
    }

    fn accept_offline_pass(&self, text: &str) -> Result<Access, FreeReason> {
        let pass = Pass::verify(text, self.verifier.as_ref()).map_err(|_| FreeReason::InvalidPass)?;
        if pass.kind != PassKind::Offline {
            return Err(FreeReason::InvalidPass);
        }
        let last_seen = self.store.load().ok().flatten().and_then(|s| s.last_seen);
        self.check(text, None, last_seen)
    }

    fn due_for_renewal(&self) -> Option<String> {
        let stored = self.store.load().ok().flatten()?;
        let token = stored.refresh_token?;
        let pass = Pass::verify(&stored.pass, self.verifier.as_ref()).ok()?;
        (pass.kind == PassKind::Online && self.clock.now() - pass.issued >= RENEW_AFTER_SECS).then_some(token)
    }

    fn forget(&self) -> Result<(), LicenceError> {
        self.store.clear()
    }

    fn machine_id(&self) -> Result<MachineId, LicenceError> {
        self.machine.machine_id()
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::expect_used)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::pass::tests::{OFFLINE_FILE, PRO_30_DAYS, keys};

    const ISSUED: i64 = 1_792_108_800;
    const DAY: i64 = SECS_PER_DAY;

    struct Fixed(Result<&'static str, ()>);
    impl MachineIdSource for Fixed {
        fn machine_id(&self) -> Result<MachineId, LicenceError> {
            self.0.map_err(|_| LicenceError::MachineId("timed out".into())).and_then(MachineId::from_output)
        }
    }

    #[derive(Clone, Default)]
    struct Mem {
        value: Arc<Mutex<Option<StoredPass>>>,
        broken: bool,
    }
    impl Mem {
        fn get(&self) -> Option<StoredPass> {
            self.value.lock().expect("lock").clone()
        }
    }
    impl PassStore for Mem {
        fn load(&self) -> Result<Option<StoredPass>, LicenceError> {
            Ok(self.get())
        }
        fn save(&self, stored: &StoredPass) -> Result<(), LicenceError> {
            if self.broken {
                return Err(LicenceError::Storage("read-only".into()));
            }
            *self.value.lock().expect("lock") = Some(stored.clone());
            Ok(())
        }
        fn clear(&self) -> Result<(), LicenceError> {
            *self.value.lock().expect("lock") = None;
            Ok(())
        }
    }

    struct At(i64);
    impl Clock for At {
        fn now(&self) -> i64 {
            self.0
        }
    }

    fn gate(machine: Result<&'static str, ()>, store: &Mem, now: i64) -> Gate {
        Gate { machine: Box::new(Fixed(machine)), verifier: Box::new(keys()), store: Box::new(store.clone()), clock: Box::new(At(now)) }
    }

    fn grant() -> Grant {
        Grant { pass: PRO_30_DAYS.into(), refresh_token: "rt-1".into() }
    }

    #[test]
    fn never_signed_in_is_plain_free() {
        let access = gate(Ok("TEST-MACHINE-ID"), &Mem::default(), ISSUED).startup();
        assert_eq!(access, Access::Free(FreeReason::NotSignedIn));
        assert!(!access.needs_attention());
        assert!(!access.allows(Feature::Reports));
        assert_eq!(access.plan(), Plan::Free);
    }

    #[test]
    fn sign_in_unlocks_and_lasts_thirty_days_offline() {
        let store = Mem::default();
        let access = gate(Ok("TEST-MACHINE-ID"), &store, ISSUED + 60).accept_grant(&grant()).expect("accepted");
        assert!(access.allows(Feature::Thermal));
        assert_eq!(access.plan(), Plan::Pro);
        assert_eq!(store.get().and_then(|s| s.refresh_token).as_deref(), Some("rt-1"));
        let day_26 = gate(Ok("TEST-MACHINE-ID"), &store, ISSUED + 26 * DAY).startup();
        assert!(matches!(day_26, Access::Subscribed { days_left: 4, .. }));
        assert!(day_26.needs_attention());
        let day_30 = gate(Ok("TEST-MACHINE-ID"), &store, ISSUED + 30 * DAY).startup();
        assert_eq!(day_30, Access::Free(FreeReason::Expired));
        // Kept, so renewing works as soon as it is online.
        assert_eq!(gate(Ok("TEST-MACHINE-ID"), &store, ISSUED + 30 * DAY).due_for_renewal().as_deref(), Some("rt-1"));
    }

    #[test]
    fn renewal_is_due_after_a_day() {
        let store = Mem::default();
        let _ = gate(Ok("TEST-MACHINE-ID"), &store, ISSUED).accept_grant(&grant());
        assert_eq!(gate(Ok("TEST-MACHINE-ID"), &store, ISSUED + 60).due_for_renewal(), None);
        assert_eq!(gate(Ok("TEST-MACHINE-ID"), &store, ISSUED + DAY).due_for_renewal().as_deref(), Some("rt-1"));
    }

    #[test]
    fn moving_the_clock_back_is_caught_between_runs() {
        let store = Mem::default();
        let _ = gate(Ok("TEST-MACHINE-ID"), &store, ISSUED).accept_grant(&grant());
        assert!(gate(Ok("TEST-MACHINE-ID"), &store, ISSUED + 20 * DAY).startup().allows(Feature::Markup));
        assert_eq!(gate(Ok("TEST-MACHINE-ID"), &store, ISSUED + 3 * DAY).startup(), Access::Free(FreeReason::ClockBack));
    }

    #[test]
    fn other_computers_and_damage_clear_the_pass() {
        let store = Mem::default();
        let _ = gate(Ok("TEST-MACHINE-ID"), &store, ISSUED).accept_grant(&grant());
        assert_eq!(gate(Ok("OTHER-PC"), &store, ISSUED).startup(), Access::Free(FreeReason::WrongMachine));
        assert_eq!(store.get(), None);
        let store = Mem {
            value: Arc::new(Mutex::new(Some(StoredPass { pass: "SECRET_ADMIN_DEBUG".into(), ..StoredPass::default() }))),
            broken: false,
        };
        assert_eq!(gate(Ok("TEST-MACHINE-ID"), &store, ISSUED).startup(), Access::Free(FreeReason::InvalidPass));
        assert_eq!(store.get(), None);
    }

    #[test]
    fn an_unreadable_machine_id_keeps_the_pass() {
        let store = Mem::default();
        let _ = gate(Ok("TEST-MACHINE-ID"), &store, ISSUED).accept_grant(&grant());
        assert_eq!(gate(Err(()), &store, ISSUED).startup(), Access::Free(FreeReason::NoMachineId));
        assert!(store.get().is_some());
    }

    #[test]
    fn offline_files_for_sites_without_internet() {
        let store = Mem::default();
        let g = gate(Ok("TEST-MACHINE-ID"), &store, ISSUED + 100 * DAY);
        assert_eq!(g.accept_offline_pass(PRO_30_DAYS), Err(FreeReason::InvalidPass));
        let access = g.accept_offline_pass(OFFLINE_FILE).expect("accepted");
        assert_eq!(access.plan(), Plan::Organisation);
        assert_eq!(store.get().and_then(|s| s.refresh_token), None);
        assert_eq!(g.due_for_renewal(), None);
    }

    #[test]
    fn unsaveable_sign_in_is_refused() {
        let store = Mem { broken: true, ..Mem::default() };
        assert_eq!(gate(Ok("TEST-MACHINE-ID"), &store, ISSUED).accept_grant(&grant()), Err(FreeReason::Storage));
    }

    #[test]
    fn every_reason_has_a_message() {
        for r in [
            FreeReason::NotSignedIn,
            FreeReason::Expired,
            FreeReason::WrongMachine,
            FreeReason::InvalidPass,
            FreeReason::ClockBack,
            FreeReason::NoMachineId,
            FreeReason::Storage,
        ] {
            assert!(r.message().ends_with('.'));
        }
    }
}
