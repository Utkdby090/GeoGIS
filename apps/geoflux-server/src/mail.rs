//! Mail until an SMTP account is set up with the hosting choice.

use geoflux_cloud::ports::{Email, Mailer};

/// Sends nothing: the route logs that the email wasn't sent (never the link, which holds a secret),
/// and the person can ask again once mail is set up. Sign-in and uploads work without email.
pub struct NoMail;

impl Mailer for NoMail {
    async fn send(&self, _to: &str, _name: &str, email: Email) -> Result<(), String> {
        let what = match email {
            Email::VerifyAddress { .. } => "address check",
            Email::ResetPassword { .. } => "password reset",
            Email::Invite { .. } => "invite",
        };
        Err(format!("no mail service is set up, so the {what} email was not sent"))
    }
}
