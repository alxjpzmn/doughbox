use chrono::{DateTime, Utc};
use tokio_postgres::GenericClient;

use super::AuthError;
use crate::database::{models::auth::*, queries::auth};

pub async fn state(
    client: &impl GenericClient,
    session: &AuthSession,
    now: DateTime<Utc>,
) -> Result<AuthState, AuthError> {
    if session.expires_at <= now || session.absolute_expires_at <= now {
        return Err(AuthError::Unauthenticated);
    }
    let user = match &session.user_id {
        Some(id) => {
            let user = auth::by_id(id, client)
                .await?
                .ok_or(AuthError::Unauthenticated)?;
            if Some(user.auth_version) != session.auth_version
                || user.status == AccountStatus::Disabled
            {
                return Err(AuthError::Unauthenticated);
            }
            Some(AuthUserView {
                id: user.id.as_str().to_owned(),
                username: user.username.ok_or(AuthError::Unauthenticated)?,
                role: user.role,
                auth_version: user.auth_version,
            })
        }
        None => None,
    };
    let link = match (&session.link_id, session.link_bound_until) {
        (Some(id), Some(bound_until)) if bound_until > now => {
            match client
                .query_opt("SELECT * FROM auth_links WHERE id=$1", &[&id])
                .await?
            {
                Some(row) => {
                    let link = auth::link_from_row(row)?;
                    let target = match &link.user_id {
                        Some(id) => auth::by_id(id, client).await?,
                        None => None,
                    };
                    if auth::check_link(&link, target.as_ref(), now).is_ok() {
                        let username = if matches!(
                            link.kind,
                            AccountLinkKind::PasswordReset | AccountLinkKind::OperatorRecovery
                        ) {
                            target.and_then(|u| u.username)
                        } else {
                            None
                        };
                        Some(AccountLinkView {
                            id: link.id,
                            kind: link.kind,
                            username,
                            expires_at: link.expires_at.min(bound_until),
                        })
                    } else {
                        None
                    }
                }
                None => None,
            }
        }
        _ => None,
    };
    Ok(AuthState {
        stage: session.stage,
        user,
        csrf_token: session.csrf_token.clone(),
        expires_at: session.expires_at,
        setup_required: auth::setup_required(client).await?,
        link,
    })
}
