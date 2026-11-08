use tokio_postgres::{Client, Row};

use crate::{
    database::{
        models::{
            auth::UserId,
            statement_import::{StatementImportFile, StatementImportJob, StatementImportStatus, StatementImportsStatus},
        },
        queries::auth,
    },
    services::auth::AuthError,
};

// A dedicated session owns this lock from recovery through the last import write.
// Separate from market-data, pricing, and Telegram advisory-lock namespaces.
const LOCK_NAMESPACE: i32 = 0x44425349;
const LOCK_KEY: i32 = 1;

pub const INTERRUPTED_ERROR: &str = "Statement import interrupted before completion. Some records may already have been imported; review them before uploading the file again.";

pub(crate) struct ClaimedStatementImport {
    pub user_id: UserId,
    pub id: i64,
    pub filename: String,
    pub contents: Vec<u8>,
}

fn job_from_row(row: Row) -> anyhow::Result<StatementImportJob> {
    let status: &str = row.get("status");
    let status = match status {
        "queued" => StatementImportStatus::Queued,
        "running" => StatementImportStatus::Running,
        "succeeded" => StatementImportStatus::Succeeded,
        "failed" => StatementImportStatus::Failed,
        _ => anyhow::bail!("Invalid persisted statement import status"),
    };
    Ok(StatementImportJob {
        id: row.get::<_, i64>("id").to_string(),
        filename: row.get("filename"),
        size_bytes: row.get("size_bytes"),
        status,
        queued_at: row.get("queued_at"),
        started_at: row.get("started_at"),
        finished_at: row.get("finished_at"),
        error: row.get("error"),
    })
}

/// Commit the entire validated batch before making any file available to workers.
pub async fn enqueue(
    user_id: &UserId,
    db: &mut Client,
    files: &[StatementImportFile],
) -> anyhow::Result<StatementImportsStatus> {
    let transaction = db.transaction().await?;
    if !auth::lock_user(user_id, &transaction).await?.fully_enrolled() {
        return Err(AuthError::Unauthenticated.into());
    }
    let mut jobs = Vec::with_capacity(files.len());
    for file in files {
        let size_bytes = i64::try_from(file.contents.len())?;
        let row = transaction
            .query_one(
                "INSERT INTO statement_import_job (user_id, filename, size_bytes, contents)
                 VALUES ($1, $2, $3, $4)
                 RETURNING id, filename, size_bytes, status, queued_at, started_at, finished_at, error",
                &[&user_id.as_str(), &file.filename, &size_bytes, &file.contents],
            )
            .await?;
        jobs.push(job_from_row(row)?);
    }
    transaction.commit().await?;
    Ok(StatementImportsStatus { jobs })
}

pub async fn list(user_id: &UserId, db: &Client) -> anyhow::Result<StatementImportsStatus> {
    let rows = db
        .query(
            "SELECT id, filename, size_bytes, status, queued_at, started_at, finished_at, error
             FROM statement_import_job
             WHERE user_id = $1 AND (status IN ('queued', 'running') OR id IN (
                 SELECT id FROM statement_import_job WHERE user_id = $1 AND status IN ('succeeded', 'failed')
                 ORDER BY id DESC LIMIT 100
             )) ORDER BY id DESC",
            &[&user_id.as_str()],
        )
        .await?;
    let jobs = rows.into_iter().map(job_from_row).collect::<anyhow::Result<_>>()?;
    Ok(StatementImportsStatus { jobs })
}

pub(crate) async fn try_lock(db: &Client) -> anyhow::Result<bool> {
    Ok(db
        .query_one(
            "SELECT pg_try_advisory_lock($1, $2)",
            &[&LOCK_NAMESPACE, &LOCK_KEY],
        )
        .await?
        .get(0))
}

/// Caller must own the dedicated global session lock. A live owner on another
/// API replica prevents recovery from ever reaching this update.
pub(crate) async fn recover_interrupted_locked(db: &Client) -> anyhow::Result<()> {
    db.execute(
        "UPDATE statement_import_job SET status = 'failed', finished_at = clock_timestamp(),
         error = $1, contents = NULL WHERE status = 'running'",
        &[&INTERRUPTED_ERROR],
    )
    .await?;
    prune_all_users(db).await
}

/// Caller retains the global session lock until this file is terminal or its
/// parser future has been dropped. Only one queued file's bytes enter memory.
pub(crate) async fn claim_next_locked(db: &Client) -> anyhow::Result<Option<ClaimedStatementImport>> {
    let row = db
        .query_opt(
            "WITH next_job AS (
                 SELECT job.id FROM statement_import_job job
                 JOIN auth_users owner ON owner.id = job.user_id
                 WHERE job.status = 'queued' AND owner.status = 'active'
                   AND owner.username IS NOT NULL AND owner.password_hash IS NOT NULL
                   AND owner.totp_secret_encrypted IS NOT NULL
                 ORDER BY job.id LIMIT 1 FOR UPDATE OF owner
             )
             UPDATE statement_import_job SET status = 'running', started_at = clock_timestamp()
             WHERE id = (SELECT id FROM next_job)
             RETURNING user_id, id, filename, contents",
            &[],
        )
        .await?;
    Ok(row.map(|row| ClaimedStatementImport {
        user_id: UserId::from_database(row.get("user_id")),
        id: row.get("id"),
        filename: row.get("filename"),
        contents: row.get("contents"),
    }))
}

pub async fn finish_locked(user_id: &UserId, db: &Client, id: i64, error: Option<&str>) -> anyhow::Result<()> {
    let changed = db
        .execute(
            "UPDATE statement_import_job
             SET status = CASE WHEN $3::text IS NULL THEN 'succeeded' ELSE 'failed' END,
                 finished_at = clock_timestamp(), error = $3, contents = NULL
             WHERE user_id = $1 AND id = $2 AND status = 'running'",
            &[&user_id.as_str(), &id, &error],
        )
        .await?;
    anyhow::ensure!(changed == 1, "Statement import was no longer running");
    prune(user_id, db).await
}

async fn prune(user_id: &UserId, db: &Client) -> anyhow::Result<()> {
    db.execute(
        "DELETE FROM statement_import_job WHERE user_id = $1 AND id IN (
             SELECT id FROM statement_import_job WHERE user_id = $1 AND status IN ('succeeded', 'failed')
             ORDER BY id DESC OFFSET 100
         )",
        &[&user_id.as_str()],
    )
    .await?;
    Ok(())
}

async fn prune_all_users(db: &Client) -> anyhow::Result<()> {
    db.execute(
        "DELETE FROM statement_import_job WHERE id IN (
             SELECT id FROM (
                 SELECT id, row_number() OVER (PARTITION BY user_id ORDER BY id DESC) AS rank
                 FROM statement_import_job WHERE status IN ('succeeded', 'failed')
             ) terminal WHERE rank > 100
         )",
        &[],
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use tokio_postgres::NoTls;

    fn file(filename: &str) -> StatementImportFile {
        StatementImportFile {
            filename: filename.to_owned(),
            contents: b"date,amount\n".to_vec(),
        }
    }

    async fn active_owner(db: &mut Client, username: &str) -> UserId {
        let user_id = UserId::new();
        let tx = db.transaction().await.unwrap();
        tx.execute(
            "INSERT INTO auth_users (id, username, password_hash, role, status, totp_secret_encrypted)
             VALUES ($1, $2, 'fixture-password-hash', 'member', 'active', 'fixture-totp-ciphertext')",
            &[&user_id.as_str(), &username],
        ).await.unwrap();
        auth::initialize_private_defaults(&user_id, &tx).await.unwrap();
        tx.commit().await.unwrap();
        user_id
    }

    #[tokio::test]
    #[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
    async fn tenant_isolation_database_statement_queue_is_atomic_fifo_and_recovers() {
        let url = std::env::var("TEST_POSTGRES_URL").expect("Set an isolated TEST_POSTGRES_URL");
        let (mut first, connection) = tokio_postgres::connect(&url, NoTls).await.unwrap();
        let first_connection = tokio::spawn(connection);
        let schema = format!(
            "statement_queue_test_{}_{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        );
        first
            .batch_execute(&format!("CREATE SCHEMA {schema}; SET search_path TO {schema}"))
            .await
            .unwrap();
        refinery::embed_migrations!("migrations");
        migrations::runner().run_async(&mut first).await.unwrap();
        let (mut second, connection) = tokio_postgres::connect(&url, NoTls).await.unwrap();
        let second_connection = tokio::spawn(connection);
        second
            .batch_execute(&format!("SET search_path TO {schema}"))
            .await
            .unwrap();
        let user_id = active_owner(&mut first, "statement-alice").await;

        // A rejected insert rolls back the whole batch, not just its final file.
        let invalid = StatementImportFile { filename: "empty.csv".to_owned(), contents: vec![] };
        assert!(enqueue(&user_id, &mut first, &[file("valid.csv"), invalid]).await.is_err());
        assert!(list(&user_id, &second).await.unwrap().jobs.is_empty());
        let accepted = enqueue(&user_id, &mut first, &[file("one.CSV"), file("two.pdf"), file("three.csv")])
            .await.unwrap();
        assert_eq!(accepted.jobs.len(), 3);
        assert!(accepted.jobs.iter().all(|job| job.status == StatementImportStatus::Queued));
        let listed = list(&user_id, &second).await.unwrap();
        assert_eq!(listed.jobs[0].id, accepted.jobs[2].id);
        assert_eq!(serde_json::to_value(&listed.jobs[0]).unwrap()["id"], accepted.jobs[2].id);

        assert!(try_lock(&first).await.unwrap());
        assert!(!try_lock(&second).await.unwrap());
        recover_interrupted_locked(&first).await.unwrap();
        let running = claim_next_locked(&first).await.unwrap().unwrap();
        assert_eq!(running.id.to_string(), accepted.jobs[0].id);
        assert_eq!(running.user_id, user_id);
        assert_eq!(running.contents, b"date,amount\n");
        let active = list(&user_id, &second).await.unwrap();
        assert_eq!(active.jobs[2].status, StatementImportStatus::Running);
        assert!(active.jobs[2].started_at.is_some());
        assert!(active.jobs[2].finished_at.is_none());
        assert!(!try_lock(&second).await.unwrap());

        // Dropping the owning connection simulates API process death. The next
        // replica can recover the interrupted file without consuming queued files.
        drop(first);
        first_connection.await.unwrap().unwrap();
        assert!(try_lock(&second).await.unwrap());
        recover_interrupted_locked(&second).await.unwrap();
        let recovered = list(&user_id, &second).await.unwrap();
        assert_eq!(recovered.jobs[2].status, StatementImportStatus::Failed);
        assert!(recovered.jobs[2].error.is_some());
        assert!(recovered.jobs[2].finished_at.is_some());
        let row = second.query_one("SELECT contents FROM statement_import_job WHERE user_id = $1 AND id = $2", &[&user_id.as_str(), &running.id]).await.unwrap();
        assert!(row.get::<_, Option<Vec<u8>>>(0).is_none());

        let next = claim_next_locked(&second).await.unwrap().unwrap();
        assert_eq!(next.id.to_string(), accepted.jobs[1].id);
        finish_locked(&user_id, &second, next.id, Some("Safe parser failure")).await.unwrap();
        let last = claim_next_locked(&second).await.unwrap().unwrap();
        assert_eq!(last.id.to_string(), accepted.jobs[2].id);
        finish_locked(&user_id, &second, last.id, None).await.unwrap();
        assert!(claim_next_locked(&second).await.unwrap().is_none());
        let row = second.query_one("SELECT status, finished_at, contents, error FROM statement_import_job WHERE user_id = $1 AND id = $2", &[&user_id.as_str(), &last.id]).await.unwrap();
        assert_eq!(row.get::<_, &str>(0), "succeeded");
        assert!(row.get::<_, Option<DateTime<Utc>>>(1).is_some());
        assert!(row.get::<_, Option<Vec<u8>>>(2).is_none());
        assert!(row.get::<_, Option<String>>(3).is_none());

        exercise_owned_queue(&user_id, &mut second).await;
        second.batch_execute(&format!("SET search_path TO public; DROP SCHEMA {schema} CASCADE"))
            .await.unwrap();
        drop(second);
        second_connection.await.unwrap().unwrap();
    }

    async fn set_status(user_id: &UserId, db: &Client, status: &str) {
        db.execute(
            "UPDATE auth_users SET status = $2 WHERE id = $1",
            &[&user_id.as_str(), &status],
        ).await.unwrap();
    }

    async fn terminal_ids(user_id: &UserId, db: &Client) -> Vec<i64> {
        db.query(
            "SELECT id FROM statement_import_job WHERE user_id = $1 AND status IN ('succeeded', 'failed')
             ORDER BY id DESC LIMIT 100",
            &[&user_id.as_str()],
        ).await.unwrap().into_iter().map(|row| row.get(0)).collect()
    }

    async fn counts(user_id: &UserId, db: &Client) -> (i64, i64) {
        let row = db.query_one(
            "SELECT count(*) FILTER (WHERE status IN ('queued', 'running')),
                    count(*) FILTER (WHERE status IN ('succeeded', 'failed'))
             FROM statement_import_job WHERE user_id = $1",
            &[&user_id.as_str()],
        ).await.unwrap();
        (row.get(0), row.get(1))
    }

    async fn exercise_owned_queue(user_id: &UserId, db: &mut Client) {
        let other_id = active_owner(db, "statement-bob").await;
        let first = enqueue(user_id, db, &[file("alice-first.csv")]).await.unwrap().jobs.remove(0);
        let mut other_batch = enqueue(&other_id, db, &[file("bob.csv"), file("bob-pending.csv")]).await.unwrap().jobs;
        let other = other_batch.remove(0);
        let pending = other_batch.remove(0);
        let last = enqueue(user_id, db, &[file("alice-last.csv")]).await.unwrap().jobs.remove(0);
        let alice_jobs = list(user_id, db).await.unwrap().jobs;
        assert!(alice_jobs.iter().any(|job| job.id == first.id));
        assert!(alice_jobs.iter().any(|job| job.id == last.id));
        assert!(!alice_jobs.iter().any(|job| job.id == other.id));
        let bob_jobs = list(&other_id, db).await.unwrap().jobs;
        assert_eq!(bob_jobs.len(), 2);
        assert_eq!(bob_jobs[0].id, pending.id);
        assert_eq!(bob_jobs[1].id, other.id);

        // An inactive owner at the front does not block the global eligible FIFO.
        set_status(user_id, db, "disabled").await;
        let claimed = claim_next_locked(db).await.unwrap().unwrap();
        assert_eq!(claimed.user_id, other_id);
        assert_eq!(claimed.id.to_string(), other.id);
        assert!(finish_locked(user_id, db, claimed.id, None).await.is_err());
        assert_eq!(list(&other_id, db).await.unwrap().jobs.iter()
            .find(|job| job.id == other.id).unwrap().status, StatementImportStatus::Running);
        set_status(&other_id, db, "disabled").await;
        assert!(enqueue(user_id, db, &[file("denied.csv")]).await.is_err());
        assert!(enqueue(&other_id, db, &[file("denied.csv")]).await.is_err());
        // Once claimed, a disable does not abandon or reassign parser writes.
        finish_locked(&claimed.user_id, db, claimed.id, None).await.unwrap();
        assert!(claim_next_locked(db).await.unwrap().is_none());

        set_status(&other_id, db, "pending").await;
        assert!(enqueue(&other_id, db, &[file("unenrolled.csv")]).await.is_err());
        set_status(user_id, db, "active").await;
        let claimed = claim_next_locked(db).await.unwrap().unwrap();
        assert_eq!(claimed.user_id, *user_id);
        assert_eq!(claimed.id.to_string(), first.id);
        finish_locked(user_id, db, claimed.id, None).await.unwrap();
        let claimed = claim_next_locked(db).await.unwrap().unwrap();
        assert_eq!(claimed.user_id, *user_id);
        assert_eq!(claimed.id.to_string(), last.id);
        finish_locked(user_id, db, claimed.id, None).await.unwrap();
        assert!(claim_next_locked(db).await.unwrap().is_none());
        set_status(&other_id, db, "active").await;
        let claimed = claim_next_locked(db).await.unwrap().unwrap();
        assert_eq!(claimed.user_id, other_id);
        assert_eq!(claimed.id.to_string(), pending.id);
        finish_locked(&other_id, db, claimed.id, None).await.unwrap();

        // Model interrupted work plus queued files for both users, then
        // interleave more than 100 terminal uploads per owner.
        let alice = enqueue(user_id, db, &[file("alice-running.csv"), file("alice-queued.csv")]).await.unwrap();
        let bob = enqueue(&other_id, db, &[file("bob-running.csv"), file("bob-queued.csv")]).await.unwrap();
        for (owner, jobs) in [(user_id, &alice.jobs), (&other_id, &bob.jobs)] {
            let id = jobs[0].id.parse::<i64>().unwrap();
            db.execute(
                "UPDATE statement_import_job SET status = 'running', started_at = clock_timestamp()
                 WHERE user_id = $1 AND id = $2",
                &[&owner.as_str(), &id],
            ).await.unwrap();
        }
        db.execute(
            "INSERT INTO statement_import_job (user_id, filename, size_bytes, status, finished_at)
             SELECT owner, 'done.csv', 1, 'succeeded', now()
             FROM generate_series(1, 105) sequence
             CROSS JOIN (VALUES ($1::text), ($2::text)) owners(owner)
             ORDER BY sequence, owner",
            &[&user_id.as_str(), &other_id.as_str()],
        ).await.unwrap();
        let alice_latest = terminal_ids(user_id, db).await;
        let bob_latest = terminal_ids(&other_id, db).await;
        assert_eq!(list(user_id, db).await.unwrap().jobs.len(), 102);
        assert_eq!(list(&other_id, db).await.unwrap().jobs.len(), 102);
        let bob_before = counts(&other_id, db).await;
        prune(user_id, db).await.unwrap();
        assert_eq!(counts(user_id, db).await, (2, 100));
        assert_eq!(counts(&other_id, db).await, bob_before);

        finish_locked(user_id, db, alice.jobs[0].id.parse().unwrap(), None).await.unwrap();
        assert_eq!(counts(user_id, db).await, (1, 100));
        assert_eq!(counts(&other_id, db).await, bob_before);
        assert_eq!(list(&other_id, db).await.unwrap().jobs.iter()
            .find(|job| job.id == bob.jobs[0].id).unwrap().status, StatementImportStatus::Running);

        recover_interrupted_locked(db).await.unwrap();
        assert_eq!(counts(user_id, db).await, (1, 100));
        assert_eq!(counts(&other_id, db).await, (1, 100));
        assert_eq!(terminal_ids(user_id, db).await, alice_latest);
        assert_eq!(terminal_ids(&other_id, db).await, bob_latest);
        for (owner, queued) in [(user_id, &alice.jobs[1]), (&other_id, &bob.jobs[1])] {
            let listed = list(owner, db).await.unwrap();
            assert_eq!(listed.jobs.len(), 101);
            assert_eq!(listed.jobs.iter().find(|job| job.id == queued.id).unwrap().status, StatementImportStatus::Queued);
            let row = db.query_one(
                "SELECT contents FROM statement_import_job WHERE user_id = $1 AND id = $2",
                &[&owner.as_str(), &queued.id.parse::<i64>().unwrap()],
            ).await.unwrap();
            assert_eq!(row.get::<_, Option<Vec<u8>>>(0).unwrap(), b"date,amount\n");
        }
    }
}
