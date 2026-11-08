use std::net::IpAddr;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use tokio_postgres::Transaction;
use typeshare::typeshare;
use utoipa::ToSchema;

use crate::database::{
    db_client,
    models::auth::{AuthSession, AuthUser, CurrentUser, PendingTotp, SessionStage, UserId},
    queries::auth,
};

use super::{
    password::validate_password,
    rate_limit::{enforce, RatePurpose},
    require_browser, require_recent,
    session_store::{
        failed_factor_locked, issue_successor_locked, lock_source, set_pending_locked,
        IssuedSession, SessionGrant,
    },
    totp, AuthError, AuthService,
};

#[typeshare]
#[derive(Serialize, ToSchema)]
pub struct TotpStartResponse {
    pub enrollment_id: String,
    pub secret_base32: String,
    pub otpauth_uri: String,
    pub qr_png_base64: String,
    pub expires_at: DateTime<Utc>,
}

#[typeshare]
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TotpConfirmRequest {
    pub enrollment_id: String,
    pub code: String,
}

#[typeshare]
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PendingTotpRequest {
    pub enrollment_id: String,
}

#[typeshare]
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PasswordChangeRequest {
    pub new_password: String,
}

#[typeshare]
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EmptyRequest {}

// Full-session callers must carry the middleware's captured actor, not an actor
// reconstructed from the account after acquiring its lock.
fn check_authority<'a>(
    source: &'a AuthSession,
    actor: Option<&CurrentUser>,
    now: DateTime<Utc>,
) -> Result<&'a UserId, AuthError> {
    match source.stage {
        SessionStage::EnrollmentRequired => {}
        SessionStage::Authenticated => {
            let actor = actor.ok_or(AuthError::Unauthenticated)?;
            require_browser(actor)?;
            if source.user_id.as_ref() != Some(&actor.id)
                || source.auth_version != Some(actor.auth_version)
            {
                return Err(AuthError::StateChanged);
            }
            require_recent(source, now)?;
        }
        SessionStage::Anonymous => return Err(AuthError::Unauthenticated),
        SessionStage::MfaRequired => return Err(AuthError::TwoFactorRequired),
    }
    source.user_id.as_ref().ok_or(AuthError::Unauthenticated)
}

async fn lock_account(
    tx: &Transaction<'_>,
    source: &AuthSession,
    actor: Option<&CurrentUser>,
) -> Result<AuthUser, AuthError> {
    let user_id = check_authority(source, actor, Utc::now())?;
    let user = auth::lock_user(user_id, tx).await?;
    if let Some(actor) = actor {
        auth::check_actor(actor, &user, false)?;
    }
    Ok(user)
}

fn preserved_full_grant(
    source: &AuthSession,
    user_id: UserId,
    auth_version: i64,
) -> Result<SessionGrant, AuthError> {
    Ok(SessionGrant::Authenticated {
        user_id,
        auth_version,
        absolute_expires_at: source.absolute_expires_at,
        reauthenticated_at: source
            .reauthenticated_at
            .ok_or(AuthError::ReauthenticationRequired)?,
        pending_totp: None,
    })
}

impl AuthService {
    /// Replace this session's unconfirmed secret; the active factor is untouched.
    /// The one-time enrollment material is exposed only after the revision commits.
    pub async fn totp_start(
        &self,
        source: &AuthSession,
        actor: Option<&CurrentUser>,
        peer: IpAddr,
    ) -> Result<(AuthSession, TotpStartResponse), AuthError> {
        let user_id = check_authority(source, actor, Utc::now())?;
        enforce(RatePurpose::Factor, user_id.as_str(), peer).await?;
        let mut db = db_client().await.map_err(|_| AuthError::Unavailable)?;
        let user = auth::by_id(user_id, &db)
            .await?
            .ok_or(AuthError::StateChanged)?;
        let username = user.username.as_deref().ok_or(AuthError::StateChanged)?;
        let secret = totp::new_secret()?;
        let encrypted = self.encrypt_totp(user_id, true, &secret)?;
        // QR generation is local, but deliberately outside account/session locks.
        let enrollment = totp::enrollment(secret, username)?;
        let enrollment_id = uuid::Uuid::new_v4().to_string();

        let tx = db.transaction().await?;
        lock_account(&tx, source, actor).await?;
        let locked = lock_source(&tx, source, Utc::now()).await?;
        let now = Utc::now();
        check_authority(&locked, actor, now)?;
        let pending = PendingTotp {
            id: enrollment_id.clone(),
            secret_encrypted: encrypted,
            expires_at: now + Duration::minutes(10),
        };
        let session = set_pending_locked(&tx, &locked, Some(&pending)).await?;
        tx.commit().await?;
        Ok((
            session,
            TotpStartResponse {
                enrollment_id,
                secret_base32: enrollment.secret_base32,
                otpauth_uri: enrollment.otpauth_uri,
                qr_png_base64: enrollment.qr_png_base64,
                expires_at: pending.expires_at,
            },
        ))
    }

    pub async fn totp_cancel(
        &self,
        source: &AuthSession,
        actor: Option<&CurrentUser>,
        enrollment_id: &str,
    ) -> Result<AuthSession, AuthError> {
        check_authority(source, actor, Utc::now())?;
        let mut db = db_client().await.map_err(|_| AuthError::Unavailable)?;
        let tx = db.transaction().await?;
        lock_account(&tx, source, actor).await?;
        let locked = lock_source(&tx, source, Utc::now()).await?;
        check_authority(&locked, actor, Utc::now())?;
        if locked
            .pending_totp
            .as_ref()
            .map(|pending| pending.id.as_str())
            != Some(enrollment_id)
        {
            return Err(AuthError::StateChanged);
        }
        let session = set_pending_locked(&tx, &locked, None).await?;
        tx.commit().await?;
        Ok(session)
    }

    pub async fn totp_confirm(
        &self,
        source: &AuthSession,
        actor: Option<&CurrentUser>,
        enrollment_id: &str,
        code: &str,
        peer: IpAddr,
    ) -> Result<(IssuedSession, Vec<String>), AuthError> {
        let user_id = check_authority(source, actor, Utc::now())?;
        enforce(RatePurpose::Factor, user_id.as_str(), peer).await?;
        let codes = totp::generate_recovery_codes()?;
        let mut db = db_client().await.map_err(|_| AuthError::Unavailable)?;
        let tx = db.transaction().await?;
        let user = lock_account(&tx, source, actor).await?;
        auth::lock_links(&user.id, &tx, false).await?;
        let locked = lock_source(&tx, source, Utc::now()).await?;
        let now = Utc::now();
        check_authority(&locked, actor, now)?;
        let pending = locked
            .pending_totp
            .as_ref()
            .ok_or(AuthError::StateChanged)?;
        if pending.id != enrollment_id {
            return Err(AuthError::StateChanged);
        }
        let accepted_step = if pending.expires_at > now {
            let secret = self.decrypt_totp(&user.id, true, &pending.secret_encrypted)?;
            let seconds = u64::try_from(now.timestamp()).map_err(|_| AuthError::Unavailable)?;
            // Replacement uses a different secret, so the old secret's counter
            // must not reject the new factor's first accepted step.
            totp::verify_totp_step(&secret, code, seconds, None).map(|step| (secret, step))
        } else {
            None
        };
        let Some((secret, step)) = accepted_step else {
            failed_factor_locked(&tx, &locked).await?;
            tx.commit().await?;
            return Err(AuthError::InvalidVerificationCode);
        };
        let active = self.encrypt_totp(&user.id, false, &secret)?;
        let version: i64 = tx.query_one(
            "UPDATE auth_users SET totp_secret_encrypted=$2,last_totp_step=$3,status='active',auth_version=auth_version+1 WHERE id=$1 RETURNING auth_version",
            &[&user.id.as_str(), &active, &step],
        ).await?.get(0);
        replace_recovery_codes_locked(&user.id, &tx, &codes).await?;
        auth::invalidate_links(&user.id, &tx, false, now).await?;
        auth::revoke_authority(&user.id, &tx, Some(&locked.id_hash), now).await?;
        let absolute_expires_at = if locked.stage == SessionStage::Authenticated {
            locked.absolute_expires_at
        } else {
            now + Duration::hours(12)
        };
        let successor = issue_successor_locked(
            &tx,
            &locked,
            SessionGrant::Authenticated {
                user_id: user.id,
                auth_version: version,
                absolute_expires_at,
                reauthenticated_at: now,
                pending_totp: None,
            },
            now,
        )
        .await?;
        tx.commit().await?;
        Ok((successor, codes))
    }

    pub async fn change_password(
        &self,
        actor: &CurrentUser,
        source: &AuthSession,
        new_password: String,
    ) -> Result<IssuedSession, AuthError> {
        require_browser(actor)?;
        require_recent(source, Utc::now())?;
        check_authority(source, Some(actor), Utc::now())?;
        validate_password(&new_password)?;
        let hash = self.passwords.hash(new_password).await?;
        let mut db = db_client().await.map_err(|_| AuthError::Unavailable)?;
        let tx = db.transaction().await?;
        let user = auth::lock_actor(&tx, actor, false).await?;
        auth::lock_links(&user.id, &tx, false).await?;
        let locked = lock_source(&tx, source, Utc::now()).await?;
        let now = Utc::now();
        require_recent(&locked, now)?;
        let version: i64 = tx.query_one(
            "UPDATE auth_users SET password_hash=$2,auth_version=auth_version+1 WHERE id=$1 RETURNING auth_version",
            &[&user.id.as_str(), &hash],
        ).await?.get(0);
        auth::invalidate_links(&user.id, &tx, false, now).await?;
        auth::revoke_authority(&user.id, &tx, Some(&locked.id_hash), now).await?;
        let grant = preserved_full_grant(&locked, user.id, version)?;
        let successor = issue_successor_locked(&tx, &locked, grant, now).await?;
        tx.commit().await?;
        Ok(successor)
    }

    /// Rotate the source and replace every recovery-code hash, without changing
    /// account version or retaining the session's unconfirmed factor material.
    pub async fn regenerate_recovery_codes(
        &self,
        actor: &CurrentUser,
        source: &AuthSession,
    ) -> Result<(IssuedSession, Vec<String>), AuthError> {
        require_browser(actor)?;
        require_recent(source, Utc::now())?;
        check_authority(source, Some(actor), Utc::now())?;
        let codes = totp::generate_recovery_codes()?;
        let mut db = db_client().await.map_err(|_| AuthError::Unavailable)?;
        let tx = db.transaction().await?;
        let user = auth::lock_actor(&tx, actor, false).await?;
        let locked = lock_source(&tx, source, Utc::now()).await?;
        let now = Utc::now();
        require_recent(&locked, now)?;
        replace_recovery_codes_locked(&user.id, &tx, &codes).await?;
        let grant = preserved_full_grant(&locked, user.id, user.auth_version)?;
        let successor = issue_successor_locked(&tx, &locked, grant, now).await?;
        tx.commit().await?;
        Ok((successor, codes))
    }
}

/// The caller holds the account and initiating session locks. Plaintext codes
/// are generated before entering the transaction and returned only after commit.
pub async fn replace_recovery_codes_locked(
    user_id: &UserId,
    tx: &Transaction<'_>,
    codes: &[String],
) -> Result<(), AuthError> {
    tx.execute(
        "DELETE FROM auth_recovery_codes WHERE user_id=$1",
        &[&user_id.as_str()],
    )
    .await?;
    for code in codes {
        let hash = totp::recovery_hash(user_id, code).ok_or(AuthError::Unavailable)?;
        tx.execute(
            "INSERT INTO auth_recovery_codes(user_id,code_hash) VALUES($1,$2)",
            &[&user_id.as_str(), &hash],
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
