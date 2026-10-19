//! Talking to the account server: signing in, renewing the pass, signing out.
//!
//! Sign-in uses the device-code flow (RFC 8628), as TVs and command-line tools do. GeoGIS shows a
//! short code and opens the account page in the browser; the person signs in there (password,
//! two-factor, or their company's single sign-on) and types the code; GeoGIS then collects its
//! pass. GeoGIS never sees the password, and no local web server is needed, so firewalls that block
//! one don't break sign-in.
//!
//! The server side and the HTTP implementation come with the Phase 7 server; this is the contract.

use thiserror::Error;

use crate::MachineId;

/// What the server returns when a sign-in starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceCode {
    /// The secret GeoGIS polls with; never shown.
    pub device_code: String,
    /// What the person types on the account page, such as `WDJB-MJHT`.
    pub user_code: String,
    /// The page to open, with the code filled in where the server supports it.
    pub verification_uri: String,
    /// How often to ask whether sign-in finished.
    pub interval_secs: u64,
    /// How long the code works.
    pub expires_in_secs: u64,
}

/// A successful sign-in or renewal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    /// A signed pass for this computer ([`crate::Pass`]).
    pub pass: String,
    /// Used once to get the next grant; the server replaces it on every renewal, so a copied one
    /// stops working as soon as the real app renews.
    pub refresh_token: String,
}

/// Where a sign-in stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignInPoll {
    /// Not finished yet; ask again after the interval.
    Pending,
    /// Asked too often; add 5 seconds to the interval.
    SlowDown,
    Granted(Grant),
    /// The person declined on the account page.
    Denied,
    /// The code ran out; start again.
    Expired,
}

/// Why the server could not be used. None of these lock the app: it carries on with the pass it
/// has, or in the free tier.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum AccountError {
    /// No connection, a proxy refused, or the server is down.
    #[error("the account server could not be reached ({0})")]
    Offline(String),
    /// The seat was taken back, the subscription ended, or the person signed out everywhere.
    #[error("this computer is no longer signed in to the account")]
    Revoked,
    /// The plan's device limit is reached; sign out on another computer first.
    #[error("the account is already in use on {devices} computers, its limit")]
    SeatLimit { devices: u32 },
    #[error("the account server refused the request ({0})")]
    Server(String),
}

/// The account server. Calls block on the network, so the window runs them on a worker thread.
pub trait AccountService: Send + Sync {
    /// Starts a sign-in for this computer.
    fn start_sign_in(&self, machine: &MachineId) -> Result<DeviceCode, AccountError>;
    /// Asks whether the person finished signing in.
    fn poll_sign_in(&self, code: &DeviceCode) -> Result<SignInPoll, AccountError>;
    /// A fresh 30-day pass, and the next refresh token.
    fn renew(&self, refresh_token: &str, machine: &MachineId) -> Result<Grant, AccountError>;
    /// Frees this computer's seat on the server.
    fn sign_out(&self, refresh_token: &str) -> Result<(), AccountError>;
}
