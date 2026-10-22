//! End to end through the HTTP router: what GeoGIS and the web app do. Each flow runs on the
//! in-memory backend, and on PostgreSQL too when `GEOFLUX_TEST_DATABASE_URL` names a database the
//! tests may create databases next to (CI sets it; locally the PostgreSQL runs are skipped without it).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use geoflux_cloud::model::{Billing, Subscription, SubscriptionState};
use geoflux_cloud::ports::{AccountStore, Clock, Email};
use geoflux_cloud::sync::{MANIFEST_FORMAT, PhotoRef, PhotoType, SurveyAsset, SurveyManifest};
use geoflux_cloud::{OrgId, Sha256};
use geoflux_domain::{Asset, AssetDraft, GeoPoint};
use geoflux_licence::{Ed25519Keys, Ed25519Signer, Feature, MachineId, Pass, PassStatus, Plan};
use geoflux_server::blobs::DiskBlobs;
use geoflux_server::crypto::{Argon2Hasher, MemoryRateLimiter, OsRandom};
use geoflux_server::memory::MemoryBackend;
use geoflux_server::postgres::{PgBackend, PgStore};
use geoflux_server::{Backend, Server, Settings, router};

#[path = "common/pg.rs"]
mod pg;
use serde_json::{Value, json};
use tower::ServiceExt;

const T0: i64 = 1_792_108_800;
const DAY: i64 = 86_400;

struct TestClock(AtomicI64);

impl Clock for TestClock {
    fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// A server on backend `B`, which records mail in memory so tests can follow emailed links.
struct App<B: Backend<Mail = MemoryBackend>> {
    server: Arc<Server<B>>,
    clock: Arc<TestClock>,
    keys: Ed25519Keys,
    upload_dir: tempdir::Dir,
    _db: Option<pg::TestDb>,
}

mod tempdir {
    /// A folder removed when the test ends.
    pub struct Dir(pub std::path::PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

struct Res {
    status: StatusCode,
    cookie: Option<String>,
    body: Value,
}

fn memory() -> App<MemoryBackend> {
    App::new(MemoryBackend::default(), None)
}

/// `None` (and the test returns early) without `GEOFLUX_TEST_DATABASE_URL`.
async fn postgres() -> Option<App<PgBackend<DiskBlobs, MemoryBackend>>> {
    let db = pg::TestDb::create().await?;
    let store = PgStore::from_pool(db.pool().await);
    store.migrate().await.unwrap();
    let photos = db.photos_dir();
    Some(App::new(PgBackend { store, blobs: DiskBlobs::new(photos), mail: MemoryBackend::default() }, Some(db)))
}

impl<B: Backend<Mail = MemoryBackend>> App<B> {
    fn new(backend: B, db: Option<pg::TestDb>) -> App<B> {
        let signer = Ed25519Signer::from_pkcs8("t1", &Ed25519Signer::generate_pkcs8().unwrap()).unwrap();
        let keys = Ed25519Keys::from_base64(&[("t1", &signer.public_key_base64())]).unwrap();
        let clock = Arc::new(TestClock(AtomicI64::new(T0)));
        let mut n = [0u8; 8];
        getrandom_fill(&mut n);
        let dir = std::env::temp_dir().join(format!("geoflux-server-test-{}", u64::from_le_bytes(n)));
        let server = Arc::new(Server {
            backend,
            hasher: Arc::new(Argon2Hasher::fast().unwrap()),
            random: Arc::new(OsRandom),
            limiter: Arc::new(MemoryRateLimiter::default()),
            clock: clock.clone(),
            signer: Arc::new(signer),
            reports: Arc::new(geoflux_server::reports::Reports),
            report_slots: Arc::new(tokio::sync::Semaphore::new(geoflux_server::reports::REPORTS_AT_ONCE)),
            settings: Settings {
                public_url: "https://cloud.test".into(),
                upload_dir: dir.clone(),
                secure_cookies: true,
                trust_proxy: false,
            },
        });
        App { server, clock, keys, upload_dir: tempdir::Dir(dir), _db: db }
    }

    fn advance(&self, secs: i64) {
        self.clock.0.fetch_add(secs, Ordering::SeqCst);
    }

    async fn send(&self, method: &str, uri: &str, auth: Auth<'_>, body: Body) -> Res {
        let mut b = Request::builder().method(method).uri(uri);
        match auth {
            Auth::None => {}
            Auth::Web(c) => b = b.header(header::COOKIE, format!("gf_session={c}")).header("x-geoflux-csrf", "1"),
            Auth::WebNoCsrf(c) => b = b.header(header::COOKIE, format!("gf_session={c}")),
            Auth::Bearer(t) => b = b.header(header::AUTHORIZATION, format!("Bearer {t}")),
        }
        let res = router(self.server.clone()).oneshot(b.body(body).unwrap()).await.unwrap();
        let status = res.status();
        let cookie = res
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("gf_session="))
            .map(|v| v.split(';').next().unwrap_or_default().to_string());
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let body = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
        Res { status, cookie, body }
    }

    /// A file response: status, headers and bytes.
    async fn raw(&self, uri: &str, auth: Auth<'_>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let mut b = Request::builder().method("GET").uri(uri);
        if let Auth::Web(c) = auth {
            b = b.header(header::COOKIE, format!("gf_session={c}"));
        }
        let res = router(self.server.clone()).oneshot(b.body(Body::empty()).unwrap()).await.unwrap();
        let (parts, body) = res.into_parts();
        let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        (parts.status, parts.headers, bytes.to_vec())
    }

    async fn json(&self, method: &str, uri: &str, auth: Auth<'_>, body: Value) -> Res {
        self.send(method, uri, auth, Body::from(body.to_string())).await
    }

    fn mail_token(&self, to: &str) -> String {
        let (_, email) = self.server.backend.mail().sent_mail().into_iter().rev().find(|(t, _)| t == to).expect("mail");
        let link = match email {
            Email::VerifyAddress { link } | Email::ResetPassword { link } | Email::Invite { link, .. } => link,
        };
        link.split("token=").nth(1).unwrap().to_string()
    }

    /// Signs up, verifies the address, returns the session.
    async fn sign_up(&self, email: &str) -> String {
        let r = self
            .json(
                "POST",
                "/api/v1/auth/signup",
                Auth::None,
                json!({"email": email, "password": "correct horse battery", "name": "Asha Verma", "organisation": "Bihar Grid Surveys"}),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
        let token = self.mail_token(email);
        let v = self.json("POST", "/api/v1/auth/email/verify", Auth::None, json!({"token": token})).await;
        assert_eq!(v.status, StatusCode::NO_CONTENT);
        r.cookie.unwrap()
    }

    /// GeoGIS signs in on `machine`, approved from the web session. Returns the grant.
    async fn desktop_sign_in(&self, session: &str, machine: &str) -> Res {
        let c = self.json("POST", "/api/v1/device/code", Auth::None, json!({"machine": machine, "deviceName": "Field laptop"})).await;
        assert_eq!(c.status, StatusCode::OK, "{}", c.body);
        let code = c.body["deviceCode"].as_str().unwrap().to_string();
        let a =
            self.json("POST", "/api/v1/device/approve", Auth::Web(session), json!({"userCode": c.body["userCode"], "approve": true})).await;
        assert_eq!(a.status, StatusCode::NO_CONTENT, "{}", a.body);
        self.json("POST", "/api/v1/device/token", Auth::None, json!({"deviceCode": code})).await
    }
}

fn getrandom_fill(b: &mut [u8]) {
    use geoflux_cloud::ports::Random;
    OsRandom.fill(b).unwrap();
}

#[derive(Clone, Copy)]
enum Auth<'a> {
    None,
    Web(&'a str),
    WebNoCsrf(&'a str),
    Bearer(&'a str),
}

fn jpeg(tag: &[u8]) -> Vec<u8> {
    let mut v = vec![0xff, 0xd8, 0xff, 0xe0];
    v.extend_from_slice(tag);
    v
}

fn manifest(photos: &[(&str, &[u8])]) -> SurveyManifest {
    let draft = AssetDraft { location: Some(GeoPoint { lat: 25.6, lon: 85.1 }), properties: Default::default() };
    let asset = Asset::from_draft("T-001", "com.geo.telecom", 0, draft);
    SurveyManifest {
        format: MANIFEST_FORMAT,
        client_survey_id: "proj-0001-abcd".into(),
        revision: 1,
        project: "Patna 132 kV".into(),
        line: Some("Line 4".into()),
        schema_id: "com.geo.telecom".into(),
        surveyed: None,
        assets: vec![SurveyAsset {
            asset,
            photos: photos
                .iter()
                .map(|(slot, b)| PhotoRef {
                    slot: (*slot).into(),
                    sha256: Sha256::of(b),
                    bytes: b.len() as u64,
                    kind: PhotoType::Jpeg,
                    file_name: format!("{slot}.jpg"),
                    markup: None,
                })
                .collect(),
        }],
    }
}

#[tokio::test]
async fn sign_up_sign_in_on_geogis_and_upload_a_survey() {
    sign_up_sign_in_on_geogis_and_upload_a_survey_on(memory()).await;
}

#[tokio::test]
async fn sign_up_sign_in_on_geogis_and_upload_a_survey_on_postgres() {
    if let Some(app) = postgres().await {
        sign_up_sign_in_on_geogis_and_upload_a_survey_on(app).await;
    }
}

async fn sign_up_sign_in_on_geogis_and_upload_a_survey_on<B: Backend<Mail = MemoryBackend>>(app: App<B>) {
    let session = app.sign_up("asha@example.in").await;
    let me = app.send("GET", "/api/v1/me", Auth::Web(&session), Body::empty()).await;
    assert_eq!(me.body["effectivePlan"], "pro");
    assert_eq!(me.body["membership"]["role"], "admin");
    assert_eq!(me.body["user"]["emailVerified"], true);

    // GeoGIS: the code is pending until approved, and polling too fast is slowed down.
    let c = app.json("POST", "/api/v1/device/code", Auth::None, json!({"machine": "test-machine-id", "deviceName": "Laptop"})).await;
    let code = c.body["deviceCode"].as_str().unwrap().to_string();
    assert!(c.body["verificationUriComplete"].as_str().unwrap().starts_with("https://cloud.test/device?code="));
    let poll = app.json("POST", "/api/v1/device/token", Auth::None, json!({"deviceCode": code})).await;
    assert_eq!(poll.body["error"], "authorization_pending");
    let poll = app.json("POST", "/api/v1/device/token", Auth::None, json!({"deviceCode": code})).await;
    assert_eq!(poll.body["error"], "slow_down");
    let a = app.json("POST", "/api/v1/device/approve", Auth::Web(&session), json!({"userCode": c.body["userCode"], "approve": true})).await;
    assert_eq!(a.status, StatusCode::NO_CONTENT);
    let g = app.json("POST", "/api/v1/device/token", Auth::None, json!({"deviceCode": code})).await;
    assert_eq!(g.status, StatusCode::OK, "{}", g.body);
    // Collected once only.
    let again = app.json("POST", "/api/v1/device/token", Auth::None, json!({"deviceCode": code})).await;
    assert_eq!(again.body["error"], "expired_token");

    // The pass the server signed is one GeoGIS accepts: Pro trial, 15 days, this computer.
    let pass = Pass::verify(g.body["pass"].as_str().unwrap(), &app.keys).unwrap();
    let machine = MachineId::from_output("test-machine-id").unwrap();
    assert_eq!(pass.plan, Plan::Pro);
    assert!(pass.allows(Feature::CloudSync));
    assert_eq!(pass.status(&machine, T0, None), PassStatus::Valid { days_left: 15 });

    // Project › Sync.
    let access = g.body["accessToken"].as_str().unwrap().to_string();
    let (one, two) = (jpeg(b"one"), jpeg(b"two"));
    let m = manifest(&[("thermal", &one), ("structure", &two)]);
    let created = app.json("POST", "/api/v1/surveys", Auth::Bearer(&access), serde_json::to_value(&m).unwrap()).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    assert_eq!(created.body["missing"].as_array().unwrap().len(), 2);
    let id = created.body["survey"].as_str().unwrap().to_string();
    // A retry returns the same survey.
    let retry = app.json("POST", "/api/v1/surveys", Auth::Bearer(&access), serde_json::to_value(&m).unwrap()).await;
    assert_eq!((retry.status, retry.body["survey"].as_str()), (StatusCode::OK, Some(id.as_str())));

    // Not committed while a photo is missing.
    let early = app.send("POST", &format!("/api/v1/surveys/{id}/commit"), Auth::Bearer(&access), Body::empty()).await;
    assert_eq!(early.body["error"], "incomplete");
    // Wrong bytes for the hash, a PNG for a JPEG, too long: all refused.
    let h1 = Sha256::of(&one);
    let url = |h: &Sha256| format!("/api/v1/surveys/{id}/photos/{h}");
    let wrong = app.send("PUT", &url(&h1), Auth::Bearer(&access), Body::from(jpeg(b"eno"))).await;
    assert_eq!(wrong.status, StatusCode::BAD_REQUEST);
    let long = app.send("PUT", &url(&h1), Auth::Bearer(&access), Body::from(jpeg(b"one+"))).await;
    assert_eq!(long.status, StatusCode::PAYLOAD_TOO_LARGE);
    let png = app.send("PUT", &url(&h1), Auth::Bearer(&access), Body::from(b"\x89PNG\r\n\x1a".to_vec())).await;
    assert_eq!(png.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    for (h, b) in [(&h1, &one), (&Sha256::of(&two), &two)] {
        let r = app.send("PUT", &url(h), Auth::Bearer(&access), Body::from(b.clone())).await;
        assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.body);
    }
    // No temporary files left behind.
    assert_eq!(std::fs::read_dir(&app.upload_dir.0).unwrap().count(), 0);
    let done = app.send("POST", &format!("/api/v1/surveys/{id}/commit"), Auth::Bearer(&access), Body::empty()).await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.body);
    assert_eq!(done.body["state"], "committed");
    let list = app.send("GET", "/api/v1/surveys", Auth::Web(&session), Body::empty()).await;
    assert_eq!(list.body["surveys"].as_array().unwrap().len(), 1);

    // Renewal swaps the refresh token; a copied old one signs the computer out.
    app.advance(10 * DAY);
    let old = g.body["refreshToken"].as_str().unwrap().to_string();
    let r = app.json("POST", "/api/v1/device/renew", Auth::None, json!({"refreshToken": old, "machine": "test-machine-id"})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let renewed = Pass::verify(r.body["pass"].as_str().unwrap(), &app.keys).unwrap();
    // Never past the trial's end.
    assert_eq!(renewed.expires, T0 + 15 * DAY);
    let replay = app.json("POST", "/api/v1/device/renew", Auth::None, json!({"refreshToken": old, "machine": "test-machine-id"})).await;
    assert_eq!(replay.body["error"], "revoked");
    let newer = r.body["refreshToken"].as_str().unwrap();
    let after = app.json("POST", "/api/v1/device/renew", Auth::None, json!({"refreshToken": newer, "machine": "test-machine-id"})).await;
    assert_eq!(after.body["error"], "revoked");
    // The web session ran out after a week unused; sign in again to look.
    let stale = app.send("GET", "/api/v1/devices", Auth::Web(&session), Body::empty()).await;
    assert_eq!(stale.status, StatusCode::UNAUTHORIZED);
    let login =
        app.json("POST", "/api/v1/auth/login", Auth::None, json!({"email": "asha@example.in", "password": "correct horse battery"})).await;
    let fresh = login.cookie.unwrap();
    let devices = app.send("GET", "/api/v1/devices", Auth::Web(&fresh), Body::empty()).await;
    assert_eq!(devices.body["devices"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn passwords_sessions_and_limits() {
    passwords_sessions_and_limits_on(memory()).await;
}

#[tokio::test]
async fn passwords_sessions_and_limits_on_postgres() {
    if let Some(app) = postgres().await {
        passwords_sessions_and_limits_on(app).await;
    }
}

async fn passwords_sessions_and_limits_on<B: Backend<Mail = MemoryBackend>>(app: App<B>) {
    let session = app.sign_up("ravi@example.in").await;
    let login =
        |pw: &'static str, email: &'static str| app.json("POST", "/api/v1/auth/login", Auth::None, json!({"email": email, "password": pw}));
    assert_eq!(login("wrong password!", "ravi@example.in").await.body["error"], "invalid_credentials");
    // An unknown address gets the same answer.
    assert_eq!(login("wrong password!", "nobody@example.in").await.body["error"], "invalid_credentials");
    let ok = login("correct horse battery", " RAVI@example.in ").await;
    assert_eq!(ok.status, StatusCode::OK);

    // Changing requests need the CSRF header.
    let no_csrf = app.send("POST", "/api/v1/auth/logout", Auth::WebNoCsrf(&session), Body::empty()).await;
    assert_eq!(no_csrf.status, StatusCode::FORBIDDEN);

    // A reset ends every session.
    let f = app.json("POST", "/api/v1/auth/password/forgot", Auth::None, json!({"email": "ravi@example.in"})).await;
    assert_eq!(f.status, StatusCode::ACCEPTED);
    let unknown = app.json("POST", "/api/v1/auth/password/forgot", Auth::None, json!({"email": "nobody@example.in"})).await;
    assert_eq!(unknown.status, StatusCode::ACCEPTED);
    let token = app.mail_token("ravi@example.in");
    let weak = app.json("POST", "/api/v1/auth/password/reset", Auth::None, json!({"token": token, "password": "short"})).await;
    assert_eq!(weak.body["fields"][0]["reason"], "too_short");
    let reset =
        app.json("POST", "/api/v1/auth/password/reset", Auth::None, json!({"token": token, "password": "a new long passphrase"})).await;
    assert_eq!(reset.status, StatusCode::NO_CONTENT, "{}", reset.body);
    let reuse =
        app.json("POST", "/api/v1/auth/password/reset", Auth::None, json!({"token": token, "password": "another passphrase"})).await;
    assert_eq!(reuse.body["error"], "expired_token");
    let me = app.send("GET", "/api/v1/me", Auth::Web(&session), Body::empty()).await;
    assert_eq!(me.status, StatusCode::UNAUTHORIZED);

    // Five wrong passwords, then a wait.
    for _ in 0..4 {
        login("wrong password!", "ravi@example.in").await;
    }
    let limited = login("a new long passphrase", "ravi@example.in").await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    app.advance(15 * 60);
    assert_eq!(login("a new long passphrase", "ravi@example.in").await.status, StatusCode::OK);

    let dup = app
        .json(
            "POST",
            "/api/v1/auth/signup",
            Auth::None,
            json!({"email": "ravi@example.in", "password": "correct horse battery", "name": "R", "organisation": "X"}),
        )
        .await;
    assert_eq!(dup.body["error"], "conflict");
}

#[tokio::test]
async fn seats_members_and_organisations_stay_apart() {
    seats_members_and_organisations_stay_apart_on(memory()).await;
}

#[tokio::test]
async fn seats_members_and_organisations_stay_apart_on_postgres() {
    if let Some(app) = postgres().await {
        seats_members_and_organisations_stay_apart_on(app).await;
    }
}

async fn seats_members_and_organisations_stay_apart_on<B: Backend<Mail = MemoryBackend>>(app: App<B>) {
    let admin = app.sign_up("lead@example.in").await;
    // Two computers per person; a third is refused.
    for m in ["machine-a", "machine-b"] {
        assert_eq!(app.desktop_sign_in(&admin, m).await.status, StatusCode::OK);
    }
    let third = app.desktop_sign_in(&admin, "machine-c").await;
    assert_eq!((third.body["error"].as_str(), third.body["devices"].as_u64()), (Some("seat_limit"), Some(2)));
    // The same computer again reuses its seat.
    assert_eq!(app.desktop_sign_in(&admin, "machine-a").await.status, StatusCode::OK);

    // Viewers need the Organisation plan.
    let v = app.json("POST", "/api/v1/org/invites", Auth::Web(&admin), json!({"email": "client@utility.in", "role": "viewer"})).await;
    assert_eq!(v.body["error"], "plan_required");
    let me = app.send("GET", "/api/v1/me", Auth::Web(&admin), Body::empty()).await;
    let org = OrgId::parse(me.body["organisation"]["id"].as_str().unwrap()).unwrap();
    app.server
        .backend
        .accounts()
        .set_subscription(Subscription {
            org,
            plan: Plan::Organisation,
            state: SubscriptionState::Active { until: T0 + 365 * DAY },
            billing: Billing::Invoice,
            seats: 2,
            viewer_seats: 1,
        })
        .await
        .unwrap();
    for (email, role) in [("client@utility.in", "viewer"), ("insp@example.in", "inspector")] {
        let i = app.json("POST", "/api/v1/org/invites", Auth::Web(&admin), json!({"email": email, "role": role})).await;
        assert_eq!(i.status, StatusCode::ACCEPTED, "{}", i.body);
        let token = app.mail_token(email);
        let acc = app
            .json("POST", "/api/v1/invites/accept", Auth::None, json!({"token": token, "name": "New", "password": "a long passphrase"}))
            .await;
        assert_eq!(acc.status, StatusCode::CREATED, "{}", acc.body);
    }
    // Seats are full now: another inspector can't join.
    app.json("POST", "/api/v1/org/invites", Auth::Web(&admin), json!({"email": "more@example.in", "role": "inspector"})).await;
    let token = app.mail_token("more@example.in");
    let full = app
        .json("POST", "/api/v1/invites/accept", Auth::None, json!({"token": token, "name": "More", "password": "a long passphrase"}))
        .await;
    assert_eq!(full.body["error"], "seat_limit");
    let members = app.send("GET", "/api/v1/org/members", Auth::Web(&admin), Body::empty()).await;
    assert_eq!(members.body["members"].as_array().unwrap().len(), 3);
    // The last admin can't remove themselves.
    let my_id = me.body["user"]["id"].as_str().unwrap();
    let last = app.send("DELETE", &format!("/api/v1/org/members/{my_id}"), Auth::Web(&admin), Body::empty()).await;
    assert_eq!(last.body["error"], "conflict");

    // Another organisation sees none of this.
    let other = app.sign_up("rival@example.in").await;
    let g = app.desktop_sign_in(&other, "rival-pc").await;
    let token = g.body["accessToken"].as_str().unwrap();
    let theirs = app.send("GET", "/api/v1/surveys", Auth::Bearer(token), Body::empty()).await;
    assert_eq!(theirs.body["surveys"].as_array().unwrap().len(), 0);
    let mine = app.send("GET", "/api/v1/devices", Auth::Web(&admin), Body::empty()).await;
    let device = mine.body["devices"][0]["id"].as_str().unwrap();
    let steal = app.send("DELETE", &format!("/api/v1/devices/{device}"), Auth::Web(&other), Body::empty()).await;
    assert_eq!(steal.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_web_app_reads_a_synced_survey() {
    the_web_app_reads_a_synced_survey_on(memory()).await;
}

#[tokio::test]
async fn the_web_app_reads_a_synced_survey_on_postgres() {
    if let Some(app) = postgres().await {
        the_web_app_reads_a_synced_survey_on(app).await;
    }
}

async fn the_web_app_reads_a_synced_survey_on<B: Backend<Mail = MemoryBackend>>(app: App<B>) {
    use geoflux_domain::annotation::{Annotation, AnnotationSet, NormPoint, Shape};

    let session = app.sign_up("asha@example.in").await;
    let g = app.desktop_sign_in(&session, "machine-a").await;
    let access = g.body["accessToken"].as_str().unwrap().to_string();
    let (one, two) = (jpeg(b"one"), jpeg(b"two"));
    let mut m = manifest(&[("THERMAL", &one), ("STRUCTURE", &two)]);
    m.assets[0].asset.properties.set("towerNumber", "101");
    m.assets[0].photos[0].markup =
        Some(AnnotationSet::from_shapes(vec![Annotation::new(Shape::Marker { at: NormPoint::new(0.5, 0.5), number: 1 })]).unwrap());
    let mut second = m.assets[0].clone();
    second.asset.id = "T/002 ..".into();
    second.asset.position = 1;
    second.asset.location = None;
    second.asset.properties = Default::default();
    second.photos.truncate(1);
    m.assets.push(second);
    let created = app.json("POST", "/api/v1/surveys", Auth::Bearer(&access), serde_json::to_value(&m).unwrap()).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let id = created.body["survey"].as_str().unwrap().to_string();
    let base = format!("/api/v1/surveys/{id}");

    // Still uploading: not on the web yet.
    let early = app.send("GET", &base, Auth::Web(&session), Body::empty()).await;
    assert_eq!(early.status, StatusCode::NOT_FOUND);
    for b in [&one, &two] {
        let r = app.send("PUT", &format!("{base}/photos/{}", Sha256::of(b)), Auth::Bearer(&access), Body::from(b.clone())).await;
        assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.body);
    }
    let done = app.send("POST", &format!("{base}/commit"), Auth::Bearer(&access), Body::empty()).await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.body);

    let survey = app.send("GET", &base, Auth::Web(&session), Body::empty()).await;
    assert_eq!(survey.status, StatusCode::OK, "{}", survey.body);
    assert_eq!(survey.body["bounds"], json!([85.1, 25.6, 85.1, 25.6]));
    assert_eq!(survey.body["revisions"].as_array().unwrap().len(), 1);

    let towers = app.send("GET", &format!("{base}/towers"), Auth::Web(&session), Body::empty()).await;
    let f = towers.body["features"].as_array().unwrap();
    assert_eq!(f.len(), 2);
    assert_eq!(f[0]["properties"]["label"], "101");
    assert_eq!(f[0]["geometry"]["coordinates"], json!([85.1, 25.6]));
    assert!(f[1]["geometry"].is_null());

    // A tower id with a slash and dots, percent-encoded as the web app sends it.
    let odd = app.send("GET", &format!("{base}/assets/T%2F002%20%2E%2E"), Auth::Web(&session), Body::empty()).await;
    assert_eq!(odd.status, StatusCode::OK, "{}", odd.body);
    assert_eq!(odd.body["asset"]["id"], "T/002 ..");
    let tower = app.send("GET", &format!("{base}/assets/T-001"), Auth::Web(&session), Body::empty()).await;
    assert_eq!(tower.body["photos"][0]["markup"][0]["kind"], "marker");
    let none = app.send("GET", &format!("{base}/assets/T-999"), Auth::Web(&session), Body::empty()).await;
    assert_eq!(none.status, StatusCode::NOT_FOUND);

    // Photos: only ones the survey lists, with the right type, cacheable for good.
    let (st, h, bytes) = app.raw(&format!("{base}/photos/{}", Sha256::of(&one)), Auth::Web(&session)).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(bytes, one);
    assert_eq!(h[header::CONTENT_TYPE], "image/jpeg");
    assert!(h[header::CACHE_CONTROL].to_str().unwrap().contains("immutable"));
    let (st, _, _) = app.raw(&format!("{base}/photos/{}", Sha256::of(b"other")), Auth::Web(&session)).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, _) = app.raw(&format!("{base}/photos/{}", Sha256::of(&one)), Auth::None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Reports from the desktop's own templates.
    let (st, h, pdf) = app.raw(&format!("{base}/assets/T-001/report.pdf"), Auth::Web(&session)).await;
    assert_eq!(st, StatusCode::OK, "{}", String::from_utf8_lossy(&pdf));
    assert!(pdf.starts_with(b"%PDF-"));
    assert_eq!(h[header::CONTENT_DISPOSITION], "attachment; filename=\"Line_4-T-001-r1.pdf\"");
    let (st, h, zip) = app.raw(&format!("{base}/report.zip"), Auth::Web(&session)).await;
    assert_eq!(st, StatusCode::OK, "{}", String::from_utf8_lossy(&zip));
    assert!(zip.starts_with(b"PK\x03\x04"));
    assert_eq!(h[header::CONTENT_LENGTH].to_str().unwrap(), zip.len().to_string());
    // The ZIP's temporary file is gone once it was sent.
    assert_eq!(std::fs::read_dir(&app.upload_dir.0).unwrap().count(), 0);

    // Schemas are open; unknown ones aren't found.
    let schema = app.send("GET", "/api/v1/schemas/com.geo.telecom", Auth::None, Body::empty()).await;
    assert_eq!(schema.body["labelField"], "towerNumber");
    let unknown = app.send("GET", "/api/v1/schemas/nope", Auth::None, Body::empty()).await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);

    // Another organisation sees none of it, not even that it exists.
    let other = app.sign_up("ravi@example.in").await;
    for uri in [base.clone(), format!("{base}/towers"), format!("{base}/assets/T-001"), format!("{base}/report.zip")] {
        let r = app.send("GET", &uri, Auth::Web(&other), Body::empty()).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{uri}");
    }
    let (st, _, _) = app.raw(&format!("{base}/photos/{}", Sha256::of(&one)), Auth::Web(&other)).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let list = app.send("GET", "/api/v1/surveys", Auth::Web(&other), Body::empty()).await;
    assert_eq!(list.body["surveys"].as_array().unwrap().len(), 0);

    // The approve page shows which computer a code would sign in, before approving.
    let c = app.json("POST", "/api/v1/device/code", Auth::None, json!({"machine": "machine-b", "deviceName": "Ravi's <b>PC</b>"})).await;
    let user_code = c.body["userCode"].as_str().unwrap().to_string();
    let pending = app.send("GET", &format!("/api/v1/device/pending?code={user_code}"), Auth::Web(&session), Body::empty()).await;
    assert_eq!(pending.status, StatusCode::OK, "{}", pending.body);
    assert_eq!(pending.body["deviceName"], "Ravi's <b>PC</b>");
    let bad = app.send("GET", "/api/v1/device/pending?code=AAAA-AAAA", Auth::Web(&session), Body::empty()).await;
    assert_eq!(bad.status, StatusCode::NOT_FOUND);
    let anon = app.send("GET", &format!("/api/v1/device/pending?code={user_code}"), Auth::None, Body::empty()).await;
    assert_eq!(anon.status, StatusCode::UNAUTHORIZED);
}
