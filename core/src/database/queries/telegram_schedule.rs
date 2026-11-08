use chrono::{DateTime, NaiveTime, Utc};
use tokio_postgres::Client;

use crate::{
    database::models::auth::UserId,
    services::shared::settings::TelegramReportScheduleStatus,
};

// The worker owns a dedicated session for the whole report; no transaction spans
// valuation or HTTP. A hash collision may serialize owners, never select rows.

pub fn parse_send_time(value: &str) -> anyhow::Result<NaiveTime> {
    let bytes = value.as_bytes();
    anyhow::ensure!(
        bytes.len() == 5
            && bytes[2] == b':'
            && bytes[0..2].iter().all(u8::is_ascii_digit)
            && bytes[3..5].iter().all(u8::is_ascii_digit),
        "Use a 24-hour UTC time in HH:MM format"
    );
    NaiveTime::parse_from_str(value, "%H:%M")
        .map_err(|_| anyhow::anyhow!("Use a 24-hour UTC time in HH:MM format"))
}

pub async fn read(user_id: &UserId, db: &Client) -> anyhow::Result<TelegramReportScheduleStatus> {
    let row = db.query_one("SELECT * FROM telegram_report_schedule WHERE user_id=$1", &[&user_id.as_str()]).await?;
    Ok(TelegramReportScheduleStatus {
        enabled: row.get("enabled"),
        send_time_utc: row.get("send_time_utc"),
        next_report_at: row.get("next_report_at"),
        last_sent_at: row.get("last_sent_at"),
        last_status: row.get("last_status"),
        last_error: row.get("last_error"),
    })
}

/// Row serialization prevents a stale tick from undoing configuration changes.
/// Re-enabling or changing time always chooses the next strictly future slot.
pub async fn update_schedule(user_id: &UserId, db: &Client, enabled: bool, send_time_utc: &str) -> anyhow::Result<()> {
    parse_send_time(send_time_utc)?;
    db.execute(
        "UPDATE telegram_report_schedule SET enabled = $2, send_time_utc = $3,
         next_report_at = CASE
             WHEN NOT $2 THEN NULL
             WHEN NOT enabled OR send_time_utc <> $3 THEN
                 ((statement_timestamp() AT TIME ZONE 'UTC')::date + $3::text::time
                  + CASE WHEN (statement_timestamp() AT TIME ZONE 'UTC')::time >= $3::text::time
                         THEN interval '1 day' ELSE interval '0 days' END) AT TIME ZONE 'UTC'
             ELSE next_report_at END
         WHERE user_id=$1",
        &[&user_id.as_str(), &enabled, &send_time_utc],
    ).await?;
    Ok(())
}

/// Call only while owning the session lock. Coalesce missed days into one send,
/// advancing to the next future daily UTC deadline rather than replaying slots.
pub async fn start_scheduled(user_id: &UserId, db: &Client, now: DateTime<Utc>) -> anyhow::Result<bool> {
    let changed = db.execute(
        "UPDATE telegram_report_schedule SET last_status = 'running', last_error = NULL,
         next_report_at = (
             ($2::timestamptz AT TIME ZONE 'UTC')::date + send_time_utc::time
             + CASE WHEN ($2::timestamptz AT TIME ZONE 'UTC')::time >= send_time_utc::time
                    THEN interval '1 day' ELSE interval '0 days' END
         ) AT TIME ZONE 'UTC'
         WHERE user_id=$1 AND enabled AND next_report_at <= $2 AND last_status <> 'running'
           AND EXISTS (SELECT 1 FROM auth_users u WHERE u.id=$1 AND u.status='active'
             AND u.username IS NOT NULL AND u.password_hash IS NOT NULL AND u.totp_secret_encrypted IS NOT NULL)",
        &[&user_id.as_str(), &now],
    ).await?;
    Ok(changed != 0)
}

pub async fn try_lock(user_id: &UserId, db: &Client) -> anyhow::Result<bool> {
    Ok(db.query_one(
        "SELECT pg_try_advisory_lock(hashtextextended('doughbox:telegram:' || $1::text || ':report',0))",
        &[&user_id.as_str()],
    ).await?.get(0))
}

pub async fn unlock(user_id: &UserId, db: &Client) -> anyhow::Result<()> {
    db.query_one(
        "SELECT pg_advisory_unlock(hashtextextended('doughbox:telegram:' || $1::text || ':report',0))",
        &[&user_id.as_str()],
    ).await?;
    Ok(())
}

/// A live worker in any replica retains this lock and cannot be recovered.
pub async fn recover_interrupted(user_id: &UserId, db: &Client) -> anyhow::Result<()> {
    if try_lock(user_id, db).await? {
        let result = db.execute(
            "UPDATE telegram_report_schedule SET last_status = 'failed',
             last_error = 'Report interrupted before delivery could be confirmed. The next daily report remains scheduled.'
             WHERE user_id=$1 AND last_status = 'running'", &[&user_id.as_str()]).await;
        unlock(user_id, db).await?;
        result?;
    }
    Ok(())
}

/// Only a confirmed Telegram delivery advances last_sent_at. Errors passed here
/// are fixed, non-sensitive messages, never upstream bodies or request URLs.
pub async fn finish(user_id: &UserId, db: &Client, error: Option<&str>) -> anyhow::Result<()> {
    db.execute(
        "UPDATE telegram_report_schedule
         SET last_status = CASE WHEN $2::text IS NULL THEN 'succeeded' ELSE 'failed' END,
             last_error = $2,
             last_sent_at = CASE WHEN $2::text IS NULL THEN clock_timestamp() ELSE last_sent_at END
         WHERE user_id=$1", &[&user_id.as_str(), &error]).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeDelta;
    use tokio_postgres::NoTls;

    #[test]
    fn send_time_requires_strict_24_hour_utc() {
        for valid in ["00:00", "18:00", "23:59"] {
            assert!(parse_send_time(valid).is_ok());
        }
        for invalid in ["", "1:00", "18:0", "24:00", "23:60", "18:00:00", " 18:00", "18:00Z", "１２:００"] {
            assert!(parse_send_time(invalid).is_err(), "{invalid}");
        }
    }

    #[tokio::test]
    #[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
    async fn tenant_isolation_database_daily_claims_and_interrupted_recovery() {
        let url = std::env::var("TEST_POSTGRES_URL").expect("Set an isolated TEST_POSTGRES_URL");
        let (mut owner, connection) = tokio_postgres::connect(&url, NoTls).await.unwrap();
        let owner_connection = tokio::spawn(connection);
        let schema = format!("telegram_test_{}_{}", std::process::id(), Utc::now().timestamp_nanos_opt().unwrap());
        owner.batch_execute(&format!("CREATE SCHEMA {schema}; SET search_path TO {schema}")).await.unwrap();
        refinery::embed_migrations!("migrations");
        migrations::runner().run_async(&mut owner).await.unwrap();
        let (replica, connection) = tokio_postgres::connect(&url, NoTls).await.unwrap();
        tokio::spawn(connection);
        replica.batch_execute(&format!("SET search_path TO {schema}")).await.unwrap();
        let user_id = crate::database::queries::auth::legacy_owner(&owner).await.unwrap();
        owner.execute(
            "UPDATE auth_users SET username='alice',password_hash='test-phc',totp_secret_encrypted='test-factor',status='active' WHERE id=$1",
            &[&user_id.as_str()],
        ).await.unwrap();
        let other_user = UserId::new();
        owner.execute(
            "INSERT INTO auth_users(id,username,password_hash,totp_secret_encrypted,role,status) VALUES($1,'bob','test-phc','test-factor','member','active')",
            &[&other_user.as_str()],
        ).await.unwrap();
        let transaction = owner.transaction().await.unwrap();
        crate::database::queries::auth::initialize_private_defaults(&other_user, &transaction).await.unwrap();
        transaction.commit().await.unwrap();

        let initial = read(&user_id, &owner).await.unwrap();
        assert!(!initial.enabled);
        assert_eq!(initial.send_time_utc, "18:00");
        assert_eq!(initial.last_status, "not_run");
        assert!(initial.next_report_at.is_none());
        update_schedule(&user_id, &owner, true, "18:00").await.unwrap();
        let deadline = read(&user_id, &owner).await.unwrap().next_report_at.unwrap();
        assert!(deadline > Utc::now());
        assert!(deadline <= Utc::now() + TimeDelta::days(1));
        update_schedule(&user_id, &replica, true, "18:00").await.unwrap();
        assert_eq!(read(&user_id, &owner).await.unwrap().next_report_at, Some(deadline));
        update_schedule(&user_id, &owner, true, "00:00").await.unwrap();
        assert_ne!(read(&user_id, &owner).await.unwrap().next_report_at, Some(deadline));
        update_schedule(&user_id, &owner, false, "00:00").await.unwrap();
        assert!(read(&user_id, &owner).await.unwrap().next_report_at.is_none());
        assert!(!start_scheduled(&user_id, &owner, Utc::now()).await.unwrap());
        assert!(update_schedule(&user_id, &owner, true, "24:00").await.is_err());
        update_schedule(&user_id, &owner, true, "18:00").await.unwrap();
        assert!(!read(&other_user, &owner).await.unwrap().enabled);

        // Concurrent replicas compete for one session lock, not just the row.
        let (first_lock, second_lock) = tokio::join!(try_lock(&user_id, &owner), try_lock(&user_id, &replica));
        let first_lock = first_lock.unwrap();
        let second_lock = second_lock.unwrap();
        assert_ne!(first_lock, second_lock);
        if first_lock {
            unlock(&user_id, &owner).await.unwrap();
        } else {
            unlock(&user_id, &replica).await.unwrap();
        }

        // The same UTC wall-clock time is strictly future, including at the
        // exact boundary, and long downtime creates just one claim.
        for (now, expected) in [
            ("2026-10-28T17:59:59Z", "2026-10-28T18:00:00Z"),
            ("2026-10-28T18:00:00Z", "2026-10-29T18:00:00Z"),
            ("2026-12-31T23:59:59Z", "2027-01-01T18:00:00Z"),
        ] {
            owner.execute("UPDATE telegram_report_schedule SET next_report_at = '2020-01-01T00:00:00Z', last_status = 'not_run' WHERE user_id=$1", &[&user_id.as_str()]).await.unwrap();
            let now = now.parse::<DateTime<Utc>>().unwrap();
            assert!(try_lock(&user_id, &owner).await.unwrap());
            assert!(!try_lock(&user_id, &replica).await.unwrap());
            assert!(try_lock(&other_user, &replica).await.unwrap());
            unlock(&other_user, &replica).await.unwrap();
            assert!(start_scheduled(&user_id, &owner, now).await.unwrap());
            recover_interrupted(&user_id, &replica).await.unwrap();
            recover_interrupted(&other_user, &replica).await.unwrap();
            assert_eq!(read(&other_user, &replica).await.unwrap().last_status, "not_run");
            assert_eq!(read(&user_id, &replica).await.unwrap().last_status, "running");
            assert_eq!(read(&user_id, &owner).await.unwrap().next_report_at.unwrap(), expected.parse::<DateTime<Utc>>().unwrap());
            assert!(!start_scheduled(&user_id, &replica, now).await.unwrap());
            finish(&user_id, &owner, None).await.unwrap();
            unlock(&user_id, &owner).await.unwrap();
        }
        let sent = read(&user_id, &owner).await.unwrap().last_sent_at;
        owner.execute("UPDATE telegram_report_schedule SET next_report_at = '2020-01-01T00:00:00Z' WHERE user_id=$1", &[&user_id.as_str()]).await.unwrap();
        assert!(try_lock(&user_id, &owner).await.unwrap());
        assert!(start_scheduled(&user_id, &owner, Utc::now()).await.unwrap());
        // Pausing a running report does not prevent delivery outcome recording.
        update_schedule(&user_id, &replica, false, "19:30").await.unwrap();
        finish(&user_id, &owner, Some("Telegram delivery failed.")).await.unwrap();
        let failed = read(&user_id, &replica).await.unwrap();
        assert_eq!(failed.last_status, "failed");
        assert_eq!(failed.last_sent_at, sent);
        assert!(failed.next_report_at.is_none());
        unlock(&user_id, &owner).await.unwrap();
        update_schedule(&user_id, &owner, true, "19:30").await.unwrap();
        owner.execute("UPDATE telegram_report_schedule SET next_report_at = '2020-01-01T00:00:00Z' WHERE user_id=$1", &[&user_id.as_str()]).await.unwrap();
        owner.execute("UPDATE auth_users SET status='disabled' WHERE id=$1", &[&user_id.as_str()]).await.unwrap();
        assert!(!start_scheduled(&user_id, &owner, Utc::now()).await.unwrap());
        owner.execute("UPDATE auth_users SET status='pending' WHERE id=$1", &[&user_id.as_str()]).await.unwrap();
        assert!(!start_scheduled(&user_id, &owner, Utc::now()).await.unwrap());
        owner.execute("UPDATE auth_users SET status='active' WHERE id=$1", &[&user_id.as_str()]).await.unwrap();
        assert!(try_lock(&user_id, &owner).await.unwrap());
        assert!(start_scheduled(&user_id, &owner, Utc::now()).await.unwrap());
        let next = read(&user_id, &owner).await.unwrap().next_report_at;
        drop(owner);
        owner_connection.await.unwrap().unwrap();
        recover_interrupted(&user_id, &replica).await.unwrap();
        let recovered = read(&user_id, &replica).await.unwrap();
        assert_eq!(recovered.last_status, "failed");
        assert!(recovered.last_error.unwrap().contains("interrupted"));
        assert_eq!(recovered.next_report_at, next);
        assert!(!start_scheduled(&user_id, &replica, Utc::now()).await.unwrap());
        replica.batch_execute(&format!("SET search_path TO public; DROP SCHEMA {schema} CASCADE")).await.unwrap();
    }
}
