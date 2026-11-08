use std::net::IpAddr;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use tokio_postgres::{GenericClient, Transaction};
use typeshare::typeshare;
use utoipa::ToSchema;

use crate::database::{
    db_client,
    models::auth::{
        AccountLinkKind, AccountLinkView, AccountStatus, AuthLink, AuthSession, AuthUser, Role,
        SessionStage, UserId,
    },
    queries::auth,
};

use super::{
    challenges::CodeType,
    digest,
    password::{canonical_username, validate_password},
    random_credential,
    rate_limit::{enforce, RatePurpose},
    session_store::{
        bind_link_locked, failed_factor_locked, issue_successor_locked, lock_source, IssuedSession,
        SessionGrant,
    },
    AuthError, AuthService,
};

#[typeshare]
#[derive(Serialize, ToSchema)]
pub struct IssuedAccountLink {
    pub id: String,
    pub url: String,
    pub expires_at: DateTime<Utc>,
}

#[typeshare]
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectLinkRequest {
    pub token: String,
}

#[typeshare]
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RegisterRequest {
    pub link_id: String,
    pub username: String,
    pub password: String,
}

#[typeshare]
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PasswordResetRequest {
    pub link_id: String,
    pub new_password: String,
    pub code: String,
    pub code_type: CodeType,
}

#[typeshare]
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RecoverRequest {
    pub link_id: String,
    pub new_password: String,
}

/// The caller owns account/link locks and must commit before exposing the URL.
pub(crate) async fn issue_link_locked(
    tx: &Transaction<'_>,
    service: &AuthService,
    kind: AccountLinkKind,
    target: Option<&AuthUser>,
    issuer: Option<&UserId>,
    now: DateTime<Utc>,
    expires_at: DateTime<Utc>,
) -> Result<IssuedAccountLink, AuthError> {
    if (kind == AccountLinkKind::Invite) != target.is_none() || expires_at <= now {
        return Err(AuthError::InvalidAccountLink);
    }
    let id = uuid::Uuid::new_v4().to_string();
    let token = random_credential("doughbox_link_")?;
    let hash = digest("doughbox-auth-link-v1", &token);
    let user_id = target.map(|user| user.id.as_str());
    let version = target.map(|user| user.auth_version);
    let issuer = issuer.map(UserId::as_str);
    let row = tx.query_one(
        "INSERT INTO auth_links(id,token_hash,kind,user_id,target_auth_version,issued_by_user_id,created_at,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8) RETURNING expires_at",
        &[&id, &hash, &kind.as_str(), &user_id, &version, &issuer, &now, &expires_at],
    ).await?;
    let expires_at = row.get("expires_at");
    let path = if matches!(kind, AccountLinkKind::Bootstrap | AccountLinkKind::Invite) {
        "register"
    } else {
        "recover"
    };
    Ok(IssuedAccountLink {
        id,
        url: format!("{}/{path}#token={token}", service.config.public_origin),
        expires_at,
    })
}

fn anonymous(source: &AuthSession) -> Result<(), AuthError> {
    if source.stage != SessionStage::Anonymous {
        return Err(AuthError::Forbidden);
    }
    Ok(())
}

fn binding(source: &AuthSession, link_id: &str, now: DateTime<Utc>) -> Result<(), AuthError> {
    anonymous(source)?;
    if source.link_id.as_deref() != Some(link_id)
        || !source.link_bound_until.is_some_and(|until| until > now)
    {
        return Err(AuthError::InvalidAccountLink);
    }
    Ok(())
}

async fn by_id(client: &impl GenericClient, id: &str) -> Result<AuthLink, AuthError> {
    let row = client
        .query_opt("SELECT * FROM auth_links WHERE id=$1", &[&id])
        .await?
        .ok_or(AuthError::InvalidAccountLink)?;
    auth::link_from_row(row)
}

async fn lock_target(tx: &Transaction<'_>, link: &AuthLink) -> Result<Option<AuthUser>, AuthError> {
    match &link.user_id {
        Some(id) => auth::lock_user(id, tx)
            .await
            .map(Some)
            .map_err(|error| match error {
                AuthError::NotFound => AuthError::InvalidAccountLink,
                other => other,
            }),
        None => Ok(None),
    }
}

async fn lock_link(tx: &Transaction<'_>, captured: &AuthLink) -> Result<AuthLink, AuthError> {
    let row = tx
        .query_opt(
            "SELECT * FROM auth_links WHERE id=$1 FOR UPDATE",
            &[&captured.id],
        )
        .await?
        .ok_or(AuthError::InvalidAccountLink)?;
    let link = auth::link_from_row(row)?;
    if link.kind != captured.kind
        || link.user_id != captured.user_id
        || link.target_auth_version != captured.target_auth_version
    {
        return Err(AuthError::InvalidAccountLink);
    }
    Ok(link)
}

fn link_view(link: &AuthLink, target: Option<&AuthUser>, until: DateTime<Utc>) -> AccountLinkView {
    AccountLinkView {
        id: link.id.clone(),
        kind: link.kind,
        username: if matches!(
            link.kind,
            AccountLinkKind::PasswordReset | AccountLinkKind::OperatorRecovery
        ) {
            target.and_then(|user| user.username.clone())
        } else {
            None
        },
        expires_at: link.expires_at.min(until),
    }
}

async fn consume(
    tx: &Transaction<'_>,
    link: &AuthLink,
    now: DateTime<Utc>,
) -> Result<(), AuthError> {
    if tx.execute(
        "UPDATE auth_links SET consumed_at=$2 WHERE id=$1 AND consumed_at IS NULL AND revoked_at IS NULL AND expires_at>$2",
        &[&link.id, &now],
    ).await? != 1 { return Err(AuthError::InvalidAccountLink); }
    Ok(())
}

fn username_error(error: tokio_postgres::Error) -> AuthError {
    if error.code() == Some(&tokio_postgres::error::SqlState::UNIQUE_VIOLATION)
        && error.as_db_error().and_then(|error| error.constraint())
            == Some("auth_users_username_key")
    {
        AuthError::UsernameUnavailable
    } else {
        AuthError::Unavailable
    }
}

impl AuthService {
    pub async fn inspect_link(
        &self,
        source: &AuthSession,
        token: &str,
        peer: IpAddr,
    ) -> Result<(AuthSession, AccountLinkView), AuthError> {
        anonymous(source)?;
        enforce(RatePurpose::Anonymous, "link-inspection", peer).await?;
        let mut db = db_client().await.map_err(|_| AuthError::Unavailable)?;
        let hash = digest("doughbox-auth-link-v1", token);
        let row = db
            .query_opt("SELECT * FROM auth_links WHERE token_hash=$1", &[&hash])
            .await?
            .ok_or(AuthError::InvalidAccountLink)?;
        let captured = auth::link_from_row(row)?;
        let tx = db.transaction().await?;
        let target = lock_target(&tx, &captured).await?;
        let link = lock_link(&tx, &captured).await?;
        let now = Utc::now();
        auth::check_link(&link, target.as_ref(), now)?;
        let locked = lock_source(&tx, source, now).await?;
        let until = (now + Duration::minutes(10)).min(link.expires_at);
        let session = bind_link_locked(&tx, &locked, &link.id, until).await?;
        let view = link_view(&link, target.as_ref(), until);
        tx.commit().await?;
        Ok((session, view))
    }

    pub async fn register(
        &self,
        source: &AuthSession,
        link_id: &str,
        username: String,
        password: String,
        peer: IpAddr,
    ) -> Result<IssuedSession, AuthError> {
        binding(source, link_id, Utc::now())?;
        let mut db = db_client().await.map_err(|_| AuthError::Unavailable)?;
        let captured = by_id(&db, link_id).await?;
        if !matches!(
            captured.kind,
            AccountLinkKind::Invite | AccountLinkKind::Bootstrap
        ) {
            return Err(AuthError::InvalidAccountLink);
        }
        enforce(
            RatePurpose::Password,
            captured.user_id.as_ref().map_or(link_id, UserId::as_str),
            peer,
        )
        .await?;
        let username = canonical_username(&username)?;
        validate_password(&password)?;
        let hash = self.passwords.hash(password).await?;
        let tx = db.transaction().await?;
        let reserved = if captured.kind == AccountLinkKind::Bootstrap {
            Some(auth::lock_instance(&tx).await?)
        } else {
            None
        };
        let target = lock_target(&tx, &captured).await?;
        let link = lock_link(&tx, &captured).await?;
        let now = Utc::now();
        auth::check_link(&link, target.as_ref(), now)?;
        let locked = lock_source(&tx, source, now).await?;
        binding(&locked, link_id, now)?;
        let (user_id, version) = match link.kind {
            AccountLinkKind::Bootstrap => {
                let target = target.as_ref().ok_or(AuthError::InvalidAccountLink)?;
                if reserved.as_ref() != Some(&target.id)
                    || target.role != Role::Admin
                    || target.status != AccountStatus::Pending
                    || target.username.is_some()
                    || target.password_hash.is_some()
                    || target.totp_secret_encrypted.is_some()
                {
                    return Err(AuthError::InvalidAccountLink);
                }
                tx.execute(
                    "UPDATE auth_users SET username=$2,password_hash=$3 WHERE id=$1",
                    &[&target.id.as_str(), &username, &hash],
                )
                .await
                .map_err(username_error)?;
                (target.id.clone(), target.auth_version)
            }
            AccountLinkKind::Invite => {
                let id = UserId::new();
                let row = tx.query_opt(
                    "INSERT INTO auth_users(id,username,password_hash,role,status) VALUES($1,$2,$3,'member','pending') ON CONFLICT(username) DO NOTHING RETURNING auth_version",
                    &[&id.as_str(), &username, &hash],
                ).await?.ok_or(AuthError::UsernameUnavailable)?;
                auth::initialize_private_defaults(&id, &tx).await?;
                (id, row.get(0))
            }
            _ => return Err(AuthError::InvalidAccountLink),
        };
        consume(&tx, &link, now).await?;
        let issued = issue_successor_locked(
            &tx,
            &locked,
            SessionGrant::EnrollmentRequired {
                user_id,
                auth_version: version,
            },
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(issued)
    }

    pub async fn reset_password(
        &self,
        source: &AuthSession,
        link_id: &str,
        new_password: String,
        code: &str,
        code_type: CodeType,
        peer: IpAddr,
    ) -> Result<IssuedSession, AuthError> {
        binding(source, link_id, Utc::now())?;
        let mut db = db_client().await.map_err(|_| AuthError::Unavailable)?;
        let captured = by_id(&db, link_id).await?;
        if captured.kind != AccountLinkKind::PasswordReset {
            return Err(AuthError::InvalidAccountLink);
        }
        let id = captured
            .user_id
            .as_ref()
            .ok_or(AuthError::InvalidAccountLink)?;
        enforce(RatePurpose::Password, id.as_str(), peer).await?;
        enforce(RatePurpose::Factor, id.as_str(), peer).await?;
        validate_password(&new_password)?;
        let hash = self.passwords.hash(new_password).await?;
        let tx = db.transaction().await?;
        let mut user = lock_target(&tx, &captured)
            .await?
            .ok_or(AuthError::InvalidAccountLink)?;
        auth::lock_links(&user.id, &tx, false).await?;
        let link = lock_link(&tx, &captured).await?;
        let now = Utc::now();
        auth::check_link(&link, Some(&user), now)?;
        if !user.fully_enrolled() {
            return Err(AuthError::InvalidAccountLink);
        }
        let locked = lock_source(&tx, source, now).await?;
        binding(&locked, link_id, now)?;
        if !self
            .verify_factor_locked(&tx, &user, code, code_type, now)
            .await?
        {
            failed_factor_locked(&tx, &locked).await?;
            tx.commit().await?;
            return Err(AuthError::InvalidVerificationCode);
        }
        consume(&tx, &link, now).await?;
        user.auth_version = tx.query_one(
            "UPDATE auth_users SET password_hash=$2,auth_version=auth_version+1 WHERE id=$1 RETURNING auth_version",
            &[&user.id.as_str(), &hash],
        ).await?.get(0);
        auth::invalidate_links(&user.id, &tx, false, now).await?;
        auth::revoke_authority(&user.id, &tx, Some(&locked.id_hash), now).await?;
        let issued =
            issue_successor_locked(&tx, &locked, SessionGrant::full(&user, now), now).await?;
        tx.commit().await?;
        Ok(issued)
    }

    pub async fn recover_account(
        &self,
        source: &AuthSession,
        link_id: &str,
        new_password: String,
        peer: IpAddr,
    ) -> Result<IssuedSession, AuthError> {
        binding(source, link_id, Utc::now())?;
        let mut db = db_client().await.map_err(|_| AuthError::Unavailable)?;
        let captured = by_id(&db, link_id).await?;
        if captured.kind != AccountLinkKind::OperatorRecovery {
            return Err(AuthError::InvalidAccountLink);
        }
        enforce(
            RatePurpose::Password,
            captured.user_id.as_ref().map_or(link_id, UserId::as_str),
            peer,
        )
        .await?;
        validate_password(&new_password)?;
        let hash = self.passwords.hash(new_password).await?;
        let tx = db.transaction().await?;
        let user = lock_target(&tx, &captured)
            .await?
            .ok_or(AuthError::InvalidAccountLink)?;
        let link = lock_link(&tx, &captured).await?;
        let now = Utc::now();
        auth::check_link(&link, Some(&user), now)?;
        if user.status != AccountStatus::Pending
            || user.username.is_none()
            || user.password_hash.is_some()
            || user.totp_secret_encrypted.is_some()
        {
            return Err(AuthError::InvalidAccountLink);
        }
        let locked = lock_source(&tx, source, now).await?;
        binding(&locked, link_id, now)?;
        tx.execute(
            "UPDATE auth_users SET password_hash=$2 WHERE id=$1",
            &[&user.id.as_str(), &hash],
        )
        .await?;
        consume(&tx, &link, now).await?;
        let issued = issue_successor_locked(
            &tx,
            &locked,
            SessionGrant::EnrollmentRequired {
                user_id: user.id,
                auth_version: user.auth_version,
            },
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(issued)
    }
}

#[cfg(test)]
mod tests;
