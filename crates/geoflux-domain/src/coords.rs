//! Coordinates the way survey sheets and GPS tools write them: `27.5123`, `-73.9`, `27°30'44.3"N`,
//! `N 27 30 44.3`, `27:30.738`, `73° 54.2' E`. Values are not range-checked here, so a swapped pair
//! can still be recognised later.

use crate::asset::GeoPoint;

/// Which way a hemisphere letter points: N/S is a latitude, E/W a longitude.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Latitude,
    Longitude,
}

/// A coordinate read from text: signed decimal degrees, and the axis its hemisphere letter named.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParsedCoordinate {
    pub degrees: f64,
    pub axis: Option<Axis>,
}

/// Longer text is not a coordinate; this bounds the work done on hostile cells.
const MAX_LEN: usize = 64;

fn is_separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '°' | 'º' | '˚' | '\'' | '′' | '’' | '"' | '″' | '”' | ':')
}

/// A number part: digits, optionally a `.` or `,` and more digits.
fn number_part(s: &str) -> Option<f64> {
    let (int, frac) = match s.find(['.', ',']) {
        Some(i) => (s.get(..i)?, Some(s.get(i + 1..)?)),
        None => (s, None),
    };
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    if !digits(int) || frac.is_some_and(|f| !digits(f)) {
        return None;
    }
    let text = match frac {
        Some(f) => format!("{int}.{f}"),
        None => int.to_string(),
    };
    text.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// None when `text` is blank or not a coordinate.
pub fn parse(text: &str) -> Option<ParsedCoordinate> {
    let s = text.trim();
    if s.is_empty() || s.chars().count() > MAX_LEN {
        return None;
    }
    // Plain decimal degrees; Rust also reads "inf" and "1e3", so keep finite values only.
    if let Ok(v) = s.parse::<f64>() {
        return v.is_finite().then_some(ParsedCoordinate { degrees: v, axis: None });
    }

    // One hemisphere letter, at the start or at the end.
    let hemisphere = |c: char| matches!(c.to_ascii_uppercase(), 'N' | 'S' | 'E' | 'W');
    let first = s.chars().next()?;
    let last = s.chars().next_back()?;
    let (letter, body) = if hemisphere(first) {
        (Some(first.to_ascii_uppercase()), s.get(first.len_utf8()..)?)
    } else if hemisphere(last) {
        (Some(last.to_ascii_uppercase()), s.get(..s.len() - last.len_utf8())?)
    } else {
        (None, s)
    };

    let body = body.trim();
    let (minus, body) = match body.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, body),
    };
    let parts: Vec<&str> = body.split(is_separator).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let values: Vec<f64> = parts.iter().map(|p| number_part(p)).collect::<Option<_>>()?;
    let deg = values.first().copied()?;
    let min = values.get(1).copied().unwrap_or(0.0);
    let sec = values.get(2).copied().unwrap_or(0.0);
    if min >= 60.0 || sec >= 60.0 {
        return None;
    }
    // Fractions belong on the last part only (27.5° 30' is ambiguous).
    if values.iter().take(values.len().saturating_sub(1)).any(|v| v.fract() != 0.0) {
        return None;
    }
    let south_or_west = matches!(letter, Some('S' | 'W'));
    if minus && south_or_west {
        return None;
    }
    let magnitude = deg + min / 60.0 + sec / 3600.0;
    let axis = match letter {
        Some('N' | 'S') => Some(Axis::Latitude),
        Some('E' | 'W') => Some(Axis::Longitude),
        _ => None,
    };
    Some(ParsedCoordinate { degrees: if minus || south_or_west { -magnitude } else { magnitude }, axis })
}

/// A latitude/longitude pair from two columns. When the hemisphere letters show the columns were
/// exchanged (E/W in the latitude column, or N/S in the longitude one), the pair is put the right
/// way round. None when either value is missing or unreadable.
pub fn parse_pair(lat_text: &str, lon_text: &str) -> Option<GeoPoint> {
    let lat = parse(lat_text)?;
    let lon = parse(lon_text)?;
    let swapped = (lat.axis == Some(Axis::Longitude) && lon.axis != Some(Axis::Longitude))
        || (lon.axis == Some(Axis::Latitude) && lat.axis != Some(Axis::Latitude));
    Some(if swapped { GeoPoint { lat: lon.degrees, lon: lat.degrees } } else { GeoPoint { lat: lat.degrees, lon: lon.degrees } })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deg(s: &str) -> Option<f64> {
        parse(s).map(|p| p.degrees)
    }

    fn close(a: Option<f64>, b: f64) -> bool {
        a.is_some_and(|a| (a - b).abs() < 1e-9)
    }

    #[test]
    fn decimal_degrees() {
        assert!(close(deg("27.5123"), 27.5123));
        assert!(close(deg("-73.9"), -73.9));
        assert!(close(deg(" 12 "), 12.0));
        assert_eq!(parse("27.5").and_then(|p| p.axis), None);
    }

    #[test]
    fn degrees_minutes_seconds_in_common_spellings() {
        let v = 27.0 + 30.0 / 60.0 + 44.3 / 3600.0;
        assert!(close(deg("27°30'44.3\"N"), v));
        assert!(close(deg("N 27 30 44.3"), v));
        assert!(close(deg("27° 30′ 44,3″ N"), v));
        assert!(close(deg("27:30.738"), 27.0 + 30.738 / 60.0));
        assert!(close(deg("73° 54.2' W"), -(73.0 + 54.2 / 60.0)));
        assert!(close(deg("27,5"), 27.5));
        assert_eq!(parse("73° 54.2' E").and_then(|p| p.axis), Some(Axis::Longitude));
    }

    #[test]
    fn rejects_what_is_not_a_coordinate() {
        for bad in ["", "   ", "abc", "27°61'", "27 30 70", "27.5 30", "-27 S", "1 2 3 4", "inf", "NaN", "N", "27..5", "1e400"] {
            assert_eq!(parse(bad), None, "{bad}");
        }
        assert_eq!(parse(&"1".repeat(200)), None);
    }

    #[test]
    fn pair_is_swapped_back_when_letters_say_so() {
        let p = parse_pair("75°10'E", "27°30'N").expect("pair");
        assert!((p.lat - 27.5).abs() < 1e-9 && (p.lon - (75.0 + 10.0 / 60.0)).abs() < 1e-9);
        let q = parse_pair("27.5", "75.1").expect("pair");
        assert_eq!(q, GeoPoint { lat: 27.5, lon: 75.1 });
        assert_eq!(parse_pair("27.5", ""), None);
    }

    #[test]
    fn hostile_text_never_panics() {
        for s in ["°", "-", "--1", "N-", "É", "1°É", "\u{0}", "١٢", "N 😀", "1,", ",1", "-N"] {
            let _ = parse(s);
            let _ = parse_pair(s, s);
        }
    }
}
