use chrono::{DateTime, Utc};
use tokio_postgres::{GenericClient, Row, Transaction};

use crate::{
    database::models::auth::{AccountStatus, AuthLink, AuthUser, CurrentUser, Role, UserId},
    services::auth::{require_browser, AuthError},
};

pub fn user_from_row(row: Row) -> Result<AuthUser, AuthError> {
    Ok(AuthUser {
        id: UserId::from_database(row.get("id")),
        username: row.get("username"),
        password_hash: row.get("password_hash"),
        role: row
            .get::<_, &str>("role")
            .parse()
            .map_err(|_| AuthError::Unavailable)?,
        status: row
            .get::<_, &str>("status")
            .parse()
            .map_err(|_| AuthError::Unavailable)?,
        totp_secret_encrypted: row.get("totp_secret_encrypted"),
        last_totp_step: row.get("last_totp_step"),
        auth_version: row.get("auth_version"),
        created_at: row.get("created_at"),
    })
}

pub async fn by_id(
    user_id: &UserId,
    client: &impl GenericClient,
) -> Result<Option<AuthUser>, AuthError> {
    client
        .query_opt("SELECT * FROM auth_users WHERE id=$1", &[&user_id.as_str()])
        .await?
        .map(user_from_row)
        .transpose()
}

pub async fn by_username(
    client: &impl GenericClient,
    username: &str,
) -> Result<Option<AuthUser>, AuthError> {
    client
        .query_opt("SELECT * FROM auth_users WHERE username=$1", &[&username])
        .await?
        .map(user_from_row)
        .transpose()
}

pub async fn lock_user(user_id: &UserId, tx: &Transaction<'_>) -> Result<AuthUser, AuthError> {
    let row = tx
        .query_opt(
            "SELECT * FROM auth_users WHERE id=$1 FOR UPDATE",
            &[&user_id.as_str()],
        )
        .await?
        .ok_or(AuthError::NotFound)?;
    user_from_row(row)
}

/// Lock all affected accounts in canonical ID order before any link/session rows.
pub async fn lock_users(tx: &Transaction<'_>, ids: &[&UserId]) -> Result<Vec<AuthUser>, AuthError> {
    let mut ids: Vec<&str> = ids.iter().map(|id| id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    tx.query(
        "SELECT * FROM auth_users WHERE id=ANY($1) ORDER BY id FOR UPDATE",
        &[&ids],
    )
    .await?
    .into_iter()
    .map(user_from_row)
    .collect()
}

pub fn check_actor(actor: &CurrentUser, locked: &AuthUser, admin: bool) -> Result<(), AuthError> {
    require_browser(actor)?;
    if locked.id != actor.id
        || !locked.fully_enrolled()
        || locked.auth_version != actor.auth_version
        || locked.role != actor.role
    {
        return Err(AuthError::StateChanged);
    }
    if admin && locked.role != Role::Admin {
        return Err(AuthError::Forbidden);
    }
    Ok(())
}

pub async fn lock_actor(
    tx: &Transaction<'_>,
    actor: &CurrentUser,
    admin: bool,
) -> Result<AuthUser, AuthError> {
    let locked = lock_user(&actor.id, tx).await?;
    check_actor(actor, &locked, admin)?;
    Ok(locked)
}

pub async fn lock_instance(tx: &Transaction<'_>) -> Result<UserId, AuthError> {
    let row = tx
        .query_one(
            "SELECT legacy_owner_user_id FROM auth_instance WHERE singleton=TRUE FOR UPDATE",
            &[],
        )
        .await?;
    Ok(UserId::from_database(row.get(0)))
}

#[cfg(test)]
pub async fn legacy_owner(client: &impl GenericClient) -> Result<UserId, AuthError> {
    Ok(UserId::from_database(
        client
            .query_one(
                "SELECT legacy_owner_user_id FROM auth_instance WHERE singleton=TRUE",
                &[],
            )
            .await?
            .get(0),
    ))
}

pub async fn setup_required(client: &impl GenericClient) -> Result<bool, AuthError> {
    Ok(client.query_one(
        "SELECT u.username IS NULL AND u.password_hash IS NULL FROM auth_instance i JOIN auth_users u ON u.id=i.legacy_owner_user_id WHERE i.singleton=TRUE",
        &[],
    ).await?.get(0))
}

pub async fn active_users(client: &impl GenericClient) -> Result<Vec<UserId>, AuthError> {
    Ok(client.query("SELECT id FROM auth_users WHERE status='active' AND username IS NOT NULL AND password_hash IS NOT NULL AND totp_secret_encrypted IS NOT NULL ORDER BY id", &[])
        .await?.into_iter().map(|row| UserId::from_database(row.get(0))).collect())
}

pub async fn initialize_private_defaults(
    user_id: &UserId,
    tx: &Transaction<'_>,
) -> Result<(), AuthError> {
    tx.execute(
        "INSERT INTO calculation_status(user_id,source) VALUES($1,'performance'),($1,'taxation')",
        &[&user_id.as_str()],
    )
    .await?;
    tx.execute(
        "INSERT INTO telegram_report_schedule(user_id) VALUES($1)",
        &[&user_id.as_str()],
    )
    .await?;
    tx.execute(
        "INSERT INTO user_notification_settings(user_id) VALUES($1)",
        &[&user_id.as_str()],
    )
    .await?;
    tx.execute(
        "INSERT INTO trading_connection(user_id,generation) VALUES($1,'unconfigured')",
        &[&user_id.as_str()],
    )
    .await?;
    Ok(())
}

pub async fn lock_links(
    user_id: &UserId,
    tx: &Transaction<'_>,
    issuer: bool,
) -> Result<(), AuthError> {
    tx.query("SELECT id FROM auth_links WHERE consumed_at IS NULL AND revoked_at IS NULL AND (user_id=$1 OR ($2 AND issued_by_user_id=$1)) ORDER BY id FOR UPDATE", &[&user_id.as_str(), &issuer]).await?;
    Ok(())
}

/// Lock affected links before session/token rows. Revoking an issuer also revokes invitations/reset links.
pub async fn invalidate_links(
    user_id: &UserId,
    tx: &Transaction<'_>,
    issuer: bool,
    now: DateTime<Utc>,
) -> Result<(), AuthError> {
    let rows = tx.query("SELECT id FROM auth_links WHERE consumed_at IS NULL AND revoked_at IS NULL AND (user_id=$1 OR ($2 AND issued_by_user_id=$1)) ORDER BY id FOR UPDATE", &[&user_id.as_str(), &issuer]).await?;
    for row in rows {
        let id: &str = row.get(0);
        tx.execute(
            "UPDATE auth_links SET revoked_at=$2 WHERE id=$1",
            &[&id, &now],
        )
        .await?;
    }
    Ok(())
}

pub async fn revoke_authority(
    user_id: &UserId,
    tx: &Transaction<'_>,
    excluding_session: Option<&str>,
    now: DateTime<Utc>,
) -> Result<(), AuthError> {
    tx.execute(
        "DELETE FROM auth_sessions WHERE user_id=$1 AND ($2::text IS NULL OR id_hash<>$2)",
        &[&user_id.as_str(), &excluding_session],
    )
    .await?;
    tx.execute(
        "UPDATE auth_api_tokens SET revoked_at=$2 WHERE user_id=$1 AND revoked_at IS NULL",
        &[&user_id.as_str(), &now],
    )
    .await?;
    tx.execute("UPDATE trading_connection SET agent_token_hash=NULL,agent_auth_version=NULL WHERE user_id=$1", &[&user_id.as_str()]).await?;
    Ok(())
}

pub async fn bump_version(user_id: &UserId, tx: &Transaction<'_>) -> Result<i64, AuthError> {
    Ok(tx
        .query_one(
            "UPDATE auth_users SET auth_version=auth_version+1 WHERE id=$1 RETURNING auth_version",
            &[&user_id.as_str()],
        )
        .await?
        .get(0))
}

pub fn link_from_row(row: Row) -> Result<AuthLink, AuthError> {
    Ok(AuthLink {
        id: row.get("id"),
        kind: row
            .get::<_, &str>("kind")
            .parse()
            .map_err(|_| AuthError::Unavailable)?,
        user_id: row
            .get::<_, Option<String>>("user_id")
            .map(UserId::from_database),
        target_auth_version: row.get("target_auth_version"),
        expires_at: row.get("expires_at"),
        consumed_at: row.get("consumed_at"),
        revoked_at: row.get("revoked_at"),
    })
}

pub fn check_link(
    link: &AuthLink,
    target: Option<&AuthUser>,
    now: DateTime<Utc>,
) -> Result<(), AuthError> {
    if link.expires_at <= now || link.consumed_at.is_some() || link.revoked_at.is_some() {
        return Err(AuthError::InvalidAccountLink);
    }
    if let Some(user_id) = &link.user_id {
        let user = target.ok_or(AuthError::InvalidAccountLink)?;
        if &user.id != user_id
            || link.target_auth_version != Some(user.auth_version)
            || user.status == AccountStatus::Disabled
        {
            return Err(AuthError::InvalidAccountLink);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
