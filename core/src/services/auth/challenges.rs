use std::net::IpAddr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio_postgres::Transaction;
use typeshare::typeshare;
use utoipa::ToSchema;

use crate::database::{
    db_client,
    models::auth::{
        AccountStatus, AuthSession, AuthUser, CurrentUser, SessionStage, SessionSummary,
    },
    queries::auth,
};

use super::{
    password::canonical_username,
    rate_limit::{enforce, RatePurpose},
    require_browser, require_recent,
    session_store::{
        delete_locked, failed_factor_locked, issue_successor_locked, lock_source, IssuedSession,
        SessionGrant,
    },
    totp::{recovery_hash, verify_totp_step},
    AuthError, AuthService,
};

#[typeshare]
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum CodeType {
    Totp,
    Recovery,
}

#[typeshare]
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[typeshare]
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VerificationRequest {
    pub code: String,
    pub code_type: CodeType,
}

#[typeshare]
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReauthRequest {
    pub password: String,
    pub code: String,
    pub code_type: CodeType,
}

fn full_source(actor: &CurrentUser, source: &AuthSession) -> Result<(), AuthError> {
    require_browser(actor)?;
    if source.stage != SessionStage::Authenticated {
        return Err(AuthError::TwoFactorRequired);
    }
    if source.user_id.as_ref() != Some(&actor.id) || source.auth_version != Some(actor.auth_version)
    {
        return Err(AuthError::StateChanged);
    }
    Ok(())
}

impl AuthService {
    pub async fn login(
        &self,
        source: &AuthSession,
        username: String,
        password: String,
        peer: IpAddr,
    ) -> Result<IssuedSession, AuthError> {
        if source.stage != SessionStage::Anonymous {
            return Err(AuthError::Forbidden);
        }
        // Invalid names use the same canonical quota path and dummy KDF as
        // unknown names, rather than revealing validation/account state.
        let (canonical, valid_name) = match canonical_username(&username) {
            Ok(canonical) => (canonical, true),
            Err(_) => (username.trim().to_ascii_lowercase(), false),
        };
        enforce(RatePurpose::Password, &canonical, peer).await?;
        let mut client = db_client().await.map_err(|_| AuthError::Unavailable)?;
        let captured = if valid_name {
            auth::by_username(&client, &canonical).await?
        } else {
            None
        };
        let eligible = captured.as_ref().filter(|user| {
            user.fully_enrolled()
                || (user.status == AccountStatus::Pending
                    && user.username.is_some()
                    && user.password_hash.is_some())
        });
        let verification = self
            .passwords
            .verify(
                password,
                eligible.and_then(|user| user.password_hash.clone()),
            )
            .await?;
        if !verification.valid {
            return Err(AuthError::InvalidCredentials);
        }
        let captured = eligible.ok_or(AuthError::InvalidCredentials)?;

        // Password work is complete before acquiring any account/session locks.
        let tx = client.transaction().await?;
        let user = auth::lock_user(&captured.id, &tx)
            .await
            .map_err(|error| match error {
                AuthError::NotFound => AuthError::InvalidCredentials,
                other => other,
            })?;
        if user.auth_version != captured.auth_version
            || user.password_hash != captured.password_hash
            || user.username != captured.username
            || user.status != captured.status
            || user.status == AccountStatus::Disabled
        {
            return Err(AuthError::InvalidCredentials);
        }
        let now = Utc::now();
        let source = lock_source(&tx, source, now).await?;
        let grant = if user.fully_enrolled() {
            SessionGrant::MfaRequired {
                user_id: user.id.clone(),
                auth_version: user.auth_version,
            }
        } else if user.status == AccountStatus::Pending
            && user.username.is_some()
            && user.password_hash.is_some()
        {
            SessionGrant::EnrollmentRequired {
                user_id: user.id.clone(),
                auth_version: user.auth_version,
            }
        } else {
            return Err(AuthError::InvalidCredentials);
        };
        if let Some(hash) = verification.replacement_hash {
            tx.execute(
                "UPDATE auth_users SET password_hash=$2 WHERE id=$1",
                &[&user.id.as_str(), &hash],
            )
            .await?;
        }
        let issued = issue_successor_locked(&tx, &source, grant, now).await?;
        tx.commit().await?;
        Ok(issued)
    }

    /// The caller holds this account's row lock before locking the source or
    /// consuming a factor. Conditional writes also reject stale in-tx snapshots.
    pub(crate) async fn verify_factor_locked(
        &self,
        tx: &Transaction<'_>,
        user: &AuthUser,
        code: &str,
        code_type: CodeType,
        now: DateTime<Utc>,
    ) -> Result<bool, AuthError> {
        if !user.fully_enrolled() {
            return Ok(false);
        }
        match code_type {
            CodeType::Totp => {
                let encrypted = user
                    .totp_secret_encrypted
                    .as_deref()
                    .ok_or(AuthError::Unavailable)?;
                let secret = self.decrypt_totp(&user.id, false, encrypted)?;
                let timestamp =
                    u64::try_from(now.timestamp()).map_err(|_| AuthError::Unavailable)?;
                let Some(step) = verify_totp_step(&secret, code, timestamp, user.last_totp_step)
                else {
                    return Ok(false);
                };
                Ok(tx.execute(
                    "UPDATE auth_users SET last_totp_step=$2 WHERE id=$1 AND auth_version=$3 AND (last_totp_step IS NULL OR last_totp_step<$2)",
                    &[&user.id.as_str(), &step, &user.auth_version],
                ).await? == 1)
            }
            CodeType::Recovery => {
                let Some(hash) = recovery_hash(&user.id, code) else {
                    return Ok(false);
                };
                Ok(tx.execute(
                    "UPDATE auth_recovery_codes SET used_at=$3 WHERE user_id=$1 AND code_hash=$2 AND used_at IS NULL",
                    &[&user.id.as_str(), &hash, &now],
                ).await? == 1)
            }
        }
    }

    pub async fn verify_mfa(
        &self,
        source: &AuthSession,
        code: &str,
        code_type: CodeType,
        peer: IpAddr,
    ) -> Result<IssuedSession, AuthError> {
        if source.stage != SessionStage::MfaRequired {
            return Err(AuthError::Forbidden);
        }
        let owner = source.user_id.as_ref().ok_or(AuthError::Unauthenticated)?;
        enforce(RatePurpose::Factor, owner.as_str(), peer).await?;
        let mut client = db_client().await.map_err(|_| AuthError::Unavailable)?;
        let tx = client.transaction().await?;
        let user = auth::lock_user(owner, &tx)
            .await
            .map_err(|error| match error {
                AuthError::NotFound => AuthError::StateChanged,
                other => other,
            })?;
        let now = Utc::now();
        let source = lock_source(&tx, source, now).await?;
        if !self
            .verify_factor_locked(&tx, &user, code, code_type, now)
            .await?
        {
            failed_factor_locked(&tx, &source).await?;
            // A returned failure must not roll back the restricted-attempt limit.
            tx.commit().await?;
            return Err(AuthError::InvalidVerificationCode);
        }
        let issued =
            issue_successor_locked(&tx, &source, SessionGrant::full(&user, now), now).await?;
        tx.commit().await?;
        Ok(issued)
    }

    pub async fn reauthenticate(
        &self,
        actor: &CurrentUser,
        source: &AuthSession,
        password: String,
        code: &str,
        code_type: CodeType,
        peer: IpAddr,
    ) -> Result<IssuedSession, AuthError> {
        full_source(actor, source)?;
        enforce(RatePurpose::Password, &actor.username, peer).await?;
        enforce(RatePurpose::Factor, actor.id.as_str(), peer).await?;
        let mut client = db_client().await.map_err(|_| AuthError::Unavailable)?;
        let captured = auth::by_id(&actor.id, &client)
            .await?
            .ok_or(AuthError::StateChanged)?;
        auth::check_actor(actor, &captured, false)?;
        let verification = self
            .passwords
            .verify(password, captured.password_hash.clone())
            .await?;
        if !verification.valid {
            return Err(AuthError::InvalidCredentials);
        }

        let tx = client.transaction().await?;
        let user = auth::lock_actor(&tx, actor, false).await?;
        let now = Utc::now();
        let source = lock_source(&tx, source, now).await?;
        if user.password_hash != captured.password_hash {
            return Err(AuthError::StateChanged);
        }
        if !self
            .verify_factor_locked(&tx, &user, code, code_type, now)
            .await?
        {
            tx.commit().await?;
            return Err(AuthError::InvalidVerificationCode);
        }
        if let Some(hash) = verification.replacement_hash {
            tx.execute(
                "UPDATE auth_users SET password_hash=$2 WHERE id=$1",
                &[&user.id.as_str(), &hash],
            )
            .await?;
        }
        let grant = SessionGrant::Authenticated {
            user_id: user.id,
            auth_version: user.auth_version,
            absolute_expires_at: source.absolute_expires_at,
            reauthenticated_at: now,
            pending_totp: source
                .pending_totp
                .clone()
                .filter(|pending| pending.expires_at > now),
        };
        let issued = issue_successor_locked(&tx, &source, grant, now).await?;
        tx.commit().await?;
        Ok(issued)
    }

    pub async fn logout(&self, source: &AuthSession) -> Result<(), AuthError> {
        let mut client = db_client().await.map_err(|_| AuthError::Unavailable)?;
        let tx = client.transaction().await?;
        let source = lock_source(&tx, source, Utc::now()).await?;
        delete_locked(&tx, &source).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn sessions(
        &self,
        actor: &CurrentUser,
        source: &AuthSession,
    ) -> Result<Vec<SessionSummary>, AuthError> {
        full_source(actor, source)?;
        let mut client = db_client().await.map_err(|_| AuthError::Unavailable)?;
        let tx = client.transaction().await?;
        auth::lock_actor(&tx, actor, false).await?;
        let now = Utc::now();
        let source = lock_source(&tx, source, now).await?;
        let sessions = tx.query(
            "SELECT id_hash,created_at,last_seen_at,expires_at FROM auth_sessions WHERE user_id=$1 AND stage='authenticated' AND auth_version=$2 AND expires_at>$3 AND absolute_expires_at>$3 ORDER BY created_at,id_hash",
            &[&actor.id.as_str(), &actor.auth_version, &now],
        ).await?.into_iter().map(|row| {
            let id: String = row.get("id_hash");
            SessionSummary {
                current: id == source.id_hash,
                id,
                created_at: row.get("created_at"),
                last_seen_at: row.get("last_seen_at"),
                expires_at: row.get("expires_at"),
            }
        }).collect();
        tx.commit().await?;
        Ok(sessions)
    }

    pub async fn revoke_session(
        &self,
        actor: &CurrentUser,
        source: &AuthSession,
        id: &str,
    ) -> Result<bool, AuthError> {
        full_source(actor, source)?;
        let mut client = db_client().await.map_err(|_| AuthError::Unavailable)?;
        let tx = client.transaction().await?;
        auth::lock_actor(&tx, actor, false).await?;
        let now = Utc::now();
        let source = lock_source(&tx, source, now).await?;
        let target = tx.query_opt(
            "SELECT id_hash FROM auth_sessions WHERE id_hash=$1 AND user_id=$2 AND stage='authenticated' AND auth_version=$3 AND expires_at>$4 AND absolute_expires_at>$4 FOR UPDATE",
            &[&id, &actor.id.as_str(), &actor.auth_version, &now],
        ).await?.ok_or(AuthError::NotFound)?;
        let target: &str = target.get(0);
        let current = target == source.id_hash;
        if !current {
            require_recent(&source, now)?;
        }
        tx.execute("DELETE FROM auth_sessions WHERE id_hash=$1", &[&target])
            .await?;
        tx.commit().await?;
        Ok(current)
    }
}

#[cfg(test)]
mod tests;
