use std::net::IpAddr;

use chrono::{DateTime, TimeZone, Utc};
use tokio_postgres::Client;

use super::{digest, AuthError};
use crate::database::db_client;

#[derive(Clone, Copy)]
pub enum RatePurpose {
    Password,
    Factor,
    Anonymous,
}

impl RatePurpose {
    fn name(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::Factor => "factor",
            Self::Anonymous => "anonymous",
        }
    }
    fn window_seconds(self) -> i64 {
        match self {
            Self::Anonymous => 60,
            _ => 900,
        }
    }
}

pub async fn enforce(purpose: RatePurpose, account: &str, peer: IpAddr) -> Result<(), AuthError> {
    let mut db = db_client().await.map_err(|_| AuthError::Unavailable)?;
    enforce_on(&mut db, purpose, account, peer, Utc::now()).await
}

async fn enforce_on(
    db: &mut Client,
    purpose: RatePurpose,
    account: &str,
    peer: IpAddr,
    now: DateTime<Utc>,
) -> Result<(), AuthError> {
    let seconds = purpose.window_seconds();
    let start = Utc
        .timestamp_opt(now.timestamp().div_euclid(seconds) * seconds, 0)
        .single()
        .ok_or(AuthError::Unavailable)?;
    let mut buckets = vec![(
        format!(
            "{}:peer:{}",
            purpose.name(),
            digest("doughbox-auth-rate-v1", &peer.to_string())
        ),
        if matches!(purpose, RatePurpose::Anonymous) {
            60
        } else {
            50
        },
    )];
    if !matches!(purpose, RatePurpose::Anonymous) {
        buckets.push((
            format!(
                "{}:account:{}",
                purpose.name(),
                digest("doughbox-auth-rate-v1", account)
            ),
            10,
        ));
    }
    buckets.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let tx = db.transaction().await?;
    let mut exhausted = false;
    for (key, budget) in buckets {
        let attempts:i32=tx.query_one("INSERT INTO auth_rate_limits(bucket_key,window_start,attempts) VALUES($1,$2,1) ON CONFLICT(bucket_key,window_start) DO UPDATE SET attempts=LEAST(auth_rate_limits.attempts+1,2147483647) RETURNING attempts", &[&key,&start]).await?.get(0);
        exhausted |= attempts > budget;
    }
    tx.commit().await?;
    if exhausted {
        return Err(AuthError::RateLimited(
            ((start + chrono::Duration::seconds(seconds)) - now)
                .num_seconds()
                .max(1) as u64,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::auth::tests::{cleanup, second_connection, test_database_at};

    #[tokio::test]
    #[ignore = "Requires a dedicated TEST_POSTGRES_URL; also run in CI"]
    async fn auth_database_attempt_windows_are_shared_and_expire_without_lockout() {
        let (mut db, schema) = test_database_at(refinery::Target::Latest).await;
        let mut other = second_connection(&schema).await;
        let now = Utc.timestamp_opt(1_800_000_001, 0).single().unwrap();
        let peer = "127.0.0.1".parse().unwrap();
        for index in 0..10 {
            let client = if index % 2 == 0 { &mut db } else { &mut other };
            enforce_on(client, RatePurpose::Password, "alice", peer, now)
                .await
                .unwrap();
        }
        assert!(matches!(
            enforce_on(&mut other, RatePurpose::Password, "alice", peer, now).await,
            Err(AuthError::RateLimited(_))
        ));
        enforce_on(&mut db, RatePurpose::Password, "bob", peer, now)
            .await
            .unwrap();
        enforce_on(
            &mut db,
            RatePurpose::Password,
            "alice",
            peer,
            now + chrono::Duration::minutes(15),
        )
        .await
        .unwrap();
        let keys: Vec<String> = db
            .query("SELECT bucket_key FROM auth_rate_limits", &[])
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.get(0))
            .collect();
        assert!(keys.iter().all(|key| !key.contains("alice")
            && !key.contains("bob")
            && !key.contains("127.0.0.1")));
        cleanup(db, &schema).await;
    }
}
