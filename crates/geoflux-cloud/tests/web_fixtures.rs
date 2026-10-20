//! The JSON the web app is written against, made from the real Rust types. The web app's tests parse
//! every file with its own schemas (`apps/geoflux-web/src/api/schemas.ts`), so a change on either
//! side that breaks the other fails CI on both sides.
//!
//! Run with `GEOFLUX_UPDATE_FIXTURES=1` to rewrite the files after a deliberate change.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use geoflux_cloud::api::{DeviceList, Me, Member, MemberList, SurveyList};
use geoflux_cloud::model::{
    Billing, Device, Membership, Organisation, Role, Subscription, SubscriptionState, SurveyState, SurveySummary, User,
};
use geoflux_cloud::ports::{DeviceCodeRecord, DeviceCodeState};
use geoflux_cloud::sync::{PhotoRef, PhotoType, SurveyAsset};
use geoflux_cloud::view::{PendingDevice, RevisionRef, SurveyDetail, TowerCollection, TowerDetail};
use geoflux_cloud::{ApiError, DeviceId, ErrorCode, FieldError, OrgId, Sha256, SurveyId, UserId};
use geoflux_domain::annotation::{Annotation, AnnotationSet, NormPoint, Shape};
use geoflux_domain::{Asset, AssetDraft, GeoPoint, Properties, RecordStatus, Severity};
use geoflux_licence::Plan;

const T0: i64 = 1_790_000_000;

fn org() -> OrgId {
    OrgId::from_random([1; 16])
}
fn user() -> UserId {
    UserId::from_random([2; 16])
}
fn survey_id() -> SurveyId {
    SurveyId::from_random([3; 16])
}

fn summary() -> SurveySummary {
    SurveySummary {
        id: survey_id(),
        org: org(),
        client_survey_id: "proj-4f1c2a9e".into(),
        revision: 2,
        project: "Patna 132 kV".into(),
        line: Some("Line 4".into()),
        schema_id: "com.geo.telecom".into(),
        state: SurveyState::Committed,
        uploaded_by: user(),
        uploaded: T0,
        committed: Some(T0 + 60),
        assets: 3,
        photos: 3,
        photo_bytes: 9_400_000,
        severity_counts: [1, 0, 1, 0, 1],
    }
}

fn photo(slot: &str, seed: &[u8], markup: Option<AnnotationSet>) -> PhotoRef {
    PhotoRef {
        slot: slot.into(),
        sha256: Sha256::of(seed),
        bytes: 3_100_000,
        kind: PhotoType::Jpeg,
        file_name: format!("DJI_0001_{slot}.JPG"),
        markup,
    }
}

fn towers() -> Vec<SurveyAsset> {
    let tower = |id: &str, n: u32, label: &str, at: Option<(f64, f64)>, sev: Severity| {
        let mut props = Properties::new();
        props.set("towerNumber", label);
        props.set("faultDescription", "Hot spot on B phase jumper");
        let mut a = Asset::from_draft(id, "com.geo.telecom", n, AssetDraft { location: None, properties: props });
        a.location = at.map(|(lat, lon)| GeoPoint { lat, lon });
        a.severity = sev;
        a.status = RecordStatus::Ready;
        a
    };
    let markup = AnnotationSet::from_shapes(vec![
        Annotation::new(Shape::Rect { a: NormPoint::new(0.25, 0.25), b: NormPoint::new(0.5, 0.75) }),
        Annotation::new(Shape::Marker { at: NormPoint::new(0.75, 0.5), number: 1 }),
    ])
    .unwrap();
    vec![
        SurveyAsset {
            asset: tower("t1", 0, "101", Some((25.6, 85.1)), Severity::Critical),
            photos: vec![photo("THERMAL", b"1", Some(markup)), photo("STRUCTURE", b"2", None)],
        },
        SurveyAsset { asset: tower("t2", 1, "102", Some((25.61, 85.12)), Severity::None), photos: vec![photo("THERMAL", b"3", None)] },
        SurveyAsset { asset: tower("t3", 2, "", None, Severity::Medium), photos: vec![] },
    ]
}

fn fixtures() -> Vec<(&'static str, serde_json::Value)> {
    let v = |x: serde_json::Result<serde_json::Value>| x.unwrap();
    let towers = towers();
    let fc = TowerCollection::from_assets(&towers, "towerNumber");
    let detail = SurveyDetail {
        summary: summary(),
        bounds: fc.bounds(),
        revisions: vec![
            RevisionRef { id: survey_id(), revision: 2, committed: Some(T0 + 60) },
            RevisionRef { id: SurveyId::from_random([4; 16]), revision: 1, committed: Some(T0 - 86_400) },
        ],
    };
    let me_user = User { id: user(), email: "asha@example.in".into(), name: "Asha Verma".into(), email_verified: true, created: T0 };
    let me = Me {
        user: me_user.clone(),
        organisation: Organisation { id: org(), name: "Bihar Grid Surveys".into(), created: T0 },
        membership: Membership { user: user(), org: org(), role: Role::Admin, joined: T0 },
        subscription: Subscription {
            org: org(),
            plan: Plan::Pro,
            state: SubscriptionState::Trial { ends: T0 + 15 * 86_400 },
            billing: Billing::Trial,
            seats: 3,
            viewer_seats: 0,
        },
        effective_plan: Plan::Pro,
    };
    let pending = DeviceCodeRecord {
        digest: Sha256::of(b"code"),
        user_code: geoflux_cloud::auth::UserCode::parse("WDJB-MJHT").unwrap(),
        machine: "M".into(),
        device_name: "Asha's laptop <script>".into(),
        state: DeviceCodeState::Pending,
        interval_secs: 5,
        last_poll: T0,
        expires: T0 + 900,
    };
    let invalid = ApiError::fields(vec![FieldError::new("password", "too_short")]);
    vec![
        ("me.json", v(serde_json::to_value(&me))),
        ("surveys.json", v(serde_json::to_value(SurveyList { surveys: vec![summary()] }))),
        ("survey.json", v(serde_json::to_value(&detail))),
        ("towers.json", v(serde_json::to_value(&fc))),
        ("tower.json", v(serde_json::to_value(TowerDetail { survey: survey_id(), uploaded: towers[0].clone() }))),
        ("pending-device.json", v(serde_json::to_value(PendingDevice::of(&pending, T0).unwrap()))),
        (
            "devices.json",
            v(serde_json::to_value(DeviceList {
                devices: vec![Device {
                    id: DeviceId::from_random([5; 16]),
                    org: org(),
                    user: user(),
                    machine: "M".into(),
                    name: "Asha's laptop".into(),
                    signed_in: T0,
                    last_seen: T0 + 100,
                    offline_until: None,
                }],
            })),
        ),
        (
            "members.json",
            v(serde_json::to_value(MemberList {
                members: vec![Member { user: me_user, role: Role::Admin, joined: T0 }],
                seats: 3,
                viewer_seats: 0,
            })),
        ),
        ("error-invalid.json", v(serde_json::to_value(&invalid))),
        ("error-rate-limited.json", v(serde_json::to_value(ApiError::rate_limited(30)))),
        ("error-not-found.json", v(serde_json::to_value(ApiError::new(ErrorCode::NotFound, "Not found.")))),
        ("routes.json", serde_json::Value::Array(geoflux_cloud::api::routes::ALL.iter().map(|(m, p)| serde_json::json!([m, p])).collect())),
        ("schema.json", v(serde_json::to_value(geoflux_domain::builtin_schema("com.geo.telecom").unwrap().unwrap()))),
    ]
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/geoflux-web/src/api/fixtures")
}

#[test]
fn web_fixtures_match_the_rust_types() {
    let update = std::env::var_os("GEOFLUX_UPDATE_FIXTURES").is_some();
    let mut stale = Vec::new();
    for (name, value) in fixtures() {
        let path = dir().join(name);
        let text = format!("{}\n", serde_json::to_string_pretty(&value).unwrap());
        if update {
            std::fs::create_dir_all(dir()).unwrap();
            std::fs::write(&path, &text).unwrap();
        } else if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            stale.push(name);
        }
    }
    assert!(stale.is_empty(), "web fixtures out of date: {stale:?}. Rerun with GEOFLUX_UPDATE_FIXTURES=1 and fix the web app's schemas.");
}
