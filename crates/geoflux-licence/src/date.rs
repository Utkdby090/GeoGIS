//! Calendar dates without a date library, for showing when a pass runs out ("valid until
//! 2026-11-15").

use std::fmt;

pub(crate) const SECS_PER_DAY: i64 = 86_400;

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

    /// Reads `YYYY-MM-DD` strictly: four-digit year, two-digit month and day, a real date,
    /// nothing around it.
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

    /// The UTC date at `secs` seconds since 1970-01-01 UTC.
    pub fn from_unix_secs(secs: i64) -> Self {
        CivilDate::from_days(secs.div_euclid(SECS_PER_DAY))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_parse_strictly() {
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
}
