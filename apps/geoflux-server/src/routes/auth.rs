//! Web sign-up, log in and out, password reset, email verification, and who is signed in.

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use geoflux_cloud::api::{ForgotPassword, LogIn, Me, ResetPassword, SignUp, VerifyEmail};
use geoflux_cloud::auth::{Principal, SecretToken, TokenKind, check_name, check_password, normalize_email};
use geoflux_cloud::model::{Organisation, User};
use geoflux_cloud::ports::{AccountStore, Email, Mailer, NewOrganisation, StoreError, TokenRecord, limits};
use geoflux_cloud::{ApiError, ErrorCode, OrgId, UserId, plans};

use crate::crypto::random_bytes;
use crate::http::{self, Ctx, Fail, Result, bad, internal, limit, new_token, parse};
use crate::{Backend, Server};

/// Sign-ups per address per hour.
const SIGN_UPS_PER_IP: geoflux_cloud::ports::Limit = geoflux_cloud::ports::Limit { attempts: 10, window_secs: 3_600 };

pub(super) async fn me_for<B: Backend>(s: &Server<B>, p: &Principal) -> Result<Me> {
    let a = s.backend.accounts();
    let user = a.user(&p.user).await?.ok_or(StoreError::NotFound)?.user;
    let organisation = a.organisation(&p.org).await?.ok_or(StoreError::NotFound)?;
    let (membership, subscription) = a.membership(&p.user).await?.ok_or(StoreError::NotFound)?;
    let effective_plan = plans::effective_plan(&subscription, s.now());
    Ok(Me { user, organisation, membership, subscription, effective_plan })
}

/// Starts a web session for `user` and answers with `body` and the cookie.
pub(super) async fn with_session<B: Backend>(s: &Server<B>, user: &UserId, status: StatusCode, body: Me) -> Result<Response> {
    let token = new_token(s, TokenKind::Session)?;
    let life = TokenKind::Session.lifetime_secs();
    let rec = TokenRecord {
        kind: TokenKind::Session,
        digest: token.digest(),
        user: user.clone(),
        device: None,
        invite: None,
        expires: s.now().saturating_add(life),
    };
    s.backend.accounts().put_token(rec).await?;
    let cookie = http::session_cookie(token.expose(), life, s.settings.secure_cookies);
    Ok((status, [(header::SET_COOKIE, cookie)], Json(body)).into_response())
}

/// Emails a single-use link (verification, reset). A mail failure is logged, not shown: the person
/// can ask again.
async fn email_link<B: Backend>(s: &Server<B>, kind: TokenKind, user: &User, path: &str) -> Result<()> {
    let token = new_token(s, kind)?;
    let rec = TokenRecord {
        kind,
        digest: token.digest(),
        user: user.id.clone(),
        device: None,
        invite: None,
        expires: s.now().saturating_add(kind.lifetime_secs()),
    };
    s.backend.accounts().put_token(rec).await?;
    let link = format!("{}{path}?token={}", s.settings.public_url, token.expose());
    let email = match kind {
        TokenKind::PasswordReset => Email::ResetPassword { link },
        _ => Email::VerifyAddress { link },
    };
    if let Err(e) = s.backend.mail().send(&user.email, &user.name, email).await {
        log::warn!("mail to {} failed: {e}", user.id);
    }
    Ok(())
}

pub async fn sign_up<B: Backend>(State(s): State<Arc<Server<B>>>, ctx: Ctx, body: Bytes) -> Result<Response> {
    limit(&s, &format!("signup-ip:{}", ctx.ip), SIGN_UPS_PER_IP)?;
    let req: SignUp = parse(&body)?;
    let mut fields = Vec::new();
    let email = normalize_email(&req.email).map_err(|e| fields.push(e)).unwrap_or_default();
    let name = check_name("name", &req.name).map_err(|e| fields.push(e)).unwrap_or_default();
    let org_name = check_name("organisation", &req.organisation).map_err(|e| fields.push(e)).unwrap_or_default();
    if let Err(e) = check_password(&req.password, &email) {
        fields.push(e);
    }
    if !fields.is_empty() {
        return Err(ApiError::fields(fields).into());
    }
    let hasher = s.hasher.clone();
    let password = req.password;
    let hash = http::blocking(move || hasher.hash(&password)).await?.map_err(|e| internal("hash", e))?;
    let now = s.now();
    let r = s.random.as_ref();
    let org_id = OrgId::from_random(random_bytes(r).map_err(|e| internal("random", e))?);
    let user_id = UserId::from_random(random_bytes(r).map_err(|e| internal("random", e))?);
    let owner = User { id: user_id.clone(), email, name, email_verified: false, created: now };
    let new = NewOrganisation {
        org: Organisation { id: org_id.clone(), name: org_name, created: now },
        owner: owner.clone(),
        password_hash: hash,
        subscription: plans::trial(org_id, now),
    };
    match s.backend.accounts().create_organisation(new).await {
        Err(StoreError::Conflict(_)) => return Err(bad(ErrorCode::Conflict, "An account with this email address already exists.")),
        other => other?,
    }
    email_link(&s, TokenKind::EmailVerify, &owner, "/verify-email").await?;
    let p = principal(&s, &user_id).await?;
    let me = me_for(&s, &p).await?;
    with_session(&s, &user_id, StatusCode::CREATED, me).await
}

/// The principal for a person just signed in.
pub(super) async fn principal<B: Backend>(s: &Server<B>, user: &UserId) -> Result<Principal> {
    let (m, sub) = s.backend.accounts().membership(user).await?.ok_or(StoreError::NotFound)?;
    Ok(Principal::new(user.clone(), m.role, geoflux_cloud::auth::AuthVia::Session, sub))
}

fn invalid_credentials() -> Fail {
    bad(ErrorCode::InvalidCredentials, "The email address or password is wrong.")
}

pub async fn log_in<B: Backend>(State(s): State<Arc<Server<B>>>, ctx: Ctx, body: Bytes) -> Result<Response> {
    limit(&s, &format!("login-ip:{}", ctx.ip), limits::LOGIN_PER_IP)?;
    let req: LogIn = parse(&body)?;
    let email = normalize_email(&req.email).map_err(|_| invalid_credentials())?;
    limit(&s, &format!("login:{email}"), limits::LOGIN_PER_EMAIL)?;
    let rec = s.backend.accounts().user_by_email(&email).await?;
    let hasher = s.hasher.clone();
    let stored = rec.as_ref().and_then(|r| r.password_hash.clone());
    let password = req.password;
    // An unknown address costs the same hashing time as a known one, so timing doesn't tell
    // which addresses have accounts.
    let ok = http::blocking(move || match stored {
        Some(h) => hasher.verify(&password, &h),
        None => {
            let _ = hasher.hash(&password);
            false
        }
    })
    .await?;
    let Some(rec) = rec.filter(|r| ok && !r.disabled) else { return Err(invalid_credentials()) };
    let p = principal(&s, &rec.user.id).await?;
    let me = me_for(&s, &p).await?;
    with_session(&s, &rec.user.id, StatusCode::OK, me).await
}

pub async fn log_out<B: Backend>(State(s): State<Arc<Server<B>>>, ctx: Ctx) -> Result<Response> {
    let (_, digest) = http::web(&s, &ctx.method, &ctx.headers).await?;
    s.backend.accounts().delete_token(TokenKind::Session, &digest).await?;
    let clear = http::session_cookie("", 0, s.settings.secure_cookies);
    Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, clear)]).into_response())
}

pub async fn forgot_password<B: Backend>(State(s): State<Arc<Server<B>>>, ctx: Ctx, body: Bytes) -> Result<StatusCode> {
    limit(&s, &format!("reset-ip:{}", ctx.ip), limits::LOGIN_PER_IP)?;
    let req: ForgotPassword = parse(&body)?;
    // Same answer whether or not the address has an account.
    let Ok(email) = normalize_email(&req.email) else { return Ok(StatusCode::ACCEPTED) };
    limit(&s, &format!("reset:{email}"), limits::RESET_PER_EMAIL)?;
    if let Some(rec) = s.backend.accounts().user_by_email(&email).await?.filter(|r| !r.disabled) {
        email_link(&s, TokenKind::PasswordReset, &rec.user, "/reset-password").await?;
    }
    Ok(StatusCode::ACCEPTED)
}

fn link_expired() -> Fail {
    bad(ErrorCode::ExpiredToken, "This link has expired or was already used. Ask for a new one.")
}

pub async fn reset_password<B: Backend>(State(s): State<Arc<Server<B>>>, ctx: Ctx, body: Bytes) -> Result<StatusCode> {
    limit(&s, &format!("reset-ip:{}", ctx.ip), limits::LOGIN_PER_IP)?;
    let req: ResetPassword = parse(&body)?;
    let token = SecretToken::parse(TokenKind::PasswordReset, &req.token).ok_or_else(link_expired)?;
    let a = s.backend.accounts();
    // Checked against the person's address before the link is used up, so a weak password can be
    // fixed with the same link.
    let rec = a.token(TokenKind::PasswordReset, &token.digest(), s.now()).await?.ok_or_else(link_expired)?;
    let user = a.user(&rec.user).await?.ok_or_else(link_expired)?;
    check_password(&req.password, &user.user.email).map_err(|e| Fail(ApiError::fields(vec![e])))?;
    a.take_token(TokenKind::PasswordReset, &token.digest(), s.now()).await?.ok_or_else(link_expired)?;
    let hasher = s.hasher.clone();
    let password = req.password;
    let hash = http::blocking(move || hasher.hash(&password)).await?.map_err(|e| internal("hash", e))?;
    // Ends every session and computer: whoever knew the old password is out.
    a.set_password(&user.user.id, &hash, None).await?;
    // Following the emailed link proves the address.
    a.set_email_verified(&user.user.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn verify_email<B: Backend>(State(s): State<Arc<Server<B>>>, body: Bytes) -> Result<StatusCode> {
    let req: VerifyEmail = parse(&body)?;
    let token = SecretToken::parse(TokenKind::EmailVerify, &req.token).ok_or_else(link_expired)?;
    let a = s.backend.accounts();
    let rec = a.take_token(TokenKind::EmailVerify, &token.digest(), s.now()).await?.ok_or_else(link_expired)?;
    a.set_email_verified(&rec.user).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn me<B: Backend>(State(s): State<Arc<Server<B>>>, ctx: Ctx) -> Result<Json<Me>> {
    let p = http::any(&s, &ctx.method, &ctx.headers).await?;
    Ok(Json(me_for(&s, &p).await?))
}
