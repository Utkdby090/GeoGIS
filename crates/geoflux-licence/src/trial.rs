//! The 15-day trial, as GeoFlux's `TrialManager.kt` ran it.
//!
//! The first launch stores the time; every launch after it counts whole days since then. The
//! trial ends after 15 days, or as soon as the clock reads more than a day before the first launch.

use crate::LicenceError;
use crate::date::{parse_instant, whole_days_between};

pub const TRIAL_DAYS: i64 = 15;

/// Where the trial stands at one moment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrialStatus {
    /// `days_left` is 15 on the first day and 1 on the last.
    Active {
        days_left: i64,
    },
    Ended,
    /// The clock reads at least a whole day before the first launch (GeoFlux's tamper check).
    ClockBack,
}

/// The trial at `now_ms`, for a trial first launched at `first_launch_ms`.
pub fn trial_status(first_launch_ms: i64, now_ms: i64) -> TrialStatus {
    let elapsed = whole_days_between(first_launch_ms, now_ms);
    if elapsed < 0 {
        TrialStatus::ClockBack
    } else if elapsed >= TRIAL_DAYS {
        TrialStatus::Ended
    } else {
        TrialStatus::Active { days_left: TRIAL_DAYS - elapsed }
    }
}

/// When the trial started, from every record found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrialStart {
    /// No record anywhere: this is the first launch.
    FirstLaunch,
    /// The earliest start found, in ms since 1970-01-01 UTC.
    Started(i64),
    /// A record exists but can't be read. GeoFlux locked in this case, so a hand-edited value
    /// can't restart the trial.
    Damaged,
}

/// The earliest of the stored first-launch values (GeoGIS's own, and the one GeoFlux left on the
/// same computer), so installing GeoGIS where GeoFlux already ran doesn't start a new trial.
pub fn earliest_start<'a>(records: impl IntoIterator<Item = Option<&'a str>>) -> TrialStart {
    let mut earliest: Option<i64> = None;
    for record in records.into_iter().flatten() {
        match parse_instant(record.trim()) {
            Some(ms) => earliest = Some(earliest.map_or(ms, |e| e.min(ms))),
            None => return TrialStart::Damaged,
        }
    }
    earliest.map_or(TrialStart::FirstLaunch, TrialStart::Started)
}

/// Where a first-launch time is kept. GeoGIS's own store is read and written; GeoFlux's old record
/// (Java preferences, `HKCU\Software\JavaSoft\Prefs\com\geospatial\processing\trial`) is only read,
/// and its `set_first_launch` does nothing.
pub trait TrialStore: Send + Sync {
    /// The stored value as written (`2026-10-01T05:12:33.123Z`), or None when there is none.
    fn first_launch(&self) -> Result<Option<String>, LicenceError>;
    fn set_first_launch(&self, value: &str) -> Result<(), LicenceError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::date::format_instant;

    const DAY: i64 = 86_400_000;

    #[test]
    fn fifteen_whole_days() {
        let start = 1_790_000_000_000;
        assert_eq!(trial_status(start, start), TrialStatus::Active { days_left: 15 });
        assert_eq!(trial_status(start, start + 14 * DAY + DAY - 1), TrialStatus::Active { days_left: 1 });
        assert_eq!(trial_status(start, start + 15 * DAY), TrialStatus::Ended);
        assert_eq!(trial_status(start, i64::MAX), TrialStatus::Ended);
    }

    #[test]
    fn clock_moved_back_a_day_locks() {
        let start = 1_790_000_000_000;
        // Java's DAYS.between truncates toward zero, so less than a day back still counts as day 0.
        assert_eq!(trial_status(start, start - DAY + 1), TrialStatus::Active { days_left: 15 });
        assert_eq!(trial_status(start, start - DAY), TrialStatus::ClockBack);
        assert_eq!(trial_status(start, i64::MIN), TrialStatus::ClockBack);
    }

    #[test]
    fn earliest_record_wins_and_damage_locks() {
        let a = format_instant(1_790_000_000_000);
        let b = format_instant(1_780_000_000_000);
        assert_eq!(earliest_start([None, None]), TrialStart::FirstLaunch);
        assert_eq!(earliest_start([Some(a.as_str()), None]), TrialStart::Started(1_790_000_000_000));
        assert_eq!(earliest_start([Some(a.as_str()), Some(b.as_str())]), TrialStart::Started(1_780_000_000_000));
        assert_eq!(earliest_start([Some(a.as_str()), Some("2026-13-01T00:00:00Z")]), TrialStart::Damaged);
        assert_eq!(earliest_start([Some("")]), TrialStart::Damaged);
    }
}
