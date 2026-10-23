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

/// Writes each email's link to the server's log instead of sending it: for trying the server on
/// your own computer before mail is set up (`GEOFLUX_MAIL_LOG=1`). Anyone who can read the log
/// can then reset any password, so never on a shared server.
pub struct LogMail;

impl Mailer for LogMail {
    async fn send(&self, to: &str, _name: &str, email: Email) -> Result<(), String> {
        log_email(to, &email);
        Ok(())
    }
}

/// One email as a log line: who it is for, what it is, and its link.
pub fn log_email(to: &str, email: &Email) {
    let (what, link) = match email {
        Email::VerifyAddress { link } => ("address check", link),
        Email::ResetPassword { link } => ("password reset", link),
        Email::Invite { link, .. } => ("invite", link),
    };
    log::info!("{what} email for {to} (not sent; link for local testing): {link}");
}
