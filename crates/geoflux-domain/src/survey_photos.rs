//! Raw drone photos from a survey day, as positions to sort onto towers.
//!
//! A day's folder holds every photo the drone took (hundreds, mixed thermal and visual), each
//! with its GPS position in its EXIF. Reading the files is the caller's job; this names what a
//! photo is from its file name, so the rules stay pure and testable.

/// What a photo is, from the suffix DJI's cameras put on its file name: `DJI_…_0001_T.JPG` is
/// the thermal picture and `DJI_…_0001_V.JPG` the visual one taken with it (`_W` wide and `_Z`
/// zoom on the dual-lens cameras). Other names are [`PhotoKind::Other`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PhotoKind {
    Thermal,
    Visual,
    Wide,
    Zoom,
    Other,
}

impl PhotoKind {
    pub const ALL: [PhotoKind; 5] = [PhotoKind::Thermal, PhotoKind::Visual, PhotoKind::Wide, PhotoKind::Zoom, PhotoKind::Other];

    /// The kind of the photo named `file_name` (a name or a path).
    pub fn of(file_name: &str) -> PhotoKind {
        let stem = stem(file_name);
        let Some((_, suffix)) = stem.rsplit_once('_') else { return PhotoKind::Other };
        match suffix.to_ascii_uppercase().as_str() {
            "T" => PhotoKind::Thermal,
            "V" => PhotoKind::Visual,
            "W" => PhotoKind::Wide,
            "Z" => PhotoKind::Zoom,
            _ => PhotoKind::Other,
        }
    }

    /// The name kept in a layer's `kind` column.
    pub fn as_str(self) -> &'static str {
        match self {
            PhotoKind::Thermal => "thermal",
            PhotoKind::Visual => "visual",
            PhotoKind::Wide => "wide",
            PhotoKind::Zoom => "zoom",
            PhotoKind::Other => "other",
        }
    }

    /// The kind named by [`PhotoKind::as_str`]; anything else is [`PhotoKind::Other`].
    pub fn from_name(name: &str) -> PhotoKind {
        PhotoKind::ALL.into_iter().find(|k| k.as_str().eq_ignore_ascii_case(name.trim())).unwrap_or(PhotoKind::Other)
    }
}

/// The shot a photo belongs to: its file name without the folder, the extension and the kind
/// suffix, so `DJI_20260601_0001_T.JPG` and `DJI_20260601_0001_V.JPG` are one shot. A name with no
/// kind suffix is its own shot.
pub fn shot_id(file_name: &str) -> &str {
    let stem = stem(file_name);
    match PhotoKind::of(file_name) {
        PhotoKind::Other => stem,
        _ => stem.rsplit_once('_').map_or(stem, |(shot, _)| shot),
    }
}

/// The file name without its folder or extension.
fn stem(file_name: &str) -> &str {
    let name = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name);
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dji_suffixes_name_the_kind() {
        assert_eq!(PhotoKind::of("DJI_20260601101500_0001_T.JPG"), PhotoKind::Thermal);
        assert_eq!(PhotoKind::of(r"RAW DATA\01-06-2026\DJI_20260601101500_0001_v.jpg"), PhotoKind::Visual);
        assert_eq!(PhotoKind::of("DJI_0001_W.JPG"), PhotoKind::Wide);
        assert_eq!(PhotoKind::of("a/b/DJI_0001_Z.jpeg"), PhotoKind::Zoom);
        assert_eq!(PhotoKind::of("IMG_1234.JPG"), PhotoKind::Other);
        assert_eq!(PhotoKind::of("tower.jpg"), PhotoKind::Other);
        assert_eq!(PhotoKind::of("DJI_0001_T"), PhotoKind::Thermal, "no extension");
    }

    #[test]
    fn a_thermal_and_visual_pair_is_one_shot() {
        assert_eq!(shot_id("x/DJI_20260601101500_0001_T.JPG"), "DJI_20260601101500_0001");
        assert_eq!(shot_id("DJI_20260601101500_0001_V.JPG"), "DJI_20260601101500_0001");
        assert_eq!(shot_id("IMG_1234.JPG"), "IMG_1234");
    }

    #[test]
    fn kind_names_round_trip() {
        for k in PhotoKind::ALL {
            assert_eq!(PhotoKind::from_name(k.as_str()), k);
        }
        assert_eq!(PhotoKind::from_name("nonsense"), PhotoKind::Other);
    }
}
