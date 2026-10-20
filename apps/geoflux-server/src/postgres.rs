//! The stores on PostgreSQL: the same database whichever free host is picked (a container on
//! Oracle's VM, Neon, or Supabase), reached by URL.
//!
//! Every port method that must not be split by another request runs in one transaction and takes a
//! row lock first (the organisation for seat counts, the person for their computers), so two
//! requests can't both take the last seat. Single-use tokens are `DELETE … RETURNING`, so only one
//! request gets the row. Queries are built at run time (no compile-time database), and every value
//! is a bound parameter.

use std::time::Duration;

use geoflux_cloud::auth::{OrgScope, TokenKind, UserCode};
use geoflux_cloud::model::{Device, Membership, Organisation, Role, Subscription, SubscriptionState, SurveySummary, User};
use geoflux_cloud::ports::{
    AccountStore, BlobStore, DeviceCodeRecord, DeviceCodeState, DeviceStore, Mailer, NewOrganisation, Seats, SeatsInUse, StoreError,
    StoreResult, SurveyStore, TokenRecord, UserRecord,
};
use geoflux_cloud::sync::{PhotoType, SurveyCreated, SurveyManifest};
use geoflux_cloud::{DeviceId, OrgId, Sha256, SurveyId, UserId};
use geoflux_domain::Severity;
use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::postgres::{PgPool, PgPoolOptions, PgRow};
use sqlx::{Executor, Postgres, Row, Transaction};

/// Schema changes, applied in order once each. Never edit a released one; add the next.
const MIGRATIONS: &[(i64, &str)] = &[(1, include_str!("../migrations/0001_accounts.sql"))];

/// Held while migrating, so two servers starting together don't both apply a change.
const MIGRATION_LOCK: i64 = 0x6765_6f66_6c75_7801;

/// The account, computer and survey stores on one connection pool.
#[derive(Clone)]
pub struct PgStore {
    pool: PgPool,
}

impl PgStore {
    /// Connects with up to `max_connections` (free tiers allow few: Neon and Supabase about 20).
    /// Nothing is checked until the first query; call [`PgStore::migrate`] at start-up.
    pub async fn connect(url: &str, max_connections: u32) -> StoreResult<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections.max(1))
            .acquire_timeout(Duration::from_secs(10))
            // Free databases close idle connections; drop ours first rather than meet a dead one.
            .idle_timeout(Duration::from_secs(300))
            .connect(url)
            .await
            .map_err(db)?;
        Ok(PgStore { pool })
    }

    pub fn from_pool(pool: PgPool) -> Self {
        PgStore { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Brings the schema up to date. Refuses a database migrated by a newer server, which this one
    /// would misread.
    pub async fn migrate(&self) -> StoreResult<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(MIGRATION_LOCK).execute(&mut *tx).await.map_err(db)?;
        // A bare string runs as a simple query, which may hold several statements.
        (&mut *tx)
            .execute(
                "CREATE TABLE IF NOT EXISTS geoflux_migrations (version bigint PRIMARY KEY, applied timestamptz NOT NULL DEFAULT now())",
            )
            .await
            .map_err(db)?;
        let done: Vec<i64> = sqlx::query_scalar("SELECT version FROM geoflux_migrations").fetch_all(&mut *tx).await.map_err(db)?;
        let known = MIGRATIONS.iter().map(|(v, _)| *v).max().unwrap_or(0);
        if let Some(v) = done.iter().find(|v| **v > known) {
            return Err(StoreError::Other(format!("the database is at schema {v}, newer than this server ({known}); update the server")));
        }
        for (version, sql) in MIGRATIONS {
            if done.contains(version) {
                continue;
            }
            (&mut *tx).execute(*sql).await.map_err(db)?;
            sqlx::query("INSERT INTO geoflux_migrations (version) VALUES ($1)").bind(version).execute(&mut *tx).await.map_err(db)?;
            log::info!("database schema {version} applied");
        }
        tx.commit().await.map_err(db)
    }

    async fn begin(&self) -> StoreResult<Transaction<'static, Postgres>> {
        self.pool.begin().await.map_err(db)
    }
}

/// Maps a database error to the port's error without passing its text to clients.
fn db(e: sqlx::Error) -> StoreError {
    match &e {
        sqlx::Error::RowNotFound => StoreError::NotFound,
        sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed | sqlx::Error::Io(_) | sqlx::Error::Tls(_) => {
            StoreError::Unavailable(e.to_string())
        }
        sqlx::Error::Database(d) => match d.code().as_deref() {
            // unique_violation
            Some("23505") => StoreError::Conflict(match d.constraint() {
                Some("users_email_key") => "email",
                Some("device_codes_user_code_key") => "user_code",
                Some("devices_user_machine") => "device",
                Some("surveys_revision") => "survey",
                _ => "unique",
            }),
            // foreign_key_violation: the row it points at was just deleted (a computer signed out).
            Some("23503") => StoreError::NotFound,
            // serialization_failure, deadlock_detected, too_many_connections, cannot_connect_now
            Some("40001" | "40P01" | "53300" | "57P03") => StoreError::Unavailable(e.to_string()),
            _ => StoreError::Other(e.to_string()),
        },
        _ => StoreError::Other(e.to_string()),
    }
}

fn corrupt(what: &str) -> StoreError {
    StoreError::Other(format!("unreadable {what} in the database"))
}

/// An API enum as its JSON name (`admin`, `trial`, `jpeg`).
fn text<T: Serialize>(v: &T) -> StoreResult<String> {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => Ok(s),
        _ => Err(StoreError::Other("value is not a name".into())),
    }
}

fn from_text<T: DeserializeOwned>(s: String, what: &str) -> StoreResult<T> {
    serde_json::from_value(serde_json::Value::String(s)).map_err(|_| corrupt(what))
}

fn kind_text(k: TokenKind) -> &'static str {
    match k {
        TokenKind::Session => "session",
        TokenKind::Access => "access",
        TokenKind::Refresh => "refresh",
        TokenKind::DeviceCode => "deviceCode",
        TokenKind::PasswordReset => "passwordReset",
        TokenKind::EmailVerify => "emailVerify",
        TokenKind::Invite => "invite",
    }
}

fn get<'r, T>(row: &'r PgRow, col: &str) -> StoreResult<T>
where
    T: sqlx::Decode<'r, Postgres> + sqlx::Type<Postgres>,
{
    row.try_get(col).map_err(db)
}

fn u32_of(v: i64, what: &str) -> StoreResult<u32> {
    u32::try_from(v).map_err(|_| corrupt(what))
}

fn u64_of(v: i64, what: &str) -> StoreResult<u64> {
    u64::try_from(v).map_err(|_| corrupt(what))
}

fn big(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn org_id(s: String) -> StoreResult<OrgId> {
    OrgId::parse(&s).map_err(|_| corrupt("organisation id"))
}
fn user_id(s: String) -> StoreResult<UserId> {
    UserId::parse(&s).map_err(|_| corrupt("user id"))
}
fn device_id(s: String) -> StoreResult<DeviceId> {
    DeviceId::parse(&s).map_err(|_| corrupt("device id"))
}
fn survey_id(s: String) -> StoreResult<SurveyId> {
    SurveyId::parse(&s).map_err(|_| corrupt("survey id"))
}
fn sha(s: String) -> StoreResult<Sha256> {
    Sha256::parse(&s).map_err(|_| corrupt("hash"))
}

const USER_COLS: &str = "u.id, u.email, u.name, u.email_verified, u.created, u.password_hash, u.disabled";

fn user_record(r: &PgRow) -> StoreResult<UserRecord> {
    Ok(UserRecord {
        user: User {
            id: user_id(get(r, "id")?)?,
            email: get(r, "email")?,
            name: get(r, "name")?,
            email_verified: get(r, "email_verified")?,
            created: get(r, "created")?,
        },
        password_hash: get(r, "password_hash")?,
        disabled: get(r, "disabled")?,
    })
}

fn user_of(r: &PgRow) -> StoreResult<User> {
    Ok(user_record(r)?.user)
}

const SUB_COLS: &str = "s.org AS s_org, s.plan, s.state AS s_state, s.state_at, s.billing, s.seats, s.viewer_seats";

fn subscription(r: &PgRow) -> StoreResult<Subscription> {
    let at: i64 = get(r, "state_at")?;
    let state = match get::<String>(r, "s_state")?.as_str() {
        "trial" => SubscriptionState::Trial { ends: at },
        "active" => SubscriptionState::Active { until: at },
        "lapsed" => SubscriptionState::Lapsed { since: at },
        _ => return Err(corrupt("subscription state")),
    };
    Ok(Subscription {
        org: org_id(get(r, "s_org")?)?,
        plan: from_text(get(r, "plan")?, "plan")?,
        state,
        billing: from_text(get(r, "billing")?, "billing")?,
        seats: u32_of(get(r, "seats")?, "seats")?,
        viewer_seats: u32_of(get(r, "viewer_seats")?, "viewer seats")?,
    })
}

fn state_cols(s: &SubscriptionState) -> (&'static str, i64) {
    match *s {
        SubscriptionState::Trial { ends } => ("trial", ends),
        SubscriptionState::Active { until } => ("active", until),
        SubscriptionState::Lapsed { since } => ("lapsed", since),
    }
}

const MEMBER_COLS: &str = "m.user_id AS m_user, m.org AS m_org, m.role, m.joined";

fn membership(r: &PgRow) -> StoreResult<Membership> {
    Ok(Membership {
        user: user_id(get(r, "m_user")?)?,
        org: org_id(get(r, "m_org")?)?,
        role: from_text(get(r, "role")?, "role")?,
        joined: get(r, "joined")?,
    })
}

const DEVICE_COLS: &str = "id, org, user_id, machine, name, signed_in, last_seen, offline_until";

fn device(r: &PgRow) -> StoreResult<Device> {
    Ok(Device {
        id: device_id(get(r, "id")?)?,
        org: org_id(get(r, "org")?)?,
        user: user_id(get(r, "user_id")?)?,
        machine: get(r, "machine")?,
        name: get(r, "name")?,
        signed_in: get(r, "signed_in")?,
        last_seen: get(r, "last_seen")?,
        offline_until: get(r, "offline_until")?,
    })
}

const TOKEN_COLS: &str = "kind, digest, user_id, device, invite_org, invite_role, invite_email, expires";

fn token(kind: TokenKind, r: &PgRow) -> StoreResult<TokenRecord> {
    let invite = match (
        get::<Option<String>>(r, "invite_org")?,
        get::<Option<String>>(r, "invite_role")?,
        get::<Option<String>>(r, "invite_email")?,
    ) {
        (Some(org), Some(role), Some(email)) => Some((org_id(org)?, from_text(role, "role")?, email)),
        (None, None, None) => None,
        _ => return Err(corrupt("invite")),
    };
    Ok(TokenRecord {
        kind,
        digest: sha(get(r, "digest")?)?,
        user: user_id(get(r, "user_id")?)?,
        device: get::<Option<String>>(r, "device")?.map(device_id).transpose()?,
        invite,
        expires: get(r, "expires")?,
    })
}

fn code_state_cols(s: &DeviceCodeState) -> (&'static str, Option<&str>) {
    match s {
        DeviceCodeState::Pending => ("pending", None),
        DeviceCodeState::Approved(u) => ("approved", Some(u.as_str())),
        DeviceCodeState::Denied => ("denied", None),
        DeviceCodeState::Collected => ("collected", None),
    }
}

const CODE_COLS: &str = "digest, user_code, machine, device_name, state, approved_user, interval_secs, last_poll, expires";

fn device_code(r: &PgRow) -> StoreResult<DeviceCodeRecord> {
    let state = match (get::<String>(r, "state")?.as_str(), get::<Option<String>>(r, "approved_user")?) {
        ("pending", None) => DeviceCodeState::Pending,
        ("approved", Some(u)) => DeviceCodeState::Approved(user_id(u)?),
        ("denied", None) => DeviceCodeState::Denied,
        ("collected", None) => DeviceCodeState::Collected,
        _ => return Err(corrupt("sign-in code state")),
    };
    Ok(DeviceCodeRecord {
        digest: sha(get(r, "digest")?)?,
        user_code: UserCode::parse(&get::<String>(r, "user_code")?).ok_or_else(|| corrupt("sign-in code"))?,
        machine: get(r, "machine")?,
        device_name: get(r, "device_name")?,
        state,
        interval_secs: u64_of(get(r, "interval_secs")?, "interval")?,
        last_poll: get(r, "last_poll")?,
        expires: get(r, "expires")?,
    })
}

const SURVEY_COLS: &str = "id, org, client_survey_id, revision, project, line, schema_id, state, uploaded_by, uploaded, committed, \
                           assets, photos, photo_bytes, severity_counts";

fn survey_row(r: &PgRow) -> StoreResult<SurveySummary> {
    let counts: Vec<i64> = get(r, "severity_counts")?;
    let mut severity_counts = [0u32; 5];
    if counts.len() != severity_counts.len() {
        return Err(corrupt("severity counts"));
    }
    for (slot, c) in severity_counts.iter_mut().zip(counts) {
        *slot = u32_of(c, "severity count")?;
    }
    Ok(SurveySummary {
        id: survey_id(get(r, "id")?)?,
        org: org_id(get(r, "org")?)?,
        client_survey_id: get(r, "client_survey_id")?,
        revision: u32_of(get(r, "revision")?, "revision")?,
        project: get(r, "project")?,
        line: get(r, "line")?,
        schema_id: get(r, "schema_id")?,
        state: from_text(get(r, "state")?, "survey state")?,
        uploaded_by: user_id(get(r, "uploaded_by")?)?,
        uploaded: get(r, "uploaded")?,
        committed: get(r, "committed")?,
        assets: u32_of(get(r, "assets")?, "assets")?,
        photos: u32_of(get(r, "photos")?, "photos")?,
        photo_bytes: u64_of(get(r, "photo_bytes")?, "photo bytes")?,
        severity_counts,
    })
}

/// Locks the organisation's row for the rest of the transaction, so seat counts and admin counts
/// can't change under the caller. `NotFound` if it doesn't exist.
async fn lock_org(tx: &mut Transaction<'static, Postgres>, org: &OrgId) -> StoreResult<()> {
    sqlx::query("SELECT 1 FROM organisations WHERE id = $1 FOR UPDATE")
        .bind(org.as_str())
        .fetch_optional(&mut **tx)
        .await
        .map_err(db)?
        .ok_or(StoreError::NotFound)?;
    Ok(())
}

/// Desktop members (admins, inspectors) and viewers.
async fn member_counts(tx: &mut Transaction<'static, Postgres>, org: &OrgId) -> StoreResult<(u32, u32)> {
    let r = sqlx::query(
        "SELECT count(*) FILTER (WHERE role <> 'viewer') AS desktop, count(*) FILTER (WHERE role = 'viewer') AS viewers \
         FROM memberships WHERE org = $1",
    )
    .bind(org.as_str())
    .fetch_one(&mut **tx)
    .await
    .map_err(db)?;
    Ok((u32_of(get(&r, "desktop")?, "members")?, u32_of(get(&r, "viewers")?, "members")?))
}

async fn seats_free(tx: &mut Transaction<'static, Postgres>, org: &OrgId, role: Role, seats: Seats) -> StoreResult<Result<(), SeatsInUse>> {
    let (d, v) = member_counts(tx, org).await?;
    let (used, limit) = if role.is_desktop() { (d, seats.desktop) } else { (v, seats.viewers) };
    Ok(if used >= limit { Err(SeatsInUse { devices: used }) } else { Ok(()) })
}

async fn admins(tx: &mut Transaction<'static, Postgres>, org: &OrgId) -> StoreResult<i64> {
    sqlx::query_scalar("SELECT count(*) FROM memberships WHERE org = $1 AND role = 'admin'")
        .bind(org.as_str())
        .fetch_one(&mut **tx)
        .await
        .map_err(db)
}

/// The person's role in the scope's organisation; `NotFound` for anyone else's member.
async fn org_role(tx: &mut Transaction<'static, Postgres>, scope: &OrgScope, user: &UserId) -> StoreResult<Role> {
    let role: String = sqlx::query_scalar("SELECT role FROM memberships WHERE user_id = $1 AND org = $2")
        .bind(user.as_str())
        .bind(scope.org().as_str())
        .fetch_optional(&mut **tx)
        .await
        .map_err(db)?
        .ok_or(StoreError::NotFound)?;
    from_text(role, "role")
}

impl AccountStore for PgStore {
    async fn create_organisation(&self, new: NewOrganisation) -> StoreResult<()> {
        let mut tx = self.begin().await?;
        let o = &new.org;
        sqlx::query("INSERT INTO organisations (id, name, created) VALUES ($1, $2, $3)")
            .bind(o.id.as_str())
            .bind(&o.name)
            .bind(o.created)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let u = &new.owner;
        sqlx::query("INSERT INTO users (id, email, name, email_verified, created, password_hash) VALUES ($1, $2, $3, $4, $5, $6)")
            .bind(u.id.as_str())
            .bind(&u.email)
            .bind(&u.name)
            .bind(u.email_verified)
            .bind(u.created)
            .bind(&new.password_hash)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        sqlx::query("INSERT INTO memberships (user_id, org, role, joined) VALUES ($1, $2, 'admin', $3)")
            .bind(u.id.as_str())
            .bind(o.id.as_str())
            .bind(o.created)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        put_subscription(&mut tx, &new.subscription).await?;
        tx.commit().await.map_err(db)
    }

    async fn user_by_email(&self, email: &str) -> StoreResult<Option<UserRecord>> {
        let r = sqlx::query(&format!("SELECT {USER_COLS} FROM users u WHERE u.email = $1"))
            .bind(email)
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?;
        r.as_ref().map(user_record).transpose()
    }

    async fn user(&self, id: &UserId) -> StoreResult<Option<UserRecord>> {
        let r = sqlx::query(&format!("SELECT {USER_COLS} FROM users u WHERE u.id = $1"))
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?;
        r.as_ref().map(user_record).transpose()
    }

    async fn organisation(&self, id: &OrgId) -> StoreResult<Option<Organisation>> {
        let r = sqlx::query("SELECT id, name, created FROM organisations WHERE id = $1")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?;
        r.map(|r| Ok(Organisation { id: org_id(get(&r, "id")?)?, name: get(&r, "name")?, created: get(&r, "created")? })).transpose()
    }

    async fn membership(&self, user: &UserId) -> StoreResult<Option<(Membership, Subscription)>> {
        let r = sqlx::query(&format!(
            "SELECT {MEMBER_COLS}, {SUB_COLS} FROM memberships m JOIN subscriptions s ON s.org = m.org WHERE m.user_id = $1"
        ))
        .bind(user.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        r.map(|r| Ok((membership(&r)?, subscription(&r)?))).transpose()
    }

    async fn set_password(&self, user: &UserId, hash: &str, keep: Option<&Sha256>) -> StoreResult<()> {
        let mut tx = self.begin().await?;
        let n = sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
            .bind(user.as_str())
            .bind(hash)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        if n.rows_affected() == 0 {
            return Err(StoreError::NotFound);
        }
        sqlx::query("DELETE FROM tokens WHERE user_id = $1 AND kind IN ('session', 'access', 'refresh') AND digest IS DISTINCT FROM $2")
            .bind(user.as_str())
            .bind(keep.map(Sha256::as_str))
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        // Their computers' tokens go with them (ON DELETE CASCADE).
        sqlx::query("DELETE FROM devices WHERE user_id = $1").bind(user.as_str()).execute(&mut *tx).await.map_err(db)?;
        tx.commit().await.map_err(db)
    }

    async fn set_email_verified(&self, user: &UserId) -> StoreResult<()> {
        let n = sqlx::query("UPDATE users SET email_verified = true WHERE id = $1")
            .bind(user.as_str())
            .execute(&self.pool)
            .await
            .map_err(db)?;
        if n.rows_affected() == 0 { Err(StoreError::NotFound) } else { Ok(()) }
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
        let mut tx = self.begin().await?;
        lock_org(&mut tx, org).await?;
        let taken: Option<i32> =
            sqlx::query_scalar("SELECT 1 FROM users WHERE email = $1").bind(&user.email).fetch_optional(&mut *tx).await.map_err(db)?;
        if taken.is_some() {
            return Err(StoreError::Conflict("email"));
        }
        if let Err(full) = seats_free(&mut tx, org, role, seats).await? {
            return Ok(Err(full));
        }
        sqlx::query("INSERT INTO users (id, email, name, email_verified, created, password_hash) VALUES ($1, $2, $3, $4, $5, $6)")
            .bind(user.id.as_str())
            .bind(&user.email)
            .bind(&user.name)
            .bind(user.email_verified)
            .bind(user.created)
            .bind(password_hash)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        sqlx::query("INSERT INTO memberships (user_id, org, role, joined) VALUES ($1, $2, $3, $4)")
            .bind(user.id.as_str())
            .bind(org.as_str())
            .bind(text(&role)?)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(Ok(()))
    }

    async fn members(&self, scope: &OrgScope) -> StoreResult<Vec<(User, Membership)>> {
        let rows = sqlx::query(&format!(
            "SELECT {USER_COLS}, {MEMBER_COLS} FROM memberships m JOIN users u ON u.id = m.user_id WHERE m.org = $1 ORDER BY u.email"
        ))
        .bind(scope.org().as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.iter().map(|r| Ok((user_of(r)?, membership(r)?))).collect()
    }

    async fn set_role(&self, scope: &OrgScope, user: &UserId, role: Role, seats: Seats) -> StoreResult<Result<(), SeatsInUse>> {
        let mut tx = self.begin().await?;
        lock_org(&mut tx, scope.org()).await?;
        let current = org_role(&mut tx, scope, user).await?;
        // Waits for a sign-in in progress ([`DeviceStore::claim_seat`]), which then sees the new role.
        sqlx::query("SELECT 1 FROM users WHERE id = $1 FOR UPDATE").bind(user.as_str()).execute(&mut *tx).await.map_err(db)?;
        if current == role {
            return Ok(Ok(()));
        }
        if current == Role::Admin && admins(&mut tx, scope.org()).await? <= 1 {
            return Err(StoreError::Conflict("last_admin"));
        }
        if current.is_desktop() != role.is_desktop()
            && let Err(full) = seats_free(&mut tx, scope.org(), role, seats).await?
        {
            return Ok(Err(full));
        }
        sqlx::query("UPDATE memberships SET role = $2 WHERE user_id = $1")
            .bind(user.as_str())
            .bind(text(&role)?)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        if !role.is_desktop() {
            // A viewer has no seat: sign out their computers.
            sqlx::query("DELETE FROM devices WHERE user_id = $1").bind(user.as_str()).execute(&mut *tx).await.map_err(db)?;
        }
        tx.commit().await.map_err(db)?;
        Ok(Ok(()))
    }

    async fn remove_member(&self, scope: &OrgScope, user: &UserId) -> StoreResult<()> {
        let mut tx = self.begin().await?;
        lock_org(&mut tx, scope.org()).await?;
        if org_role(&mut tx, scope, user).await? == Role::Admin && admins(&mut tx, scope.org()).await? <= 1 {
            return Err(StoreError::Conflict("last_admin"));
        }
        // Membership, tokens and computers go with the person (ON DELETE CASCADE).
        sqlx::query("DELETE FROM users WHERE id = $1").bind(user.as_str()).execute(&mut *tx).await.map_err(db)?;
        tx.commit().await.map_err(db)
    }

    async fn set_subscription(&self, sub: Subscription) -> StoreResult<()> {
        let mut tx = self.begin().await?;
        lock_org(&mut tx, &sub.org).await?;
        put_subscription(&mut tx, &sub).await?;
        tx.commit().await.map_err(db)
    }

    async fn put_token(&self, t: TokenRecord) -> StoreResult<()> {
        let (org, role, email) = match &t.invite {
            Some((o, r, e)) => (Some(o.as_str()), Some(text(r)?), Some(e.as_str())),
            None => (None, None, None),
        };
        sqlx::query(
            "INSERT INTO tokens (kind, digest, user_id, device, invite_org, invite_role, invite_email, expires) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (kind, digest) DO UPDATE SET user_id = EXCLUDED.user_id, device = EXCLUDED.device, \
             invite_org = EXCLUDED.invite_org, invite_role = EXCLUDED.invite_role, invite_email = EXCLUDED.invite_email, \
             expires = EXCLUDED.expires",
        )
        .bind(kind_text(t.kind))
        .bind(t.digest.as_str())
        .bind(t.user.as_str())
        .bind(t.device.as_ref().map(DeviceId::as_str))
        .bind(org)
        .bind(role)
        .bind(email)
        .bind(t.expires)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn token(&self, kind: TokenKind, digest: &Sha256, now: i64) -> StoreResult<Option<TokenRecord>> {
        let r = sqlx::query(&format!("SELECT {TOKEN_COLS} FROM tokens WHERE kind = $1 AND digest = $2 AND expires > $3"))
            .bind(kind_text(kind))
            .bind(digest.as_str())
            .bind(now)
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?;
        r.map(|r| token(kind, &r)).transpose()
    }

    async fn take_token(&self, kind: TokenKind, digest: &Sha256, now: i64) -> StoreResult<Option<TokenRecord>> {
        // Deleted even when expired, as the memory store does; only a live one is returned.
        let r = sqlx::query(&format!("DELETE FROM tokens WHERE kind = $1 AND digest = $2 RETURNING {TOKEN_COLS}"))
            .bind(kind_text(kind))
            .bind(digest.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?;
        let t = r.map(|r| token(kind, &r)).transpose()?;
        Ok(t.filter(|t| t.expires > now))
    }

    async fn delete_token(&self, kind: TokenKind, digest: &Sha256) -> StoreResult<()> {
        sqlx::query("DELETE FROM tokens WHERE kind = $1 AND digest = $2")
            .bind(kind_text(kind))
            .bind(digest.as_str())
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn extend_token(&self, kind: TokenKind, digest: &Sha256, expires: i64) -> StoreResult<()> {
        sqlx::query("UPDATE tokens SET expires = GREATEST(expires, $3) WHERE kind = $1 AND digest = $2")
            .bind(kind_text(kind))
            .bind(digest.as_str())
            .bind(expires)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn purge_expired(&self, now: i64) -> StoreResult<u64> {
        let mut n = 0u64;
        for table in ["tokens", "device_codes", "retired_refresh"] {
            let r = sqlx::query(&format!("DELETE FROM {table} WHERE expires <= $1")).bind(now).execute(&self.pool).await.map_err(db)?;
            n = n.saturating_add(r.rows_affected());
        }
        Ok(n)
    }
}

async fn put_subscription(tx: &mut Transaction<'static, Postgres>, s: &Subscription) -> StoreResult<()> {
    let (state, at) = state_cols(&s.state);
    sqlx::query(
        "INSERT INTO subscriptions (org, plan, state, state_at, billing, seats, viewer_seats) VALUES ($1, $2, $3, $4, $5, $6, $7) \
         ON CONFLICT (org) DO UPDATE SET plan = EXCLUDED.plan, state = EXCLUDED.state, state_at = EXCLUDED.state_at, \
         billing = EXCLUDED.billing, seats = EXCLUDED.seats, viewer_seats = EXCLUDED.viewer_seats",
    )
    .bind(s.org.as_str())
    .bind(text(&s.plan)?)
    .bind(state)
    .bind(at)
    .bind(text(&s.billing)?)
    .bind(i64::from(s.seats))
    .bind(i64::from(s.viewer_seats))
    .execute(&mut **tx)
    .await
    .map_err(db)?;
    Ok(())
}

impl DeviceStore for PgStore {
    async fn put_device_code(&self, c: DeviceCodeRecord) -> StoreResult<()> {
        let (state, approved) = code_state_cols(&c.state);
        sqlx::query(&format!("INSERT INTO device_codes ({CODE_COLS}) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"))
            .bind(c.digest.as_str())
            .bind(c.user_code.as_str())
            .bind(&c.machine)
            .bind(&c.device_name)
            .bind(state)
            .bind(approved)
            .bind(big(c.interval_secs))
            .bind(c.last_poll)
            .bind(c.expires)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn device_code(&self, digest: &Sha256) -> StoreResult<Option<DeviceCodeRecord>> {
        let r = sqlx::query(&format!("SELECT {CODE_COLS} FROM device_codes WHERE digest = $1"))
            .bind(digest.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?;
        r.as_ref().map(device_code).transpose()
    }

    async fn device_code_by_user_code(&self, code: &UserCode, now: i64) -> StoreResult<Option<DeviceCodeRecord>> {
        let r = sqlx::query(&format!("SELECT {CODE_COLS} FROM device_codes WHERE user_code = $1 AND expires > $2"))
            .bind(code.as_str())
            .bind(now)
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?;
        r.as_ref().map(device_code).transpose()
    }

    async fn set_device_code_state(&self, digest: &Sha256, from: &DeviceCodeState, to: DeviceCodeState, now: i64) -> StoreResult<bool> {
        let (from_state, from_user) = code_state_cols(from);
        let (to_state, to_user) = code_state_cols(&to);
        // Compare-and-set in one statement: of two requests, only one sees `from`.
        let n = sqlx::query(
            "UPDATE device_codes SET state = $4, approved_user = $5, last_poll = $6 \
             WHERE digest = $1 AND state = $2 AND approved_user IS NOT DISTINCT FROM $3",
        )
        .bind(digest.as_str())
        .bind(from_state)
        .bind(from_user)
        .bind(to_state)
        .bind(to_user)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(n.rows_affected() == 1)
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
        let mut tx = self.begin().await?;
        // One sign-in per person at a time, so two computers can't both take the last place. The
        // role is read under the same lock [`AccountStore::set_role`] takes, so a person made a
        // viewer meanwhile gets no computer.
        let role: String = sqlx::query_scalar(
            "SELECT m.role FROM users u JOIN memberships m ON m.user_id = u.id WHERE u.id = $1 AND m.org = $2 FOR UPDATE OF u",
        )
        .bind(user.as_str())
        .bind(org.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?
        .ok_or(StoreError::NotFound)?;
        if !from_text::<Role>(role, "role")?.is_desktop() {
            return Err(StoreError::NotFound);
        }
        let same = sqlx::query(&format!(
            "UPDATE devices SET name = $3, last_seen = $4 WHERE user_id = $1 AND machine = $2 RETURNING {DEVICE_COLS}"
        ))
        .bind(user.as_str())
        .bind(machine)
        .bind(name)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?;
        if let Some(r) = same {
            let d = device(&r)?;
            tx.commit().await.map_err(db)?;
            return Ok(Ok(d));
        }
        let used: i64 = sqlx::query_scalar("SELECT count(*) FROM devices WHERE user_id = $1")
            .bind(user.as_str())
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?;
        let used = u32::try_from(used).unwrap_or(u32::MAX);
        if used >= limit {
            return Ok(Err(SeatsInUse { devices: used }));
        }
        let d = Device {
            id: DeviceId::from_random(crate::crypto::random_id()),
            org: org.clone(),
            user: user.clone(),
            machine: machine.to_string(),
            name: name.to_string(),
            signed_in: now,
            last_seen: now,
            offline_until: None,
        };
        sqlx::query(&format!("INSERT INTO devices ({DEVICE_COLS}) VALUES ($1, $2, $3, $4, $5, $6, $7, NULL)"))
            .bind(d.id.as_str())
            .bind(org.as_str())
            .bind(user.as_str())
            .bind(machine)
            .bind(name)
            .bind(now)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(Ok(d))
    }

    async fn rotate_refresh(&self, old: &Sha256, next: &Sha256, expires: i64, now: i64) -> StoreResult<Option<Device>> {
        let mut tx = self.begin().await?;
        // A second request with the same token waits here on the row, then finds it gone and the
        // retired record the first one left: a copy, so the computer is signed out.
        let live = sqlx::query("DELETE FROM tokens WHERE kind = 'refresh' AND digest = $1 RETURNING user_id, device, expires")
            .bind(old.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
        let live = match live {
            Some(r) if get::<i64>(&r, "expires")? > now => Some(r),
            _ => None,
        };
        let Some(rec) = live else {
            let retired: Option<String> = sqlx::query_scalar("DELETE FROM retired_refresh WHERE digest = $1 RETURNING device")
                .bind(old.as_str())
                .fetch_optional(&mut *tx)
                .await
                .map_err(db)?;
            if let Some(device) = retired {
                sqlx::query("DELETE FROM devices WHERE id = $1").bind(device).execute(&mut *tx).await.map_err(db)?;
            }
            tx.commit().await.map_err(db)?;
            return Ok(None);
        };
        let user: String = get(&rec, "user_id")?;
        let Some(device_ref) = get::<Option<String>>(&rec, "device")? else {
            tx.commit().await.map_err(db)?;
            return Ok(None);
        };
        let old_expires: i64 = get(&rec, "expires")?;
        let Some(r) = sqlx::query(&format!("UPDATE devices SET last_seen = $2 WHERE id = $1 RETURNING {DEVICE_COLS}"))
            .bind(&device_ref)
            .bind(now)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?
        else {
            tx.commit().await.map_err(db)?;
            return Ok(None);
        };
        let d = device(&r)?;
        // Kept as long as the old token would have lived; after that it can't be presented anyway.
        sqlx::query("INSERT INTO retired_refresh (digest, device, expires) VALUES ($1, $2, $3) ON CONFLICT (digest) DO NOTHING")
            .bind(old.as_str())
            .bind(&device_ref)
            .bind(old_expires)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        sqlx::query("INSERT INTO tokens (kind, digest, user_id, device, expires) VALUES ('refresh', $1, $2, $3, $4)")
            .bind(next.as_str())
            .bind(user)
            .bind(&device_ref)
            .bind(expires)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(Some(d))
    }

    async fn devices(&self, scope: &OrgScope, user: Option<&UserId>) -> StoreResult<Vec<Device>> {
        let rows = sqlx::query(&format!(
            "SELECT {DEVICE_COLS} FROM devices WHERE org = $1 AND ($2::text IS NULL OR user_id = $2) ORDER BY last_seen DESC, id"
        ))
        .bind(scope.org().as_str())
        .bind(user.map(UserId::as_str))
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.iter().map(device).collect()
    }

    async fn revoke_device(&self, scope: &OrgScope, device: &DeviceId) -> StoreResult<()> {
        let n = sqlx::query("DELETE FROM devices WHERE id = $1 AND org = $2")
            .bind(device.as_str())
            .bind(scope.org().as_str())
            .execute(&self.pool)
            .await
            .map_err(db)?;
        if n.rows_affected() == 0 { Err(StoreError::NotFound) } else { Ok(()) }
    }

    async fn mark_offline(&self, scope: &OrgScope, device: &DeviceId, until: i64) -> StoreResult<()> {
        let n = sqlx::query("UPDATE devices SET offline_until = GREATEST(COALESCE(offline_until, $3), $3) WHERE id = $1 AND org = $2")
            .bind(device.as_str())
            .bind(scope.org().as_str())
            .bind(until)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        if n.rows_affected() == 0 { Err(StoreError::NotFound) } else { Ok(()) }
    }

    async fn sign_out(&self, refresh: &Sha256) -> StoreResult<()> {
        sqlx::query("DELETE FROM devices WHERE id = (SELECT device FROM tokens WHERE kind = 'refresh' AND digest = $1)")
            .bind(refresh.as_str())
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }
}

/// Photos a survey lists that the organisation doesn't store yet, in hash order.
async fn missing<'e, E>(ex: E, org: &OrgId, survey: &SurveyId) -> StoreResult<Vec<Sha256>>
where
    E: sqlx::Executor<'e, Database = Postgres>,
{
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT sp.sha FROM survey_photos sp LEFT JOIN photos p ON p.org = $1 AND p.sha = sp.sha \
         WHERE sp.survey = $2 AND p.sha IS NULL ORDER BY sp.sha",
    )
    .bind(org.as_str())
    .bind(survey.as_str())
    .fetch_all(ex)
    .await
    .map_err(db)?;
    rows.into_iter().map(sha).collect()
}

impl SurveyStore for PgStore {
    async fn create_survey(&self, scope: &OrgScope, manifest: &SurveyManifest, now: i64) -> StoreResult<SurveyCreated> {
        let photos = manifest.photos().map_err(|_| StoreError::Conflict("photo"))?;
        let org = scope.org();
        let existing = |r: Option<String>| r.map(survey_id).transpose();
        let find = "SELECT id FROM surveys WHERE org = $1 AND client_survey_id = $2 AND revision = $3";
        let rev = i64::from(manifest.revision);
        let found: Option<String> = sqlx::query_scalar(find)
            .bind(org.as_str())
            .bind(&manifest.client_survey_id)
            .bind(rev)
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?;
        if let Some(id) = existing(found)? {
            let missing = missing(&self.pool, org, &id).await?;
            return Ok(SurveyCreated { survey: id, missing, existing: true });
        }
        let mut counts = [0i64; 5];
        for a in &manifest.assets {
            if let Some(i) = Severity::ALL.iter().position(|s| *s == a.asset.severity) {
                counts[i] = counts[i].saturating_add(1);
            }
        }
        let id = SurveyId::from_random(crate::crypto::random_id());
        let mut tx = self.begin().await?;
        let inserted: Option<String> = sqlx::query_scalar(&format!(
            "INSERT INTO surveys ({SURVEY_COLS}) VALUES ($1, $2, $3, $4, $5, $6, $7, 'uploading', $8, $9, NULL, $10, $11, $12, $13) \
             ON CONFLICT ON CONSTRAINT surveys_revision DO NOTHING RETURNING id"
        ))
        .bind(id.as_str())
        .bind(org.as_str())
        .bind(&manifest.client_survey_id)
        .bind(rev)
        .bind(&manifest.project)
        .bind(&manifest.line)
        .bind(&manifest.schema_id)
        .bind(scope.user().as_str())
        .bind(now)
        .bind(big(manifest.assets.len() as u64))
        .bind(big(photos.len() as u64))
        .bind(big(manifest.photo_bytes()))
        .bind(counts.to_vec())
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?;
        if inserted.is_none() {
            // The same upload arrived twice at once; the other request made it.
            drop(tx);
            let found: Option<String> = sqlx::query_scalar(find)
                .bind(org.as_str())
                .bind(&manifest.client_survey_id)
                .bind(rev)
                .fetch_optional(&self.pool)
                .await
                .map_err(db)?;
            let id = existing(found)?.ok_or(StoreError::Conflict("survey"))?;
            let missing = missing(&self.pool, org, &id).await?;
            return Ok(SurveyCreated { survey: id, missing, existing: true });
        }
        // One statement for every photo (up to MAX_ASSETS × photos each), as three arrays.
        let mut shas = Vec::with_capacity(photos.len());
        let mut sizes = Vec::with_capacity(photos.len());
        let mut kinds = Vec::with_capacity(photos.len());
        for (h, b, k) in &photos {
            shas.push(h.as_str().to_string());
            sizes.push(big(*b));
            kinds.push(text(k)?);
        }
        sqlx::query(
            "INSERT INTO survey_photos (survey, sha, bytes, kind) SELECT $1, * FROM UNNEST($2::text[], $3::bigint[], $4::text[]) \
             ON CONFLICT (survey, sha) DO NOTHING",
        )
        .bind(id.as_str())
        .bind(shas)
        .bind(sizes)
        .bind(kinds)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        let missing = missing(&mut *tx, org, &id).await?;
        tx.commit().await.map_err(db)?;
        Ok(SurveyCreated { survey: id, missing, existing: false })
    }

    async fn expected_photo(&self, scope: &OrgScope, survey: &SurveyId, sha: &Sha256) -> StoreResult<Option<(u64, PhotoType)>> {
        let r = sqlx::query(
            "SELECT s.id, sp.bytes, sp.kind FROM surveys s LEFT JOIN survey_photos sp ON sp.survey = s.id AND sp.sha = $3 \
             WHERE s.id = $1 AND s.org = $2",
        )
        .bind(survey.as_str())
        .bind(scope.org().as_str())
        .bind(sha.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?
        .ok_or(StoreError::NotFound)?;
        match (get::<Option<i64>>(&r, "bytes")?, get::<Option<String>>(&r, "kind")?) {
            (Some(b), Some(k)) => Ok(Some((u64_of(b, "photo size")?, from_text(k, "photo type")?))),
            _ => Ok(None),
        }
    }

    async fn record_photo(&self, scope: &OrgScope, sha: &Sha256, bytes: u64) -> StoreResult<()> {
        sqlx::query("INSERT INTO photos (org, sha, bytes) VALUES ($1, $2, $3) ON CONFLICT (org, sha) DO UPDATE SET bytes = EXCLUDED.bytes")
            .bind(scope.org().as_str())
            .bind(sha.as_str())
            .bind(big(bytes))
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn commit(&self, scope: &OrgScope, survey: &SurveyId, now: i64) -> StoreResult<Result<SurveySummary, Vec<Sha256>>> {
        let mut tx = self.begin().await?;
        sqlx::query("SELECT 1 FROM surveys WHERE id = $1 AND org = $2 FOR UPDATE")
            .bind(survey.as_str())
            .bind(scope.org().as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?
            .ok_or(StoreError::NotFound)?;
        let m = missing(&mut *tx, scope.org(), survey).await?;
        if !m.is_empty() {
            return Ok(Err(m));
        }
        let r = sqlx::query(&format!(
            "UPDATE surveys SET state = 'committed', committed = COALESCE(committed, $2) WHERE id = $1 RETURNING {SURVEY_COLS}"
        ))
        .bind(survey.as_str())
        .bind(now)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        let s = survey_row(&r)?;
        tx.commit().await.map_err(db)?;
        Ok(Ok(s))
    }

    async fn surveys(&self, scope: &OrgScope) -> StoreResult<Vec<SurveySummary>> {
        let rows = sqlx::query(&format!("SELECT {SURVEY_COLS} FROM surveys WHERE org = $1 ORDER BY uploaded DESC, id"))
            .bind(scope.org().as_str())
            .fetch_all(&self.pool)
            .await
            .map_err(db)?;
        rows.iter().map(survey_row).collect()
    }

    async fn storage_used(&self, scope: &OrgScope) -> StoreResult<u64> {
        let n: i64 = sqlx::query_scalar("SELECT COALESCE(sum(bytes), 0)::bigint FROM photos WHERE org = $1")
            .bind(scope.org().as_str())
            .fetch_one(&self.pool)
            .await
            .map_err(db)?;
        u64_of(n, "storage")
    }
}

/// PostgreSQL for the records, with photos in `Bl` (the server's disk now; R2 or Supabase Storage
/// with the hosting choice) and mail through `M`.
pub struct PgBackend<Bl, M> {
    pub store: PgStore,
    pub blobs: Bl,
    pub mail: M,
}

impl<Bl: BlobStore + 'static, M: Mailer + 'static> crate::Backend for PgBackend<Bl, M> {
    type Accounts = PgStore;
    type Devices = PgStore;
    type Surveys = PgStore;
    type Blobs = Bl;
    type Mail = M;

    fn accounts(&self) -> &PgStore {
        &self.store
    }
    fn devices(&self) -> &PgStore {
        &self.store
    }
    fn surveys(&self) -> &PgStore {
        &self.store
    }
    fn blobs(&self) -> &Bl {
        &self.blobs
    }
    fn mail(&self) -> &M {
        &self.mail
    }
}
