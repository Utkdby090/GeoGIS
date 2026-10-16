//! Why a licence, a machine ID or the trial record could not be used.

use thiserror::Error;

/// What went wrong reading a key, the machine ID or the stored licence and trial.
///
/// These are details for the log and the support screen. What the person sees is a
/// [`crate::LockReason`], which is the same short list GeoFlux showed.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum LicenceError {
    /// The key isn't `payload.signature` in URL-safe base64, or the payload isn't `MachineID|YYYY-MM-DD`.
    #[error("the licence key is not in the expected form ({0})")]
    Malformed(&'static str),
    /// The key is longer than any key the server issues.
    #[error("the licence key is longer than {max} characters")]
    TooLong { max: usize },
    /// The signature doesn't match: the key was changed, or another server signed it.
    #[error("the licence key's signature does not match")]
    BadSignature,
    /// The built-in public key couldn't be read (a build problem, never the customer's).
    #[error("the licence public key is not usable ({0})")]
    BadPublicKey(&'static str),
    /// The machine ID could not be read, or the system returned an error or a placeholder instead.
    #[error("this computer's ID could not be read ({0})")]
    MachineId(String),
    /// Reading or saving the stored licence or trial failed.
    #[error("the licence settings could not be read or saved ({0})")]
    Storage(String),
}
