//! Typed ids, so an organisation id can't be passed where a person's is expected.
//!
//! An id is a prefix and 32 lowercase hex digits from 16 random bytes (`org_3f2a…`). The prefix
//! makes ids readable in logs and support emails; the random part makes them unguessable, so an id
//! in a URL reveals nothing about how many organisations or surveys exist.

use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::Digest;
use thiserror::Error;

/// Why a text is not an id of the expected kind.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[error("not a valid {kind} id")]
pub struct IdError {
    pub kind: &'static str,
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from(DIGITS[usize::from(b >> 4)]));
        s.push(char::from(DIGITS[usize::from(b & 0x0f)]));
    }
    s
}

fn is_lower_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

macro_rules! id {
    ($(#[$doc:meta])* $name:ident, $prefix:literal, $kind:literal) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub const PREFIX: &'static str = $prefix;

            /// A new id from 16 bytes of [`crate::ports::Random`].
            pub fn from_random(bytes: [u8; 16]) -> Self {
                $name(format!("{}_{}", $prefix, hex(&bytes)))
            }

            pub fn parse(text: &str) -> Result<Self, IdError> {
                match text.strip_prefix(concat!($prefix, "_")) {
                    Some(rest) if is_lower_hex(rest, 32) => Ok($name(text.to_string())),
                    _ => Err(IdError { kind: $kind }),
                }
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdError;
            fn try_from(text: String) -> Result<Self, IdError> {
                $name::parse(&text)
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> String {
                id.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

id!(
    /// An organisation: a utility, a survey company, or one person on the Pro plan.
    OrgId,
    "org",
    "organisation"
);
id!(
    /// A person's account. Also the `account` in their desktop pass.
    UserId,
    "usr",
    "user"
);
id!(
    /// One signed-in computer (a seat).
    DeviceId,
    "dev",
    "device"
);
id!(
    /// One uploaded survey revision.
    SurveyId,
    "srv",
    "survey"
);

/// A photo's SHA-256, as 64 lowercase hex digits. Photos are stored and deduplicated by it, so a
/// photo sent twice (a retried upload, the next survey revision) is stored once per organisation.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Sha256(String);

impl Sha256 {
    pub fn of(bytes: &[u8]) -> Self {
        Sha256(hex(&sha2::Sha256::digest(bytes)))
    }

    /// The hash of a digest computed elsewhere (a streamed upload).
    pub fn from_digest(digest: [u8; 32]) -> Self {
        Sha256(hex(&digest))
    }

    pub fn parse(text: &str) -> Result<Self, IdError> {
        if is_lower_hex(text, 64) { Ok(Sha256(text.to_string())) } else { Err(IdError { kind: "SHA-256" }) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Sha256 {
    type Error = IdError;
    fn try_from(text: String) -> Result<Self, IdError> {
        Sha256::parse(&text)
    }
}

impl From<Sha256> for String {
    fn from(h: Sha256) -> String {
        h.0
    }
}

impl fmt::Display for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_refuse_other_kinds() {
        let org = OrgId::from_random([0xab; 16]);
        assert_eq!(org.as_str(), format!("org_{}", "ab".repeat(16)));
        assert_eq!(OrgId::parse(org.as_str()), Ok(org.clone()));
        let json = serde_json::to_string(&org).unwrap();
        assert_eq!(serde_json::from_str::<OrgId>(&json).unwrap(), org);
        // Another kind's prefix, upper case, short, or with a path in it.
        for bad in [
            format!("usr_{}", "ab".repeat(16)),
            format!("org_{}", "AB".repeat(16)),
            "org_abc".to_string(),
            format!("org_{}/../x", "ab".repeat(15)),
            String::new(),
        ] {
            assert!(OrgId::parse(&bad).is_err(), "{bad}");
            assert!(serde_json::from_str::<OrgId>(&serde_json::to_string(&bad).unwrap()).is_err());
        }
    }

    #[test]
    fn photo_hashes() {
        let h = Sha256::of(b"abc");
        assert_eq!(h.as_str(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(Sha256::parse(h.as_str()), Ok(h));
        assert!(Sha256::parse("BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD").is_err());
        assert!(Sha256::parse("ba78").is_err());
    }
}
