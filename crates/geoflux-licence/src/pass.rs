//! The offline pass: what the account server signs so the desktop app can check a subscription
//! without the internet.
//!
//! A pass is `gfp1.<base64url(JSON)>.<base64url(Ed25519 signature)>`, the signature covering the
//! text before the last dot (so the JSON is checked exactly as signed, never re-encoded). The JSON
//! names the account, the plan, the features it unlocks, the computer it is for, and when it was
//! issued and runs out. A sign-in pass lasts [`ONLINE_PASS_DAYS`] and is renewed silently whenever
//! the app is online; an offline pass file, for sites with no internet, can last longer.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

use crate::date::SECS_PER_DAY;
use crate::{LicenceError, MachineId};

/// The format tag at the front of every pass.
pub const PASS_PREFIX: &str = "gfp1";
/// The only payload version this build reads.
pub const PASS_VERSION: u32 = 1;
/// How long the app keeps working without reaching the server after a sign-in or renewal.
pub const ONLINE_PASS_DAYS: i64 = 30;
/// The longest an offline pass file may last.
pub const MAX_OFFLINE_PASS_DAYS: i64 = 400;
/// Longer than any pass the server issues.
pub const MAX_PASS_LEN: usize = 8192;
/// How far the clock may read before the issue time (time zones, a slow clock) before it counts as
/// moved back.
pub const CLOCK_SLACK_SECS: i64 = SECS_PER_DAY;

/// The account server's signing keys, by key id, as base64 Ed25519 public keys. Empty until the
/// server is live (Phase 7a); a key is added before the server signs with it, and an old one stays
/// until every pass it signed has run out.
pub const PRODUCTION_KEYS: &[(&str, &str)] = &[];

/// A paid capability. Free use (the GIS, opening inspection layers, the Inspection pane
/// read-only) needs none.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Feature {
    /// Changing inspection forms, photo links and severity.
    EditForms,
    /// Thermal View, measurements, Record Finding and Scan Line.
    Thermal,
    /// The photo markup editor.
    Markup,
    /// One tower's PDF report.
    Reports,
    /// A ZIP of reports for a whole line.
    BulkReports,
    /// Project › Import GeoFlux Project.
    GeoxImport,
    /// Sending a project to the web app.
    CloudSync,
    /// A feature a newer server knows and this build doesn't; ignored.
    #[serde(other)]
    Unknown,
}

/// The subscription plan, for display; [`Feature`]s decide what is unlocked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Plan {
    Free,
    Pro,
    Organisation,
    #[serde(other)]
    Unknown,
}

/// How the pass reached the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PassKind {
    /// From signing in; renewed when online.
    Online,
    /// A file downloaded from the account page for a computer with no internet.
    Offline,
}

#[derive(Deserialize)]
struct Payload {
    v: u32,
    kid: String,
    account: String,
    #[serde(default)]
    org: Option<String>,
    #[serde(default)]
    name: String,
    plan: Plan,
    #[serde(default)]
    features: Vec<Feature>,
    machine: String,
    kind: PassKind,
    issued: i64,
    expires: i64,
}

/// A pass whose signature and contents checked out. Whether it fits this computer and today is
/// decided by [`Pass::status`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pass {
    pub account: String,
    pub org: Option<String>,
    /// Who it belongs to, as the account page shows them (name, company).
    pub name: String,
    pub plan: Plan,
    /// Known features only, without repeats.
    pub features: Vec<Feature>,
    pub machine_id: String,
    pub kind: PassKind,
    /// Seconds since 1970-01-01 UTC.
    pub issued: i64,
    pub expires: i64,
}

/// Checks Ed25519 signatures by key id. The desktop app uses [`Ed25519Keys`]; tests pass their own.
pub trait PassVerifier: Send + Sync {
    fn verify(&self, kid: &str, message: &[u8], signature: &[u8]) -> bool;
}

/// Signs passes: the account server's side of [`PassVerifier`]. The server uses [`Ed25519Signer`]
/// with a key from its secret store; the key never leaves the server.
pub trait PassSigner: Send + Sync {
    /// The id the desktop app looks the public key up by ([`PRODUCTION_KEYS`]).
    fn kid(&self) -> &str;
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, LicenceError>;
}

/// What the account server puts in a pass. [`PassClaims::sign`] refuses anything
/// [`Pass::verify`] would not read back as written, so the server can't issue a pass the app
/// rejects or reads differently.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassClaims {
    pub account: String,
    pub org: Option<String>,
    pub name: String,
    pub plan: Plan,
    pub features: Vec<Feature>,
    /// [`MachineId::as_str`] of the computer it is for.
    pub machine: String,
    pub kind: PassKind,
    /// Seconds since 1970-01-01 UTC.
    pub issued: i64,
    pub expires: i64,
}

/// The JSON as signed, borrowing the claims (field order is the wire order).
#[derive(Serialize)]
struct PayloadRef<'a> {
    v: u32,
    kid: &'a str,
    account: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    org: Option<&'a str>,
    name: &'a str,
    plan: Plan,
    features: &'a [Feature],
    machine: &'a str,
    kind: PassKind,
    issued: i64,
    expires: i64,
}

impl PassClaims {
    /// The pass text, `gfp1.<payload>.<signature>`.
    pub fn sign(&self, signer: &dyn PassSigner) -> Result<String, LicenceError> {
        let unsignable = |why| Err(LicenceError::Unsignable(why));
        if signer.kid().is_empty() || self.account.is_empty() || self.machine.is_empty() {
            return unsignable("a key id, account and machine are required");
        }
        if self.plan == Plan::Unknown || self.features.contains(&Feature::Unknown) {
            return unsignable("unknown plan or feature");
        }
        if self.features.iter().enumerate().any(|(i, f)| self.features[..i].contains(f)) {
            return unsignable("a feature is listed twice");
        }
        let longest = match self.kind {
            PassKind::Online => ONLINE_PASS_DAYS,
            PassKind::Offline => MAX_OFFLINE_PASS_DAYS,
        };
        // No clock slack here: the slack is for the reader's clock, not the issuer's.
        if self.expires <= self.issued || self.expires.saturating_sub(self.issued) > longest * SECS_PER_DAY {
            return unsignable("dates out of range");
        }
        let payload = PayloadRef {
            v: PASS_VERSION,
            kid: signer.kid(),
            account: &self.account,
            org: self.org.as_deref(),
            name: &self.name,
            plan: self.plan,
            features: &self.features,
            machine: &self.machine,
            kind: self.kind,
            issued: self.issued,
            expires: self.expires,
        };
        let json = serde_json::to_vec(&payload).map_err(|_| LicenceError::Unsignable("not serialisable"))?;
        let signed = format!("{PASS_PREFIX}.{}", URL_SAFE_NO_PAD.encode(json));
        let signature = signer.sign(signed.as_bytes())?;
        let text = format!("{signed}.{}", URL_SAFE_NO_PAD.encode(signature));
        if text.len() > MAX_PASS_LEN {
            return Err(LicenceError::TooLong { max: MAX_PASS_LEN });
        }
        Ok(text)
    }
}

/// An Ed25519 private key that signs passes, through `ring`. Server only.
#[cfg(not(target_arch = "wasm32"))]
pub struct Ed25519Signer {
    kid: String,
    pair: ring::signature::Ed25519KeyPair,
}

#[cfg(not(target_arch = "wasm32"))]
impl std::fmt::Debug for Ed25519Signer {
    // Never prints the key.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ed25519Signer").field("kid", &self.kid).finish_non_exhaustive()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Ed25519Signer {
    /// A key from its PKCS#8 document (as `openssl genpkey -algorithm ed25519` or
    /// [`Ed25519Signer::generate_pkcs8`] write it, DER-encoded).
    pub fn from_pkcs8(kid: &str, der: &[u8]) -> Result<Self, LicenceError> {
        let pair = ring::signature::Ed25519KeyPair::from_pkcs8_maybe_unchecked(der)
            .map_err(|_| LicenceError::BadPrivateKey("not an Ed25519 PKCS#8 key"))?;
        Ok(Ed25519Signer { kid: kid.to_string(), pair })
    }

    /// A new private key, for the server's key-rotation tool.
    pub fn generate_pkcs8() -> Result<Vec<u8>, LicenceError> {
        let doc = ring::signature::Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
            .map_err(|_| LicenceError::BadPrivateKey("the system's random source failed"))?;
        Ok(doc.as_ref().to_vec())
    }

    /// The public half, as [`PRODUCTION_KEYS`] lists it.
    pub fn public_key_base64(&self) -> String {
        use ring::signature::KeyPair;
        base64::engine::general_purpose::STANDARD.encode(self.pair.public_key().as_ref())
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl PassSigner for Ed25519Signer {
    fn kid(&self) -> &str {
        &self.kid
    }

    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, LicenceError> {
        Ok(self.pair.sign(message).as_ref().to_vec())
    }
}

/// Ed25519 public keys by id, checked through `ring`.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug, Default)]
pub struct Ed25519Keys {
    keys: Vec<(String, [u8; 32])>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Ed25519Keys {
    /// The account server's keys ([`PRODUCTION_KEYS`]).
    pub fn production() -> Result<Self, LicenceError> {
        Ed25519Keys::from_base64(PRODUCTION_KEYS)
    }

    /// Keys as `(id, base64 of the 32-byte public key)`.
    pub fn from_base64(keys: &[(&str, &str)]) -> Result<Self, LicenceError> {
        let keys = keys
            .iter()
            .map(|(kid, b64)| {
                let bytes =
                    base64::engine::general_purpose::STANDARD.decode(b64.trim()).map_err(|_| LicenceError::BadPublicKey("not base64"))?;
                let key: [u8; 32] = bytes.try_into().map_err(|_| LicenceError::BadPublicKey("not 32 bytes"))?;
                Ok((kid.to_string(), key))
            })
            .collect::<Result<Vec<_>, LicenceError>>()?;
        Ok(Ed25519Keys { keys })
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl PassVerifier for Ed25519Keys {
    fn verify(&self, kid: &str, message: &[u8], signature: &[u8]) -> bool {
        use ring::signature::{ED25519, UnparsedPublicKey};
        self.keys.iter().any(|(id, key)| id == kid && UnparsedPublicKey::new(&ED25519, key).verify(message, signature).is_ok())
    }
}

impl Pass {
    /// Checks the format, the signature, the version and the dates' order. Nothing about this
    /// computer or today yet.
    pub fn verify(text: &str, verifier: &dyn PassVerifier) -> Result<Pass, LicenceError> {
        let text = text.trim();
        if text.len() > MAX_PASS_LEN {
            return Err(LicenceError::TooLong { max: MAX_PASS_LEN });
        }
        let (signed, signature) = text.rsplit_once('.').ok_or(LicenceError::Malformed("not three parts"))?;
        let (prefix, payload) = signed.split_once('.').ok_or(LicenceError::Malformed("not three parts"))?;
        if prefix != PASS_PREFIX {
            return Err(LicenceError::Malformed("not a GeoGIS pass"));
        }
        let json = URL_SAFE_NO_PAD.decode(payload).map_err(|_| LicenceError::Malformed("payload is not base64"))?;
        let signature = URL_SAFE_NO_PAD.decode(signature).map_err(|_| LicenceError::Malformed("signature is not base64"))?;
        let p: Payload = serde_json::from_slice(&json).map_err(|_| LicenceError::Malformed("payload is not a pass"))?;
        // The key id is read before the signature is checked only to pick the key; nothing else in
        // the payload is trusted until the signature matches.
        if !verifier.verify(&p.kid, signed.as_bytes(), &signature) {
            return Err(LicenceError::BadSignature);
        }
        if p.v != PASS_VERSION {
            return Err(LicenceError::Malformed("a newer pass version; update GeoGIS"));
        }
        let longest = match p.kind {
            PassKind::Online => ONLINE_PASS_DAYS,
            PassKind::Offline => MAX_OFFLINE_PASS_DAYS,
        };
        if p.expires <= p.issued || p.expires - p.issued > longest * SECS_PER_DAY + CLOCK_SLACK_SECS {
            return Err(LicenceError::Malformed("dates out of range"));
        }
        let mut features: Vec<Feature> = Vec::with_capacity(p.features.len());
        for f in p.features {
            if f != Feature::Unknown && !features.contains(&f) {
                features.push(f);
            }
        }
        Ok(Pass {
            account: p.account,
            org: p.org,
            name: p.name,
            plan: p.plan,
            features,
            machine_id: p.machine,
            kind: p.kind,
            issued: p.issued,
            expires: p.expires,
        })
    }

    /// What this pass means on `machine` at `now` (seconds since 1970-01-01 UTC). `last_seen` is
    /// the latest time the app has run with this pass, so moving the clock back to stretch a pass
    /// is caught even after it was issued.
    pub fn status(&self, machine: &MachineId, now: i64, last_seen: Option<i64>) -> PassStatus {
        if self.machine_id != machine.as_str() {
            return PassStatus::WrongMachine;
        }
        let latest = last_seen.map_or(self.issued, |seen| seen.max(self.issued));
        if now < latest.saturating_sub(CLOCK_SLACK_SECS) {
            return PassStatus::ClockBack;
        }
        if now >= self.expires {
            return PassStatus::Expired;
        }
        // Rounded up: 1 on the last day, 30 just after a renewal.
        PassStatus::Valid { days_left: (self.expires - now + SECS_PER_DAY - 1) / SECS_PER_DAY }
    }

    pub fn allows(&self, feature: Feature) -> bool {
        self.features.contains(&feature)
    }
}

/// What a verified pass means here and now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassStatus {
    Valid {
        days_left: i64,
    },
    /// Not renewed in time: the app needs to reach the server (or a new offline file).
    Expired,
    WrongMachine,
    /// The clock reads more than a day before the pass was issued or last used.
    ClockBack,
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::expect_used)]
pub(crate) mod tests {
    use super::*;

    /// A throwaway test key; its private half signed the passes below and was then deleted.
    const TEST_KEY: &str = include_str!("../testdata/test-ed25519-public.b64");
    const ISSUED: i64 = 1_792_108_800; // 2026-10-16T00:00:00Z
    const DAY: i64 = SECS_PER_DAY;

    pub(crate) const PRO_30_DAYS: &str = "gfp1.eyJ2IjoxLCJraWQiOiJ0ZXN0LTEiLCJhY2NvdW50IjoiYWNjXzEyMyIsIm9yZyI6Im9yZ185IiwibmFtZSI6IkFzaGEgVmVybWEiLCJwbGFuIjoicHJvIiwiZmVhdHVyZXMiOlsiZWRpdEZvcm1zIiwidGhlcm1hbCIsIm1hcmt1cCIsInJlcG9ydHMiLCJidWxrUmVwb3J0cyIsImdlb3hJbXBvcnQiLCJjbG91ZFN5bmMiLCJmdXR1cmVUaGluZyJdLCJtYWNoaW5lIjoiVEVTVC1NQUNISU5FLUlEIiwia2luZCI6Im9ubGluZSIsImlzc3VlZCI6MTc5MjEwODgwMCwiZXhwaXJlcyI6MTc5NDcwMDgwMH0.LBzaSFuJb8CP_G3rLaTC_0wYzt52r9Vq_8rSJF_PrK3L_txsJpu0vqG1cVgopuV3qnxkE-5LGG8FS7C0_qtfBA";
    const OTHER_MACHINE: &str = "gfp1.eyJ2IjoxLCJraWQiOiJ0ZXN0LTEiLCJhY2NvdW50IjoiYWNjXzEyMyIsIm9yZyI6Im9yZ185IiwibmFtZSI6IkFzaGEgVmVybWEiLCJwbGFuIjoicHJvIiwiZmVhdHVyZXMiOlsiZWRpdEZvcm1zIiwidGhlcm1hbCIsIm1hcmt1cCIsInJlcG9ydHMiLCJidWxrUmVwb3J0cyIsImdlb3hJbXBvcnQiLCJjbG91ZFN5bmMiLCJmdXR1cmVUaGluZyJdLCJtYWNoaW5lIjoiT1RIRVItTUFDSElORSIsImtpbmQiOiJvbmxpbmUiLCJpc3N1ZWQiOjE3OTIxMDg4MDAsImV4cGlyZXMiOjE3OTQ3MDA4MDB9.lVXvv8P-KWfbVOt3YZCTNuhmZQRF3jEhFfhUsPPZvHqWcxZg8fwxlZ7n63BtGtML58yO2wC9-YBh9u67DR64DA";
    pub(crate) const OFFLINE_FILE: &str = "gfp1.eyJ2IjoxLCJraWQiOiJ0ZXN0LTEiLCJhY2NvdW50IjoiYWNjXzEyMyIsIm9yZyI6Im9yZ185IiwibmFtZSI6IkFzaGEgVmVybWEiLCJwbGFuIjoib3JnYW5pc2F0aW9uIiwiZmVhdHVyZXMiOlsiZWRpdEZvcm1zIiwidGhlcm1hbCIsIm1hcmt1cCIsInJlcG9ydHMiLCJidWxrUmVwb3J0cyIsImdlb3hJbXBvcnQiLCJjbG91ZFN5bmMiLCJmdXR1cmVUaGluZyJdLCJtYWNoaW5lIjoiVEVTVC1NQUNISU5FLUlEIiwia2luZCI6Im9mZmxpbmUiLCJpc3N1ZWQiOjE3OTIxMDg4MDAsImV4cGlyZXMiOjE4MjM2NDQ4MDB9.i_4m0ZdOtVp_2j3bKsnAjWwLUGPZ3ZfK9BNMBIYOWJocsStm75I8eLwujZONWypzj7Jt_6-Fz_47XgHQwTL8Dw";
    const FREE_PLAN: &str = "gfp1.eyJ2IjoxLCJraWQiOiJ0ZXN0LTEiLCJhY2NvdW50IjoiYWNjXzEyMyIsIm9yZyI6Im9yZ185IiwibmFtZSI6IkFzaGEgVmVybWEiLCJwbGFuIjoiZnJlZSIsImZlYXR1cmVzIjpbXSwibWFjaGluZSI6IlRFU1QtTUFDSElORS1JRCIsImtpbmQiOiJvbmxpbmUiLCJpc3N1ZWQiOjE3OTIxMDg4MDAsImV4cGlyZXMiOjE3OTQ3MDA4MDB9.-gO7W8K3slqDNenl54dtByJrodJ263OmK3rYL6WC64iEjXP2nfeeYzOfZ6_G_ooSXZtXnhm3zTblti3FH3jVDg";
    const WRONG_VERSION: &str = "gfp1.eyJ2IjoyLCJraWQiOiJ0ZXN0LTEiLCJhY2NvdW50IjoiYWNjXzEyMyIsIm9yZyI6Im9yZ185IiwibmFtZSI6IkFzaGEgVmVybWEiLCJwbGFuIjoicHJvIiwiZmVhdHVyZXMiOlsiZWRpdEZvcm1zIiwidGhlcm1hbCIsIm1hcmt1cCIsInJlcG9ydHMiLCJidWxrUmVwb3J0cyIsImdlb3hJbXBvcnQiLCJjbG91ZFN5bmMiLCJmdXR1cmVUaGluZyJdLCJtYWNoaW5lIjoiVEVTVC1NQUNISU5FLUlEIiwia2luZCI6Im9ubGluZSIsImlzc3VlZCI6MTc5MjEwODgwMCwiZXhwaXJlcyI6MTc5NDcwMDgwMH0.ic5UXhVPmlTuIpmlzEnvnwHMZGj6O6yrEpdRbZ38JKZZ_apIpv3m9WNxacN4IQzdQq8g3Dbkm93krhBo_gy2BQ";
    const UNKNOWN_KID: &str = "gfp1.eyJ2IjoxLCJraWQiOiJub3BlIiwiYWNjb3VudCI6ImFjY18xMjMiLCJvcmciOiJvcmdfOSIsIm5hbWUiOiJBc2hhIFZlcm1hIiwicGxhbiI6InBybyIsImZlYXR1cmVzIjpbImVkaXRGb3JtcyIsInRoZXJtYWwiLCJtYXJrdXAiLCJyZXBvcnRzIiwiYnVsa1JlcG9ydHMiLCJnZW94SW1wb3J0IiwiY2xvdWRTeW5jIiwiZnV0dXJlVGhpbmciXSwibWFjaGluZSI6IlRFU1QtTUFDSElORS1JRCIsImtpbmQiOiJvbmxpbmUiLCJpc3N1ZWQiOjE3OTIxMDg4MDAsImV4cGlyZXMiOjE3OTQ3MDA4MDB9.IaSfvSxuStWAIj9SXeKYIALLf6GHk8AovSrHxvGlyTacRRzhxEjTgZgnWZNa6KKLtdlDFMasw7z2l7B1gYiwCQ";

    pub(crate) fn keys() -> Ed25519Keys {
        Ed25519Keys::from_base64(&[("test-1", TEST_KEY)]).expect("test key")
    }

    pub(crate) fn machine() -> MachineId {
        MachineId::from_output("test-machine-id").expect("id")
    }

    #[test]
    fn a_signed_pass_reads() {
        let p = Pass::verify(PRO_30_DAYS, &keys()).expect("valid");
        assert_eq!(p.plan, Plan::Pro);
        assert_eq!(p.kind, PassKind::Online);
        assert_eq!(p.name, "Asha Verma");
        assert_eq!(p.org.as_deref(), Some("org_9"));
        // "futureThing" is dropped, the rest kept.
        assert_eq!(p.features.len(), 7);
        assert!(p.allows(Feature::Thermal) && p.allows(Feature::BulkReports));
        assert_eq!(p.expires - p.issued, 30 * DAY);
    }

    #[test]
    fn thirty_days_offline_then_it_needs_the_server() {
        let p = Pass::verify(PRO_30_DAYS, &keys()).expect("valid");
        let m = machine();
        assert_eq!(p.status(&m, ISSUED, None), PassStatus::Valid { days_left: 30 });
        assert_eq!(p.status(&m, ISSUED + 29 * DAY + 1, None), PassStatus::Valid { days_left: 1 });
        assert_eq!(p.status(&m, ISSUED + 30 * DAY, None), PassStatus::Expired);
        assert_eq!(p.status(&m, i64::MAX, None), PassStatus::Expired);
    }

    #[test]
    fn clock_moved_back_is_caught() {
        let p = Pass::verify(PRO_30_DAYS, &keys()).expect("valid");
        let m = machine();
        assert_eq!(p.status(&m, ISSUED - DAY + 1, None), PassStatus::Valid { days_left: 31 });
        assert_eq!(p.status(&m, ISSUED - DAY - 1, None), PassStatus::ClockBack);
        // Used on day 20, then the clock set back to day 5 to stretch the pass.
        assert_eq!(p.status(&m, ISSUED + 5 * DAY, Some(ISSUED + 20 * DAY)), PassStatus::ClockBack);
        assert!(matches!(p.status(&m, ISSUED + 20 * DAY, Some(ISSUED + 20 * DAY)), PassStatus::Valid { .. }));
        assert_eq!(p.status(&m, i64::MIN, None), PassStatus::ClockBack);
    }

    #[test]
    fn other_computers_and_plans() {
        let k = keys();
        let m = machine();
        let other = Pass::verify(OTHER_MACHINE, &k).expect("signed");
        assert_eq!(other.status(&m, ISSUED, None), PassStatus::WrongMachine);
        let free = Pass::verify(FREE_PLAN, &k).expect("signed");
        assert_eq!(free.plan, Plan::Free);
        assert!(!free.allows(Feature::EditForms));
        let offline = Pass::verify(OFFLINE_FILE, &k).expect("signed");
        assert_eq!((offline.kind, offline.plan), (PassKind::Offline, Plan::Organisation));
        assert_eq!(offline.status(&m, ISSUED + 200 * DAY, None), PassStatus::Valid { days_left: 165 });
    }

    #[test]
    fn tampered_or_foreign_passes_are_refused() {
        let k = keys();
        let (signed, sig) = PRO_30_DAYS.rsplit_once('.').expect("parts");
        let (_, payload) = signed.split_once('.').expect("parts");
        // The same JSON with a later expiry, under the original signature.
        let json = String::from_utf8(URL_SAFE_NO_PAD.decode(payload).expect("b64")).expect("utf8");
        let stretched = URL_SAFE_NO_PAD.encode(json.replace("1794700800", "1894700800"));
        let mut flipped = sig.to_string();
        flipped.replace_range(5..6, if &sig[5..6] == "A" { "B" } else { "A" });
        for (bad, why) in [
            (format!("gfp1.{stretched}.{sig}"), LicenceError::BadSignature),
            (format!("{signed}.{flipped}"), LicenceError::BadSignature),
            (format!("gfp2.{payload}.{sig}"), LicenceError::Malformed("not a GeoGIS pass")),
            (payload.to_string(), LicenceError::Malformed("not three parts")),
            (String::new(), LicenceError::Malformed("not three parts")),
            ("SECRET_ADMIN_DEBUG".into(), LicenceError::Malformed("not three parts")),
            (UNKNOWN_KID.into(), LicenceError::BadSignature),
            (WRONG_VERSION.into(), LicenceError::Malformed("a newer pass version; update GeoGIS")),
            ("x".repeat(MAX_PASS_LEN + 1), LicenceError::TooLong { max: MAX_PASS_LEN }),
        ] {
            assert_eq!(Pass::verify(&bad, &k), Err(why), "{bad}");
        }
        // No production key yet, so nothing verifies against it.
        assert_eq!(Pass::verify(PRO_30_DAYS, &Ed25519Keys::production().expect("keys")), Err(LicenceError::BadSignature));
    }

    fn claims() -> PassClaims {
        PassClaims {
            account: "acc_1".into(),
            org: Some("org_1".into()),
            name: "Asha Verma".into(),
            plan: Plan::Pro,
            features: vec![Feature::EditForms, Feature::Thermal],
            machine: machine().as_str().to_string(),
            kind: PassKind::Online,
            issued: ISSUED,
            expires: ISSUED + ONLINE_PASS_DAYS * DAY,
        }
    }

    #[test]
    fn what_the_server_signs_the_app_reads_back() {
        let signer = Ed25519Signer::from_pkcs8("k1", &Ed25519Signer::generate_pkcs8().expect("key")).expect("signer");
        assert!(!format!("{signer:?}").contains(&signer.public_key_base64()));
        let keys = Ed25519Keys::from_base64(&[("k1", &signer.public_key_base64())]).expect("keys");
        let c = claims();
        let text = c.sign(&signer).expect("signed");
        let p = Pass::verify(&text, &keys).expect("reads");
        assert_eq!((p.account.as_str(), p.org.as_deref(), p.plan, p.kind), ("acc_1", Some("org_1"), Plan::Pro, PassKind::Online));
        assert_eq!((&p.features, p.issued, p.expires), (&c.features, c.issued, c.expires));
        assert!(matches!(p.status(&machine(), ISSUED, None), PassStatus::Valid { days_left: 30 }));
        // Another key id doesn't verify it.
        let other = Ed25519Keys::from_base64(&[("k2", &signer.public_key_base64())]).expect("keys");
        assert_eq!(Pass::verify(&text, &other), Err(LicenceError::BadSignature));
    }

    #[test]
    fn passes_the_app_would_reject_are_never_signed() {
        let signer = Ed25519Signer::from_pkcs8("k1", &Ed25519Signer::generate_pkcs8().expect("key")).expect("signer");
        let bad: [fn(&mut PassClaims); 7] = [
            |c| c.plan = Plan::Unknown,
            |c| c.features.push(Feature::Unknown),
            |c| c.features.push(Feature::Thermal),
            |c| c.expires = c.issued,
            |c| c.expires = c.issued + ONLINE_PASS_DAYS * DAY + 1,
            |c| c.machine.clear(),
            |c| c.name = "x".repeat(MAX_PASS_LEN),
        ];
        for (i, change) in bad.iter().enumerate() {
            let mut c = claims();
            change(&mut c);
            assert!(c.sign(&signer).is_err(), "case {i}");
        }
        let mut long = claims();
        long.kind = PassKind::Offline;
        long.expires = long.issued + MAX_OFFLINE_PASS_DAYS * DAY;
        assert!(long.sign(&signer).is_ok());
        assert!(Ed25519Signer::from_pkcs8("k", b"not a key").is_err());
    }

    #[test]
    fn bad_public_keys_are_refused() {
        for bad in ["", "not base64!", "AAAA"] {
            assert!(Ed25519Keys::from_base64(&[("k", bad)]).is_err(), "{bad}");
        }
    }
}
