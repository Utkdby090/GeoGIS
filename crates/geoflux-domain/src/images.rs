//! Linking a folder of inspection photos to assets and their image slots.
//!
//! The photos are named by the caller as paths relative to the linked folder (`/` between
//! parts); nothing here touches the file system. A photo belongs to the asset whose label is
//! one of its folders (`76_0/DJI_0001_T.JPG`), or, at the top level, the label its file name
//! starts with (`76_0_thermal.jpg`). Its slot comes from the words of its file name (see
//! [`ImageSlotDef`]). Every photo that can't be placed is reported with the reason.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::schema::ImageSlotDef;
use crate::sequence::natural_cmp;

/// The file types linked as photos.
pub const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png"];
/// Most files looked at in one link.
pub const MAX_FILES: usize = 50_000;
/// Most unplaced photos listed one by one (all are counted).
pub const MAX_UNMATCHED_LISTED: usize = 200;
/// The longest relative path accepted.
const MAX_PATH: usize = 1_024;

/// One slot's photos, in natural order of path; the first is the one shown.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotPhotos {
    pub slot: String,
    pub photos: Vec<String>,
}

/// One asset's photos, by slot (in the schema's slot order, only slots with photos).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetPhotos {
    /// The asset's label, as written in its data.
    pub label: String,
    pub slots: Vec<SlotPhotos>,
}

impl AssetPhotos {
    pub fn slot(&self, id: &str) -> &[String] {
        self.slots.iter().find(|s| s.slot == id).map_or(&[], |s| s.photos.as_slice())
    }
}

/// Why a file was not linked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Unplaced {
    /// Not a photo type GeoFlux links.
    NotAnImage,
    /// Absolute, or climbs out of the folder (`..`), or too long.
    BadPath,
    /// No folder or file-name start matches an asset's label.
    NoAsset,
    /// Its name has no slot's word and the schema has no fallback slot.
    NoSlot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnplacedPhoto {
    pub path: String,
    pub reason: Unplaced,
}

/// What a link found.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkReport {
    /// Assets with at least one photo, in natural order of label.
    pub assets: Vec<AssetPhotos>,
    /// Photos placed in a slot.
    pub linked: usize,
    /// The first [`MAX_UNMATCHED_LISTED`] files not placed.
    pub unplaced: Vec<UnplacedPhoto>,
    pub unplaced_count: usize,
    /// More than [`MAX_FILES`] files were given; the rest were not looked at.
    pub truncated: bool,
}

/// The lowercase words of a file name without its extension: split at anything that isn't a
/// letter or digit.
pub fn words(file_name: &str) -> Vec<String> {
    let stem = file_name.rsplit_once('.').map_or(file_name, |(s, _)| s);
    stem.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

/// Whether a file name has a photo extension.
pub fn is_image(file_name: &str) -> bool {
    file_name.rsplit_once('.').is_some_and(|(_, ext)| IMAGE_EXTENSIONS.iter().any(|e| ext.eq_ignore_ascii_case(e)))
}

/// The slot a photo named `file_name` goes in: the first thermal slot with one of its words,
/// else the first other slot, else the fallback.
pub fn slot_for<'a>(slots: &'a [ImageSlotDef], file_name: &str) -> Option<&'a str> {
    let w = words(file_name);
    // Compared lowercase without allocating per comparison.
    let hit = |s: &&ImageSlotDef| s.matches.iter().any(|m| w.iter().any(|x| m.chars().flat_map(char::to_lowercase).eq(x.chars())));
    slots
        .iter()
        .filter(|s| s.thermal)
        .find(hit)
        .or_else(|| slots.iter().filter(|s| !s.thermal).find(hit))
        .or_else(|| slots.iter().find(|s| s.fallback))
        .map(|s| s.id.as_str())
}

/// Whether a relative path (`/` between parts) stays inside the folder it is relative to.
pub fn is_safe_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_PATH
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains(':')
        && path.split('/').all(|p| !p.is_empty() && p != "." && p != "..")
}

/// Links `files` (paths relative to the folder named `root`) to the assets labelled `labels`.
/// A photo goes to the asset named by its deepest folder, else by the start of its file name,
/// else by `root` itself (a single tower's folder picked on its own).
pub fn link(slots: &[ImageSlotDef], labels: &[&str], root: &str, files: &[String]) -> LinkReport {
    // Label (trimmed, lowercase) → the label as written. The first spelling wins.
    let mut by_key: HashMap<String, &str> = HashMap::new();
    for l in labels {
        let k = l.trim().to_lowercase();
        if !k.is_empty() {
            by_key.entry(k).or_insert(l.trim());
        }
    }
    // Longest labels first, so `76_0` wins over `76` for `76_0_T.jpg`.
    let from_root = by_key.get(&root.trim().to_lowercase()).copied();
    let mut prefixes: Vec<&String> = by_key.keys().collect();
    prefixes.sort_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));

    let mut report = LinkReport { truncated: files.len() > MAX_FILES, ..LinkReport::default() };
    let mut placed: HashMap<&str, HashMap<&str, Vec<String>>> = HashMap::new();
    let unplaced = |path: &str, reason: Unplaced, report: &mut LinkReport| {
        report.unplaced_count += 1;
        if report.unplaced.len() < MAX_UNMATCHED_LISTED {
            report.unplaced.push(UnplacedPhoto { path: path.to_string(), reason });
        }
    };

    for path in files.iter().take(MAX_FILES) {
        if !is_safe_path(path) {
            unplaced(path, Unplaced::BadPath, &mut report);
            continue;
        }
        let (dirs, name) = path.rsplit_once('/').map_or(("", path.as_str()), |(d, n)| (d, n));
        if !is_image(name) {
            unplaced(path, Unplaced::NotAnImage, &mut report);
            continue;
        }
        // The deepest folder named like an asset, else the start of the file name.
        let from_dir = dirs.rsplit('/').find_map(|d| by_key.get(&d.trim().to_lowercase()).copied());
        let lower = name.to_lowercase();
        let from_name = || {
            prefixes
                .iter()
                .find(|p| lower.strip_prefix(p.as_str()).is_some_and(|rest| rest.chars().next().is_none_or(|c| !c.is_alphanumeric())))
        };
        let Some(label) = from_dir.or_else(|| from_name().and_then(|p| by_key.get(p.as_str()).copied())).or(from_root) else {
            unplaced(path, Unplaced::NoAsset, &mut report);
            continue;
        };
        let Some(slot) = slot_for(slots, name) else {
            unplaced(path, Unplaced::NoSlot, &mut report);
            continue;
        };
        placed.entry(label).or_default().entry(slot).or_default().push(path.clone());
        report.linked += 1;
    }

    let mut assets: Vec<AssetPhotos> = placed
        .into_iter()
        .map(|(label, mut by_slot)| AssetPhotos {
            label: label.to_string(),
            slots: slots
                .iter()
                .filter_map(|s| by_slot.remove(s.id.as_str()).map(|p| (s, p)))
                .map(|(s, mut photos)| {
                    photos.sort_by(|a, b| natural_cmp(a, b));
                    SlotPhotos { slot: s.id.clone(), photos }
                })
                .collect(),
        })
        .collect();
    assets.sort_by(|a, b| natural_cmp(&a.label, &b.label));
    report.assets = assets;
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slots() -> Vec<ImageSlotDef> {
        let slot = |id: &str, thermal, words: &[&str], fallback| ImageSlotDef {
            id: id.into(),
            label: id.into(),
            thermal,
            matches: words.iter().map(|w| w.to_string()).collect(),
            fallback,
        };
        vec![
            slot("LOCATION", false, &["location", "v"], true),
            slot("THERMAL", true, &["thermal", "ir", "t"], false),
            slot("STRUCTURE", false, &["tower", "wide", "w"], false),
            slot("RGB_ZOOM", false, &["zoom", "rgb", "z"], false),
        ]
    }

    #[test]
    fn slots_come_from_whole_words() {
        let s = slots();
        assert_eq!(slot_for(&s, "76_0_IR.jpg"), Some("THERMAL"));
        assert_eq!(slot_for(&s, "DJI_20240101_0001_T.JPG"), Some("THERMAL"));
        assert_eq!(slot_for(&s, "DJI_20240101_0001_Z.JPG"), Some("RGB_ZOOM"));
        assert_eq!(slot_for(&s, "DJI_0001_W.JPG"), Some("STRUCTURE"));
        assert_eq!(slot_for(&s, "wire repair.jpg"), Some("LOCATION"), "`ir` inside a word is not thermal");
        assert_eq!(slot_for(&s, "tower thermal.jpg"), Some("THERMAL"), "thermal slots are tried first");
        assert_eq!(slot_for(&s[1..], "photo.jpg"), None, "no fallback");
        assert_eq!(slot_for(&s, "T.jpg"), Some("THERMAL"));
    }

    #[test]
    fn photos_find_their_tower_by_folder_or_name() {
        let files: Vec<String> = [
            "76_0/DJI_0002_T.JPG",
            "76_0/DJI_0001_T.JPG",
            "76_0/notes.txt",
            "line A/77_0/zoom.png",
            "76_0_tower.jpg",
            "76_1_tower.jpg",
            "760_tower.jpg",
            "../secret.jpg",
            "C:/x/76_0.jpg",
            "999/thermal.jpg",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let r = link(&slots(), &["76_0", "77_0", " 76_0 "], "photos", &files);
        assert_eq!(r.linked, 4);
        let labels: Vec<&str> = r.assets.iter().map(|a| a.label.as_str()).collect();
        assert_eq!(labels, ["76_0", "77_0"]);
        let a = &r.assets[0];
        assert_eq!(a.slot("THERMAL"), ["76_0/DJI_0001_T.JPG", "76_0/DJI_0002_T.JPG"], "natural order");
        assert_eq!(a.slot("STRUCTURE"), ["76_0_tower.jpg"]);
        assert_eq!(r.assets[1].slot("RGB_ZOOM"), ["line A/77_0/zoom.png"]);
        let reasons: Vec<(&str, Unplaced)> = r.unplaced.iter().map(|u| (u.path.as_str(), u.reason)).collect();
        assert_eq!(
            reasons,
            [
                ("76_0/notes.txt", Unplaced::NotAnImage),
                ("76_1_tower.jpg", Unplaced::NoAsset),
                ("760_tower.jpg", Unplaced::NoAsset),
                ("../secret.jpg", Unplaced::BadPath),
                ("C:/x/76_0.jpg", Unplaced::BadPath),
                ("999/thermal.jpg", Unplaced::NoAsset),
            ]
        );
        assert_eq!(r.unplaced_count, 6);
    }

    #[test]
    fn a_short_label_is_used_when_no_longer_one_fits() {
        let r = link(&slots(), &["76", "76_0"], "", &["76_9_t.jpg".to_string(), "76.jpg".to_string()]);
        assert_eq!(r.assets.len(), 1);
        assert_eq!(r.assets[0].label, "76");
        assert_eq!(r.linked, 2);
    }

    #[test]
    fn a_single_towers_folder_links_to_that_tower() {
        let files: Vec<String> =
            ["DJI_0001_T.JPG", "DJI_0001_Z.JPG", "77_0_thermal.jpg", "sub/77_0/x_w.jpg"].iter().map(|s| s.to_string()).collect();
        let r = link(&slots(), &["76_0", "77_0"], "76_0", &files);
        assert_eq!(r.linked, 4);
        assert_eq!(r.assets[0].slot("THERMAL"), ["DJI_0001_T.JPG"]);
        assert_eq!(r.assets[0].slot("RGB_ZOOM"), ["DJI_0001_Z.JPG"]);
        assert_eq!(r.assets[1].slot("THERMAL"), ["77_0_thermal.jpg"], "a file's own tower still wins");
        assert_eq!(r.assets[1].slot("STRUCTURE"), ["sub/77_0/x_w.jpg"]);
        assert_eq!(link(&slots(), &["76_0"], "76_2", &files[..1]).unplaced_count, 1, "a folder named for no tower links nothing");
    }

    #[test]
    fn lists_are_bounded() {
        let files: Vec<String> = (0..MAX_FILES + 10).map(|i| format!("x{i}.jpg")).collect();
        let r = link(&slots(), &["1"], "", &files);
        assert!(r.truncated);
        assert_eq!(r.unplaced_count, MAX_FILES);
        assert_eq!(r.unplaced.len(), MAX_UNMATCHED_LISTED);
        assert!(link(&slots(), &[], "", &[]).assets.is_empty());
    }
}
