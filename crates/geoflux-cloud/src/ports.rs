//! What the server plugs in: storage, photos, mail, hashing, randomness, rate limits, single
//! sign-on. Each hosting choice is a set of adapters for these traits; the rules in this crate and
//! the routes built on them stay the same:
//!
//! | Port | Oracle Always Free (F1) | Render + Neon + R2 (F2) | Supabase + Render (F3) | Tests |
//! |---|---|---|---|---|
//! | [`AccountStore`], [`DeviceStore`], [`SurveyStore`] | PostgreSQL in Docker | Neon PostgreSQL | Supabase PostgreSQL | in memory |
//! | [`BlobStore`] | the VM's disk | Cloudflare R2 (S3 API) | Supabase Storage (S3 API) | in memory |
//! | [`Mailer`] | SMTP | SMTP | SMTP | a list |
//!
//! The stores are async (`fn … -> impl Future + Send`), so the server's runtime never blocks on
//! them. [`PasswordHasher`] is deliberately blocking: Argon2 is CPU work the server runs on its
//! blocking pool, never on the request threads.
//!
//! Atomicity: wherever a check and a write must not be split by another request (a seat count and a
//! new device, a single-use token and its use, a refresh token and its replacement), the method does
//! both and says so; an SQL adapter does it in one transaction.

use std::future::Future;
use std::path::Path;

use thiserror::Error;

use crate::auth::{OrgScope, TokenKind, UserCode};
use crate::model::{Device, Membership, Organisation, Role, Subscription, SurveySummary, User};
use crate::sync::{PhotoType, SurveyCreated, SurveyManifest};
use crate::{ApiError, DeviceId, ErrorCode, OrgId, Sha256, SurveyId, UserId};

pub use geoflux_licence::{Clock, PassSigner};

/// Why a store call failed. Mapped to an [`ApiError`] without leaking details.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum StoreError {
    #[error("not found")]
    NotFound,
    /// A unique value already exists (`email`), or a compare-and-set lost a race.
    #[error("conflict: {0}")]
    Conflict(&'static str),
    /// The database or bucket is unreachable or timed out; safe to retry.
    #[error("unavailable: {0}")]
    Unavailable(String),
    /// Anything else; logged, shown to the client as an internal error.
    #[error("{0}")]
    Other(String),
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::NotFound => ApiError::not_found(),
            StoreError::Conflict(_) => ApiError::new(ErrorCode::Conflict, "That already exists or was just changed."),
            StoreError::Unavailable(_) => ApiError::new(ErrorCode::Unavailable, "The service is busy. Try again shortly."),
            StoreError::Other(_) => ApiError::internal(),
        }
    }
}

pub type StoreResult<T> = Result<T, StoreError>;

/// A person with what only the server may see.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserRecord {
    pub user: User,
    /// Argon2id in PHC form (`$argon2id$v=19$…`); `None` for people who only use single sign-on.
    pub password_hash: Option<String>,
    pub disabled: bool,
}

/// A new organisation with its first admin, made together on sign-up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewOrganisation {
    pub org: Organisation,
    pub owner: User,
    pub password_hash: String,
    pub subscription: Subscription,
}

/// A stored secret: only its digest, what it is for and until when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenRecord {
    pub kind: TokenKind,
    pub digest: Sha256,
    pub user: UserId,
    /// For access and refresh tokens: the computer.
    pub device: Option<DeviceId>,
    /// For invites: the organisation, role and address invited.
    pub invite: Option<(OrgId, Role, String)>,
    pub expires: i64,
}

pub trait AccountStore: Send + Sync {
    /// Fails with `Conflict("email")` when the address is taken.
    fn create_organisation(&self, new: NewOrganisation) -> impl Future<Output = StoreResult<()>> + Send;
    fn user_by_email(&self, email: &str) -> impl Future<Output = StoreResult<Option<UserRecord>>> + Send;
    fn user(&self, id: &UserId) -> impl Future<Output = StoreResult<Option<UserRecord>>> + Send;
    fn membership(&self, user: &UserId) -> impl Future<Output = StoreResult<Option<(Membership, Subscription)>>> + Send;
    /// Also ends every session and token of the person except `keep`, so a reset locks out
    /// whoever knew the old password.
    fn set_password(&self, user: &UserId, hash: &str, keep: Option<&Sha256>) -> impl Future<Output = StoreResult<()>> + Send;
    fn set_email_verified(&self, user: &UserId) -> impl Future<Output = StoreResult<()>> + Send;
    /// Adds an invited person (seat limits checked by the caller in the same scope).
    fn add_member(&self, scope: &OrgScope, user: User, role: Role, password_hash: &str) -> impl Future<Output = StoreResult<()>> + Send;
    fn members(&self, scope: &OrgScope) -> impl Future<Output = StoreResult<Vec<(User, Membership)>>> + Send;
    fn set_role(&self, scope: &OrgScope, user: &UserId, role: Role) -> impl Future<Output = StoreResult<()>> + Send;
    /// Removes the person and signs out their computers.
    fn remove_member(&self, scope: &OrgScope, user: &UserId) -> impl Future<Output = StoreResult<()>> + Send;
    /// GeoFlux staff only (activate by invoice); not reachable from customer routes.
    fn set_subscription(&self, sub: Subscription) -> impl Future<Output = StoreResult<()>> + Send;

    fn put_token(&self, token: TokenRecord) -> impl Future<Output = StoreResult<()>> + Send;
    /// A live token of `kind`, not consumed (sessions, access tokens).
    fn token(&self, kind: TokenKind, digest: &Sha256, now: i64) -> impl Future<Output = StoreResult<Option<TokenRecord>>> + Send;
    /// A live single-use token, deleted in the same step, so two requests can't both use it.
    fn take_token(&self, kind: TokenKind, digest: &Sha256, now: i64) -> impl Future<Output = StoreResult<Option<TokenRecord>>> + Send;
    fn delete_token(&self, kind: TokenKind, digest: &Sha256) -> impl Future<Output = StoreResult<()>> + Send;
    /// Slides a session's expiry forward on use (at most once a minute per session).
    fn extend_token(&self, kind: TokenKind, digest: &Sha256, expires: i64) -> impl Future<Output = StoreResult<()>> + Send;
    /// Deletes expired tokens and device codes; run hourly.
    fn purge_expired(&self, now: i64) -> impl Future<Output = StoreResult<u64>> + Send;
}

/// A desktop sign-in in progress (RFC 8628).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceCodeRecord {
    pub digest: Sha256,
    pub user_code: UserCode,
    pub machine: String,
    pub device_name: String,
    pub state: DeviceCodeState,
    pub interval_secs: u64,
    pub last_poll: i64,
    pub expires: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceCodeState {
    Pending,
    Approved(UserId),
    Denied,
    /// The grant was handed out; a second poll gets `expired_token`.
    Collected,
}

pub trait DeviceStore: Send + Sync {
    fn put_device_code(&self, code: DeviceCodeRecord) -> impl Future<Output = StoreResult<()>> + Send;
    fn device_code(&self, digest: &Sha256) -> impl Future<Output = StoreResult<Option<DeviceCodeRecord>>> + Send;
    fn device_code_by_user_code(&self, code: &UserCode, now: i64) -> impl Future<Output = StoreResult<Option<DeviceCodeRecord>>> + Send;
    /// Moves a code from `from` to `to` only if it is still in `from` (compare-and-set), so one
    /// approval is collected once. Also records the poll time for `slow_down`.
    fn set_device_code_state(
        &self,
        digest: &Sha256,
        from: &DeviceCodeState,
        to: DeviceCodeState,
        now: i64,
    ) -> impl Future<Output = StoreResult<bool>> + Send;

    /// Signs a computer in for a person: reuses their device for the same machine, else adds one
    /// if they have fewer than `limit`. The count and the insert happen together.
    fn claim_seat(
        &self,
        org: &OrgId,
        user: &UserId,
        machine: &str,
        name: &str,
        limit: u32,
        now: i64,
    ) -> impl Future<Output = StoreResult<Result<Device, SeatsInUse>>> + Send;
    /// The device a refresh token belongs to, swapping the token for `next` in the same step.
    /// A token that was already swapped (copied and used twice) revokes the device and returns
    /// `Ok(None)`.
    fn rotate_refresh(&self, old: &Sha256, next: &TokenRecord, now: i64) -> impl Future<Output = StoreResult<Option<Device>>> + Send;
    fn devices(&self, scope: &OrgScope, user: Option<&UserId>) -> impl Future<Output = StoreResult<Vec<Device>>> + Send;
    /// Signs a computer out: deletes its refresh and access tokens.
    fn revoke_device(&self, scope: &OrgScope, device: &DeviceId) -> impl Future<Output = StoreResult<()>> + Send;
    /// Signs out the computer holding this refresh token (GeoGIS's own Sign out).
    fn sign_out(&self, refresh: &Sha256) -> impl Future<Output = StoreResult<()>> + Send;
}

/// [`DeviceStore::claim_seat`] found the person's computers all in use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeatsInUse {
    pub devices: u32,
}

pub trait SurveyStore: Send + Sync {
    /// Step 1: stores the manifest, or returns the existing survey for the same
    /// `(org, clientSurveyId, revision)`; lists the photos the organisation doesn't have.
    fn create_survey(
        &self,
        scope: &OrgScope,
        manifest: &SurveyManifest,
        now: i64,
    ) -> impl Future<Output = StoreResult<SurveyCreated>> + Send;
    /// Whether the photo is listed in the survey, with the size and type it must have.
    fn expected_photo(
        &self,
        scope: &OrgScope,
        survey: &SurveyId,
        sha: &Sha256,
    ) -> impl Future<Output = StoreResult<Option<(u64, PhotoType)>>> + Send;
    /// Step 2, after [`BlobStore::put_file`] stored it.
    fn record_photo(&self, scope: &OrgScope, sha: &Sha256, bytes: u64) -> impl Future<Output = StoreResult<()>> + Send;
    /// Step 3: commits if every photo is stored, else returns the missing ones.
    fn commit(
        &self,
        scope: &OrgScope,
        survey: &SurveyId,
        now: i64,
    ) -> impl Future<Output = StoreResult<Result<SurveySummary, Vec<Sha256>>>> + Send;
    fn surveys(&self, scope: &OrgScope) -> impl Future<Output = StoreResult<Vec<SurveySummary>>> + Send;
    /// Bytes of photos the organisation stores, for the plan's limit.
    fn storage_used(&self, scope: &OrgScope) -> impl Future<Output = StoreResult<u64>> + Send;
}

/// Where a stored photo can be read from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlobSource {
    /// A file the server streams (disk storage).
    File(std::path::PathBuf),
    /// A short-lived signed URL the client is redirected to (S3-style storage).
    Url(String),
}

/// Photo storage, by organisation and hash. Keys are built from typed ids and hashes only, never
/// from client text, so no path or key can escape the organisation's folder.
pub trait BlobStore: Send + Sync {
    fn exists(&self, org: &OrgId, sha: &Sha256) -> impl Future<Output = StoreResult<bool>> + Send;
    /// Stores an already verified file (hash and size checked by the route); the file may be moved.
    fn put_file(&self, org: &OrgId, sha: &Sha256, file: &Path, kind: PhotoType) -> impl Future<Output = StoreResult<()>> + Send;
    fn source(&self, org: &OrgId, sha: &Sha256, now: i64) -> impl Future<Output = StoreResult<BlobSource>> + Send;
}

/// An email the server sends. Links carry a secret token and are never logged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Email {
    VerifyAddress { link: String },
    ResetPassword { link: String },
    Invite { organisation: String, inviter: String, link: String },
}

pub trait Mailer: Send + Sync {
    fn send(&self, to: &str, name: &str, email: Email) -> impl Future<Output = Result<(), String>> + Send;
}

/// Argon2id (OWASP: 19 MiB memory, 2 passes, 1 lane). Blocking; see the module notes.
pub trait PasswordHasher: Send + Sync {
    fn hash(&self, password: &str) -> Result<String, String>;
    /// Constant-time. `false` for a malformed hash too.
    fn verify(&self, password: &str, hash: &str) -> bool;
    /// Whether a stored hash used weaker settings and should be replaced at the next sign-in.
    fn needs_rehash(&self, hash: &str) -> bool;
}

/// The operating system's secure random source.
pub trait Random: Send + Sync {
    fn fill(&self, bytes: &mut [u8]) -> Result<(), String>;
}

/// Attempts per key and window. The keys the server uses: `login:<email>`, `login-ip:<ip>`,
/// `reset:<email>`, `device-code:<ip>`, `user-code:<ip>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limit {
    pub attempts: u32,
    pub window_secs: u64,
}

pub mod limits {
    use super::Limit;
    pub const LOGIN_PER_EMAIL: Limit = Limit { attempts: 5, window_secs: 15 * 60 };
    pub const LOGIN_PER_IP: Limit = Limit { attempts: 30, window_secs: 15 * 60 };
    pub const RESET_PER_EMAIL: Limit = Limit { attempts: 3, window_secs: 3_600 };
    pub const DEVICE_CODES_PER_IP: Limit = Limit { attempts: 10, window_secs: 3_600 };
    /// Typing user codes on the account page.
    pub const USER_CODE_PER_IP: Limit = Limit { attempts: 10, window_secs: 15 * 60 };
}

pub trait RateLimiter: Send + Sync {
    /// Counts one attempt; `Err(seconds)` to wait when over the limit.
    fn hit(&self, key: &str, limit: Limit, now: i64) -> Result<(), u64>;
}

/// A person proven by a single sign-on provider (OpenID Connect).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalIdentity {
    pub issuer: String,
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: bool,
}

/// Single sign-on: the hook Keycloak, Azure AD or Google Workspace plugs into when a tender asks
/// for it. Not used in 7a; email and password come first.
pub trait IdentityProvider: Send + Sync {
    /// Shown on the sign-in page.
    fn id(&self) -> &str;
    /// Where to send the browser, with the state and nonce the callback must return.
    fn authorize_url(&self, state: &str, nonce: &str) -> String;
    fn exchange(&self, code: &str, nonce: &str) -> impl Future<Output = Result<ExternalIdentity, String>> + Send;
}
