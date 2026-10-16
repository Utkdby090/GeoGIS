//! The decision at start-up and on activation: licensed, in trial, or locked, and why.
//!
//! Everything the decision reads (machine ID, stored key, trial record, clock) comes through a
//! trait, so the rules are tested here without Windows, and the window only shows the result.

use crate::date::format_instant;
use crate::trial::{TrialStart, TrialStatus, earliest_start, trial_status};
use crate::{CivilDate, KeyStatus, LicenceError, MachineId, MachineIdSource, SignatureVerifier, TrialStore, check_key};

/// Days left at or under which the status bar turns red, as in GeoFlux.
pub const LICENCE_WARN_DAYS: i64 = 5;
pub const TRIAL_WARN_DAYS: i64 = 3;

/// Where the activated key is kept. GeoGIS's own store is read and written; GeoFlux's old one
/// (`HKCU\Software\JavaSoft\Prefs\com\geospatial\app`, value `saved_license_key`) is only read, so
/// a customer moving from GeoFlux doesn't have to enter the key again.
pub trait LicenceStore: Send + Sync {
    fn load(&self) -> Result<Option<String>, LicenceError>;
    fn save(&self, key: &str) -> Result<(), LicenceError>;
    fn clear(&self) -> Result<(), LicenceError>;
}

/// The time. `today` is the computer's local date, which GeoFlux compared expiry dates with.
pub trait Clock: Send + Sync {
    /// Milliseconds since 1970-01-01 UTC.
    fn now_ms(&self) -> i64;
    fn today(&self) -> CivilDate;
}

/// What the app may do now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    Licensed { days_left: i64 },
    Trial { days_left: i64 },
    Locked(LockReason),
}

impl Access {
    pub fn is_open(&self) -> bool {
        !matches!(self, Access::Locked(_))
    }

    /// True when the status bar should be red: a licence with 5 days or fewer left, a trial with 3
    /// or fewer, or a lock.
    pub fn needs_attention(&self) -> bool {
        match self {
            Access::Licensed { days_left } => *days_left <= LICENCE_WARN_DAYS,
            Access::Trial { days_left } => *days_left <= TRIAL_WARN_DAYS,
            Access::Locked(_) => true,
        }
    }
}

/// Why the app is locked, or why a key was refused. Each has one message for the lock screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockReason {
    TrialEnded,
    Expired,
    WrongMachine,
    InvalidKey,
    /// The machine ID could not be read. The saved key is kept (GeoFlux cleared it, so one slow
    /// PowerShell start made customers enter their key again).
    NoMachineId,
    /// The clock reads before the trial started.
    ClockBack,
    /// The trial record exists but can't be read.
    TrialDamaged,
    /// The licence or trial record could not be saved, so the trial would restart on every launch.
    Storage,
}

impl LockReason {
    pub fn message(self) -> &'static str {
        match self {
            LockReason::TrialEnded => "Your 15-day trial has ended. Enter a licence key to continue.",
            LockReason::Expired => "Your licence has expired. Enter a new key to continue.",
            LockReason::WrongMachine => "This licence key is for a different computer.",
            LockReason::InvalidKey => "This licence key is not valid.",
            LockReason::NoMachineId => {
                "GeoGIS could not read this computer's ID to check the licence. Restart GeoGIS; if it happens again, contact support."
            }
            LockReason::ClockBack => {
                "The computer's date is earlier than when the trial started. Correct the date and time, then restart GeoGIS."
            }
            LockReason::TrialDamaged => "The trial record is damaged. Enter a licence key to continue.",
            LockReason::Storage => {
                "GeoGIS could not save its licence settings. Check that your user folder is writable, then restart GeoGIS."
            }
        }
    }
}

/// What the window asks of licensing. Every call may run a program (the machine ID) or touch the
/// disk, so the window calls it on a worker thread and shows "Checking licence…" meanwhile.
pub trait Licensing: Send + Sync {
    /// The decision at start-up.
    fn startup(&self) -> Access;
    /// Checks and saves a key typed on the lock screen.
    fn activate(&self, key: &str) -> Result<Access, LockReason>;
    /// The ID shown on the lock screen, for the customer to send when buying a key.
    fn machine_id(&self) -> Result<MachineId, LicenceError>;
}

/// GeoFlux's rules over pluggable parts.
pub struct Gate {
    pub machine: Box<dyn MachineIdSource>,
    pub verifier: Box<dyn SignatureVerifier>,
    pub licence: Box<dyn LicenceStore>,
    /// GeoFlux's saved key on this computer, read once to carry it over.
    pub legacy_licence: Option<Box<dyn LicenceStore>>,
    pub trial: Box<dyn TrialStore>,
    /// GeoFlux's trial record on this computer; the earlier start counts.
    pub legacy_trial: Option<Box<dyn TrialStore>>,
    pub clock: Box<dyn Clock>,
}

impl Gate {
    fn key_access(&self, key: &str) -> Result<i64, LockReason> {
        let machine = self.machine.machine_id().map_err(|_| LockReason::NoMachineId)?;
        match check_key(key, &machine, self.clock.today(), self.verifier.as_ref()) {
            KeyStatus::Valid { days_left } => Ok(days_left),
            KeyStatus::Expired => Err(LockReason::Expired),
            KeyStatus::WrongMachine => Err(LockReason::WrongMachine),
            KeyStatus::Invalid => Err(LockReason::InvalidKey),
        }
    }

    fn trial_access(&self) -> Access {
        let own = match self.trial.first_launch() {
            Ok(v) => v,
            Err(_) => return Access::Locked(LockReason::Storage),
        };
        // GeoFlux's record is a bonus: unreadable storage there just means it isn't counted.
        let legacy = self.legacy_trial.as_ref().and_then(|t| t.first_launch().ok().flatten());
        let now = self.clock.now_ms();
        match earliest_start([own.as_deref(), legacy.as_deref()]) {
            TrialStart::FirstLaunch => match self.trial.set_first_launch(&format_instant(now)) {
                Ok(()) => Access::Trial { days_left: crate::TRIAL_DAYS },
                Err(_) => Access::Locked(LockReason::Storage),
            },
            TrialStart::Damaged => Access::Locked(LockReason::TrialDamaged),
            TrialStart::Started(start) => match trial_status(start, now) {
                TrialStatus::Active { days_left } => Access::Trial { days_left },
                TrialStatus::Ended => Access::Locked(LockReason::TrialEnded),
                TrialStatus::ClockBack => Access::Locked(LockReason::ClockBack),
            },
        }
    }
}

impl Licensing for Gate {
    /// GeoFlux's start-up: a saved key decides when there is one (an expired, foreign or broken key
    /// is cleared and locks this launch); otherwise the trial decides. A key carried over from
    /// GeoFlux is saved to GeoGIS's store once it checks out, and ignored when it doesn't.
    fn startup(&self) -> Access {
        let saved = match self.licence.load() {
            Ok(saved) => saved,
            Err(_) => return Access::Locked(LockReason::Storage),
        };
        if let Some(key) = saved {
            return match self.key_access(&key) {
                Ok(days_left) => Access::Licensed { days_left },
                Err(LockReason::NoMachineId) => Access::Locked(LockReason::NoMachineId),
                Err(reason) => {
                    let _ = self.licence.clear();
                    Access::Locked(reason)
                }
            };
        }
        if let Some(key) = self.legacy_licence.as_ref().and_then(|l| l.load().ok().flatten()) {
            match self.key_access(&key) {
                Ok(days_left) => {
                    let _ = self.licence.save(key.trim());
                    return Access::Licensed { days_left };
                }
                Err(LockReason::NoMachineId) => return Access::Locked(LockReason::NoMachineId),
                Err(_) => {}
            }
        }
        self.trial_access()
    }

    fn activate(&self, key: &str) -> Result<Access, LockReason> {
        let days_left = self.key_access(key)?;
        self.licence.save(key.trim()).map_err(|_| LockReason::Storage)?;
        Ok(Access::Licensed { days_left })
    }

    fn machine_id(&self) -> Result<MachineId, LicenceError> {
        self.machine.machine_id()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::date::parse_instant;

    /// Accepts a "signature" equal to the payload reversed, so tests can make keys.
    struct Mirror;
    impl SignatureVerifier for Mirror {
        fn verify(&self, message: &[u8], signature: &[u8]) -> bool {
            message.iter().rev().eq(signature.iter())
        }
    }

    fn key(payload: &str) -> String {
        use base64::Engine;
        let e = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let sig: Vec<u8> = payload.bytes().rev().collect();
        format!("{}.{}", e.encode(payload), e.encode(sig))
    }

    struct Fixed(Result<&'static str, ()>);
    impl MachineIdSource for Fixed {
        fn machine_id(&self) -> Result<MachineId, LicenceError> {
            self.0.map_err(|_| LicenceError::MachineId("timed out".into())).and_then(MachineId::from_output)
        }
    }

    #[derive(Clone, Default)]
    struct Mem {
        value: Arc<Mutex<Option<String>>>,
        broken: bool,
    }
    impl Mem {
        fn with(v: &str) -> Self {
            Mem { value: Arc::new(Mutex::new(Some(v.into()))), broken: false }
        }
        fn get(&self) -> Option<String> {
            self.value.lock().expect("lock").clone()
        }
        fn put(&self, v: Option<&str>) -> Result<(), LicenceError> {
            if self.broken {
                return Err(LicenceError::Storage("read-only".into()));
            }
            *self.value.lock().expect("lock") = v.map(str::to_string);
            Ok(())
        }
    }
    impl LicenceStore for Mem {
        fn load(&self) -> Result<Option<String>, LicenceError> {
            Ok(self.get())
        }
        fn save(&self, key: &str) -> Result<(), LicenceError> {
            self.put(Some(key))
        }
        fn clear(&self) -> Result<(), LicenceError> {
            self.put(None)
        }
    }
    impl TrialStore for Mem {
        fn first_launch(&self) -> Result<Option<String>, LicenceError> {
            Ok(self.get())
        }
        fn set_first_launch(&self, value: &str) -> Result<(), LicenceError> {
            self.put(Some(value))
        }
    }

    struct At(i64, CivilDate);
    impl Clock for At {
        fn now_ms(&self) -> i64 {
            self.0
        }
        fn today(&self) -> CivilDate {
            self.1
        }
    }

    const NOW: i64 = 1_792_108_800_000; // 2026-10-16T00:00:00Z
    const DAY: i64 = 86_400_000;

    fn today() -> CivilDate {
        CivilDate::from_unix_ms(NOW)
    }

    struct Parts {
        licence: Mem,
        legacy_licence: Mem,
        trial: Mem,
        legacy_trial: Mem,
    }

    fn gate(machine: Result<&'static str, ()>, p: &Parts, now: i64) -> Gate {
        Gate {
            machine: Box::new(Fixed(machine)),
            verifier: Box::new(Mirror),
            licence: Box::new(p.licence.clone()),
            legacy_licence: Some(Box::new(p.legacy_licence.clone())),
            trial: Box::new(p.trial.clone()),
            legacy_trial: Some(Box::new(p.legacy_trial.clone())),
            clock: Box::new(At(now, CivilDate::from_unix_ms(now))),
        }
    }

    fn parts() -> Parts {
        Parts { licence: Mem::default(), legacy_licence: Mem::default(), trial: Mem::default(), legacy_trial: Mem::default() }
    }

    #[test]
    fn first_launch_starts_the_trial_and_saves_it() {
        let p = parts();
        assert_eq!(gate(Ok("PC-1"), &p, NOW).startup(), Access::Trial { days_left: 15 });
        assert_eq!(p.trial.get().and_then(|t| parse_instant(&t)), Some(NOW));
        assert_eq!(gate(Ok("PC-1"), &p, NOW + 10 * DAY).startup(), Access::Trial { days_left: 5 });
        assert_eq!(gate(Ok("PC-1"), &p, NOW + 15 * DAY).startup(), Access::Locked(LockReason::TrialEnded));
        assert_eq!(gate(Ok("PC-1"), &p, NOW - DAY).startup(), Access::Locked(LockReason::ClockBack));
    }

    #[test]
    fn the_trial_works_without_a_machine_id() {
        let p = parts();
        assert_eq!(gate(Err(()), &p, NOW).startup(), Access::Trial { days_left: 15 });
    }

    #[test]
    fn geoflux_trial_on_the_same_computer_counts() {
        let p = parts();
        *p.legacy_trial.value.lock().expect("lock") = Some(format_instant(NOW - 20 * DAY));
        assert_eq!(gate(Ok("PC-1"), &p, NOW).startup(), Access::Locked(LockReason::TrialEnded));
    }

    #[test]
    fn unsaveable_or_damaged_trial_locks() {
        let mut p = parts();
        p.trial.broken = true;
        assert_eq!(gate(Ok("PC-1"), &p, NOW).startup(), Access::Locked(LockReason::Storage));
        let p = Parts { trial: Mem::with("tomorrow"), ..parts() };
        assert_eq!(gate(Ok("PC-1"), &p, NOW).startup(), Access::Locked(LockReason::TrialDamaged));
    }

    #[test]
    fn a_saved_key_decides() {
        let good = key(&format!("PC-1|{}", CivilDate::from_unix_ms(NOW + 30 * DAY)));
        let p = Parts { licence: Mem::with(&good), ..parts() };
        let access = gate(Ok("pc-1"), &p, NOW).startup();
        assert_eq!(access, Access::Licensed { days_left: 30 });
        assert!(!access.needs_attention());
        assert!(gate(Ok("pc-1"), &p, NOW + 25 * DAY).startup().needs_attention());
        // Past its date: cleared and locked, like GeoFlux.
        assert_eq!(gate(Ok("PC-1"), &p, NOW + 31 * DAY).startup(), Access::Locked(LockReason::Expired));
        assert_eq!(p.licence.get(), None);
    }

    #[test]
    fn foreign_or_broken_keys_are_cleared() {
        let foreign = key(&format!("PC-2|{}", today()));
        let p = Parts { licence: Mem::with(&foreign), ..parts() };
        assert_eq!(gate(Ok("PC-1"), &p, NOW).startup(), Access::Locked(LockReason::WrongMachine));
        assert_eq!(p.licence.get(), None);
        let p = Parts { licence: Mem::with("SECRET_ADMIN_DEBUG"), ..parts() };
        assert_eq!(gate(Ok("PC-1"), &p, NOW).startup(), Access::Locked(LockReason::InvalidKey));
    }

    #[test]
    fn an_unreadable_machine_id_keeps_the_key() {
        let good = key(&format!("PC-1|{}", today()));
        let p = Parts { licence: Mem::with(&good), ..parts() };
        assert_eq!(gate(Err(()), &p, NOW).startup(), Access::Locked(LockReason::NoMachineId));
        assert_eq!(p.licence.get(), Some(good.clone()));
        assert_eq!(gate(Ok("PC-1"), &p, NOW).startup(), Access::Licensed { days_left: 0 });
    }

    #[test]
    fn a_geoflux_key_carries_over_once_it_checks_out() {
        let good = key(&format!("PC-1|{}", today()));
        let p = Parts { legacy_licence: Mem::with(&format!("{good}\n")), ..parts() };
        assert_eq!(gate(Ok("PC-1"), &p, NOW).startup(), Access::Licensed { days_left: 0 });
        assert_eq!(p.licence.get(), Some(good));
        // An expired GeoFlux key is ignored, not cleared; the trial decides.
        let old = key(&format!("PC-1|{}", CivilDate::from_unix_ms(NOW - DAY)));
        let p = Parts { legacy_licence: Mem::with(&old), ..parts() };
        assert_eq!(gate(Ok("PC-1"), &p, NOW).startup(), Access::Trial { days_left: 15 });
        assert_eq!(p.legacy_licence.get(), Some(old));
    }

    #[test]
    fn activation_checks_then_saves() {
        let p = parts();
        let g = gate(Ok("PC-1"), &p, NOW);
        assert_eq!(g.activate("SECRET_ADMIN_DEBUG"), Err(LockReason::InvalidKey));
        assert_eq!(g.activate(&key(&format!("PC-2|{}", today()))), Err(LockReason::WrongMachine));
        assert_eq!(g.activate(&key(&format!("PC-1|{}", CivilDate::from_unix_ms(NOW - DAY)))), Err(LockReason::Expired));
        assert_eq!(p.licence.get(), None);
        let good = key(&format!("PC-1|{}", CivilDate::from_unix_ms(NOW + 100 * DAY)));
        assert_eq!(g.activate(&format!(" {good} ")), Ok(Access::Licensed { days_left: 100 }));
        assert_eq!(p.licence.get(), Some(good));
        let mut q = parts();
        q.licence.broken = true;
        assert_eq!(gate(Ok("PC-1"), &q, NOW).activate(&key(&format!("PC-1|{}", today()))), Err(LockReason::Storage));
        assert_eq!(gate(Err(()), &q, NOW).activate("x.y"), Err(LockReason::NoMachineId));
    }

    #[test]
    fn every_reason_has_a_message() {
        for r in [
            LockReason::TrialEnded,
            LockReason::Expired,
            LockReason::WrongMachine,
            LockReason::InvalidKey,
            LockReason::NoMachineId,
            LockReason::ClockBack,
            LockReason::TrialDamaged,
            LockReason::Storage,
        ] {
            assert!(r.message().ends_with('.'));
        }
    }
}
