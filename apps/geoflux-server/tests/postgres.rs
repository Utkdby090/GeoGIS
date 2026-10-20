//! The PostgreSQL stores under concurrent requests: the races the port contracts rule out (two
//! computers taking the last seat, a copied refresh token used twice, a single-use link used twice)
//! must not happen with many connections either. Skipped without `GEOFLUX_TEST_DATABASE_URL`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use geoflux_cloud::auth::{AuthVia, OrgScope, Principal, TokenKind, UserCode};
use geoflux_cloud::model::{Organisation, Role, User};
use geoflux_cloud::plans::{self, Permission};
use geoflux_cloud::ports::{
    AccountStore, DeviceCodeRecord, DeviceCodeState, DeviceStore, NewOrganisation, Seats, StoreError, SurveyStore, TokenRecord,
};
use geoflux_cloud::sync::{MANIFEST_FORMAT, PhotoRef, PhotoType, SurveyAsset, SurveyManifest};
use geoflux_cloud::{OrgId, Sha256, UserId};
use geoflux_domain::{Asset, AssetDraft, GeoPoint};
use geoflux_server::crypto::random_id;
use geoflux_server::postgres::PgStore;
use sqlx::Executor;

#[path = "common/pg.rs"]
mod pg;

const NOW: i64 = 1_792_108_800;

struct Db {
    store: Arc<PgStore>,
    _db: pg::TestDb,
}

async fn db() -> Option<Db> {
    let db = pg::TestDb::create().await?;
    let store = PgStore::from_pool(db.pool().await);
    store.migrate().await.unwrap();
    Some(Db { store: Arc::new(store), _db: db })
}

fn user(email: &str) -> User {
    User { id: UserId::from_random(random_id()), email: email.into(), name: "Asha Verma".into(), email_verified: true, created: NOW }
}

/// An organisation with its admin; returns the admin's scope.
async fn org(store: &PgStore, email: &str) -> OrgScope {
    let id = OrgId::from_random(random_id());
    let owner = user(email);
    let new = NewOrganisation {
        org: Organisation { id: id.clone(), name: "Bihar Grid Surveys".into(), created: NOW },
        owner: owner.clone(),
        password_hash: "$argon2id$x".into(),
        subscription: plans::trial(id, NOW),
    };
    store.create_organisation(new).await.unwrap();
    scope(store, &owner.id).await
}

async fn scope(store: &PgStore, user: &UserId) -> OrgScope {
    let (m, sub) = store.membership(user).await.unwrap().unwrap();
    Principal::new(user.clone(), m.role, AuthVia::Session, sub).require(Permission::ManageMembers, NOW).unwrap()
}

fn digest(n: u8) -> Sha256 {
    Sha256::of(&[n; 4])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn migrations_run_once_and_refuse_a_newer_database() {
    let Some(db) = db().await else { return };
    let runs: Vec<_> = (0..4)
        .map(|_| {
            let s = db.store.clone();
            tokio::spawn(async move { s.migrate().await })
        })
        .collect();
    for r in runs {
        r.await.unwrap().unwrap();
    }
    db.store.pool().execute("INSERT INTO geoflux_migrations (version) VALUES (999)").await.unwrap();
    let e = db.store.migrate().await.unwrap_err();
    assert!(e.to_string().contains("newer than this server"), "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn records_read_back_as_written() {
    let Some(db) = db().await else { return };
    let s = &*db.store;
    let admin = org(s, "lead@example.in").await;
    assert_eq!(
        s.create_organisation(NewOrganisation {
            org: Organisation { id: OrgId::from_random(random_id()), name: "Other".into(), created: NOW },
            owner: user("lead@example.in"),
            password_hash: "h".into(),
            subscription: plans::trial(OrgId::from_random(random_id()), NOW),
        })
        .await,
        Err(StoreError::Conflict("email"))
    );
    let rec = s.user_by_email("lead@example.in").await.unwrap().unwrap();
    assert_eq!(rec.password_hash.as_deref(), Some("$argon2id$x"));
    let (m, sub) = s.membership(admin.user()).await.unwrap().unwrap();
    assert_eq!((m.role, sub), (Role::Admin, plans::trial(admin.org().clone(), NOW)));

    let invite = TokenRecord {
        kind: TokenKind::Invite,
        digest: digest(1),
        user: admin.user().clone(),
        device: None,
        invite: Some((admin.org().clone(), Role::Inspector, "insp@example.in".into())),
        expires: NOW + 60,
    };
    s.put_token(invite.clone()).await.unwrap();
    assert_eq!(s.token(TokenKind::Invite, &digest(1), NOW).await.unwrap(), Some(invite.clone()));
    assert_eq!(s.token(TokenKind::Invite, &digest(1), NOW + 60).await.unwrap(), None, "expired");
    assert_eq!(s.token(TokenKind::Session, &digest(1), NOW).await.unwrap(), None, "other kind");
    s.extend_token(TokenKind::Invite, &digest(1), NOW + 30).await.unwrap();
    assert_eq!(s.token(TokenKind::Invite, &digest(1), NOW).await.unwrap().unwrap().expires, NOW + 60, "never shortened");
    assert_eq!(s.purge_expired(NOW + 61).await.unwrap(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_last_seat_goes_to_one_computer() {
    let Some(db) = db().await else { return };
    let admin = org(&db.store, "lead@example.in").await;
    let tries: Vec<_> = (0..8)
        .map(|i| {
            let (s, a) = (db.store.clone(), admin.clone());
            tokio::spawn(async move { s.claim_seat(a.org(), a.user(), &format!("machine-{i}"), "Laptop", 2, NOW).await.unwrap() })
        })
        .collect();
    let mut ok = 0;
    for t in tries {
        if t.await.unwrap().is_ok() {
            ok += 1;
        }
    }
    assert_eq!(ok, 2);
    assert_eq!(db.store.devices(&admin, None).await.unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refresh_token_used_twice_signs_the_computer_out() {
    let Some(db) = db().await else { return };
    let admin = org(&db.store, "lead@example.in").await;
    let d = db.store.claim_seat(admin.org(), admin.user(), "machine-a", "Laptop", 2, NOW).await.unwrap().unwrap();
    let rec = TokenRecord {
        kind: TokenKind::Refresh,
        digest: digest(1),
        user: admin.user().clone(),
        device: Some(d.id.clone()),
        invite: None,
        expires: NOW + 100,
    };
    db.store.put_token(rec).await.unwrap();
    let tries: Vec<_> = (0..6u8)
        .map(|i| {
            let s = db.store.clone();
            tokio::spawn(async move { s.rotate_refresh(&digest(1), &digest(10 + i), NOW + 200, NOW).await.unwrap() })
        })
        .collect();
    let mut renewed = 0;
    for t in tries {
        if t.await.unwrap().is_some() {
            renewed += 1;
        }
    }
    assert_eq!(renewed, 1, "only one renewal wins");
    // The losers presented a retired token: the computer and its new token are gone.
    assert!(db.store.devices(&admin, None).await.unwrap().is_empty());
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM tokens").fetch_one(db.store.pool()).await.unwrap();
    assert_eq!(left, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn single_use_links_and_sign_in_codes_work_once() {
    let Some(db) = db().await else { return };
    let admin = org(&db.store, "lead@example.in").await;
    let rec = TokenRecord {
        kind: TokenKind::PasswordReset,
        digest: digest(2),
        user: admin.user().clone(),
        device: None,
        invite: None,
        expires: NOW + 60,
    };
    db.store.put_token(rec).await.unwrap();
    let takes: Vec<_> = (0..6)
        .map(|_| {
            let s = db.store.clone();
            tokio::spawn(async move { s.take_token(TokenKind::PasswordReset, &digest(2), NOW).await.unwrap() })
        })
        .collect();
    let mut got = 0;
    for t in takes {
        got += usize::from(t.await.unwrap().is_some());
    }
    assert_eq!(got, 1);

    let code = DeviceCodeRecord {
        digest: digest(3),
        user_code: UserCode::from_random([1; 8]),
        machine: "machine-a".into(),
        device_name: "Laptop".into(),
        state: DeviceCodeState::Pending,
        interval_secs: 5,
        last_poll: NOW,
        expires: NOW + 900,
    };
    db.store.put_device_code(code.clone()).await.unwrap();
    let again = DeviceCodeRecord { digest: digest(4), ..code.clone() };
    assert_eq!(db.store.put_device_code(again).await, Err(StoreError::Conflict("user_code")));
    assert_eq!(db.store.device_code_by_user_code(&code.user_code, NOW).await.unwrap(), Some(code.clone()));
    let approved = DeviceCodeState::Approved(admin.user().clone());
    assert!(db.store.set_device_code_state(&digest(3), &DeviceCodeState::Pending, approved.clone(), NOW + 1).await.unwrap());
    let collects: Vec<_> = (0..6)
        .map(|_| {
            let (s, from) = (db.store.clone(), approved.clone());
            tokio::spawn(async move { s.set_device_code_state(&digest(3), &from, DeviceCodeState::Collected, NOW + 2).await.unwrap() })
        })
        .collect();
    let mut won = 0;
    for c in collects {
        won += usize::from(c.await.unwrap());
    }
    assert_eq!(won, 1);
    assert_eq!(db.store.device_code(&digest(3)).await.unwrap().unwrap().state, DeviceCodeState::Collected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn seats_and_the_last_admin_hold_under_load() {
    let Some(db) = db().await else { return };
    let admin = org(&db.store, "lead@example.in").await;
    let seats = Seats { desktop: 3, viewers: 0 };
    let joins: Vec<_> = (0..6)
        .map(|i| {
            let (s, o) = (db.store.clone(), admin.org().clone());
            tokio::spawn(async move { s.add_member(&o, user(&format!("insp{i}@example.in")), Role::Admin, "h", seats, NOW).await.unwrap() })
        })
        .collect();
    let mut joined = 0;
    for j in joins {
        joined += usize::from(j.await.unwrap().is_ok());
    }
    assert_eq!(joined, 2, "three desktop seats, one already the admin's");

    // Every admin demotes every other admin at once: one must stay.
    let admins: Vec<UserId> = db.store.members(&admin).await.unwrap().into_iter().map(|(u, _)| u.id).collect();
    assert_eq!(admins.len(), 3);
    // Scopes first: once the tasks start, an admin may already be an inspector.
    let mut scopes = Vec::new();
    for a in &admins {
        scopes.push(scope(&db.store, a).await);
    }
    let mut tries = Vec::new();
    for (a, sc) in admins.iter().zip(scopes) {
        for b in admins.iter().filter(|b| *b != a) {
            let (s, sc, b) = (db.store.clone(), sc.clone(), b.clone());
            tries.push(tokio::spawn(async move { s.set_role(&sc, &b, Role::Inspector, seats).await }));
        }
    }
    for t in tries {
        let _ = t.await.unwrap();
    }
    let left = db.store.members(&admin).await.unwrap().into_iter().filter(|(_, m)| m.role == Role::Admin).count();
    assert_eq!(left, 1);

    // Another organisation can't touch these members or computers.
    let other = org(&db.store, "other@example.in").await;
    assert_eq!(db.store.remove_member(&other, &admins[0]).await, Err(StoreError::NotFound));
    assert!(db.store.members(&other).await.unwrap().len() == 1);
}

fn manifest(photos: &[&[u8]]) -> SurveyManifest {
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
            photos: photos
                .iter()
                .enumerate()
                .map(|(i, b)| PhotoRef {
                    slot: format!("s{i}"),
                    sha256: Sha256::of(b),
                    bytes: b.len() as u64,
                    kind: PhotoType::Jpeg,
                    file_name: format!("{i}.jpg"),
                    markup: None,
                })
                .collect(),
        }],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_survey_uploaded_twice_at_once_is_one_survey() {
    let Some(db) = db().await else { return };
    let admin = org(&db.store, "lead@example.in").await;
    let m = Arc::new(manifest(&[b"one", b"two"]));
    let creates: Vec<_> = (0..5)
        .map(|_| {
            let (s, sc, m) = (db.store.clone(), admin.clone(), m.clone());
            tokio::spawn(async move { s.create_survey(&sc, &m, NOW).await.unwrap() })
        })
        .collect();
    let mut made = Vec::new();
    for c in creates {
        made.push(c.await.unwrap());
    }
    assert_eq!(made.iter().filter(|c| !c.existing).count(), 1);
    let id = made[0].survey.clone();
    assert!(made.iter().all(|c| c.survey == id && c.missing.len() == 2));
    assert_eq!(db.store.expected_photo(&admin, &id, &Sha256::of(b"one")).await.unwrap(), Some((3, PhotoType::Jpeg)));
    assert_eq!(db.store.expected_photo(&admin, &id, &Sha256::of(b"nope")).await.unwrap(), None);
    let other = org(&db.store, "other@example.in").await;
    assert_eq!(db.store.expected_photo(&other, &id, &Sha256::of(b"one")).await, Err(StoreError::NotFound));

    db.store.record_photo(&admin, &Sha256::of(b"one"), 3).await.unwrap();
    assert_eq!(db.store.commit(&admin, &id, NOW).await.unwrap(), Err(vec![Sha256::of(b"two")]));
    db.store.record_photo(&admin, &Sha256::of(b"two"), 3).await.unwrap();
    let s = db.store.commit(&admin, &id, NOW + 5).await.unwrap().unwrap();
    assert_eq!((s.committed, s.photos, s.photo_bytes, s.assets), (Some(NOW + 5), 2, 6, 1));
    assert_eq!(db.store.commit(&admin, &id, NOW + 9).await.unwrap().unwrap().committed, Some(NOW + 5), "committed once");
    assert_eq!(db.store.surveys(&admin).await.unwrap(), vec![s]);
    assert_eq!(db.store.storage_used(&admin).await.unwrap(), 6);
    assert!(db.store.surveys(&other).await.unwrap().is_empty());
}
