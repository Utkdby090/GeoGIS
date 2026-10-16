//! Dates and instants in the text forms GeoFlux stored them, without a date library.
//!
//! - A licence's expiry is a calendar date, `YYYY-MM-DD` (Java's `LocalDate`).
//! - The trial's first launch is an instant, `2026-10-01T05:12:33.123Z` (Java's `Instant`).

use std::fmt;

const MS_PER_DAY: i64 = 86_400_000;

/// A calendar date (proleptic Gregorian, years 0 to 9999).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CivilDate {
    year: i32,
    month: u8,
    day: u8,
}

impl CivilDate {
    /// None unless it is a real date (no 30 February).
    pub fn new(year: i32, month: u8, day: u8) -> Option<Self> {
        ((0..=9999).contains(&year) && (1..=12).contains(&month) && day >= 1 && day <= days_in_month(year, month)).then_some(CivilDate {
            year,
            month,
            day,
        })
    }

    /// Reads `YYYY-MM-DD` as strictly as Java's `LocalDate.parse`: four-digit year, two-digit month
    /// and day, a real date, nothing around it.
    pub fn parse(text: &str) -> Option<Self> {
        let b = text.as_bytes();
        if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
            return None;
        }
        let year = digits(&b[0..4])?;
        let month = digits(&b[5..7])?;
        let day = digits(&b[8..10])?;
        CivilDate::new(year as i32, month as u8, day as u8)
    }

    pub fn year(self) -> i32 {
        self.year
    }

    pub fn month(self) -> u8 {
        self.month
    }

    pub fn day(self) -> u8 {
        self.day
    }

    /// Days since 1970-01-01 (negative before it).
    pub fn to_days(self) -> i64 {
        // Howard Hinnant's days_from_civil.
        let y = i64::from(self.year) - i64::from(self.month <= 2);
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let m = i64::from(self.month);
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(self.day) - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    /// The date `days` after 1970-01-01, clamped to years 0 to 9999.
    pub fn from_days(days: i64) -> Self {
        let days = days.clamp(CivilDate::MIN.to_days(), CivilDate::MAX.to_days());
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
        let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u8;
        let year = (yoe + era * 400 + i64::from(month <= 2)) as i32;
        CivilDate { year, month, day }
    }

    /// The UTC date at `ms` milliseconds since 1970-01-01 UTC.
    pub fn from_unix_ms(ms: i64) -> Self {
        CivilDate::from_days(ms.div_euclid(MS_PER_DAY))
    }

    /// Days from `self` to `later`; negative when `later` is earlier.
    pub fn days_until(self, later: CivilDate) -> i64 {
        later.to_days() - self.to_days()
    }

    pub const MIN: CivilDate = CivilDate { year: 0, month: 1, day: 1 };
    pub const MAX: CivilDate = CivilDate { year: 9999, month: 12, day: 31 };
}

impl fmt::Display for CivilDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// ASCII digits only (no sign, no spaces), as a number.
fn digits(b: &[u8]) -> Option<u32> {
    b.iter().try_fold(0u32, |n, &c| c.is_ascii_digit().then(|| n * 10 + u32::from(c - b'0')))
}

/// Reads an instant as Java's `Instant.toString` writes it (`2026-10-01T05:12:33.123456Z`) into
/// milliseconds since 1970-01-01 UTC. Seconds and the fraction are optional, `Z` is required;
/// digits past milliseconds are dropped.
pub fn parse_instant(text: &str) -> Option<i64> {
    let (date, time) = text.split_once('T')?;
    let time = time.strip_suffix('Z')?;
    let date = CivilDate::parse(date)?;
    let (hms, fraction) = match time.split_once('.') {
        Some((hms, f)) if !f.is_empty() && f.len() <= 9 => (hms, Some(f)),
        Some(_) => return None,
        None => (time, None),
    };
    let b = hms.as_bytes();
    let (h, m, s) = match b.len() {
        5 if fraction.is_none() && b[2] == b':' => (digits(&b[0..2])?, digits(&b[3..5])?, 0),
        8 if b[2] == b':' && b[5] == b':' => (digits(&b[0..2])?, digits(&b[3..5])?, digits(&b[6..8])?),
        _ => return None,
    };
    if h > 23 || m > 59 || s > 59 {
        return None;
    }
    let ms = match fraction {
        Some(f) => {
            digits(f.as_bytes())?;
            let padded = format!("{f:0<3}");
            digits(&padded.as_bytes()[..3])?
        }
        None => 0,
    };
    Some(date.to_days() * MS_PER_DAY + (i64::from(h) * 3600 + i64::from(m) * 60 + i64::from(s)) * 1000 + i64::from(ms))
}

/// Writes `ms` since 1970-01-01 UTC the way Java's `Instant.toString` does, to the millisecond
/// (`2026-10-01T05:12:33.123Z`, or without the fraction when it is zero), so GeoFlux and GeoGIS
/// read each other's trial start.
pub fn format_instant(ms: i64) -> String {
    let date = CivilDate::from_unix_ms(ms);
    let in_day = ms.rem_euclid(MS_PER_DAY);
    let (secs, millis) = (in_day / 1000, in_day % 1000);
    let hms = format!("{:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60);
    if millis == 0 { format!("{date}T{hms}Z") } else { format!("{date}T{hms}.{millis:03}Z") }
}

/// Whole days from `start_ms` to `now_ms`, truncated toward zero like Java's
/// `ChronoUnit.DAYS.between(start, now)`: 23 hours either way is 0 days.
pub fn whole_days_between(start_ms: i64, now_ms: i64) -> i64 {
    now_ms.saturating_sub(start_ms) / MS_PER_DAY
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_parse_as_strictly_as_java() {
        assert_eq!(CivilDate::parse("2027-01-31").map(|d| d.to_string()), Some("2027-01-31".into()));
        assert!(CivilDate::parse("2028-02-29").is_some());
        for bad in [
            "2026-02-29",
            "2026-02-30",
            "2026-13-01",
            "2026-00-10",
            "2026-1-31",
            "2026-01-31 ",
            " 2026-01-31",
            "+026-01-31",
            "2026/01/31",
            "",
        ] {
            assert_eq!(CivilDate::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn day_numbers_round_trip() {
        assert_eq!(CivilDate::parse("1970-01-01").map(CivilDate::to_days), Some(0));
        assert_eq!(CivilDate::parse("2000-03-01").map(CivilDate::to_days), Some(11_017));
        for days in [-719_528, -1, 0, 59, 60, 10_957, 20_742, 2_932_896] {
            assert_eq!(CivilDate::from_days(days).to_days(), days);
        }
        let a = CivilDate::new(2026, 10, 16);
        let b = CivilDate::new(2027, 1, 31);
        assert_eq!(a.zip(b).map(|(a, b)| a.days_until(b)), Some(107));
        assert_eq!(CivilDate::from_days(i64::MAX), CivilDate::MAX);
        assert_eq!(CivilDate::from_days(i64::MIN), CivilDate::MIN);
    }

    #[test]
    fn instants_read_what_java_writes() {
        let base = CivilDate::new(2026, 10, 1).map(|d| d.to_days() * MS_PER_DAY);
        let at = |h: i64, m: i64, s: i64, ms: i64| base.map(|b| b + ((h * 60 + m) * 60 + s) * 1000 + ms);
        assert_eq!(parse_instant("2026-10-01T05:12:33.123456789Z"), at(5, 12, 33, 123));
        assert_eq!(parse_instant("2026-10-01T05:12:33.1Z"), at(5, 12, 33, 100));
        assert_eq!(parse_instant("2026-10-01T05:12:33Z"), at(5, 12, 33, 0));
        assert_eq!(parse_instant("2026-10-01T05:12Z"), at(5, 12, 0, 0));
        for bad in [
            "2026-10-01T05:12:33",
            "2026-10-01 05:12:33Z",
            "2026-10-01T24:00:00Z",
            "2026-10-01T05:12:33.Z",
            "2026-10-01T05:12.5Z",
            "2026-10-01T05:12:33.12a4Z",
            "x",
            "",
        ] {
            assert_eq!(parse_instant(bad), None, "{bad}");
        }
    }

    #[test]
    fn instants_round_trip() {
        for ms in [0, 1, 999, 1_791_609_153_123, 1_791_609_153_000, -1] {
            assert_eq!(parse_instant(&format_instant(ms)), Some(ms), "{ms}");
        }
        assert_eq!(format_instant(1_791_609_153_000), "2026-10-10T05:12:33Z");
    }

    #[test]
    fn whole_days_truncate_toward_zero() {
        let h = 3_600_000;
        assert_eq!(whole_days_between(0, 23 * h), 0);
        assert_eq!(whole_days_between(0, 24 * h), 1);
        assert_eq!(whole_days_between(0, -23 * h), 0);
        assert_eq!(whole_days_between(0, -25 * h), -1);
        assert_eq!(whole_days_between(i64::MIN, i64::MAX), i64::MAX / MS_PER_DAY);
    }
}
