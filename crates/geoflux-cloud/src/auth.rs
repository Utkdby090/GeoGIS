//! Signing in: email and password rules, secret tokens, device codes, and the signed-in principal.
//!
//! Secrets (sessions, refresh tokens, reset links…) are 32 random bytes shown to the client once;
//! the server stores only their SHA-256 ([`SecretToken::digest`]), so a database leak hands out no
//! working token. Passwords are hashed with Argon2id by [`crate::ports::PasswordHasher`].

use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use crate::error::{ApiError, FieldError};
use crate::model::{Role, Subscription};
use crate::plans::{self, Permission};
use crate::{DeviceId, OrgId, Sha256, UserId};

pub const MIN_PASSWORD_CHARS: usize = 10;
pub const MAX_PASSWORD_CHARS: usize = 128;
pub const MAX_EMAIL_LEN: usize = 254;
pub const MAX_NAME_CHARS: usize = 100;

/// An email address as stored and compared: trimmed and lower-cased. Only a sanity check; the
/// verification email proves the address.
pub fn normalize_email(text: &str) -> Result<String, FieldError> {
    let e = text.trim().to_lowercase();
    let bad = |reason| Err(FieldError::new("email", reason));
    if e.is_empty() || e.len() > MAX_EMAIL_LEN {
        return bad("not_an_email");
    }
    if e.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | ',' | ';' | '"')) {
        return bad("not_an_email");
    }
    match e.split_once('@') {
        Some((local, domain))
            if !local.is_empty()
                && !domain.contains('@')
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
                && !domain.contains("..") =>
        {
            Ok(e)
        }
        _ => bad("not_an_email"),
    }
}

/// A new password's rules (NIST 800-63B: length over composition rules). Checked against
/// breached-password lists is a later addition.
pub fn check_password(password: &str, email: &str) -> Result<(), FieldError> {
    let n = password.chars().count();
    let bad = |reason| Err(FieldError::new("password", reason));
    if n < MIN_PASSWORD_CHARS {
        return bad("too_short");
    }
    if n > MAX_PASSWORD_CHARS {
        return bad("too_long");
    }
    let lower = password.to_lowercase();
    let local = email.split('@').next().unwrap_or_default();
    if lower == email || (!local.is_empty() && lower == local) {
        return bad("same_as_email");
    }
    let mut chars = password.chars();
    if let Some(first) = chars.next()
        && chars.all(|c| c == first)
    {
        return bad("too_simple");
    }
    Ok(())
}

/// A display name: trimmed, 1 to [`MAX_NAME_CHARS`] characters, no control characters.
pub fn check_name(field: &str, text: &str) -> Result<String, FieldError> {
    let t = text.trim();
    let n = t.chars().count();
    if n == 0 || n > MAX_NAME_CHARS || t.chars().any(char::is_control) {
        return Err(FieldError::new(field, if n == 0 { "required" } else { "invalid" }));
    }
    Ok(t.to_string())
}

/// What a secret token is for. Each kind has its own prefix, so one can't be replayed as another,
/// and its own lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// The web app's sign-in, in an HttpOnly, Secure, SameSite=Strict cookie.
    Session,
    /// The desktop app's bearer token for uploads.
    Access,
    /// The desktop app's renewal token; replaced on every renewal, and a reused one signs the
    /// computer out (it was copied).
    Refresh,
    /// RFC 8628 device code, polled by GeoGIS while the person approves in the browser.
    DeviceCode,
    PasswordReset,
    EmailVerify,
    Invite,
}

impl TokenKind {
    pub fn prefix(self) -> &'static str {
        match self {
            TokenKind::Session => "gfs",
            TokenKind::Access => "gfa",
            TokenKind::Refresh => "gfr",
            TokenKind::DeviceCode => "gfd",
            TokenKind::PasswordReset => "gfp",
            TokenKind::EmailVerify => "gfv",
            TokenKind::Invite => "gfi",
        }
    }

    /// How long it works, in seconds. Sessions also end after this much inactivity.
    pub fn lifetime_secs(self) -> i64 {
        match self {
            TokenKind::Session => 7 * 86_400,
            TokenKind::Access => 3_600,
            // A little longer than the pass, so a computer that renews on day 30 still can.
            TokenKind::Refresh => 45 * 86_400,
            TokenKind::DeviceCode => 15 * 60,
            TokenKind::PasswordReset => 3_600,
            TokenKind::EmailVerify => 3 * 86_400,
            TokenKind::Invite => 7 * 86_400,
        }
    }

    /// Used once, then deleted ([`crate::ports::AccountStore::take_token`]).
    pub fn single_use(self) -> bool {
        matches!(self, TokenKind::PasswordReset | TokenKind::EmailVerify | TokenKind::Invite)
    }
}

/// A secret as the client holds it: `<prefix>_<base64url of 32 random bytes>`. `Debug` never
/// prints it.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretToken {
    kind: TokenKind,
    text: String,
}

impl fmt::Debug for SecretToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecretToken({}_…)", self.kind.prefix())
    }
}

impl SecretToken {
    /// A new token from 32 bytes of [`crate::ports::Random`].
    pub fn from_random(kind: TokenKind, bytes: [u8; 32]) -> Self {
        SecretToken { kind, text: format!("{}_{}", kind.prefix(), URL_SAFE_NO_PAD.encode(bytes)) }
    }

    /// A token a client sent, if it has the right kind and shape.
    pub fn parse(kind: TokenKind, text: &str) -> Option<Self> {
        let rest = text.strip_prefix(kind.prefix())?.strip_prefix('_')?;
        let bytes = URL_SAFE_NO_PAD.decode(rest).ok()?;
        (bytes.len() == 32).then(|| SecretToken { kind, text: text.to_string() })
    }

    pub fn kind(&self) -> TokenKind {
        self.kind
    }

    /// What the client is given. Only ever written to a response, never to a log.
    pub fn expose(&self) -> &str {
        &self.text
    }

    /// What the server stores and looks up by.
    pub fn digest(&self) -> Sha256 {
        Sha256::of(self.text.as_bytes())
    }
}

/// The code a person types on the account page for a desktop sign-in, such as `WDJB-MJHT`.
/// RFC 8628 §6.1: consonants only (no words, no 0/O or 1/I mix-ups), 8 of 20 letters, about 34.6
/// bits; with a 15-minute life and rate-limited attempts that is out of reach of guessing.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct UserCode(String);

const USER_CODE_ALPHABET: &[u8; 20] = b"BCDFGHJKLMNPQRSTVWXZ";

impl UserCode {
    /// From 8 bytes of [`crate::ports::Random`]. Reduced from 64 bits, so the bias is about 10⁻⁹.
    pub fn from_random(bytes: [u8; 8]) -> Self {
        let mut n = u64::from_le_bytes(bytes);
        let mut s = String::with_capacity(9);
        for i in 0..8 {
            if i == 4 {
                s.push('-');
            }
            s.push(char::from(USER_CODE_ALPHABET[(n % 20) as usize]));
            n /= 20;
        }
        UserCode(s)
    }

    /// A code as typed: any case, with or without the dash or spaces.
    pub fn parse(text: &str) -> Option<Self> {
        let letters: String = text.chars().filter(|c| !matches!(c, '-' | ' ')).map(|c| c.to_ascii_uppercase()).collect();
        if letters.len() != 8 || !letters.bytes().all(|b| USER_CODE_ALPHABET.contains(&b)) {
            return None;
        }
        Some(UserCode(format!("{}-{}", &letters[..4], &letters[4..])))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// How the request proved who it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthVia {
    /// The web app's session cookie.
    Session,
    /// A desktop access token, for one computer.
    Device(DeviceId),
}

/// A signed-in person, as the server's sign-in middleware makes it after checking a session or
/// access token and loading the membership and subscription. Routes never build one themselves.
#[derive(Clone, Debug)]
pub struct Principal {
    pub user: UserId,
    pub org: OrgId,
    pub role: Role,
    pub via: AuthVia,
    subscription: Subscription,
}

impl Principal {
    pub fn new(user: UserId, role: Role, via: AuthVia, subscription: Subscription) -> Self {
        Principal { user, org: subscription.org.clone(), role, via, subscription }
    }

    pub fn subscription(&self) -> &Subscription {
        &self.subscription
    }

    /// The scope organisation data is read and written through, if the role and plan allow `p`.
    pub fn require(&self, p: Permission, now: i64) -> Result<OrgScope, ApiError> {
        plans::check(&self.subscription, self.role, p, now)?;
        Ok(OrgScope { org: self.org.clone(), user: self.user.clone(), permission: p })
    }
}

/// Proof that a request may touch one organisation's data for one purpose. Only
/// [`Principal::require`] makes one, and every store method that reads or writes organisation data
/// takes one, so a route that forgot the check doesn't compile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrgScope {
    org: OrgId,
    user: UserId,
    permission: Permission,
}

impl OrgScope {
    pub fn org(&self) -> &OrgId {
        &self.org
    }

    /// Who acts, for `uploaded_by` and the audit log.
    pub fn user(&self) -> &UserId {
        &self.user
    }

    pub fn permission(&self) -> Permission {
        self.permission
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::model::{Billing, SubscriptionState};
    use geoflux_licence::Plan;

    #[test]
    fn emails() {
        assert_eq!(normalize_email("  Asha.Verma@Example.IN ").unwrap(), "asha.verma@example.in");
        for bad in ["", "asha", "asha@", "@x.in", "a@b", "a@@b.in", "a b@x.in", "a@x..in", "a@.x.in", "a@x.in.", "<a@x.in>"] {
            assert!(normalize_email(bad).is_err(), "{bad}");
        }
        assert!(normalize_email(&format!("{}@x.in", "a".repeat(250))).is_err());
    }

    #[test]
    fn passwords() {
        let email = "asha@example.in";
        assert!(check_password("correct horse battery", email).is_ok());
        assert_eq!(check_password("short", email).unwrap_err().reason, "too_short");
        assert_eq!(check_password(&"x".repeat(129), email).unwrap_err().reason, "too_long");
        assert_eq!(check_password("aaaaaaaaaaaa", email).unwrap_err().reason, "too_simple");
        assert_eq!(check_password("ASHA@example.in", email).unwrap_err().reason, "same_as_email");
        // Counted in characters, not bytes.
        assert!(check_password("पासवर्डपासवर्ड", email).is_ok());
        assert_eq!(check_name("name", "  Asha  ").unwrap(), "Asha");
        assert!(check_name("name", " ").is_err() && check_name("name", "a\u{0}b").is_err());
    }

    #[test]
    fn tokens_keep_to_their_kind() {
        let t = SecretToken::from_random(TokenKind::Refresh, [7; 32]);
        assert!(t.expose().starts_with("gfr_"));
        assert!(!format!("{t:?}").contains(&t.expose()[4..]));
        assert_eq!(SecretToken::parse(TokenKind::Refresh, t.expose()), Some(t.clone()));
        assert_eq!(SecretToken::parse(TokenKind::Access, t.expose()), None);
        assert_eq!(SecretToken::parse(TokenKind::Refresh, "gfr_short"), None);
        assert_ne!(t.digest(), SecretToken::from_random(TokenKind::Refresh, [8; 32]).digest());
        assert!(TokenKind::Refresh.lifetime_secs() > geoflux_licence::ONLINE_PASS_DAYS * 86_400);
    }

    #[test]
    fn user_codes() {
        let c = UserCode::from_random([0xff; 8]);
        assert_eq!(c.as_str().len(), 9);
        assert_eq!(UserCode::parse(&c.as_str().to_lowercase().replace('-', " ")), Some(c));
        assert_eq!(UserCode::parse("wdjb mjht").unwrap().as_str(), "WDJB-MJHT");
        for bad in ["WDJB-MJH", "WDJB-MJHA", "WDJB-MJHT-X", "0DJB-MJHT"] {
            assert_eq!(UserCode::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn scopes_only_come_from_a_permitted_principal() {
        let sub = Subscription {
            org: OrgId::from_random([1; 16]),
            plan: Plan::Organisation,
            state: SubscriptionState::Active { until: i64::MAX },
            billing: Billing::Invoice,
            seats: 1,
            viewer_seats: 1,
        };
        let viewer = Principal::new(UserId::from_random([2; 16]), Role::Viewer, AuthVia::Session, sub);
        let scope = viewer.require(Permission::ViewSurveys, 0).unwrap();
        assert_eq!((scope.org(), scope.permission()), (&viewer.org, Permission::ViewSurveys));
        assert!(viewer.require(Permission::UploadSurveys, 0).is_err());
    }
}
