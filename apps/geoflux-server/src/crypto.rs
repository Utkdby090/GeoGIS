//! Password hashing, randomness, time and rate limits: the ports every hosting choice shares.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use argon2::password_hash::{PasswordHash, PasswordHasher as _, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use geoflux_cloud::ports::{Clock, Limit, PasswordHasher, Random, RateLimiter};

/// OWASP's Argon2id settings: 19 MiB, 2 passes, 1 lane.
const M_COST_KIB: u32 = 19 * 1024;
const T_COST: u32 = 2;
const P_COST: u32 = 1;

/// Argon2id with OWASP's settings. Tests may use [`Argon2Hasher::fast`].
pub struct Argon2Hasher {
    params: Params,
}

impl Argon2Hasher {
    pub fn owasp() -> Result<Self, String> {
        Ok(Argon2Hasher { params: Params::new(M_COST_KIB, T_COST, P_COST, None).map_err(|e| e.to_string())? })
    }

    /// The smallest settings Argon2 allows, for tests only: hashes are still real, just quick.
    pub fn fast() -> Result<Self, String> {
        Ok(Argon2Hasher { params: Params::new(8, 1, 1, None).map_err(|e| e.to_string())? })
    }

    fn argon(&self) -> Argon2<'static> {
        Argon2::new(Algorithm::Argon2id, Version::V0x13, self.params.clone())
    }
}

impl PasswordHasher for Argon2Hasher {
    fn hash(&self, password: &str) -> Result<String, String> {
        let mut salt = [0u8; 16];
        getrandom::fill(&mut salt).map_err(|e| e.to_string())?;
        let salt = SaltString::encode_b64(&salt).map_err(|e| e.to_string())?;
        self.argon().hash_password(password.as_bytes(), &salt).map(|h| h.to_string()).map_err(|e| e.to_string())
    }

    fn verify(&self, password: &str, hash: &str) -> bool {
        PasswordHash::new(hash).is_ok_and(|h| self.argon().verify_password(password.as_bytes(), &h).is_ok())
    }

    fn needs_rehash(&self, hash: &str) -> bool {
        let Ok(h) = PasswordHash::new(hash) else { return true };
        let Ok(p) = Params::try_from(&h) else { return true };
        h.algorithm.as_str() != "argon2id" || p.m_cost() < self.params.m_cost() || p.t_cost() < self.params.t_cost()
    }
}

/// The operating system's random source.
pub struct OsRandom;

impl Random for OsRandom {
    fn fill(&self, bytes: &mut [u8]) -> Result<(), String> {
        getrandom::fill(bytes).map_err(|e| e.to_string())
    }
}

/// `N` random bytes, or an error the route turns into a 500.
pub fn random_bytes<const N: usize>(r: &dyn Random) -> Result<[u8; N], String> {
    let mut b = [0u8; N];
    r.fill(&mut b)?;
    Ok(b)
}

/// 16 random bytes for a new id. Ids only need to be unique, so if the OS source fails the
/// current time in nanoseconds is used instead.
pub fn random_id() -> [u8; 16] {
    let mut b = [0u8; 16];
    if getrandom::fill(&mut b).is_err() {
        b = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()).to_le_bytes();
    }
    b
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
    }
}

/// Keys kept before the oldest are dropped, so a flood of new keys (random emails, spoofed
/// addresses) can't grow memory without bound.
const MAX_KEYS: usize = 100_000;

/// Sliding-window limits in memory. One server process; a second instance would keep its own
/// counts (fine for one VM; a shared store comes with scaling out).
#[derive(Default)]
pub struct MemoryRateLimiter {
    hits: Mutex<HashMap<String, VecDeque<i64>>>,
}

impl RateLimiter for MemoryRateLimiter {
    fn hit(&self, key: &str, limit: Limit, now: i64) -> Result<(), u64> {
        let window = i64::try_from(limit.window_secs).unwrap_or(i64::MAX);
        let mut map = match self.hits.lock() {
            Ok(m) => m,
            // A panic while holding the lock can't leave the counts inconsistent in a way that
            // matters; keep limiting.
            Err(poisoned) => poisoned.into_inner(),
        };
        if map.len() >= MAX_KEYS && !map.contains_key(key) {
            map.retain(|_, q| q.back().is_some_and(|t| now - t < window));
            if map.len() >= MAX_KEYS {
                map.clear();
            }
        }
        let q = map.entry(key.to_string()).or_default();
        while q.front().is_some_and(|t| now - t >= window) {
            q.pop_front();
        }
        if q.len() >= limit.attempts as usize {
            let oldest = q.front().copied().unwrap_or(now);
            return Err(u64::try_from(oldest + window - now).unwrap_or(1).max(1));
        }
        q.push_back(now);
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn hashes_verify_and_report_weak_settings() {
        let fast = Argon2Hasher::fast().unwrap();
        let h = fast.hash("correct horse battery").unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(fast.verify("correct horse battery", &h));
        assert!(!fast.verify("wrong", &h));
        assert!(!fast.verify("x", "not a hash"));
        assert!(Argon2Hasher::owasp().unwrap().needs_rehash(&h));
        assert!(!fast.needs_rehash(&h));
    }

    #[test]
    fn limits_slide() {
        let l = MemoryRateLimiter::default();
        let lim = Limit { attempts: 2, window_secs: 60 };
        assert!(l.hit("k", lim, 0).is_ok() && l.hit("k", lim, 10).is_ok());
        assert_eq!(l.hit("k", lim, 20), Err(40));
        assert!(l.hit("other", lim, 20).is_ok());
        assert!(l.hit("k", lim, 61).is_ok());
    }
}
