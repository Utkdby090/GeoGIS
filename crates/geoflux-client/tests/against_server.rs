//! The client against the real account server, started on a local port: what GeoGIS does when a
//! person signs in and syncs a survey.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use geoflux_client::{AccessTokens, CloudClient, DevicePoll, Progress, SyncError};
use geoflux_cloud::Sha256;
use geoflux_cloud::api::DeviceGrant;
use geoflux_cloud::ports::Email;
use geoflux_cloud::sync::{MANIFEST_FORMAT, PhotoRef, PhotoType, SurveyAsset, SurveyManifest};
use geoflux_domain::{Asset, AssetDraft, GeoPoint};
use geoflux_licence::{AccountError, AccountService, MachineId};
use geoflux_server::crypto::{Argon2Hasher, MemoryRateLimiter, OsRandom, SystemClock};
use geoflux_server::memory::MemoryBackend;
use geoflux_server::{Server, Settings, router};

/// A server on a free local port, running until the test ends.
struct Local {
    server: Arc<Server<MemoryBackend>>,
    url: String,
    dir: PathBuf,
    _rt: tokio::runtime::Runtime,
}

impl Drop for Local {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn start() -> Local {
    let mut n = [0u8; 8];
    getrandom(&mut n);
    let dir = std::env::temp_dir().join(format!("geoflux-client-test-{}", u64::from_le_bytes(n)));
    let signer = geoflux_licence::Ed25519Signer::from_pkcs8("t1", &geoflux_licence::Ed25519Signer::generate_pkcs8().unwrap()).unwrap();
    let server = Arc::new(Server {
        backend: MemoryBackend::default(),
        hasher: Arc::new(Argon2Hasher::fast().unwrap()),
        random: Arc::new(OsRandom),
        limiter: Arc::new(MemoryRateLimiter::default()),
        clock: Arc::new(SystemClock),
        signer: Arc::new(signer),
        reports: Arc::new(geoflux_server::reports::Reports),
        report_slots: Arc::new(tokio::sync::Semaphore::new(geoflux_server::reports::REPORTS_AT_ONCE)),
        settings: Settings {
            public_url: "http://localhost".into(),
            upload_dir: dir.join("uploads"),
            secure_cookies: false,
            trust_proxy: false,
        },
    });
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router(server.clone()).into_make_service_with_connect_info::<SocketAddr>();
    rt.spawn(async move { axum::serve(listener, app).await });
    Local { server, url: format!("http://127.0.0.1:{}", addr.port()), dir, _rt: rt }
}

fn getrandom(b: &mut [u8]) {
    use geoflux_cloud::ports::Random;
    OsRandom.fill(b).unwrap();
}

/// A web app request; returns the status and the session cookie it set, if any.
fn web_post(web: &ureq::Agent, url: &str, cookie: &str, body: serde_json::Value) -> (u16, Option<String>) {
    let res = web
        .post(url)
        .header("content-type", "application/json")
        .header("x-geoflux-csrf", "1")
        .header("cookie", cookie)
        .send(body.to_string())
        .unwrap();
    let set = res.headers().get("set-cookie").and_then(|v| v.to_str().ok()).map(|v| v.split(';').next().unwrap_or_default().to_string());
    (res.status().as_u16(), set)
}

fn web_agent() -> ureq::Agent {
    ureq::Agent::config_builder().http_status_as_error(false).build().into()
}

fn sign_up_and_approve(local: &Local, web: &ureq::Agent, user_code: &str) {
    let api = format!("{}/api/v1", local.url);
    let body = serde_json::json!({"email": "asha@example.in", "password": "correct horse battery", "name": "Asha", "organisation": "Grid"});
    let (status, cookie) = web_post(web, &format!("{api}/auth/signup"), "", body);
    assert_eq!(status, 201);
    let cookie = cookie.unwrap();
    let link = match local.server.backend.sent_mail().pop().unwrap().1 {
        Email::VerifyAddress { link } => link,
        e => panic!("{e:?}"),
    };
    let token = link.split("token=").nth(1).unwrap();
    assert_eq!(web_post(web, &format!("{api}/auth/email/verify"), "", serde_json::json!({"token": token})).0, 204);
    let approve = serde_json::json!({"userCode": user_code, "approve": true});
    assert_eq!(web_post(web, &format!("{api}/device/approve"), &cookie, approve).0, 204);
}

fn machine(id: &str) -> MachineId {
    MachineId::from_output(id).unwrap()
}

/// What GeoGIS keeps after signing in, renewing when asked for a fresh access token.
struct Session<'a> {
    client: &'a CloudClient,
    machine: MachineId,
    grant: DeviceGrant,
    renewals: usize,
}

impl AccessTokens for Session<'_> {
    fn access_token(&mut self, fresh: bool) -> Result<String, AccountError> {
        if fresh {
            self.grant = self.client.renew_device(&self.grant.refresh_token, &self.machine)?;
            self.renewals += 1;
        }
        Ok(self.grant.access_token.clone())
    }
}

fn jpeg(tag: &[u8]) -> Vec<u8> {
    let mut v = vec![0xff, 0xd8, 0xff, 0xe0];
    v.extend_from_slice(tag);
    v
}

fn manifest(photos: &[(&str, &[u8])]) -> SurveyManifest {
    let draft = AssetDraft { location: Some(GeoPoint { lat: 25.6, lon: 85.1 }), properties: Default::default() };
    SurveyManifest {
        format: MANIFEST_FORMAT,
        client_survey_id: "proj-0001-abcd".into(),
        revision: 1,
        project: "Patna 132 kV".into(),
        line: None,
        schema_id: "com.geo.telecom".into(),
        surveyed: None,
        assets: vec![SurveyAsset {
            asset: Asset::from_draft("T-001", "com.geo.telecom", 0, draft),
            findings: Vec::new(),
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

#[test]
fn sign_in_upload_resume_renew_and_sign_out() {
    let local = start();
    let client = CloudClient::new(&local.url, "Field laptop").unwrap();
    let m = machine("4C4C4544-0042-3510-8051-B4C04F4E3732");

    // GeoGIS starts; the person approves on the web; GeoGIS collects the grant.
    let code = client.start_sign_in(&m).unwrap();
    assert!(code.verification_uri.ends_with(&format!("?code={}", code.user_code)));
    assert_eq!(client.poll_device(&code.device_code).unwrap(), DevicePoll::Pending);
    // Polled again at once: the server asks to slow down.
    assert!(matches!(client.poll_device(&code.device_code).unwrap(), DevicePoll::SlowDown(s) if s >= 5));
    let web = web_agent();
    sign_up_and_approve(&local, &web, &code.user_code);
    let DevicePoll::Granted(grant) = client.poll_device(&code.device_code).unwrap() else { panic!("not granted") };
    assert_eq!(client.poll_device(&code.device_code).unwrap(), DevicePoll::Expired, "collected once");

    // Photos on disk, as in a project folder.
    std::fs::create_dir_all(&local.dir).unwrap();
    let photos = [("thermal", jpeg(b"heat")), ("structure", jpeg(b"tower")), ("location", jpeg(b"gps"))];
    let paths: Vec<(Sha256, PathBuf)> = photos
        .iter()
        .map(|(slot, b)| {
            let p = local.dir.join(format!("{slot}.jpg"));
            std::fs::write(&p, b).unwrap();
            (Sha256::of(b), p)
        })
        .collect();
    let find = |sha: &Sha256| paths.iter().find(|(h, _)| h == sha).map(|(_, p)| p.clone());
    let refs: Vec<(&str, &[u8])> = photos.iter().map(|(s, b)| (*s, b.as_slice())).collect();
    let survey = manifest(&refs);

    // Cancelled after the first photo: a later run sends only the other two.
    let mut session = Session { client: &client, machine: m.clone(), grant, renewals: 0 };
    let mut seen = Vec::new();
    let r = client.upload(&mut session, &survey, &find, &mut |p: Progress| {
        seen.push(p);
        p.sent < 1
    });
    assert!(matches!(r, Err(SyncError::Cancelled)));
    assert_eq!(seen.last().map(|p| (p.sent, p.photos)), Some((1, 3)));

    // An expired access token is renewed once, transparently.
    session.grant.access_token = "gfa_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into();
    let mut last = Progress::default();
    let done = client
        .upload(&mut session, &survey, &find, &mut |p| {
            last = p;
            true
        })
        .unwrap();
    assert_eq!(session.renewals, 1);
    assert_eq!((done.sent, last.photos, done.survey.photos, done.existing), (2, 2, 3, true));
    assert!(done.survey.committed.is_some());

    // A photo changed on disk since the manifest was made is reported, not sent.
    let changed = manifest(&[("extra", &jpeg(b"new one"))]);
    let changed = SurveyManifest { revision: 2, ..changed };
    let extra = local.dir.join("extra.jpg");
    std::fs::write(&extra, jpeg(b"new one, edited")).unwrap();
    let r = client.upload(&mut session, &changed, &|_| Some(extra.clone()), &mut |_| true);
    assert!(matches!(r, Err(SyncError::Photo { .. })), "{r:?}");

    // Renewing hands out a new refresh token; the old one then signs this computer out.
    let old = session.grant.refresh_token.clone();
    let renewed = client.renew(&old, &m).unwrap();
    assert_ne!(renewed.refresh_token, old);
    assert_eq!(client.renew(&old, &m), Err(AccountError::Revoked));
    assert_eq!(client.renew(&renewed.refresh_token, &m), Err(AccountError::Revoked), "the copy signed it out");

    // Signing out is quiet even for an unknown token.
    client.sign_out("gfr_nothing").unwrap();
}

#[test]
fn an_unreachable_server_is_offline_not_an_error() {
    // Nothing listens on port 9 (discard) on test machines.
    let client = CloudClient::new("http://127.0.0.1:9", "Laptop").unwrap();
    let r = client.start_sign_in(&machine("4C4C4544-0042-3510-8051-B4C04F4E3732"));
    assert!(matches!(r, Err(AccountError::Offline(_))), "{r:?}");
    assert!(CloudClient::new("http://cloud.example", "Laptop").is_err(), "plain HTTP to another computer");
}
