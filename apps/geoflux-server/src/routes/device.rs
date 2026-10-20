//! GeoGIS's sign-in (RFC 8628 device code), renewal and sign-out, the account page's approval,
//! the computer list, and offline pass files.

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use geoflux_cloud::api::{
    DeviceApprove, DeviceCodeRequest, DeviceCodeResponse, DeviceGrant, DeviceList, DeviceRenew, DeviceSignOut, DeviceTokenRequest,
    OfflinePass, OfflinePassRequest,
};
use geoflux_cloud::auth::{SecretToken, TokenKind, UserCode, check_name};
use geoflux_cloud::model::{Device, Role, Subscription, SubscriptionState};
use geoflux_cloud::plans::{self, DEVICES_PER_SEAT, PassSubject, Permission};
use geoflux_cloud::ports::{AccountStore, DeviceCodeRecord, DeviceCodeState, DeviceStore, TokenRecord, limits};
use geoflux_cloud::{ApiError, DeviceId, ErrorCode, UserId};
use geoflux_licence::{MachineId, PassKind, Plan};

use crate::crypto::random_bytes;
use crate::http::{self, Ctx, Fail, Result, bad, internal, limit, new_token, parse};
use crate::{Backend, Server};

/// Seconds GeoGIS waits between polls (RFC 8628 default).
const POLL_SECS: u64 = 5;
const DAY: i64 = 86_400;

fn machine(text: &str) -> Result<MachineId> {
    MachineId::from_output(text).map_err(|_| Fail(ApiError::fields(vec![geoflux_cloud::FieldError::new("machine", "invalid")])))
}

fn revoked() -> Fail {
    bad(ErrorCode::Revoked, "This computer is no longer signed in. Sign in again.")
}

/// A pass, a fresh access token and the given refresh token for `device`.
async fn grant<B: Backend>(
    s: &Server<B>,
    user: &UserId,
    role: Role,
    sub: &Subscription,
    device: &Device,
    refresh: String,
) -> Result<DeviceGrant> {
    let a = s.backend.accounts();
    let rec = a.user(user).await?.ok_or_else(revoked)?;
    let now = s.now();
    let claims = plans::online_pass(sub, role, PassSubject { user, display_name: &rec.user.name, machine: &device.machine }, now)?;
    let pass = claims.sign(s.signer.as_ref()).map_err(|e| internal("sign pass", e))?;
    let access = new_token(s, TokenKind::Access)?;
    let life = TokenKind::Access.lifetime_secs();
    a.put_token(TokenRecord {
        kind: TokenKind::Access,
        digest: access.digest(),
        user: user.clone(),
        device: Some(device.id.clone()),
        invite: None,
        expires: now.saturating_add(life),
    })
    .await?;
    Ok(DeviceGrant {
        pass,
        refresh_token: refresh,
        access_token: access.expose().to_string(),
        access_expires_in: u64::try_from(life).unwrap_or(0),
        device: device.id.clone(),
    })
}

pub async fn code<B: Backend>(State(s): State<Arc<Server<B>>>, ctx: Ctx, body: Bytes) -> Result<Json<DeviceCodeResponse>> {
    limit(&s, &format!("device-code:{}", ctx.ip), limits::DEVICE_CODES_PER_IP)?;
    let req: DeviceCodeRequest = parse(&body)?;
    let m = machine(&req.machine)?;
    let name = check_name("deviceName", &req.device_name).map_err(|e| Fail(ApiError::fields(vec![e])))?;
    let now = s.now();
    let life = TokenKind::DeviceCode.lifetime_secs();
    let token = new_token(&s, TokenKind::DeviceCode)?;
    // A user code clash among live codes is near impossible; try a few times anyway.
    let mut stored = None;
    for _ in 0..3 {
        let user_code = UserCode::from_random(random_bytes(s.random.as_ref()).map_err(|e| internal("random", e))?);
        let rec = DeviceCodeRecord {
            digest: token.digest(),
            user_code: user_code.clone(),
            machine: m.as_str().to_string(),
            device_name: name.clone(),
            state: DeviceCodeState::Pending,
            interval_secs: POLL_SECS,
            last_poll: 0,
            expires: now.saturating_add(life),
        };
        match s.backend.devices().put_device_code(rec).await {
            Ok(()) => {
                stored = Some(user_code);
                break;
            }
            Err(geoflux_cloud::ports::StoreError::Conflict(_)) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    let user_code = stored.ok_or_else(|| internal("device code", "user code clashes"))?;
    let page = format!("{}/device", s.settings.public_url);
    Ok(Json(DeviceCodeResponse {
        device_code: token.expose().to_string(),
        user_code: user_code.as_str().to_string(),
        verification_uri_complete: format!("{page}?code={}", user_code.as_str()),
        verification_uri: page,
        expires_in: u64::try_from(life).unwrap_or(0),
        interval: POLL_SECS,
    }))
}

pub async fn token<B: Backend>(State(s): State<Arc<Server<B>>>, body: Bytes) -> Result<Json<DeviceGrant>> {
    let req: DeviceTokenRequest = parse(&body)?;
    let expired = || bad(ErrorCode::ExpiredToken, "The sign-in code has run out. Start again.");
    let token = SecretToken::parse(TokenKind::DeviceCode, &req.device_code).ok_or_else(expired)?;
    let digest = token.digest();
    let d = s.backend.devices();
    let now = s.now();
    let rec = d.device_code(&digest).await?.filter(|c| c.expires > now).ok_or_else(expired)?;
    let since = now.saturating_sub(rec.last_poll);
    match &rec.state {
        DeviceCodeState::Pending => {
            // Records the poll; a lost compare-and-set means it changed meanwhile, so it's still
            // worth asking again.
            d.set_device_code_state(&digest, &DeviceCodeState::Pending, DeviceCodeState::Pending, now).await?;
            if since < i64::try_from(rec.interval_secs).unwrap_or(5) {
                let mut e = ApiError::new(ErrorCode::SlowDown, "Polling too often.");
                e.retry_after = Some(rec.interval_secs + 5);
                return Err(e.into());
            }
            Err(bad(ErrorCode::AuthorizationPending, "Waiting for approval on the account page."))
        }
        DeviceCodeState::Denied => Err(bad(ErrorCode::AccessDenied, "The sign-in was declined.")),
        DeviceCodeState::Collected => Err(expired()),
        DeviceCodeState::Approved(user) => {
            // Collected exactly once, even if two polls arrive together.
            if !d.set_device_code_state(&digest, &rec.state, DeviceCodeState::Collected, now).await? {
                return Err(expired());
            }
            let user = user.clone();
            let (m, sub) = s.backend.accounts().membership(&user).await?.ok_or_else(revoked)?;
            if !m.role.is_desktop() {
                return Err(ApiError::forbidden().into());
            }
            let device = match d.claim_seat(&sub.org, &user, &rec.machine, &rec.device_name, DEVICES_PER_SEAT, now).await? {
                Ok(dev) => dev,
                Err(full) => return Err(ApiError::seat_limit(full.devices).into()),
            };
            let refresh = new_token(&s, TokenKind::Refresh)?;
            s.backend
                .accounts()
                .put_token(TokenRecord {
                    kind: TokenKind::Refresh,
                    digest: refresh.digest(),
                    user: user.clone(),
                    device: Some(device.id.clone()),
                    invite: None,
                    expires: now.saturating_add(TokenKind::Refresh.lifetime_secs()),
                })
                .await?;
            Ok(Json(grant(&s, &user, m.role, &sub, &device, refresh.expose().to_string()).await?))
        }
    }
}

pub async fn approve<B: Backend>(State(s): State<Arc<Server<B>>>, ctx: Ctx, body: Bytes) -> Result<StatusCode> {
    let (p, _) = http::web(&s, &ctx.method, &ctx.headers).await?;
    limit(&s, &format!("user-code:{}", ctx.ip), limits::USER_CODE_PER_IP)?;
    let req: DeviceApprove = parse(&body)?;
    if !p.role.is_desktop() {
        return Err(ApiError::forbidden().into());
    }
    let user = s.backend.accounts().user(&p.user).await?.ok_or_else(revoked)?;
    if !user.user.email_verified {
        return Err(bad(ErrorCode::EmailNotVerified, "Confirm your email address first; we sent you a link."));
    }
    let not_found = || bad(ErrorCode::NotFound, "That code is not valid or has run out.");
    let code = UserCode::parse(&req.user_code).ok_or_else(not_found)?;
    let now = s.now();
    let d = s.backend.devices();
    let rec = d.device_code_by_user_code(&code, now).await?.filter(|c| c.state == DeviceCodeState::Pending).ok_or_else(not_found)?;
    let to = if req.approve { DeviceCodeState::Approved(p.user.clone()) } else { DeviceCodeState::Denied };
    if !d.set_device_code_state(&rec.digest, &DeviceCodeState::Pending, to, rec.last_poll).await? {
        return Err(not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn renew<B: Backend>(State(s): State<Arc<Server<B>>>, body: Bytes) -> Result<Json<DeviceGrant>> {
    let req: DeviceRenew = parse(&body)?;
    let old = SecretToken::parse(TokenKind::Refresh, &req.refresh_token).ok_or_else(revoked)?;
    let m = machine(&req.machine)?;
    let now = s.now();
    let next = new_token(&s, TokenKind::Refresh)?;
    let expires = now.saturating_add(TokenKind::Refresh.lifetime_secs());
    let device = s.backend.devices().rotate_refresh(&old.digest(), &next.digest(), expires, now).await?.ok_or_else(revoked)?;
    // A token copied to another computer: refused here, and the real computer's next renewal
    // (with the now retired token) signs this device out.
    if device.machine != m.as_str() {
        return Err(revoked());
    }
    let (mem, sub) = s.backend.accounts().membership(&device.user).await?.ok_or_else(revoked)?;
    if !mem.role.is_desktop() || mem.org != device.org {
        return Err(revoked());
    }
    Ok(Json(grant(&s, &device.user, mem.role, &sub, &device, next.expose().to_string()).await?))
}

pub async fn sign_out<B: Backend>(State(s): State<Arc<Server<B>>>, body: Bytes) -> Result<StatusCode> {
    let req: DeviceSignOut = parse(&body)?;
    if let Some(t) = SecretToken::parse(TokenKind::Refresh, &req.refresh_token) {
        s.backend.devices().sign_out(&t.digest()).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list<B: Backend>(State(s): State<Arc<Server<B>>>, ctx: Ctx) -> Result<Json<DeviceList>> {
    let (p, _) = http::web(&s, &ctx.method, &ctx.headers).await?;
    let now = s.now();
    let devices = if p.role == Role::Admin {
        let scope = p.require(Permission::ManageDevices, now)?;
        s.backend.devices().devices(&scope, None).await?
    } else {
        let scope = p.require(Permission::ViewSurveys, now)?;
        s.backend.devices().devices(&scope, Some(&p.user)).await?
    };
    Ok(Json(DeviceList { devices }))
}

pub async fn revoke<B: Backend>(State(s): State<Arc<Server<B>>>, ctx: Ctx, Path(id): Path<String>) -> Result<StatusCode> {
    let (p, _) = http::web(&s, &ctx.method, &ctx.headers).await?;
    let id = DeviceId::parse(&id).map_err(|_| Fail(ApiError::not_found()))?;
    let now = s.now();
    let d = s.backend.devices();
    if p.role == Role::Admin {
        d.revoke_device(&p.require(Permission::ManageDevices, now)?, &id).await?;
    } else {
        // Anyone may sign out their own computers.
        let scope = p.require(Permission::ViewSurveys, now)?;
        if !d.devices(&scope, Some(&p.user)).await?.iter().any(|x| x.id == id) {
            return Err(ApiError::not_found().into());
        }
        d.revoke_device(&scope, &id).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn offline_pass<B: Backend>(State(s): State<Arc<Server<B>>>, ctx: Ctx, body: Bytes) -> Result<Json<OfflinePass>> {
    let (p, _) = http::web(&s, &ctx.method, &ctx.headers).await?;
    let now = s.now();
    let scope = p.require(Permission::ManageDevices, now)?;
    let sub = p.subscription();
    if plans::effective_plan(sub, now) != Plan::Organisation {
        return Err(bad(ErrorCode::PlanRequired, "Offline pass files come with the Organisation plan."));
    }
    let req: OfflinePassRequest = parse(&body)?;
    let m = machine(&req.machine)?;
    let name = check_name("deviceName", &req.device_name).map_err(|e| Fail(ApiError::fields(vec![e])))?;
    let a = s.backend.accounts();
    let (mem, _) = a.membership(&req.user).await?.filter(|(m, _)| m.org == p.org).ok_or_else(|| Fail(ApiError::not_found()))?;
    if !mem.role.is_desktop() {
        return Err(bad(ErrorCode::Forbidden, "Viewers don't use GeoGIS, so they need no pass."));
    }
    let rec = a.user(&req.user).await?.ok_or_else(|| Fail(ApiError::not_found()))?;
    let days = i64::from(req.days.clamp(1, 400));
    let mut expires = now.saturating_add(days * DAY);
    if let SubscriptionState::Active { until } | SubscriptionState::Trial { ends: until } = sub.state {
        expires = expires.min(until);
    }
    let device = match s.backend.devices().claim_seat(&p.org, &req.user, m.as_str(), &name, DEVICES_PER_SEAT, now).await? {
        Ok(d) => d,
        Err(full) => return Err(ApiError::seat_limit(full.devices).into()),
    };
    let claims = geoflux_licence::PassClaims {
        account: req.user.to_string(),
        org: Some(p.org.to_string()),
        name: rec.user.name,
        plan: Plan::Organisation,
        features: plans::features(Plan::Organisation).to_vec(),
        machine: m.as_str().to_string(),
        kind: PassKind::Offline,
        issued: now,
        expires,
    };
    let pass = claims.sign(s.signer.as_ref()).map_err(|e| internal("sign offline pass", e))?;
    s.backend.devices().mark_offline(&scope, &device.id, expires).await?;
    Ok(Json(OfflinePass { pass, expires }))
}
