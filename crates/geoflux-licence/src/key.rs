//! Licence keys as GeoFlux's licence server issues them, checked the way `LicenseManager.kt` did.
//!
//! A key is `base64url(payload) "." base64url(signature)`. The payload is `MachineID|YYYY-MM-DD`
//! and the signature is RSA PKCS#1 v1.5 with SHA-256 (Java's `SHA256withRSA`) over the payload's
//! bytes, made with the server's private key. Only the public key ships in the app.

use base64::Engine;
use base64::alphabet;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};

use crate::{CivilDate, LicenceError, MachineId};

/// The public key of GeoFlux's licence server (X.509 SubjectPublicKeyInfo, base64), copied from
/// `LicenseManager.kt`. Keys it signed keep working in GeoGIS.
pub const GEOFLUX_PUBLIC_KEY: &str = "MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA8XGvR5J3Upt2LkS+loli6jrr7zAfL7QqBkaqIXGikAnRzGpw9Vu5WoMrhzwG7L34ltzACv+jsYZyl++l02vzlTKPCkGdpPslNhoqcCaPB1+PalOY1XvgYRQiT0TcYazd8Klz5o8qQ3M5KugyI1Klbw45zWHIKZY26S/D5wxQY71NAPq40Zl+708CfZdZyaFXJtYrkMcXbdoNmoo82uprtvMfJEWVLzVm7ab5cm+2c5Tae6Jv8wHnOi2Sxtapl4DJ0/jKuw1sGU1oUR7jZHL0OjizMH/zRTt8eYRoP2Kv4BYFNVXUsiILIIFqstd1s8aeXAEbvQncRGWTqJUAmRAvGQIDAQAB";

/// Longer than any key the server issues (about 400 characters for a 2048-bit key).
pub const MAX_KEY_LEN: usize = 4096;

/// URL-safe base64 with or without `=` padding, as Java's `Base64.getUrlDecoder()` reads it.
const URL_SAFE_LENIENT: GeneralPurpose = GeneralPurpose::new(
    &alphabet::URL_SAFE,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent).with_decode_allow_trailing_bits(true),
);

/// Checks a signature over a message. The desktop app uses [`RsaSha256`]; tests can use their own key.
pub trait SignatureVerifier: Send + Sync {
    fn verify(&self, message: &[u8], signature: &[u8]) -> bool;
}

/// RSA PKCS#1 v1.5 with SHA-256, 2048 to 8192 bits, through `ring` (verification only: the app
/// never holds a private key).
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug)]
pub struct RsaSha256 {
    /// The `RSAPublicKey` (PKCS#1) inside the SubjectPublicKeyInfo.
    pkcs1: Vec<u8>,
}

#[cfg(not(target_arch = "wasm32"))]
impl RsaSha256 {
    /// The key GeoFlux's server signs with ([`GEOFLUX_PUBLIC_KEY`]).
    pub fn geoflux() -> Result<Self, LicenceError> {
        RsaSha256::from_spki_base64(GEOFLUX_PUBLIC_KEY)
    }

    /// Reads a SubjectPublicKeyInfo in standard base64, tidied as GeoFlux tidied it (`-` and `_`
    /// read as `+` and `/`, whitespace ignored).
    pub fn from_spki_base64(text: &str) -> Result<Self, LicenceError> {
        let clean: String = text
            .chars()
            .filter(|c| !c.is_whitespace())
            .map(|c| {
                if c == '-' {
                    '+'
                } else if c == '_' {
                    '/'
                } else {
                    c
                }
            })
            .collect();
        let der = base64::engine::general_purpose::STANDARD.decode(clean).map_err(|_| LicenceError::BadPublicKey("not base64"))?;
        Ok(RsaSha256 { pkcs1: spki_rsa_key(&der)?.to_vec() })
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl SignatureVerifier for RsaSha256 {
    fn verify(&self, message: &[u8], signature: &[u8]) -> bool {
        use ring::signature::{RSA_PKCS1_2048_8192_SHA256, UnparsedPublicKey};
        UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, &self.pkcs1).verify(message, signature).is_ok()
    }
}

/// The DER object identifier 1.2.840.113549.1.1.1 (rsaEncryption).
const RSA_ENCRYPTION_OID: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];

/// The `RSAPublicKey` inside a SubjectPublicKeyInfo:
/// `SEQUENCE { SEQUENCE { OID rsaEncryption, NULL }, BIT STRING { 0x00, RSAPublicKey } }`.
fn spki_rsa_key(der: &[u8]) -> Result<&[u8], LicenceError> {
    let bad = |why| LicenceError::BadPublicKey(why);
    let (spki, rest) = der_take(der, 0x30).ok_or(bad("not a SubjectPublicKeyInfo"))?;
    if !rest.is_empty() {
        return Err(bad("data after the key"));
    }
    let (algorithm, rest) = der_take(spki, 0x30).ok_or(bad("no algorithm"))?;
    let (oid, params) = der_take(algorithm, 0x06).ok_or(bad("no algorithm id"))?;
    if oid != RSA_ENCRYPTION_OID || !(params.is_empty() || params == [0x05, 0x00]) {
        return Err(bad("not an RSA key"));
    }
    let (bits, rest) = der_take(rest, 0x03).ok_or(bad("no key bits"))?;
    if !rest.is_empty() {
        return Err(bad("data after the key bits"));
    }
    match bits.split_first() {
        Some((0, key)) if !key.is_empty() => Ok(key),
        _ => Err(bad("key bits not byte-aligned")),
    }
}

/// One DER element with tag `tag`: its contents and what follows it.
fn der_take(input: &[u8], tag: u8) -> Option<(&[u8], &[u8])> {
    let (&t, rest) = input.split_first()?;
    if t != tag {
        return None;
    }
    let (&first, rest) = rest.split_first()?;
    let (len, rest) = if first < 0x80 {
        (usize::from(first), rest)
    } else {
        let n = usize::from(first & 0x7f);
        if n == 0 || n > 4 || rest.len() < n {
            return None;
        }
        let len = rest[..n].iter().fold(0usize, |acc, &b| (acc << 8) | usize::from(b));
        (len, &rest[n..])
    };
    (rest.len() >= len).then(|| rest.split_at(len))
}

/// A licence key whose signature checked out, before the machine and the date are compared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedPayload {
    /// The machine ID exactly as signed.
    pub machine_id: String,
    /// The expiry exactly as signed; GeoFlux read it only after the machine matched.
    pub expires_text: String,
}

impl SignedPayload {
    /// Splits the key, checks its signature and splits the payload, in GeoFlux's order. The key is
    /// trimmed first, as GeoFlux did.
    pub fn verify(key: &str, verifier: &dyn SignatureVerifier) -> Result<SignedPayload, LicenceError> {
        let key = key.trim();
        if key.len() > MAX_KEY_LEN {
            return Err(LicenceError::TooLong { max: MAX_KEY_LEN });
        }
        let mut parts = key.split('.');
        let (Some(payload), Some(signature), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(LicenceError::Malformed("not two parts"));
        };
        let payload = URL_SAFE_LENIENT.decode(payload).map_err(|_| LicenceError::Malformed("payload is not base64"))?;
        let signature = URL_SAFE_LENIENT.decode(signature).map_err(|_| LicenceError::Malformed("signature is not base64"))?;
        if !verifier.verify(&payload, &signature) {
            return Err(LicenceError::BadSignature);
        }
        let text = String::from_utf8_lossy(&payload);
        let mut fields = text.split('|');
        let (Some(machine_id), Some(expires), None) = (fields.next(), fields.next(), fields.next()) else {
            return Err(LicenceError::Malformed("payload is not MachineID|date"));
        };
        Ok(SignedPayload { machine_id: machine_id.to_string(), expires_text: expires.to_string() })
    }
}

/// What a key means on this computer today: GeoFlux's `LicenseStatus`, with the days left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyStatus {
    /// Usable through its expiry date (inclusive); `days_left` is 0 on the last day.
    Valid { days_left: i64 },
    /// Signed for this computer, but today is past its expiry date.
    Expired,
    /// Signed, but for another computer.
    WrongMachine,
    /// Changed, signed by someone else, or not a key at all.
    Invalid,
}

/// Checks `key` for `machine` on `today` (the computer's local date), with GeoFlux's rules and in
/// its order: signature, payload shape, machine, date format, expiry.
pub fn check_key(key: &str, machine: &MachineId, today: CivilDate, verifier: &dyn SignatureVerifier) -> KeyStatus {
    let Ok(signed) = SignedPayload::verify(key, verifier) else {
        return KeyStatus::Invalid;
    };
    if signed.machine_id != machine.as_str() {
        return KeyStatus::WrongMachine;
    }
    match CivilDate::parse(&signed.expires_text) {
        None => KeyStatus::Invalid,
        Some(expires) if today > expires => KeyStatus::Expired,
        Some(expires) => KeyStatus::Valid { days_left: today.days_until(expires) },
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    /// A throwaway 2048-bit test key; its private half signed the keys below and was then deleted.
    const TEST_PUBLIC_KEY: &str = include_str!("../testdata/test-public-key.b64");
    const FOR_TEST_MACHINE: &str = "VEVTVC1NQUNISU5FLUlEfDIwMjctMDEtMzE.ZbAi8lHgk6oaHKdCs5Vwf0hz2C0Z_kDWNe_2EHtG_6Qp_gbq4WRitJ1dMcXhxFXMMINVkJnHqSWAEHgu7QeGNL1WOI2uz4dZdhPF70AA-7mZEH2N4xn1Oc_SLSqtwsgFCUNo2AJs7B2pHU8En7YB6UFQsfhuKuygG_zwHyDKZ1ZYuR6-HBEAAhWE965iXRtR7867_66TIwOZ4EP8qv2aCzTOmphwuHGnoDozgmEgKgywRyIw28gi-2B9HHBEKWGFjyhGtMe3EKgxt13d2GucFDMpO77wO04iNA6OcToHnB--78o9mCadP7OFOZMYIRbH5i675_CazpGJS8Dt-xCbCA";
    const FOR_OTHER_MACHINE: &str = "T1RIRVItTUFDSElORXwyMDI3LTAxLTMx.i0ktnCgB0DF5-DvSyLgnpNOfIjl40nP2WqI8W7P295y959-ia5EA1tSYtOc1k2YE2nnzE5v_MdKwMTiAheZFpcSPCxQdEj7zfaUQ99UjwOtRyPkN1CrKzz29J1PnOVyVPEL18_xQU8eXYLGiBN8LGDaVaL497sYEVN-EahNzaL_hVNLVk7w0vgpK3A3eVkgpCXnZJJ2a44cNrjyTh-zB5OAfAj65hFohSPHOOmQ5pTNFsS5q2XSOeKDeOFinD_CuZKBWVyZJqeNHRoYSVAWVtCZRntrzcF1Wa4XnT1UnyMfzA0IFckYY3e_bdfpIKwTBrEbzTSd3G76FGLo45yE5Zg";
    const BAD_DATE: &str = "VEVTVC1NQUNISU5FLUlEfDIwMjYtMDItMzA.RBgIFCutCJklka6_PASi58P2WNODZ4xvoXOb7Q2tAnFsqN0zT6p7kzg6XRPyVVwIBFAFtrXxVKNrrKqek8er7fzWb28kMjE9clg-einmx_fShRx8SBEdm1jIejIRst0SiSCXQAnVQaZfR-cNSBA1D2c7dAJTym1F3VVUzP3UubPzeibVpP8yjVe2Gu4oIbDiNzQ5MPOt__o0TWWH99kZbB3MMA-ieUKxwexGx_da6sGDNoc1vrRNxTtFucFIbJIzwEhp44qYtk-f-NmNPzQbCyvewcl-LlnTJqK0R3i6xzHCDWZeIZxOwvzdKoms4qAlBjp8weEXKUOWdNTBnl1j9g";
    const NO_DATE: &str = "VEVTVC1NQUNISU5FLUlE.MfbOxguX_IX71kRNaxQRMLGKZcd5KNMqXRMzmYzX7JDEIsgJAYXUnVGFpQivZoyTv-nMFA7sPM2lQD9j4Rt6LBcoeV4f_jAj8hNQwST2e3VF1VuS4NEV_NPMHcVC3G2xgBDdE3O2s-T2GGEx4ltV6VIyuBWsqh-CL1jbyn7mmImaVAdqlm-ggSVD5DjZ1U9MCNDRK6Fyqr7JmHDVNVkHDnk2i6iWkPdVYj9DY77U_GyQsEnmpQe5CKppthRxmys2LIeO63Zffg2XyRLIdJqRaAQ7RTQFrhVWoqWAxuliwyVlL-xNV0sLDCmk_lalHb1sntjwuJMSzGPVfZRKXbCrWg";

    fn verifier() -> RsaSha256 {
        RsaSha256::from_spki_base64(TEST_PUBLIC_KEY).expect("test key")
    }

    fn machine() -> MachineId {
        MachineId::from_output("test-machine-id").expect("id")
    }

    fn day(y: i32, m: u8, d: u8) -> CivilDate {
        CivilDate::new(y, m, d).expect("date")
    }

    #[test]
    fn geoflux_server_key_is_readable() {
        assert!(RsaSha256::geoflux().is_ok());
    }

    #[test]
    fn keys_check_like_geoflux() {
        let v = verifier();
        let m = machine();
        assert_eq!(check_key(FOR_TEST_MACHINE, &m, day(2026, 10, 16), &v), KeyStatus::Valid { days_left: 107 });
        assert_eq!(check_key(FOR_TEST_MACHINE, &m, day(2027, 1, 31), &v), KeyStatus::Valid { days_left: 0 });
        assert_eq!(check_key(FOR_TEST_MACHINE, &m, day(2027, 2, 1), &v), KeyStatus::Expired);
        assert_eq!(check_key(&format!("  {FOR_TEST_MACHINE}\r\n"), &m, day(2026, 10, 16), &v), KeyStatus::Valid { days_left: 107 });
        assert_eq!(check_key(FOR_OTHER_MACHINE, &m, day(2026, 10, 16), &v), KeyStatus::WrongMachine);
        assert_eq!(check_key(BAD_DATE, &m, day(2026, 10, 16), &v), KeyStatus::Invalid);
        assert_eq!(check_key(NO_DATE, &m, day(2026, 10, 16), &v), KeyStatus::Invalid);
    }

    #[test]
    fn padded_base64_reads_too() {
        let (payload, sig) = FOR_TEST_MACHINE.split_once('.').expect("two parts");
        let padded = format!("{payload}{}.{sig}", "=".repeat((4 - payload.len() % 4) % 4));
        assert!(matches!(check_key(&padded, &machine(), day(2026, 10, 16), &verifier()), KeyStatus::Valid { .. }));
    }

    #[test]
    fn tampered_keys_are_invalid() {
        let v = verifier();
        let m = machine();
        let (payload, sig) = FOR_TEST_MACHINE.split_once('.').expect("two parts");
        // The payload for 2099 under the 2027 key's signature.
        let later = URL_SAFE_LENIENT.encode("TEST-MACHINE-ID|2099-01-31");
        let mut flipped = sig.to_string();
        flipped.replace_range(10..11, if &sig[10..11] == "A" { "B" } else { "A" });
        for bad in [
            format!("{later}.{sig}"),
            format!("{payload}.{flipped}"),
            format!("{payload}.{sig}.x"),
            payload.to_string(),
            String::new(),
            format!("{payload}+.{sig}"),
            "SECRET_ADMIN_DEBUG".into(),
            "A".repeat(MAX_KEY_LEN + 1),
        ] {
            assert_eq!(check_key(&bad, &m, day(2026, 10, 16), &v), KeyStatus::Invalid, "{bad}");
        }
        // Signed by our test key, so the production key refuses it.
        assert_eq!(check_key(FOR_TEST_MACHINE, &m, day(2026, 10, 16), &RsaSha256::geoflux().expect("key")), KeyStatus::Invalid);
    }

    #[test]
    fn public_key_parsing_refuses_junk() {
        for bad in ["", "not base64!", "MIIB", "AAAA"] {
            assert!(RsaSha256::from_spki_base64(bad).is_err(), "{bad}");
        }
        // GeoFlux's tidying: URL-safe characters and line breaks in the key text still read.
        let urlish: String = TEST_PUBLIC_KEY.trim().replace('+', "-").replace('/', "_");
        let wrapped = urlish.as_bytes().chunks(64).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join("\n");
        assert!(RsaSha256::from_spki_base64(&wrapped).is_ok());
    }

    #[test]
    fn der_lengths_are_bounds_checked() {
        assert_eq!(der_take(&[0x30, 0x82, 0xff, 0xff, 0x00], 0x30), None);
        assert_eq!(der_take(&[0x30, 0x85, 1, 1, 1, 1, 1], 0x30), None);
        assert_eq!(der_take(&[0x30, 0x80], 0x30), None);
        assert_eq!(der_take(&[0x30], 0x30), None);
        assert_eq!(der_take(&[0x30, 0x01, 0x07, 0x09], 0x30), Some((&[0x07][..], &[0x09][..])));
    }
}
