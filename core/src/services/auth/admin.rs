use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use tokio_postgres::Transaction;
use typeshare::typeshare;
use utoipa::ToSchema;

use crate::database::{
    db_client,
    models::auth::{
        AccountLinkKind, AccountStatus, AuthSession, AuthUser, CurrentUser, Role, SessionStage,
        UserSummary,
    },
    queries::auth,
};

use super::{
    links::{issue_link_locked, IssuedAccountLink},
    require_browser, require_recent,
    session_store::lock_source,
    AuthError, AuthService,
};

#[typeshare]
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UserPatchRequest {
    pub role: Option<Role>,
    pub disabled: Option<bool>,
}

#[typeshare]
#[derive(Serialize, ToSchema)]
pub struct UsersResponse {
    pub users: Vec<UserSummary>,
}

#[typeshare]
#[derive(Serialize, ToSchema)]
pub struct InviteSummary {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub consumed_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[typeshare]
#[derive(Serialize, ToSchema)]
pub struct InvitesResponse {
    pub invites: Vec<InviteSummary>,
}

fn browser_admin(actor: &CurrentUser, source: &AuthSession) -> Result<(), AuthError> {
    require_browser(actor)?;
    if actor.role != Role::Admin {
        return Err(AuthError::Forbidden);
    }
    if source.stage != SessionStage::Authenticated {
        return Err(AuthError::TwoFactorRequired);
    }
    if source.user_id.as_ref() != Some(&actor.id) || source.auth_version != Some(actor.auth_version)
    {
        return Err(AuthError::StateChanged);
    }
    Ok(())
}

async fn source_authority(
    tx: &Transaction<'_>,
    actor: &CurrentUser,
    source: &AuthSession,
    recent: bool,
) -> Result<(), AuthError> {
    let locked = lock_source(tx, source, Utc::now()).await?;
    browser_admin(actor, &locked)?;
    if recent {
        require_recent(&locked, Utc::now())?;
    }
    Ok(())
}

// HTTP IDs select a database row, never construct ownership authority.
async fn lock_target(
    tx: &Transaction<'_>,
    actor: &CurrentUser,
    id: &str,
) -> Result<AuthUser, AuthError> {
    let target = auth::user_from_row(
        tx.query_opt("SELECT * FROM auth_users WHERE id=$1", &[&id])
            .await?
            .ok_or(AuthError::NotFound)?,
    )?;
    let users = auth::lock_users(tx, &[&actor.id, &target.id]).await?;
    let locked_actor = users
        .iter()
        .find(|user| user.id == actor.id)
        .ok_or(AuthError::StateChanged)?;
    auth::check_actor(actor, locked_actor, true)?;
    users
        .into_iter()
        .find(|user| user.id == target.id)
        .ok_or(AuthError::NotFound)
}

impl AuthService {
    pub async fn list_users(
        &self,
        actor: &CurrentUser,
        source: &AuthSession,
    ) -> Result<UsersResponse, AuthError> {
        browser_admin(actor, source)?;
        let mut db = db_client().await.map_err(|_| AuthError::Unavailable)?;
        let tx = db.transaction().await?;
        auth::lock_actor(&tx, actor, true).await?;
        source_authority(&tx, actor, source, false).await?;
        let users = tx
            .query("SELECT * FROM auth_users ORDER BY created_at,id", &[])
            .await?
            .into_iter()
            .map(|row| auth::user_from_row(row).map(|user| UserSummary::from(&user)))
            .collect::<Result<_, _>>()?;
        tx.commit().await?;
        Ok(UsersResponse { users })
    }

    pub async fn update_user(
        &self,
        actor: &CurrentUser,
        source: &AuthSession,
        id: &str,
        patch: UserPatchRequest,
    ) -> Result<UserSummary, AuthError> {
        browser_admin(actor, source)?;
        let mut db = db_client().await.map_err(|_| AuthError::Unavailable)?;
        let tx = db.transaction().await?;
        auth::lock_instance(&tx).await?;
        let mut target = lock_target(&tx, actor, id).await?;
        let role = patch.role.unwrap_or(target.role);
        let status = match patch.disabled {
            Some(true) => AccountStatus::Disabled,
            Some(false)
                if target.username.is_some()
                    && target.password_hash.is_some()
                    && target.totp_secret_encrypted.is_some() =>
            {
                AccountStatus::Active
            }
            Some(false) => AccountStatus::Pending,
            None => target.status,
        };
        let changed = role != target.role || status != target.status;
        let issuer_invalidated = (target.role == Role::Admin && role != Role::Admin)
            || (target.status != AccountStatus::Disabled && status == AccountStatus::Disabled);
        auth::lock_links(&target.id, &tx, issuer_invalidated).await?;
        source_authority(&tx, actor, source, true).await?;
        if changed {
            if target.role == Role::Admin
                && target.fully_enrolled()
                && (role != Role::Admin || status != AccountStatus::Active)
            {
                let count: i64 = tx.query_one("SELECT count(*) FROM auth_users WHERE role='admin' AND status='active' AND username IS NOT NULL AND password_hash IS NOT NULL AND totp_secret_encrypted IS NOT NULL", &[]).await?.get(0);
                if count <= 1 {
                    return Err(AuthError::LastAdministrator);
                }
            }
            let now = Utc::now();
            tx.execute(
                "UPDATE auth_users SET role=$2,status=$3 WHERE id=$1",
                &[&target.id.as_str(), &role.as_str(), &status.as_str()],
            )
            .await?;
            target.auth_version = auth::bump_version(&target.id, &tx).await?;
            auth::invalidate_links(&target.id, &tx, issuer_invalidated, now).await?;
            auth::revoke_authority(&target.id, &tx, None, now).await?;
            target.role = role;
            target.status = status;
        }
        tx.commit().await?;
        Ok(UserSummary::from(&target))
    }

    pub async fn revoke_user_sessions(
        &self,
        actor: &CurrentUser,
        source: &AuthSession,
        id: &str,
    ) -> Result<(), AuthError> {
        browser_admin(actor, source)?;
        let mut db = db_client().await.map_err(|_| AuthError::Unavailable)?;
        let tx = db.transaction().await?;
        let target = lock_target(&tx, actor, id).await?;
        auth::lock_links(&target.id, &tx, false).await?;
        source_authority(&tx, actor, source, true).await?;
        let now = Utc::now();
        auth::bump_version(&target.id, &tx).await?;
        auth::invalidate_links(&target.id, &tx, false, now).await?;
        auth::revoke_authority(&target.id, &tx, None, now).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn password_reset_link(
        &self,
        actor: &CurrentUser,
        source: &AuthSession,
        id: &str,
    ) -> Result<IssuedAccountLink, AuthError> {
        browser_admin(actor, source)?;
        let mut db = db_client().await.map_err(|_| AuthError::Unavailable)?;
        let tx = db.transaction().await?;
        let target = lock_target(&tx, actor, id).await?;
        auth::lock_links(&target.id, &tx, false).await?;
        source_authority(&tx, actor, source, true).await?;
        if !target.fully_enrolled() {
            return Err(AuthError::Forbidden);
        }
        let now = Utc::now();
        let issued = issue_link_locked(
            &tx,
            self,
            AccountLinkKind::PasswordReset,
            Some(&target),
            Some(&actor.id),
            now,
            now + Duration::hours(1),
        )
        .await?;
        tx.commit().await?;
        Ok(issued)
    }

    pub async fn list_invites(
        &self,
        actor: &CurrentUser,
        source: &AuthSession,
    ) -> Result<InvitesResponse, AuthError> {
        browser_admin(actor, source)?;
        let mut db = db_client().await.map_err(|_| AuthError::Unavailable)?;
        let tx = db.transaction().await?;
        auth::lock_actor(&tx, actor, true).await?;
        source_authority(&tx, actor, source, false).await?;
        let invites = tx.query("SELECT id,created_at,expires_at,consumed_at,revoked_at FROM auth_links WHERE kind='invite' ORDER BY created_at,id", &[]).await?
            .into_iter().map(|row| InviteSummary { id: row.get("id"), created_at: row.get("created_at"), expires_at: row.get("expires_at"), consumed_at: row.get("consumed_at"), revoked_at: row.get("revoked_at") }).collect();
        tx.commit().await?;
        Ok(InvitesResponse { invites })
    }

    pub async fn create_invite(
        &self,
        actor: &CurrentUser,
        source: &AuthSession,
    ) -> Result<IssuedAccountLink, AuthError> {
        browser_admin(actor, source)?;
        let mut db = db_client().await.map_err(|_| AuthError::Unavailable)?;
        let tx = db.transaction().await?;
        auth::lock_actor(&tx, actor, true).await?;
        source_authority(&tx, actor, source, true).await?;
        let now = Utc::now();
        let issued = issue_link_locked(
            &tx,
            self,
            AccountLinkKind::Invite,
            None,
            Some(&actor.id),
            now,
            now + Duration::days(7),
        )
        .await?;
        tx.commit().await?;
        Ok(issued)
    }

    pub async fn revoke_invite(
        &self,
        actor: &CurrentUser,
        source: &AuthSession,
        id: &str,
    ) -> Result<(), AuthError> {
        browser_admin(actor, source)?;
        let mut db = db_client().await.map_err(|_| AuthError::Unavailable)?;
        let tx = db.transaction().await?;
        auth::lock_actor(&tx, actor, true).await?;
        let row = tx
            .query_opt(
                "SELECT consumed_at FROM auth_links WHERE id=$1 AND kind='invite' FOR UPDATE",
                &[&id],
            )
            .await?
            .ok_or(AuthError::NotFound)?;
        source_authority(&tx, actor, source, true).await?;
        if row.get::<_, Option<DateTime<Utc>>>(0).is_none() {
            tx.execute(
                "UPDATE auth_links SET revoked_at=COALESCE(revoked_at,$2) WHERE id=$1",
                &[&id, &Utc::now()],
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests;
