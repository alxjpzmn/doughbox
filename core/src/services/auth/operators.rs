use chrono::{Duration, Utc};

use crate::database::{
    db_client,
    models::auth::{AccountLinkKind, AccountStatus},
    queries::auth,
};

use super::{
    links::{issue_link_locked, IssuedAccountLink},
    password::canonical_username,
    AuthError, AuthService,
};

impl AuthService {
    /// Console authority only: rotate the reserved owner's initial setup link.
    pub async fn bootstrap_link(&self) -> Result<IssuedAccountLink, AuthError> {
        let mut db = db_client().await.map_err(|_| AuthError::Unavailable)?;
        let tx = db.transaction().await?;
        let owner_id = auth::lock_instance(&tx).await?;
        let owner = auth::lock_user(&owner_id, &tx).await?;
        if owner.username.is_some() || owner.password_hash.is_some() {
            return Err(AuthError::InvalidInput(
                "Bootstrap is unavailable after account credentials have been chosen. Use login or console recovery.",
            ));
        }
        auth::lock_links(&owner.id, &tx, false).await?;
        let now = Utc::now();
        tx.execute(
            "UPDATE auth_links SET revoked_at=$2 WHERE user_id=$1 AND kind='bootstrap' AND consumed_at IS NULL AND revoked_at IS NULL",
            &[&owner.id.as_str(), &now],
        ).await?;
        let issued = issue_link_locked(
            &tx,
            self,
            AccountLinkKind::Bootstrap,
            Some(&owner),
            None,
            now,
            now + Duration::hours(24),
        )
        .await?;
        tx.commit().await?;
        Ok(issued)
    }

    /// Trusted console/database authority replaces both factors without ever
    /// decrypting the old authenticator. Redemption still requires enrollment.
    pub async fn operator_recover(
        &self,
        username: &str,
        enable: bool,
    ) -> Result<IssuedAccountLink, AuthError> {
        let username = canonical_username(username)?;
        let mut db = db_client().await.map_err(|_| AuthError::Unavailable)?;
        let tx = db.transaction().await?;
        auth::lock_instance(&tx).await?;
        let row = tx
            .query_opt(
                "SELECT * FROM auth_users WHERE username=$1 FOR UPDATE",
                &[&username],
            )
            .await?
            .ok_or(AuthError::NotFound)?;
        let mut user = auth::user_from_row(row)?;
        if user.status == AccountStatus::Disabled && !enable {
            return Err(AuthError::InvalidInput(
                "The account is disabled. Console recovery requires --enable to restore it.",
            ));
        }
        auth::lock_links(&user.id, &tx, true).await?;
        let now = Utc::now();
        user = auth::user_from_row(tx.query_one(
            "UPDATE auth_users SET password_hash=NULL,totp_secret_encrypted=NULL,last_totp_step=NULL,status='pending',auth_version=auth_version+1 WHERE id=$1 RETURNING *",
            &[&user.id.as_str()],
        ).await?)?;
        auth::invalidate_links(&user.id, &tx, true, now).await?;
        auth::revoke_authority(&user.id, &tx, None, now).await?;
        tx.execute(
            "DELETE FROM auth_recovery_codes WHERE user_id=$1",
            &[&user.id.as_str()],
        )
        .await?;
        let issued = issue_link_locked(
            &tx,
            self,
            AccountLinkKind::OperatorRecovery,
            Some(&user),
            None,
            now,
            now + Duration::minutes(30),
        )
        .await?;
        tx.commit().await?;
        Ok(issued)
    }
}

#[cfg(test)]
mod tests;
