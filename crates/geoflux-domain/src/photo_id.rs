//! A photo's identity, from its content rather than where it is stored.
//!
//! Findings, the report's chosen photo and (later) markup refer to a photo by its [`PhotoId`], so
//! moving, renaming or relinking a survey folder never orphans them, and the desktop, the server
//! and the web app name the same photo the same way.
//!
//! The id is SHA-256 over a fixed tag, the file's length, its first 64 KiB and its last 64 KiB.
//! A drone photo's first 64 KiB hold its EXIF (capture time to the second, camera serial, GPS)
//! and the start of its image data; its last 64 KiB are compressed image data. Two different
//! photos agreeing on all of that are not a practical concern, and reading 128 KiB per photo
//! instead of the whole file keeps a 740-photo survey day at about 95 MB read instead of ~6 GB.
//! GeoFlux never rewrites a photo (markup is kept beside it), so a sampled id stays valid.
//!
//! The text form is `p1:` and 64 lowercase hex digits; `p1` names this sampling, so a later scheme
//! can sit beside it without old ids changing meaning.

use std::fmt;
use std::ops::Range;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Bytes read from each end of the file.
pub const SAMPLE: u64 = 64 * 1024;
const PREFIX: &str = "p1:";
const TAG: &[u8] = b"geoflux-photo-id-v1\0";

/// A photo's content id (see the module docs).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PhotoId(String);

/// Why text is not a photo id, or the sampled bytes don't fit the file's length.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PhotoIdError {
    #[error("'{0}' isn't a photo id (p1: and 64 hex digits)")]
    BadText(String),
    #[error("the bytes read ({head} + {tail}) don't match a {len}-byte file")]
    BadSample { len: u64, head: usize, tail: usize },
}

impl PhotoId {
    /// The byte range to read for the head sample of a `len`-byte file.
    pub fn head_range(len: u64) -> Range<u64> {
        0..len.min(SAMPLE)
    }

    /// The byte range to read for the tail sample. It never overlaps the head, so a file of
    /// 128 KiB or less is read exactly once, whole.
    pub fn tail_range(len: u64) -> Range<u64> {
        let head_end = len.min(SAMPLE);
        len.saturating_sub(SAMPLE).max(head_end)..len
    }

    /// The id of a `len`-byte file from the bytes of [`PhotoId::head_range`] and
    /// [`PhotoId::tail_range`]. Fails when their sizes don't match those ranges, which means the
    /// file changed or was read short.
    pub fn from_samples(len: u64, head: &[u8], tail: &[u8]) -> Result<Self, PhotoIdError> {
        let fits = |r: Range<u64>, b: &[u8]| r.end - r.start == b.len() as u64;
        if !fits(Self::head_range(len), head) || !fits(Self::tail_range(len), tail) {
            return Err(PhotoIdError::BadSample { len, head: head.len(), tail: tail.len() });
        }
        Ok(Self::digest(len, head, tail))
    }

    fn digest(len: u64, head: &[u8], tail: &[u8]) -> Self {
        let mut h = Sha256::new();
        h.update(TAG);
        h.update(len.to_le_bytes());
        h.update(head);
        h.update(tail);
        let digest = h.finalize();
        let mut text = String::with_capacity(PREFIX.len() + 64);
        text.push_str(PREFIX);
        for b in digest {
            // Writing to a String cannot fail.
            let _ = fmt::Write::write_fmt(&mut text, format_args!("{b:02x}"));
        }
        PhotoId(text)
    }

    /// The id of a photo already in memory (the web app, tests).
    pub fn of_bytes(bytes: &[u8]) -> Self {
        let len = bytes.len() as u64;
        let head = &bytes[..Self::head_range(len).end as usize];
        let tail = &bytes[Self::tail_range(len).start as usize..];
        Self::digest(len, head, tail)
    }

    /// Reads an id's text form.
    pub fn parse(text: &str) -> Result<Self, PhotoIdError> {
        let hex = text.strip_prefix(PREFIX).filter(|h| h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
        match hex {
            Some(_) => Ok(PhotoId(text.to_string())),
            None => Err(PhotoIdError::BadText(text.chars().take(80).collect())),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PhotoId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for PhotoId {
    type Error = PhotoIdError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        PhotoId::parse(&s)
    }
}

impl From<PhotoId> for String {
    fn from(id: PhotoId) -> String {
        id.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(len: usize, seed: u8) -> Vec<u8> {
        (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
    }

    #[test]
    fn ranges_cover_small_files_once_and_large_files_at_both_ends() {
        assert_eq!(PhotoId::head_range(0), 0..0);
        assert_eq!(PhotoId::tail_range(0), 0..0);
        assert_eq!(PhotoId::head_range(1000), 0..1000);
        assert_eq!(PhotoId::tail_range(1000), 1000..1000);
        assert_eq!(PhotoId::head_range(100_000), 0..SAMPLE);
        assert_eq!(PhotoId::tail_range(100_000), SAMPLE..100_000);
        assert_eq!(PhotoId::tail_range(10_000_000), 10_000_000 - SAMPLE..10_000_000);
    }

    #[test]
    fn same_content_same_id_and_any_sampled_change_changes_it() {
        let a = file(300_000, 1);
        let id = PhotoId::of_bytes(&a);
        assert!(id.as_str().starts_with("p1:"));
        assert_eq!(PhotoId::parse(id.as_str()), Ok(id.clone()));
        assert_eq!(PhotoId::of_bytes(&a.clone()), id);
        for at in [0, 70_000 - 1, a.len() - 1] {
            let mut b = a.clone();
            b[at] ^= 1;
            let changed = PhotoId::of_bytes(&b) != id;
            // Bytes 64 KiB .. len-64 KiB are not sampled.
            assert_eq!(changed, at < SAMPLE as usize || at >= a.len() - SAMPLE as usize, "byte {at}");
        }
        let mut longer = a.clone();
        longer.push(0);
        assert_ne!(PhotoId::of_bytes(&longer), id);
    }

    #[test]
    fn samples_must_match_the_length() {
        let a = file(200_000, 2);
        let len = a.len() as u64;
        let head = &a[..SAMPLE as usize];
        let tail = &a[a.len() - SAMPLE as usize..];
        assert_eq!(PhotoId::from_samples(len, head, tail), Ok(PhotoId::of_bytes(&a)));
        assert!(matches!(PhotoId::from_samples(len, head, &tail[1..]), Err(PhotoIdError::BadSample { .. })));
        // A 100,000-byte file's tail is the 34,464 bytes after the head: a wrong length shows.
        let small = file(100_000, 3);
        let (h, t) = small.split_at(SAMPLE as usize);
        assert_eq!(PhotoId::from_samples(100_000, h, t), Ok(PhotoId::of_bytes(&small)));
        assert!(matches!(PhotoId::from_samples(100_001, h, t), Err(PhotoIdError::BadSample { .. })));
    }

    #[test]
    fn text_form_is_checked_when_read() {
        assert!(PhotoId::parse("p1:abc").is_err());
        assert!(PhotoId::parse(&format!("p2:{}", "0".repeat(64))).is_err());
        assert!(PhotoId::parse(&format!("p1:{}", "A".repeat(64))).is_err());
        assert!(serde_json::from_str::<PhotoId>("\"nope\"").is_err());
        let id = PhotoId::of_bytes(b"photo");
        let json = serde_json::to_string(&id).expect("serialises");
        assert_eq!(serde_json::from_str::<PhotoId>(&json).expect("reads back"), id);
    }
}
