//! Everything in memory, behind one lock: for tests and for trying the server on a laptop.
//! Each method takes the lock once and never across an `.await`, so every method is atomic, which
//! is exactly what the port contracts ask of the SQL adapters' transactions.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use geoflux_cloud::auth::{OrgScope, TokenKind, UserCode};
use geoflux_cloud::model::{Device, Membership, Organisation, Role, Subscription, SurveyState, SurveySummary, User};
use geoflux_cloud::ports::{
    AccountStore, BlobSource, BlobStore, DeviceCodeRecord, DeviceCodeState, DeviceStore, Email, Mailer, NewOrganisation, Seats, SeatsInUse,
    StoreError, StoreResult, SurveyStore, TokenRecord, UserRecord,
};
use geoflux_cloud::sync::{PhotoType, SurveyCreated, SurveyManifest};
use geoflux_cloud::{DeviceId, OrgId, Sha256, SurveyId, UserId};
use geoflux_domain::Severity;

use crate::Backend;

struct SurveyRec {
    summary: SurveySummary,
    expected: HashMap<Sha256, (u64, PhotoType)>,
}

#[derive(Default)]
struct Inner {
    orgs: HashMap<OrgId, Organisation>,
    users: HashMap<UserId, UserRecord>,
    by_email: HashMap<String, UserId>,
    members: HashMap<UserId, Membership>,
    subs: HashMap<OrgId, Subscription>,
    tokens: HashMap<(TokenKind, Sha256), TokenRecord>,
    /// Refresh tokens already swapped, by the device they belonged to: a second use means a copy.
    retired_refresh: HashMap<Sha256, DeviceId>,
    codes: HashMap<Sha256, DeviceCodeRecord>,
    devices: HashMap<DeviceId, Device>,
    surveys: HashMap<SurveyId, SurveyRec>,
    survey_keys: HashMap<(OrgId, String, u32), SurveyId>,
    photos: HashMap<OrgId, HashMap<Sha256, u64>>,
    blobs: HashMap<(OrgId, Sha256), Vec<u8>>,
    mail: Vec<(String, Email)>,
}

impl Inner {
    fn drop_device(&mut self, id: &DeviceId) {
        self.devices.remove(id);
        self.tokens.retain(|_, t| t.device.as_ref() != Some(id));
    }

    fn desktop_and_viewers(&self, org: &OrgId) -> (u32, u32) {
        let mut d = 0;
        let mut v = 0;
        for m in self.members.values().filter(|m| &m.org == org) {
            if m.role.is_desktop() {
                d += 1;
            } else {
                v += 1;
            }
        }
        (d, v)
    }

    fn admins(&self, org: &OrgId) -> usize {
        self.members.values().filter(|m| &m.org == org && m.role == Role::Admin).count()
    }

    fn seats_free(&self, org: &OrgId, role: Role, seats: Seats) -> Result<(), SeatsInUse> {
        let (d, v) = self.desktop_and_viewers(org);
        let (used, limit) = if role.is_desktop() { (d, seats.desktop) } else { (v, seats.viewers) };
        if used >= limit { Err(SeatsInUse { devices: used }) } else { Ok(()) }
    }
}

/// The in-memory backend. Also its own stores, blob store and mailer.
#[derive(Default)]
pub struct MemoryBackend {
    inner: Mutex<Inner>,
}

impl MemoryBackend {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panic in another request can't leave half-applied changes: every method validates
        // before it writes. Keep serving.
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Emails "sent" so far, for tests and the local trial's log.
    pub fn sent_mail(&self) -> Vec<(String, Email)> {
        self.lock().mail.clone()
    }
}

fn random_id() -> [u8; 16] {
    let mut b = [0u8; 16];
    // Ids only need to be unique here; fall back to a counter-like value if the OS source fails.
    if getrandom::fill(&mut b).is_err() {
        b = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()).to_le_bytes();
    }
    b
}

impl Backend for MemoryBackend {
    type Accounts = Self;
    type Devices = Self;
    type Surveys = Self;
    type Blobs = Self;
    type Mail = Self;

    fn accounts(&self) -> &Self {
        self
    }
    fn devices(&self) -> &Self {
        self
    }
    fn surveys(&self) -> &Self {
        self
    }
    fn blobs(&self) -> &Self {
        self
    }
    fn mail(&self) -> &Self {
        self
    }
}

fn org_member(inner: &Inner, scope: &OrgScope, user: &UserId) -> StoreResult<Membership> {
    inner.members.get(user).filter(|m| &m.org == scope.org()).cloned().ok_or(StoreError::NotFound)
}

impl AccountStore for MemoryBackend {
    async fn create_organisation(&self, new: NewOrganisation) -> StoreResult<()> {
        let mut g = self.lock();
        if g.by_email.contains_key(&new.owner.email) {
            return Err(StoreError::Conflict("email"));
        }
        let m = Membership { user: new.owner.id.clone(), org: new.org.id.clone(), role: Role::Admin, joined: new.org.created };
        g.by_email.insert(new.owner.email.clone(), new.owner.id.clone());
        g.members.insert(new.owner.id.clone(), m);
        g.subs.insert(new.org.id.clone(), new.subscription);
        g.orgs.insert(new.org.id.clone(), new.org);
        g.users.insert(new.owner.id.clone(), UserRecord { user: new.owner, password_hash: Some(new.password_hash), disabled: false });
        Ok(())
    }

    async fn user_by_email(&self, email: &str) -> StoreResult<Option<UserRecord>> {
        let g = self.lock();
        Ok(g.by_email.get(email).and_then(|id| g.users.get(id)).cloned())
    }

    async fn user(&self, id: &UserId) -> StoreResult<Option<UserRecord>> {
        Ok(self.lock().users.get(id).cloned())
    }

    async fn membership(&self, user: &UserId) -> StoreResult<Option<(Membership, Subscription)>> {
        let g = self.lock();
        Ok(g.members.get(user).and_then(|m| g.subs.get(&m.org).map(|s| (m.clone(), s.clone()))))
    }

    async fn organisation(&self, id: &OrgId) -> StoreResult<Option<Organisation>> {
        Ok(self.lock().orgs.get(id).cloned())
    }

    async fn set_password(&self, user: &UserId, hash: &str, keep: Option<&Sha256>) -> StoreResult<()> {
        let mut g = self.lock();
        let rec = g.users.get_mut(user).ok_or(StoreError::NotFound)?;
        rec.password_hash = Some(hash.to_string());
        g.tokens.retain(|(kind, digest), t| {
            &t.user != user || !matches!(kind, TokenKind::Session | TokenKind::Access | TokenKind::Refresh) || keep == Some(digest)
        });
        let devices: Vec<DeviceId> = g.devices.values().filter(|d| &d.user == user).map(|d| d.id.clone()).collect();
        for d in devices {
            g.drop_device(&d);
        }
        Ok(())
    }

    async fn set_email_verified(&self, user: &UserId) -> StoreResult<()> {
        let mut g = self.lock();
        g.users.get_mut(user).ok_or(StoreError::NotFound)?.user.email_verified = true;
        Ok(())
    }

    async fn add_member(
        &self,
        org: &OrgId,
        user: User,
        role: Role,
        password_hash: &str,
        seats: Seats,
        now: i64,
    ) -> StoreResult<Result<(), SeatsInUse>> {
        let mut g = self.lock();
        if g.by_email.contains_key(&user.email) {
            return Err(StoreError::Conflict("email"));
        }
        if !g.orgs.contains_key(org) {
            return Err(StoreError::NotFound);
        }
        if let Err(e) = g.seats_free(org, role, seats) {
            return Ok(Err(e));
        }
        g.by_email.insert(user.email.clone(), user.id.clone());
        g.members.insert(user.id.clone(), Membership { user: user.id.clone(), org: org.clone(), role, joined: now });
        g.users.insert(user.id.clone(), UserRecord { user, password_hash: Some(password_hash.to_string()), disabled: false });
        Ok(Ok(()))
    }

    async fn members(&self, scope: &OrgScope) -> StoreResult<Vec<(User, Membership)>> {
        let g = self.lock();
        let mut v: Vec<(User, Membership)> = g
            .members
            .values()
            .filter(|m| &m.org == scope.org())
            .filter_map(|m| g.users.get(&m.user).map(|u| (u.user.clone(), m.clone())))
            .collect();
        v.sort_by(|a, b| a.0.email.cmp(&b.0.email));
        Ok(v)
    }

    async fn set_role(&self, scope: &OrgScope, user: &UserId, role: Role, seats: Seats) -> StoreResult<Result<(), SeatsInUse>> {
        let mut g = self.lock();
        let m = org_member(&g, scope, user)?;
        if m.role == role {
            return Ok(Ok(()));
        }
        if m.role == Role::Admin && g.admins(scope.org()) <= 1 {
            return Err(StoreError::Conflict("last_admin"));
        }
        if m.role.is_desktop() != role.is_desktop()
            && let Err(e) = g.seats_free(scope.org(), role, seats)
        {
            return Ok(Err(e));
        }
        if let Some(m) = g.members.get_mut(user) {
            m.role = role;
        }
        if !role.is_desktop() {
            // A viewer has no seat: sign out their computers.
            let devices: Vec<DeviceId> = g.devices.values().filter(|d| &d.user == user).map(|d| d.id.clone()).collect();
            for d in devices {
                g.drop_device(&d);
            }
        }
        Ok(Ok(()))
    }

    async fn remove_member(&self, scope: &OrgScope, user: &UserId) -> StoreResult<()> {
        let mut g = self.lock();
        let m = org_member(&g, scope, user)?;
        if m.role == Role::Admin && g.admins(scope.org()) <= 1 {
            return Err(StoreError::Conflict("last_admin"));
        }
        g.members.remove(user);
        if let Some(rec) = g.users.remove(user) {
            g.by_email.remove(&rec.user.email);
        }
        g.tokens.retain(|_, t| &t.user != user);
        let devices: Vec<DeviceId> = g.devices.values().filter(|d| &d.user == user).map(|d| d.id.clone()).collect();
        for d in devices {
            g.drop_device(&d);
        }
        Ok(())
    }

    async fn set_subscription(&self, sub: Subscription) -> StoreResult<()> {
        let mut g = self.lock();
        if !g.orgs.contains_key(&sub.org) {
            return Err(StoreError::NotFound);
        }
        g.subs.insert(sub.org.clone(), sub);
        Ok(())
    }

    async fn put_token(&self, token: TokenRecord) -> StoreResult<()> {
        self.lock().tokens.insert((token.kind, token.digest.clone()), token);
        Ok(())
    }

    async fn token(&self, kind: TokenKind, digest: &Sha256, now: i64) -> StoreResult<Option<TokenRecord>> {
        Ok(self.lock().tokens.get(&(kind, digest.clone())).filter(|t| t.expires > now).cloned())
    }

    async fn take_token(&self, kind: TokenKind, digest: &Sha256, now: i64) -> StoreResult<Option<TokenRecord>> {
        Ok(self.lock().tokens.remove(&(kind, digest.clone())).filter(|t| t.expires > now))
    }

    async fn delete_token(&self, kind: TokenKind, digest: &Sha256) -> StoreResult<()> {
        self.lock().tokens.remove(&(kind, digest.clone()));
        Ok(())
    }

    async fn extend_token(&self, kind: TokenKind, digest: &Sha256, expires: i64) -> StoreResult<()> {
        if let Some(t) = self.lock().tokens.get_mut(&(kind, digest.clone())) {
            t.expires = t.expires.max(expires);
        }
        Ok(())
    }

    async fn purge_expired(&self, now: i64) -> StoreResult<u64> {
        let mut g = self.lock();
        let before = g.tokens.len() + g.codes.len();
        g.tokens.retain(|_, t| t.expires > now);
        g.codes.retain(|_, c| c.expires > now);
        Ok((before - g.tokens.len() - g.codes.len()) as u64)
    }
}

impl DeviceStore for MemoryBackend {
    async fn put_device_code(&self, code: DeviceCodeRecord) -> StoreResult<()> {
        let mut g = self.lock();
        if g.codes.values().any(|c| c.user_code == code.user_code) {
            return Err(StoreError::Conflict("user_code"));
        }
        g.codes.insert(code.digest.clone(), code);
        Ok(())
    }

    async fn device_code(&self, digest: &Sha256) -> StoreResult<Option<DeviceCodeRecord>> {
        Ok(self.lock().codes.get(digest).cloned())
    }

    async fn device_code_by_user_code(&self, code: &UserCode, now: i64) -> StoreResult<Option<DeviceCodeRecord>> {
        Ok(self.lock().codes.values().find(|c| &c.user_code == code && c.expires > now).cloned())
    }

    async fn set_device_code_state(&self, digest: &Sha256, from: &DeviceCodeState, to: DeviceCodeState, now: i64) -> StoreResult<bool> {
        let mut g = self.lock();
        match g.codes.get_mut(digest) {
            Some(c) if &c.state == from => {
                c.state = to;
                c.last_poll = now;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn claim_seat(
        &self,
        org: &OrgId,
        user: &UserId,
        machine: &str,
        name: &str,
        limit: u32,
        now: i64,
    ) -> StoreResult<Result<Device, SeatsInUse>> {
        let mut g = self.lock();
        if let Some(d) = g.devices.values_mut().find(|d| &d.user == user && d.machine == machine) {
            d.name = name.to_string();
            d.last_seen = now;
            return Ok(Ok(d.clone()));
        }
        let used = g.devices.values().filter(|d| &d.user == user).count();
        let used = u32::try_from(used).unwrap_or(u32::MAX);
        if used >= limit {
            return Ok(Err(SeatsInUse { devices: used }));
        }
        let d = Device {
            id: DeviceId::from_random(random_id()),
            org: org.clone(),
            user: user.clone(),
            machine: machine.to_string(),
            name: name.to_string(),
            signed_in: now,
            last_seen: now,
            offline_until: None,
        };
        g.devices.insert(d.id.clone(), d.clone());
        Ok(Ok(d))
    }

    async fn rotate_refresh(&self, old: &Sha256, next: &Sha256, expires: i64, now: i64) -> StoreResult<Option<Device>> {
        let mut g = self.lock();
        let live = g.tokens.remove(&(TokenKind::Refresh, old.clone())).filter(|t| t.expires > now);
        let Some(rec) = live else {
            if let Some(device) = g.retired_refresh.remove(old) {
                // Used twice: someone copied it. Sign that computer out.
                g.drop_device(&device);
            }
            return Ok(None);
        };
        let Some(device_id) = rec.device.clone() else { return Ok(None) };
        let Some(device) = g.devices.get_mut(&device_id) else { return Ok(None) };
        device.last_seen = now;
        let device = device.clone();
        g.retired_refresh.insert(old.clone(), device_id.clone());
        let next =
            TokenRecord { kind: TokenKind::Refresh, digest: next.clone(), user: rec.user, device: Some(device_id), invite: None, expires };
        g.tokens.insert((TokenKind::Refresh, next.digest.clone()), next);
        Ok(Some(device))
    }

    async fn devices(&self, scope: &OrgScope, user: Option<&UserId>) -> StoreResult<Vec<Device>> {
        let g = self.lock();
        let mut v: Vec<Device> =
            g.devices.values().filter(|d| &d.org == scope.org() && user.is_none_or(|u| &d.user == u)).cloned().collect();
        v.sort_by_key(|d| std::cmp::Reverse(d.last_seen));
        Ok(v)
    }

    async fn revoke_device(&self, scope: &OrgScope, device: &DeviceId) -> StoreResult<()> {
        let mut g = self.lock();
        if g.devices.get(device).is_none_or(|d| &d.org != scope.org()) {
            return Err(StoreError::NotFound);
        }
        g.drop_device(device);
        Ok(())
    }

    async fn mark_offline(&self, scope: &OrgScope, device: &DeviceId, until: i64) -> StoreResult<()> {
        let mut g = self.lock();
        match g.devices.get_mut(device) {
            Some(d) if &d.org == scope.org() => {
                d.offline_until = Some(d.offline_until.map_or(until, |u| u.max(until)));
                Ok(())
            }
            _ => Err(StoreError::NotFound),
        }
    }

    async fn sign_out(&self, refresh: &Sha256) -> StoreResult<()> {
        let mut g = self.lock();
        if let Some(device) = g.tokens.get(&(TokenKind::Refresh, refresh.clone())).and_then(|t| t.device.clone()) {
            g.drop_device(&device);
        }
        Ok(())
    }
}

fn missing(g: &Inner, org: &OrgId, expected: &HashMap<Sha256, (u64, PhotoType)>) -> Vec<Sha256> {
    let have = g.photos.get(org);
    let mut m: Vec<Sha256> = expected.keys().filter(|h| !have.is_some_and(|p| p.contains_key(*h))).cloned().collect();
    m.sort();
    m
}

impl SurveyStore for MemoryBackend {
    async fn create_survey(&self, scope: &OrgScope, manifest: &SurveyManifest, now: i64) -> StoreResult<SurveyCreated> {
        let photos = manifest.photos().map_err(|_| StoreError::Conflict("photo"))?;
        let mut g = self.lock();
        let key = (scope.org().clone(), manifest.client_survey_id.clone(), manifest.revision);
        if let Some(id) = g.survey_keys.get(&key).cloned() {
            let missing = g.surveys.get(&id).map(|s| missing(&g, scope.org(), &s.expected)).unwrap_or_default();
            return Ok(SurveyCreated { survey: id, missing, existing: true });
        }
        let mut counts = [0u32; 5];
        for a in &manifest.assets {
            if let Some(i) = Severity::ALL.iter().position(|s| *s == a.asset.severity) {
                counts[i] = counts[i].saturating_add(1);
            }
        }
        let id = SurveyId::from_random(random_id());
        let expected: HashMap<Sha256, (u64, PhotoType)> = photos.into_iter().map(|(h, b, k)| (h, (b, k))).collect();
        let summary = SurveySummary {
            id: id.clone(),
            org: scope.org().clone(),
            client_survey_id: manifest.client_survey_id.clone(),
            revision: manifest.revision,
            project: manifest.project.clone(),
            line: manifest.line.clone(),
            schema_id: manifest.schema_id.clone(),
            state: SurveyState::Uploading,
            uploaded_by: scope.user().clone(),
            uploaded: now,
            committed: None,
            assets: u32::try_from(manifest.assets.len()).unwrap_or(u32::MAX),
            photos: u32::try_from(expected.len()).unwrap_or(u32::MAX),
            photo_bytes: manifest.photo_bytes(),
            severity_counts: counts,
        };
        let missing = missing(&g, scope.org(), &expected);
        g.survey_keys.insert(key, id.clone());
        g.surveys.insert(id.clone(), SurveyRec { summary, expected });
        Ok(SurveyCreated { survey: id, missing, existing: false })
    }

    async fn expected_photo(&self, scope: &OrgScope, survey: &SurveyId, sha: &Sha256) -> StoreResult<Option<(u64, PhotoType)>> {
        let g = self.lock();
        let s = g.surveys.get(survey).filter(|s| &s.summary.org == scope.org()).ok_or(StoreError::NotFound)?;
        Ok(s.expected.get(sha).copied())
    }

    async fn record_photo(&self, scope: &OrgScope, sha: &Sha256, bytes: u64) -> StoreResult<()> {
        self.lock().photos.entry(scope.org().clone()).or_default().insert(sha.clone(), bytes);
        Ok(())
    }

    async fn commit(&self, scope: &OrgScope, survey: &SurveyId, now: i64) -> StoreResult<Result<SurveySummary, Vec<Sha256>>> {
        let mut g = self.lock();
        let rec = g.surveys.get(survey).filter(|s| &s.summary.org == scope.org()).ok_or(StoreError::NotFound)?;
        let m = missing(&g, scope.org(), &rec.expected);
        if !m.is_empty() {
            return Ok(Err(m));
        }
        let Some(rec) = g.surveys.get_mut(survey) else { return Err(StoreError::NotFound) };
        if rec.summary.state == SurveyState::Uploading {
            rec.summary.state = SurveyState::Committed;
            rec.summary.committed = Some(now);
        }
        Ok(Ok(rec.summary.clone()))
    }

    async fn surveys(&self, scope: &OrgScope) -> StoreResult<Vec<SurveySummary>> {
        let g = self.lock();
        let mut v: Vec<SurveySummary> = g.surveys.values().filter(|s| &s.summary.org == scope.org()).map(|s| s.summary.clone()).collect();
        v.sort_by_key(|s| std::cmp::Reverse(s.uploaded));
        Ok(v)
    }

    async fn storage_used(&self, scope: &OrgScope) -> StoreResult<u64> {
        Ok(self.lock().photos.get(scope.org()).map_or(0, |p| p.values().fold(0u64, |a, b| a.saturating_add(*b))))
    }
}

impl BlobStore for MemoryBackend {
    async fn exists(&self, org: &OrgId, sha: &Sha256) -> StoreResult<bool> {
        Ok(self.lock().blobs.contains_key(&(org.clone(), sha.clone())))
    }

    async fn put_file(&self, org: &OrgId, sha: &Sha256, file: &Path, _kind: PhotoType) -> StoreResult<()> {
        let bytes = tokio::fs::read(file).await.map_err(|e| StoreError::Unavailable(e.to_string()))?;
        let _ = tokio::fs::remove_file(file).await;
        self.lock().blobs.insert((org.clone(), sha.clone()), bytes);
        Ok(())
    }

    async fn source(&self, org: &OrgId, sha: &Sha256, _now: i64) -> StoreResult<BlobSource> {
        // Tests only check presence; a real deployment uses disk or object storage.
        if self.lock().blobs.contains_key(&(org.clone(), sha.clone())) {
            Ok(BlobSource::Url(format!("memory://{org}/{sha}")))
        } else {
            Err(StoreError::NotFound)
        }
    }
}

impl Mailer for MemoryBackend {
    async fn send(&self, to: &str, _name: &str, email: Email) -> Result<(), String> {
        self.lock().mail.push((to.to_string(), email));
        Ok(())
    }
}
