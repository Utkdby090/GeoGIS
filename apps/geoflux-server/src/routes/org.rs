//! Members and invites.

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use geoflux_cloud::api::{AcceptInvite, Invite, Member, MemberList, SetRole};
use geoflux_cloud::auth::{SecretToken, TokenKind, check_name, check_password, normalize_email};
use geoflux_cloud::model::{Role, User};
use geoflux_cloud::plans::Permission;
use geoflux_cloud::ports::{AccountStore, Email, Mailer, Seats, StoreError, TokenRecord, limits};
use geoflux_cloud::{ApiError, ErrorCode, UserId};
use geoflux_licence::Plan;

use super::auth::{me_for, principal, with_session};
use crate::crypto::random_bytes;
use crate::http::{self, Ctx, Fail, Result, bad, internal, limit, new_token, parse};
use crate::{Backend, Server};

/// Invitations an organisation may send per hour (each sends an email).
const INVITES_PER_ORG: geoflux_cloud::ports::Limit = geoflux_cloud::ports::Limit { attempts: 50, window_secs: 3_600 };

fn no_seats(role: Role) -> Fail {
    let what = if role.is_desktop() { "GeoGIS seats" } else { "viewer logins" };
    bad(ErrorCode::SeatLimit, &format!("All of your plan's {what} are in use."))
}

fn last_admin(e: StoreError) -> Fail {
    match e {
        StoreError::Conflict("last_admin") => bad(ErrorCode::Conflict, "The organisation needs at least one admin."),
        e => e.into(),
    }
}

pub async fn members<B: Backend>(State(s): State<Arc<Server<B>>>, ctx: Ctx) -> Result<Json<MemberList>> {
    let (p, _) = http::web(&s, &ctx.method, &ctx.headers).await?;
    let scope = p.require(Permission::ManageMembers, s.now())?;
    let members =
        s.backend.accounts().members(&scope).await?.into_iter().map(|(user, m)| Member { user, role: m.role, joined: m.joined }).collect();
    let sub = p.subscription();
    Ok(Json(MemberList { members, seats: sub.seats, viewer_seats: sub.viewer_seats }))
}

pub async fn invite<B: Backend>(State(s): State<Arc<Server<B>>>, ctx: Ctx, body: Bytes) -> Result<StatusCode> {
    let (p, _) = http::web(&s, &ctx.method, &ctx.headers).await?;
    let now = s.now();
    p.require(Permission::ManageMembers, now)?;
    let req: Invite = parse(&body)?;
    let email = normalize_email(&req.email).map_err(|e| Fail(ApiError::fields(vec![e])))?;
    if req.role == Role::Viewer && geoflux_cloud::plans::effective_plan(p.subscription(), now) != Plan::Organisation {
        return Err(bad(ErrorCode::PlanRequired, "Viewer logins come with the Organisation plan."));
    }
    limit(&s, &format!("invite:{}", p.org), INVITES_PER_ORG)?;
    let a = s.backend.accounts();
    if a.user_by_email(&email).await?.is_some() {
        return Err(bad(ErrorCode::Conflict, "This person already has an account."));
    }
    let token = new_token(&s, TokenKind::Invite)?;
    a.put_token(TokenRecord {
        kind: TokenKind::Invite,
        digest: token.digest(),
        user: p.user.clone(),
        device: None,
        invite: Some((p.org.clone(), req.role, email.clone())),
        expires: now.saturating_add(TokenKind::Invite.lifetime_secs()),
    })
    .await?;
    let inviter = a.user(&p.user).await?.map(|u| u.user.name).unwrap_or_default();
    let organisation = a.organisation(&p.org).await?.map(|o| o.name).unwrap_or_default();
    let link = format!("{}/invite?token={}", s.settings.public_url, token.expose());
    if let Err(e) = s.backend.mail().send(&email, "", Email::Invite { organisation, inviter, link }).await {
        log::warn!("invite mail failed: {e}");
    }
    Ok(StatusCode::ACCEPTED)
}

pub async fn accept_invite<B: Backend>(State(s): State<Arc<Server<B>>>, ctx: Ctx, body: Bytes) -> Result<Response> {
    limit(&s, &format!("invite-ip:{}", ctx.ip), limits::LOGIN_PER_IP)?;
    let req: AcceptInvite = parse(&body)?;
    let expired = || bad(ErrorCode::ExpiredToken, "This invitation has expired or was already used.");
    let token = SecretToken::parse(TokenKind::Invite, &req.token).ok_or_else(expired)?;
    let a = s.backend.accounts();
    let now = s.now();
    let rec = a.token(TokenKind::Invite, &token.digest(), now).await?.ok_or_else(expired)?;
    let Some((org, role, email)) = rec.invite.clone() else { return Err(expired()) };
    let mut fields = Vec::new();
    let name = check_name("name", &req.name).map_err(|e| fields.push(e)).unwrap_or_default();
    if let Err(e) = check_password(&req.password, &email) {
        fields.push(e);
    }
    if !fields.is_empty() {
        return Err(ApiError::fields(fields).into());
    }
    let (_, sub) = a.membership(&rec.user).await?.ok_or_else(expired)?;
    a.take_token(TokenKind::Invite, &token.digest(), now).await?.ok_or_else(expired)?;
    let hasher = s.hasher.clone();
    let password = req.password;
    let hash = http::blocking(move || hasher.hash(&password)).await?.map_err(|e| internal("hash", e))?;
    let id = UserId::from_random(random_bytes(s.random.as_ref()).map_err(|e| internal("random", e))?);
    // The invitation link went to this address, which proves it.
    let user = User { id: id.clone(), email, name, email_verified: true, created: now };
    match a.add_member(&org, user, role, &hash, Seats::of(&sub), now).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) => return Err(no_seats(role)),
        Err(StoreError::Conflict(_)) => return Err(bad(ErrorCode::Conflict, "This email address already has an account.")),
        Err(e) => return Err(e.into()),
    }
    let p = principal(&s, &id).await?;
    let me = me_for(&s, &p).await?;
    with_session(&s, &id, StatusCode::CREATED, me).await
}

pub async fn set_role<B: Backend>(State(s): State<Arc<Server<B>>>, ctx: Ctx, Path(id): Path<String>, body: Bytes) -> Result<StatusCode> {
    let (p, _) = http::web(&s, &ctx.method, &ctx.headers).await?;
    let scope = p.require(Permission::ManageMembers, s.now())?;
    let id = UserId::parse(&id).map_err(|_| Fail(ApiError::not_found()))?;
    let req: SetRole = parse(&body)?;
    match s.backend.accounts().set_role(&scope, &id, req.role, Seats::of(p.subscription())).await.map_err(last_admin)? {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(_) => Err(no_seats(req.role)),
    }
}

pub async fn remove_member<B: Backend>(State(s): State<Arc<Server<B>>>, ctx: Ctx, Path(id): Path<String>) -> Result<StatusCode> {
    let (p, _) = http::web(&s, &ctx.method, &ctx.headers).await?;
    let scope = p.require(Permission::ManageMembers, s.now())?;
    let id = UserId::parse(&id).map_err(|_| Fail(ApiError::not_found()))?;
    s.backend.accounts().remove_member(&scope, &id).await.map_err(last_admin)?;
    Ok(StatusCode::NO_CONTENT)
}
