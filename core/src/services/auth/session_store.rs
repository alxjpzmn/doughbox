use chrono::{DateTime, Duration, Utc};
use tokio_postgres::{GenericClient, Row, Transaction};

use crate::database::{
    models::auth::{AccountStatus, AuthSession, AuthUser, PendingTotp, SessionStage, UserId},
    queries::auth,
};

use super::{digest, random_credential, AuthError};

pub(crate) enum SessionGrant {
    Anonymous,
    MfaRequired {
        user_id: UserId,
        auth_version: i64,
    },
    EnrollmentRequired {
        user_id: UserId,
        auth_version: i64,
    },
    Authenticated {
        user_id: UserId,
        auth_version: i64,
        absolute_expires_at: DateTime<Utc>,
        reauthenticated_at: DateTime<Utc>,
        pending_totp: Option<PendingTotp>,
    },
}

impl SessionGrant {
    pub(crate) fn full(user: &AuthUser, now: DateTime<Utc>) -> Self {
        Self::Authenticated {
            user_id: user.id.clone(),
            auth_version: user.auth_version,
            absolute_expires_at: now + Duration::hours(12),
            reauthenticated_at: now,
            pending_totp: None,
        }
    }
}

// The raw cookie may only leave a handler after the caller's transaction commits.
// Deliberately neither Debug nor Serialize.
pub struct IssuedSession {
    pub session: AuthSession,
    pub raw_cookie: String,
}

pub fn session_from_row(row: Row) -> Result<AuthSession, AuthError> {
    let pending_id: Option<String> = row.get("pending_totp_id");
    let pending_secret: Option<String> = row.get("pending_totp_secret_encrypted");
    let pending_expiry: Option<DateTime<Utc>> = row.get("pending_totp_expires_at");
    let pending_totp = match (pending_id, pending_secret, pending_expiry) {
        (None, None, None) => None,
        (Some(id), Some(secret_encrypted), Some(expires_at)) => Some(PendingTotp {
            id,
            secret_encrypted,
            expires_at,
        }),
        _ => return Err(AuthError::Unavailable),
    };
    Ok(AuthSession {
        id_hash: row.get("id_hash"),
        stage: row
            .get::<_, &str>("stage")
            .parse()
            .map_err(|_| AuthError::Unavailable)?,
        revision: row.get("revision"),
        user_id: row
            .get::<_, Option<String>>("user_id")
            .map(UserId::from_database),
        auth_version: row.get("auth_version"),
        csrf_token: row.get("csrf_token"),
        last_seen_at: row.get("last_seen_at"),
        expires_at: row.get("expires_at"),
        absolute_expires_at: row.get("absolute_expires_at"),
        reauthenticated_at: row.get("reauthenticated_at"),
        failed_factor_attempts: row.get("failed_factor_attempts"),
        pending_totp,
        link_id: row.get("link_id"),
        link_bound_until: row.get("link_bound_until"),
    })
}

pub fn session_digest(raw: &str) -> String {
    digest("doughbox-auth-session-v1", raw)
}

fn unexpired(session: &AuthSession, now: DateTime<Utc>) -> bool {
    session.expires_at > now && session.absolute_expires_at > now
}

fn check_account(session: &AuthSession, user: Option<&AuthUser>) -> Result<(), AuthError> {
    if session.stage == SessionStage::Anonymous {
        if session.user_id.is_some() || session.auth_version.is_some() {
            return Err(AuthError::Unavailable);
        }
        return Ok(());
    }
    let user = user.ok_or(AuthError::Unauthenticated)?;
    if session.user_id.as_ref() != Some(&user.id)
        || session.auth_version != Some(user.auth_version)
        || user.status == AccountStatus::Disabled
    {
        return Err(AuthError::Unauthenticated);
    }
    match session.stage {
        SessionStage::Authenticated | SessionStage::MfaRequired if !user.fully_enrolled() => {
            Err(AuthError::Unauthenticated)
        }
        SessionStage::EnrollmentRequired
            if user.status != AccountStatus::Pending
                || user.username.is_none()
                || user.password_hash.is_none() =>
        {
            Err(AuthError::Unauthenticated)
        }
        _ => Ok(()),
    }
}

/// Load and touch only idle metadata. A stale request cannot restore auth state.
pub async fn load(
    client: &impl GenericClient,
    raw: &str,
    now: DateTime<Utc>,
) -> Result<Option<AuthSession>, AuthError> {
    if raw.len() != 43
        || !raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Ok(None);
    }
    let id_hash = session_digest(raw);
    let Some(row) = client
        .query_opt("SELECT * FROM auth_sessions WHERE id_hash=$1", &[&id_hash])
        .await?
    else {
        return Ok(None);
    };
    let mut session = session_from_row(row)?;
    if !unexpired(&session, now) {
        return Ok(None);
    }
    let user = match &session.user_id {
        Some(user_id) => auth::by_id(user_id, client).await?,
        None => None,
    };
    if check_account(&session, user.as_ref()).is_err() {
        return Ok(None);
    }
    if session.stage == SessionStage::Authenticated {
        let expires_at = (now + Duration::minutes(30)).min(session.absolute_expires_at);
        let row = client.query_opt(
            "UPDATE auth_sessions SET last_seen_at=GREATEST(last_seen_at,$3),expires_at=GREATEST(expires_at,$4) WHERE id_hash=$1 AND revision=$2 AND expires_at>$3 AND absolute_expires_at>$3 RETURNING last_seen_at,expires_at",
            &[&id_hash, &session.revision, &now, &expires_at],
        ).await?;
        let Some(row) = row else {
            return Err(AuthError::StateChanged);
        };
        session.last_seen_at = row.get(0);
        session.expires_at = row.get(1);
    }
    Ok(Some(session))
}

/// Caller must already hold auth_instance (when needed), then all affected user
/// and link locks. Re-locking the source owner here cannot upgrade stale authority.
pub async fn lock_source(
    tx: &Transaction<'_>,
    expected: &AuthSession,
    now: DateTime<Utc>,
) -> Result<AuthSession, AuthError> {
    let user =
        match &expected.user_id {
            Some(user_id) => Some(auth::lock_user(user_id, tx).await.map_err(
                |error| match error {
                    AuthError::NotFound => AuthError::StateChanged,
                    other => other,
                },
            )?),
            None => None,
        };
    let row = tx
        .query_opt(
            "SELECT * FROM auth_sessions WHERE id_hash=$1 FOR UPDATE",
            &[&expected.id_hash],
        )
        .await?
        .ok_or(AuthError::StateChanged)?;
    let locked = session_from_row(row)?;
    if locked.revision != expected.revision
        || locked.stage != expected.stage
        || locked.user_id != expected.user_id
        || locked.auth_version != expected.auth_version
        || locked.link_id != expected.link_id
        || locked.link_bound_until != expected.link_bound_until
        || locked.pending_totp.as_ref().map(|p| (&p.id, p.expires_at))
            != expected
                .pending_totp
                .as_ref()
                .map(|p| (&p.id, p.expires_at))
        || !unexpired(&locked, now)
    {
        return Err(AuthError::StateChanged);
    }
    check_account(&locked, user.as_ref()).map_err(|_| AuthError::StateChanged)?;
    Ok(locked)
}

pub(crate) async fn issue_successor_locked(
    tx: &Transaction<'_>,
    source: &AuthSession,
    grant: SessionGrant,
    now: DateTime<Utc>,
) -> Result<IssuedSession, AuthError> {
    // No account/version re-read here: a self-service transition may have just
    // bumped its own version while retaining the locked source for consumption.
    let deleted = tx
        .execute(
            "DELETE FROM auth_sessions WHERE id_hash=$1 AND revision=$2",
            &[&source.id_hash, &source.revision],
        )
        .await?;
    if deleted != 1 {
        return Err(AuthError::StateChanged);
    }
    insert(tx, grant, now).await
}

pub(crate) async fn create_anonymous(
    tx: &Transaction<'_>,
    now: DateTime<Utc>,
) -> Result<IssuedSession, AuthError> {
    insert(tx, SessionGrant::Anonymous, now).await
}

async fn insert(
    tx: &Transaction<'_>,
    grant: SessionGrant,
    now: DateTime<Utc>,
) -> Result<IssuedSession, AuthError> {
    let (stage, user_id, version, absolute, proof, pending) = match grant {
        SessionGrant::Anonymous => (
            SessionStage::Anonymous,
            None,
            None,
            now + Duration::minutes(30),
            None,
            None,
        ),
        SessionGrant::MfaRequired {
            user_id,
            auth_version,
        } => (
            SessionStage::MfaRequired,
            Some(user_id),
            Some(auth_version),
            now + Duration::minutes(5),
            None,
            None,
        ),
        SessionGrant::EnrollmentRequired {
            user_id,
            auth_version,
        } => (
            SessionStage::EnrollmentRequired,
            Some(user_id),
            Some(auth_version),
            now + Duration::minutes(10),
            None,
            None,
        ),
        SessionGrant::Authenticated {
            user_id,
            auth_version,
            absolute_expires_at,
            reauthenticated_at,
            pending_totp,
        } => {
            if absolute_expires_at <= now || reauthenticated_at > now {
                return Err(AuthError::Unauthenticated);
            }
            (
                SessionStage::Authenticated,
                Some(user_id),
                Some(auth_version),
                absolute_expires_at,
                Some(reauthenticated_at),
                pending_totp.filter(|p| p.expires_at > now),
            )
        }
    };
    let expires_at = if stage == SessionStage::Authenticated {
        (now + Duration::minutes(30)).min(absolute)
    } else {
        absolute
    };
    let owner = user_id.as_ref().map(UserId::as_str);
    let pending_id = pending.as_ref().map(|p| p.id.as_str());
    let pending_secret = pending.as_ref().map(|p| p.secret_encrypted.as_str());
    let pending_expiry = pending.as_ref().map(|p| p.expires_at);
    for _ in 0..4 {
        let raw_cookie = random_credential("")?;
        let id_hash = session_digest(&raw_cookie);
        let csrf = random_credential("")?;
        let row = tx.query_opt(
            "INSERT INTO auth_sessions(id_hash,stage,user_id,auth_version,csrf_token,created_at,last_seen_at,expires_at,absolute_expires_at,reauthenticated_at,pending_totp_id,pending_totp_secret_encrypted,pending_totp_expires_at) VALUES($1,$2,$3,$4,$5,$6,$6,$7,$8,$9,$10,$11,$12) ON CONFLICT(id_hash) DO NOTHING RETURNING *",
            &[&id_hash, &stage.as_str(), &owner, &version, &csrf, &now, &expires_at, &absolute, &proof, &pending_id, &pending_secret, &pending_expiry],
        ).await?;
        if let Some(row) = row {
            return Ok(IssuedSession {
                session: session_from_row(row)?,
                raw_cookie,
            });
        }
    }
    Err(AuthError::Unavailable)
}

pub async fn delete_locked(tx: &Transaction<'_>, source: &AuthSession) -> Result<(), AuthError> {
    if tx
        .execute(
            "DELETE FROM auth_sessions WHERE id_hash=$1 AND revision=$2",
            &[&source.id_hash, &source.revision],
        )
        .await?
        != 1
    {
        return Err(AuthError::StateChanged);
    }
    Ok(())
}

/// Return the failure only AFTER the caller commits this in-place mutation.
pub async fn failed_factor_locked(
    tx: &Transaction<'_>,
    source: &AuthSession,
) -> Result<(), AuthError> {
    if matches!(
        source.stage,
        SessionStage::MfaRequired | SessionStage::EnrollmentRequired
    ) {
        if source.failed_factor_attempts >= 4 {
            return delete_locked(tx, source).await;
        }
        tx.execute("UPDATE auth_sessions SET failed_factor_attempts=failed_factor_attempts+1,revision=revision+1 WHERE id_hash=$1 AND revision=$2", &[&source.id_hash, &source.revision]).await?;
    }
    Ok(())
}

pub async fn set_pending_locked(
    tx: &Transaction<'_>,
    source: &AuthSession,
    pending: Option<&PendingTotp>,
) -> Result<AuthSession, AuthError> {
    let id = pending.map(|p| p.id.as_str());
    let secret = pending.map(|p| p.secret_encrypted.as_str());
    let expiry = pending.map(|p| p.expires_at);
    let row = tx.query_opt("UPDATE auth_sessions SET pending_totp_id=$3,pending_totp_secret_encrypted=$4,pending_totp_expires_at=$5,revision=revision+1 WHERE id_hash=$1 AND revision=$2 RETURNING *", &[&source.id_hash, &source.revision, &id, &secret, &expiry]).await?.ok_or(AuthError::StateChanged)?;
    session_from_row(row)
}

pub async fn bind_link_locked(
    tx: &Transaction<'_>,
    source: &AuthSession,
    link_id: &str,
    until: DateTime<Utc>,
) -> Result<AuthSession, AuthError> {
    let row = tx.query_opt("UPDATE auth_sessions SET link_id=$3,link_bound_until=$4,revision=revision+1 WHERE id_hash=$1 AND revision=$2 RETURNING *", &[&source.id_hash, &source.revision, &link_id, &until]).await?.ok_or(AuthError::StateChanged)?;
    session_from_row(row)
}
