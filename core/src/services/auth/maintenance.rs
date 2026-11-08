use chrono::{DateTime, Utc};
use tokio::{
    sync::watch,
    task::JoinHandle,
    time::{interval, Duration},
};
use tokio_postgres::Client;

use super::AuthError;
use crate::database::db_client;

pub struct AuthMaintenance {
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl AuthMaintenance {
    pub fn start() -> Self {
        let (stop, mut stopped) = watch::channel(false);
        let task = tokio::spawn(async move {
            let mut hourly = interval(Duration::from_secs(3600));
            loop {
                tokio::select! {
                    _=hourly.tick()=>{
                        if maintain().await.is_err(){tracing::warn!("Authentication maintenance is temporarily unavailable");}
                    },
                    _=stopped.changed()=>break,
                }
            }
        });
        Self { stop, task }
    }
    pub async fn shutdown(self) {
        let _ = self.stop.send(true);
        let _ = self.task.await;
    }
}

async fn maintain() -> Result<(), AuthError> {
    let mut db = db_client().await.map_err(|_| AuthError::Unavailable)?;
    maintain_on(&mut db, Utc::now()).await
}

async fn maintain_on(db: &mut Client, now: DateTime<Utc>) -> Result<(), AuthError> {
    // This transaction takes link locks BEFORE touching referencing sessions.
    // Busy links are left to a later pass, never waited on while holding sessions.
    let tx = db.transaction().await?;
    let links:Vec<String>=tx.query("SELECT id FROM auth_links WHERE expires_at<=$1 ORDER BY id LIMIT 1000 FOR UPDATE SKIP LOCKED",&[&now]).await?.into_iter().map(|row|row.get(0)).collect();
    if !links.is_empty() {
        tx.execute("UPDATE auth_sessions SET link_id=NULL,link_bound_until=NULL,revision=revision+1 WHERE link_id=ANY($1)",&[&links]).await?;
        tx.execute("DELETE FROM auth_links WHERE id=ANY($1)", &[&links])
            .await?;
    }
    tx.commit().await?;

    let tx = db.transaction().await?;
    tx.execute(
        "DELETE FROM auth_sessions WHERE expires_at<=$1 OR absolute_expires_at<=$1",
        &[&now],
    )
    .await?;
    tx.execute("UPDATE auth_sessions SET pending_totp_id=NULL,pending_totp_secret_encrypted=NULL,pending_totp_expires_at=NULL,revision=revision+1 WHERE pending_totp_expires_at<=$1",&[&now]).await?;
    tx.commit().await?;

    let tx = db.transaction().await?;
    tx.execute("DELETE FROM auth_api_tokens WHERE expires_at<=$1", &[&now])
        .await?;
    tx.commit().await?;

    let tx = db.transaction().await?;
    tx.execute(
        "DELETE FROM auth_rate_limits WHERE window_start<$1::timestamptz-INTERVAL '2 days'",
        &[&now],
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        database::queries::auth,
        services::auth::{
            session_store::{create_anonymous, load, lock_source},
            tests::{cleanup, second_connection, test_database_at},
        },
    };
    use chrono::Duration;

    #[tokio::test]
    #[ignore = "Requires a dedicated TEST_POSTGRES_URL; also run in CI"]
    async fn auth_database_maintenance_skips_busy_links_and_invalidates_bound_revisions() {
        let (mut db, schema) = test_database_at(refinery::Target::Latest).await;
        let now = Utc::now();
        let owner = auth::legacy_owner(&db).await.unwrap();
        let tx = db.transaction().await.unwrap();
        let anonymous = create_anonymous(&tx, now).await.unwrap();
        tx.commit().await.unwrap();
        for id in ["busy-expired", "expired"] {
            db.execute("INSERT INTO auth_links(id,token_hash,kind,user_id,target_auth_version,created_at,expires_at) VALUES($1,$1,'bootstrap',$2,1,$3::timestamptz-INTERVAL '1 day',$3::timestamptz-INTERVAL '1 second')",&[&id,&owner.as_str(),&now]).await.unwrap();
        }
        db.execute("UPDATE auth_sessions SET link_id='expired',link_bound_until=$2::timestamptz-INTERVAL '1 second',pending_totp_id='pending',pending_totp_secret_encrypted='encrypted',pending_totp_expires_at=$2::timestamptz-INTERVAL '1 second',revision=revision+1 WHERE id_hash=$1",&[&anonymous.session.id_hash,&now]).await.unwrap();
        let stale = load(&db, &anonymous.raw_cookie, now)
            .await
            .unwrap()
            .unwrap();
        let mut other = second_connection(&schema).await;
        let busy = other.transaction().await.unwrap();
        busy.query_one(
            "SELECT id FROM auth_links WHERE id='busy-expired' FOR UPDATE",
            &[],
        )
        .await
        .unwrap();
        maintain_on(&mut db, now).await.unwrap();
        assert!(db
            .query_opt("SELECT id FROM auth_links WHERE id='expired'", &[])
            .await
            .unwrap()
            .is_none());
        assert!(db
            .query_opt("SELECT id FROM auth_links WHERE id='busy-expired'", &[])
            .await
            .unwrap()
            .is_some());
        let current = load(&db, &anonymous.raw_cookie, now)
            .await
            .unwrap()
            .unwrap();
        assert!(
            current.link_id.is_none()
                && current.link_bound_until.is_none()
                && current.pending_totp.is_none()
        );
        assert!(current.revision > stale.revision);
        let tx = db.transaction().await.unwrap();
        assert!(matches!(
            lock_source(&tx, &stale, now).await,
            Err(AuthError::StateChanged)
        ));
        tx.rollback().await.unwrap();
        busy.commit().await.unwrap();
        maintain_on(&mut db, now).await.unwrap();
        assert!(db
            .query_opt("SELECT id FROM auth_links WHERE id='busy-expired'", &[])
            .await
            .unwrap()
            .is_none());
        cleanup(db, &schema).await;
    }

    #[tokio::test]
    #[ignore = "Requires a dedicated TEST_POSTGRES_URL; also run in CI"]
    async fn auth_database_maintenance_prunes_only_expired_credentials_and_old_windows() {
        let (mut db, schema) = test_database_at(refinery::Target::Latest).await;
        let now = Utc::now();
        let owner = auth::legacy_owner(&db).await.unwrap();
        let tx = db.transaction().await.unwrap();
        let expired = create_anonymous(&tx, now - Duration::hours(1))
            .await
            .unwrap();
        let current = create_anonymous(&tx, now).await.unwrap();
        tx.commit().await.unwrap();
        db.execute("INSERT INTO auth_api_tokens(id,user_id,token_hash,name,scope,auth_version,created_at,expires_at) VALUES(gen_random_uuid(),$1,'expired','expired','read',1,$2::timestamptz-INTERVAL '90 days',$2),(gen_random_uuid(),$1,'current','current','read',1,$2,$2::timestamptz+INTERVAL '90 days')",&[&owner.as_str(),&now]).await.unwrap();
        db.execute("INSERT INTO auth_rate_limits(bucket_key,window_start,attempts) VALUES('old',$1::timestamptz-INTERVAL '3 days',1),('current',$1,1)",&[&now]).await.unwrap();
        maintain_on(&mut db, now).await.unwrap();
        assert!(load(&db, &expired.raw_cookie, now).await.unwrap().is_none());
        assert!(load(&db, &current.raw_cookie, now).await.unwrap().is_some());
        let tokens: Vec<String> = db
            .query("SELECT token_hash FROM auth_api_tokens", &[])
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get(0))
            .collect();
        assert_eq!(tokens, vec!["current"]);
        let buckets: Vec<String> = db
            .query("SELECT bucket_key FROM auth_rate_limits", &[])
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get(0))
            .collect();
        assert_eq!(buckets, vec!["current"]);
        cleanup(db, &schema).await;
    }
}
