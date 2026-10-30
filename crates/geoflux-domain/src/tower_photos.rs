//! Sorting a survey day's photos onto the line's towers: each photo goes to the nearest tower,
//! and the photos of one shot (a thermal picture and the visual one taken with it) go together.
//!
//! No distance limit is applied: every photo goes to some tower, and the distance is kept so the
//! person reviewing can see the far ones. The folder names the sorted copies go into are made
//! here too, so `34/1` becomes the `34_1` folder people already use.

use std::collections::HashMap;

/// A tower to sort onto: its position in degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TowerPoint {
    pub lat: f64,
    pub lon: f64,
}

/// A photo to sort: its position in degrees and the shot it belongs to (see
/// [`crate::survey_photos::shot_id`]).
#[derive(Clone, Debug, PartialEq)]
pub struct PhotoPoint {
    pub shot: String,
    pub lat: f64,
    pub lon: f64,
}

/// Where a photo went: the index of its tower, and how far the photo was from it in metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub tower: usize,
    pub metres: f64,
}

/// The visit folder used when none is given: the first visit, `C1`.
pub const DEFAULT_VISIT: &str = "C1";

/// Each photo's tower, in the photos' order (`None` when there are no towers or the photo's
/// position is not a number). The photos of one shot are placed by their mean position, so a
/// pair never splits between two towers.
pub fn assign(towers: &[TowerPoint], photos: &[PhotoPoint]) -> Vec<Option<Placement>> {
    // The mean position of each shot.
    let mut shots: HashMap<&str, (f64, f64, u32)> = HashMap::new();
    for p in photos.iter().filter(|p| usable(p.lat, p.lon)) {
        let e = shots.entry(p.shot.as_str()).or_insert((0.0, 0.0, 0));
        e.0 += p.lat;
        e.1 += p.lon;
        e.2 += 1;
    }
    let tower_of: HashMap<&str, usize> = shots
        .into_iter()
        .filter_map(|(shot, (lat, lon, n))| nearest(towers, lat / f64::from(n), lon / f64::from(n)).map(|t| (shot, t)))
        .collect();
    photos
        .iter()
        .map(|p| {
            let tower = *tower_of.get(p.shot.as_str())?;
            let t = towers.get(tower)?;
            Some(Placement { tower, metres: metres(p.lat, p.lon, t.lat, t.lon) })
        })
        .collect()
}

/// The tower nearest to a position, by distance on the ground.
fn nearest(towers: &[TowerPoint], lat: f64, lon: f64) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for (i, t) in towers.iter().enumerate().filter(|(_, t)| usable(t.lat, t.lon)) {
        let d = metres(lat, lon, t.lat, t.lon);
        if best.is_none_or(|(_, b)| d < b) {
            best = Some((i, d));
        }
    }
    best.map(|(i, _)| i)
}

fn usable(lat: f64, lon: f64) -> bool {
    lat.is_finite() && lon.is_finite()
}

/// The distance between two positions in metres (haversine, on a sphere of the Earth's mean
/// radius: well within a metre over a span).
pub fn metres(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const R: f64 = 6_371_008.8;
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = p2 - p1;
    let dl = (lon2 - lon1).to_radians();
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * R * a.sqrt().min(1.0).asin()
}

/// A tower number or visit as a folder name that works on Windows, macOS and Linux: `/`, `\`
/// and the other characters Windows refuses become `_` (so `34/1` is `34_1`), and full stops
/// and spaces at the end are dropped. Never empty, never `.` or `..`.
pub fn folder_name(label: &str) -> String {
    let cleaned: String = label
        .trim()
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim_end_matches(['.', ' ']).to_string();
    // Names Windows keeps for devices can't be folders.
    let reserved = ["CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "LPT1", "LPT2", "LPT3"];
    if cleaned.is_empty() || cleaned.chars().all(|c| c == '.') {
        "_".to_string()
    } else if reserved.iter().any(|r| r.eq_ignore_ascii_case(&cleaned)) {
        format!("{cleaned}_")
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(lat: f64, lon: f64) -> TowerPoint {
        TowerPoint { lat, lon }
    }

    fn p(shot: &str, lat: f64, lon: f64) -> PhotoPoint {
        PhotoPoint { shot: shot.into(), lat, lon }
    }

    #[test]
    fn each_photo_goes_to_its_nearest_tower() {
        // Two towers about 400 m apart, east-west.
        let towers = [t(27.5, 75.100), t(27.5, 75.104)];
        let photos = [p("a", 27.5001, 75.1001), p("b", 27.4999, 75.1038), p("c", 27.5, 75.1019)];
        let got: Vec<usize> = assign(&towers, &photos).iter().map(|x| x.expect("placed").tower).collect();
        assert_eq!(got, [0, 1, 0]);
        let first = assign(&towers, &photos)[0].expect("placed");
        assert!((first.metres - 14.86).abs() < 0.05, "{}", first.metres);
    }

    #[test]
    fn a_thermal_and_visual_pair_stays_together() {
        let towers = [t(27.5, 75.100), t(27.5, 75.104)];
        // The pair straddles the middle; its mean is nearer tower 0.
        let photos = [p("DJI_0001", 27.5, 75.1021), p("DJI_0001", 27.5, 75.1017)];
        let got: Vec<usize> = assign(&towers, &photos).iter().map(|x| x.expect("placed").tower).collect();
        assert_eq!(got, [0, 0]);
    }

    #[test]
    fn nothing_is_placed_without_towers_or_positions() {
        assert_eq!(assign(&[], &[p("a", 27.5, 75.1)]), [None]);
        assert_eq!(assign(&[t(27.5, 75.1)], &[p("a", f64::NAN, 75.1)]), [None]);
        assert!(assign(&[t(27.5, 75.1)], &[]).is_empty());
    }

    #[test]
    fn metres_matches_a_known_distance() {
        // One degree of latitude is about 111.2 km.
        assert!((metres(27.0, 75.0, 28.0, 75.0) - 111_195.0).abs() < 50.0);
        assert_eq!(metres(27.5, 75.1, 27.5, 75.1), 0.0);
    }

    #[test]
    fn folder_names_are_safe_everywhere() {
        assert_eq!(folder_name("34/1"), "34_1");
        assert_eq!(folder_name("AP-73"), "AP-73");
        assert_eq!(folder_name(" 8/0. "), "8_0");
        assert_eq!(folder_name("a\\b:c*d?e\"f<g>h|i"), "a_b_c_d_e_f_g_h_i");
        assert_eq!(folder_name(""), "_");
        assert_eq!(folder_name(".."), "_");
        assert_eq!(folder_name("con"), "con_");
        assert_eq!(folder_name(DEFAULT_VISIT), "C1");
    }
}
