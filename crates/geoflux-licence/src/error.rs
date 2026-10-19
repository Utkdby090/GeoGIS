//! Why a pass, a machine ID or the stored sign-in could not be used.

use thiserror::Error;

/// Details for the log and the support screen. What the person sees is a [`crate::FreeReason`].
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum LicenceError {
    /// Not `gfp1.<payload>.<signature>`, or the payload isn't a pass this build reads.
    #[error("the pass is not in the expected form ({0})")]
    Malformed(&'static str),
    #[error("the pass is longer than {max} characters")]
    TooLong { max: usize },
    /// The signature doesn't match: the pass was changed, or another server signed it.
    #[error("the pass's signature does not match")]
    BadSignature,
    /// A built-in public key couldn't be read (a build problem, never the customer's).
    #[error("the account server's public key is not usable ({0})")]
    BadPublicKey(&'static str),
    /// The machine ID could not be read, or the system returned an error or a placeholder instead.
    #[error("this computer's ID could not be read ({0})")]
    MachineId(String),
    /// Reading or saving the stored sign-in failed.
    #[error("the sign-in could not be read or saved ({0})")]
    Storage(String),
}
