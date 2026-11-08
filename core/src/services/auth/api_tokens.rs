use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use tokio_postgres::GenericClient;
use typeshare::typeshare;
use utoipa::ToSchema;

use crate::database::{
    db_client,
    models::auth::{
        ApiTokenScope, ApiTokenSummary, AuthSession, CurrentUser, SessionStage, UserId,
    },
    queries::auth,
};

use super::{
    digest, random_credential, require_browser, require_recent, session_store::lock_source,
    AuthError, AuthService,
};

#[typeshare]
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateApiTokenRequest {
    pub name: String,
    pub scope: ApiTokenScope,
}

// The plaintext credential is serializable only for its committed, one-time response.
#[typeshare]
#[derive(Serialize, ToSchema)]
pub struct IssuedApiToken {
    pub id: String,
    pub token: String,
    pub scope: ApiTokenScope,
    pub expires_at: DateTime<Utc>,
}

#[typeshare]
#[derive(Serialize, ToSchema)]
pub struct ApiTokensResponse {
    pub tokens: Vec<ApiTokenSummary>,
}

fn full_source(actor: &CurrentUser, source: &AuthSession) -> Result<(), AuthError> {
    require_browser(actor)?;
    match source.stage {
        SessionStage::Anonymous => return Err(AuthError::Unauthenticated),
        SessionStage::MfaRequired | SessionStage::EnrollmentRequired => {
            return Err(AuthError::TwoFactorRequired)
        }
        SessionStage::Authenticated => {}
    }
    if source.user_id.as_ref() != Some(&actor.id) || source.auth_version != Some(actor.auth_version)
    {
        return Err(AuthError::StateChanged);
    }
    Ok(())
}

fn token_name(name: &str) -> Result<&str, AuthError> {
    let name = name.trim();
    if !(1..=80).contains(&name.chars().count()) {
        return Err(AuthError::InvalidInput(
            "Token names must contain 1–80 characters.",
        ));
    }
    Ok(name)
}

async fn summaries(
    user_id: &UserId,
    client: &impl GenericClient,
    now: DateTime<Utc>,
) -> Result<Vec<ApiTokenSummary>, AuthError> {
    client.query(
        "SELECT id::text,name,scope,created_at,expires_at FROM auth_api_tokens WHERE user_id=$1 AND revoked_at IS NULL AND expires_at>$2 ORDER BY created_at,id",
        &[&user_id.as_str(), &now],
    ).await?.into_iter().map(|row| {
        let scope = match row.get::<_, &str>("scope") {
            "read" => ApiTokenScope::Read,
            "write" => ApiTokenScope::Write,
            _ => return Err(AuthError::Unavailable),
        };
        Ok(ApiTokenSummary {
            id: row.get("id"),
            name: row.get("name"),
            scope,
            created_at: row.get("created_at"),
            expires_at: row.get("expires_at"),
        })
    }).collect()
}

impl AuthService {
    pub async fn api_tokens(
        &self,
        actor: &CurrentUser,
        source: &AuthSession,
    ) -> Result<Vec<ApiTokenSummary>, AuthError> {
        full_source(actor, source)?;
        let mut db = db_client().await.map_err(|_| AuthError::Unavailable)?;
        let tx = db.transaction().await?;
        let user = auth::lock_actor(&tx, actor, false).await?;
        lock_source(&tx, source, Utc::now()).await?;
        let tokens = summaries(&user.id, &tx, Utc::now()).await?;
        tx.commit().await?;
        Ok(tokens)
    }

    pub async fn create_api_token(
        &self,
        actor: &CurrentUser,
        source: &AuthSession,
        name: String,
        scope: ApiTokenScope,
    ) -> Result<IssuedApiToken, AuthError> {
        full_source(actor, source)?;
        let name = token_name(&name)?;
        let token = random_credential("doughbox_api_")?;
        let hash = digest("doughbox-auth-api-v1", &token);
        let id = uuid::Uuid::new_v4().to_string();
        let mut db = db_client().await.map_err(|_| AuthError::Unavailable)?;
        let tx = db.transaction().await?;
        let user = auth::lock_actor(&tx, actor, false).await?;
        let source = lock_source(&tx, source, Utc::now()).await?;
        let now = Utc::now();
        require_recent(&source, now)?;
        let row = tx.query_one(
            "INSERT INTO auth_api_tokens(id,user_id,token_hash,name,scope,auth_version,created_at,expires_at) VALUES($1::text::uuid,$2,$3,$4,$5,$6,$7,$8) RETURNING id::text,expires_at",
            &[&id, &user.id.as_str(), &hash, &name, &scope.as_str(), &user.auth_version, &now, &(now + Duration::days(90))],
        ).await?;
        let id = row.get("id");
        let expires_at = row.get("expires_at");
        tx.commit().await?;
        Ok(IssuedApiToken {
            id,
            token,
            scope,
            expires_at,
        })
    }

    pub async fn revoke_api_token(
        &self,
        actor: &CurrentUser,
        source: &AuthSession,
        id: &str,
    ) -> Result<(), AuthError> {
        full_source(actor, source)?;
        let mut db = db_client().await.map_err(|_| AuthError::Unavailable)?;
        let tx = db.transaction().await?;
        let user = auth::lock_actor(&tx, actor, false).await?;
        let source = lock_source(&tx, source, Utc::now()).await?;
        let now = Utc::now();
        require_recent(&source, now)?;
        if tx.execute(
            "UPDATE auth_api_tokens SET revoked_at=COALESCE(revoked_at,$3) WHERE user_id=$1 AND id::text=$2",
            &[&user.id.as_str(), &id, &now],
        ).await? != 1 {
            return Err(AuthError::NotFound);
        }
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
