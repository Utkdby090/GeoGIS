//! The routes of `geoflux_cloud::api::routes`, wired to their handlers.

mod auth;
mod device;
mod org;
mod surveys;
mod view;

use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, patch, post, put};
use geoflux_cloud::api::routes as r;
use geoflux_cloud::sync::MAX_MANIFEST_BYTES;

use crate::{Backend, Server};

/// JSON bodies other than a survey manifest are small.
const MAX_BODY_BYTES: usize = 64 * 1024;

pub fn router<B: Backend>(server: Arc<Server<B>>) -> Router {
    Router::new()
        .route(r::SIGN_UP.1, post(auth::sign_up::<B>))
        .route(r::LOG_IN.1, post(auth::log_in::<B>))
        .route(r::LOG_OUT.1, post(auth::log_out::<B>))
        .route(r::FORGOT_PASSWORD.1, post(auth::forgot_password::<B>))
        .route(r::RESET_PASSWORD.1, post(auth::reset_password::<B>))
        .route(r::VERIFY_EMAIL.1, post(auth::verify_email::<B>))
        .route(r::ME.1, get(auth::me::<B>))
        .route(r::DEVICE_CODE.1, post(device::code::<B>))
        .route(r::DEVICE_TOKEN.1, post(device::token::<B>))
        .route(r::DEVICE_RENEW.1, post(device::renew::<B>))
        .route(r::DEVICE_SIGN_OUT.1, post(device::sign_out::<B>))
        .route(r::DEVICE_APPROVE.1, post(device::approve::<B>))
        .route(r::DEVICES.1, get(device::list::<B>))
        .route(r::REVOKE_DEVICE.1, delete(device::revoke::<B>))
        .route(r::OFFLINE_PASS.1, post(device::offline_pass::<B>))
        .route(r::MEMBERS.1, get(org::members::<B>))
        .route(r::INVITE.1, post(org::invite::<B>))
        .route(r::ACCEPT_INVITE.1, post(org::accept_invite::<B>))
        .route(r::SET_ROLE.1, patch(org::set_role::<B>))
        .route(r::REMOVE_MEMBER.1, delete(org::remove_member::<B>))
        .route(r::CREATE_SURVEY.1, post(surveys::create::<B>).layer(DefaultBodyLimit::max(MAX_MANIFEST_BYTES)))
        // Streamed to disk with its own limit (the photo's declared size).
        .route(r::PUT_PHOTO.1, put(surveys::put_photo::<B>).layer(DefaultBodyLimit::disable()))
        .route(r::COMMIT_SURVEY.1, post(surveys::commit::<B>))
        .route(r::SURVEYS.1, get(surveys::list::<B>))
        .route(r::SURVEY.1, get(view::survey::<B>))
        .route(r::SURVEY_TOWERS.1, get(view::towers::<B>))
        .route(r::SURVEY_ASSET.1, get(view::asset::<B>))
        .route(r::SURVEY_HEALTH.1, get(view::line_health::<B>))
        .route(r::TOWER_HEALTH.1, get(view::tower_health::<B>))
        // GET beside the upload's PUT on the same path.
        .route(r::PHOTO.1, get(view::photo::<B>))
        .route(r::TOWER_REPORT.1, get(view::tower_report::<B>))
        .route(r::LINE_REPORT.1, get(view::line_report::<B>))
        .route(r::SCHEMA.1, get(view::schema))
        .route(r::DEVICE_PENDING.1, get(view::pending_device::<B>))
        .route(r::HEALTH.1, get(health))
        // Everything else is the web app (or the API's "not found").
        .fallback(crate::web::serve)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(server)
}

async fn health() -> axum::Json<geoflux_cloud::api::Health> {
    axum::Json(geoflux_cloud::api::Health { ok: true, version: env!("CARGO_PKG_VERSION").to_string() })
}
