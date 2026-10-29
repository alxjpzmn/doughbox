use tokio_postgres::{Client, Row};

use crate::database::models::statement_import::{
    StatementImportFile, StatementImportJob, StatementImportStatus, StatementImportsStatus,
};

// A dedicated session owns this lock from recovery through the last import write.
// Separate from market-data, pricing, and Telegram advisory-lock namespaces.
const LOCK_NAMESPACE: i32 = 0x44425349;
const LOCK_KEY: i32 = 1;

pub const INTERRUPTED_ERROR: &str = "Statement import interrupted before completion. Some records may already have been imported; review them before uploading the file again.";

pub struct ClaimedStatementImport {
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
    db: &mut Client,
    files: &[StatementImportFile],
) -> anyhow::Result<StatementImportsStatus> {
    let transaction = db.transaction().await?;
    let mut jobs = Vec::with_capacity(files.len());
    for file in files {
        let size_bytes = i64::try_from(file.contents.len())?;
        let row = transaction
            .query_one(
                "INSERT INTO statement_import_job (filename, size_bytes, contents)
                 VALUES ($1, $2, $3)
                 RETURNING id, filename, size_bytes, status, queued_at, started_at, finished_at, error",
                &[&file.filename, &size_bytes, &file.contents],
            )
            .await?;
        jobs.push(job_from_row(row)?);
    }
    transaction.commit().await?;
    Ok(StatementImportsStatus { jobs })
}

pub async fn list(db: &Client) -> anyhow::Result<StatementImportsStatus> {
    let rows = db
        .query(
            "SELECT id, filename, size_bytes, status, queued_at, started_at, finished_at, error
             FROM statement_import_job
             WHERE status IN ('queued', 'running') OR id IN (
                 SELECT id FROM statement_import_job WHERE status IN ('succeeded', 'failed')
                 ORDER BY id DESC LIMIT 100
             ) ORDER BY id DESC",
            &[],
        )
        .await?;
    let jobs = rows.into_iter().map(job_from_row).collect::<anyhow::Result<_>>()?;
    Ok(StatementImportsStatus { jobs })
}

pub async fn try_lock(db: &Client) -> anyhow::Result<bool> {
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
pub async fn recover_interrupted_locked(db: &Client) -> anyhow::Result<()> {
    db.execute(
        "UPDATE statement_import_job SET status = 'failed', finished_at = clock_timestamp(),
         error = $1, contents = NULL WHERE status = 'running'",
        &[&INTERRUPTED_ERROR],
    )
    .await?;
    prune(db).await
}

/// Caller retains the global session lock until this file is terminal or its
/// parser future has been dropped. Only one queued file's bytes enter memory.
pub async fn claim_next_locked(db: &Client) -> anyhow::Result<Option<ClaimedStatementImport>> {
    let row = db
        .query_opt(
            "UPDATE statement_import_job SET status = 'running', started_at = clock_timestamp()
             WHERE id = (SELECT id FROM statement_import_job WHERE status = 'queued'
                         ORDER BY id LIMIT 1)
             RETURNING id, filename, contents",
            &[],
        )
        .await?;
    Ok(row.map(|row| ClaimedStatementImport {
        id: row.get("id"),
        filename: row.get("filename"),
        contents: row.get("contents"),
    }))
}

pub async fn finish_locked(db: &Client, id: i64, error: Option<&str>) -> anyhow::Result<()> {
    let changed = db
        .execute(
            "UPDATE statement_import_job
             SET status = CASE WHEN $2::text IS NULL THEN 'succeeded' ELSE 'failed' END,
                 finished_at = clock_timestamp(), error = $2, contents = NULL
             WHERE id = $1 AND status = 'running'",
            &[&id, &error],
        )
        .await?;
    anyhow::ensure!(changed == 1, "Statement import was no longer running");
    prune(db).await
}

async fn prune(db: &Client) -> anyhow::Result<()> {
    db.execute(
        "DELETE FROM statement_import_job WHERE id IN (
             SELECT id FROM statement_import_job WHERE status IN ('succeeded', 'failed')
             ORDER BY id DESC OFFSET 100
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

    #[tokio::test]
    #[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
    async fn statement_queue_is_atomic_fifo_serial_and_recovers_after_connection_loss() {
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
        tokio::spawn(async move { connection.await.unwrap() });
        second
            .batch_execute(&format!("SET search_path TO {schema}"))
            .await
            .unwrap();

        // A rejected insert rolls back the whole batch, not just its final file.
        let invalid = StatementImportFile { filename: "empty.csv".to_owned(), contents: vec![] };
        assert!(enqueue(&mut first, &[file("valid.csv"), invalid]).await.is_err());
        assert!(list(&second).await.unwrap().jobs.is_empty());
        let accepted = enqueue(&mut first, &[file("one.CSV"), file("two.pdf"), file("three.csv")])
            .await.unwrap();
        assert_eq!(accepted.jobs.len(), 3);
        assert!(accepted.jobs.iter().all(|job| job.status == StatementImportStatus::Queued));
        let listed = list(&second).await.unwrap();
        assert_eq!(listed.jobs[0].id, accepted.jobs[2].id);
        assert_eq!(serde_json::to_value(&listed.jobs[0]).unwrap()["id"], accepted.jobs[2].id);

        assert!(try_lock(&first).await.unwrap());
        assert!(!try_lock(&second).await.unwrap());
        recover_interrupted_locked(&first).await.unwrap();
        let running = claim_next_locked(&first).await.unwrap().unwrap();
        assert_eq!(running.id.to_string(), accepted.jobs[0].id);
        assert_eq!(running.contents, b"date,amount\n");
        let active = list(&second).await.unwrap();
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
        let recovered = list(&second).await.unwrap();
        assert_eq!(recovered.jobs[2].status, StatementImportStatus::Failed);
        assert!(recovered.jobs[2].error.as_ref().unwrap().contains("Some records"));
        assert!(recovered.jobs[2].finished_at.is_some());
        let row = second.query_one("SELECT contents FROM statement_import_job WHERE id = $1", &[&running.id]).await.unwrap();
        assert!(row.get::<_, Option<Vec<u8>>>(0).is_none());

        let next = claim_next_locked(&second).await.unwrap().unwrap();
        assert_eq!(next.id.to_string(), accepted.jobs[1].id);
        finish_locked(&second, next.id, Some("Safe parser failure")).await.unwrap();
        let last = claim_next_locked(&second).await.unwrap().unwrap();
        assert_eq!(last.id.to_string(), accepted.jobs[2].id);
        finish_locked(&second, last.id, None).await.unwrap();
        assert!(claim_next_locked(&second).await.unwrap().is_none());
        let row = second.query_one("SELECT status, finished_at, contents, error FROM statement_import_job WHERE id = $1", &[&last.id]).await.unwrap();
        assert_eq!(row.get::<_, &str>(0), "succeeded");
        assert!(row.get::<_, Option<DateTime<Utc>>>(1).is_some());
        assert!(row.get::<_, Option<Vec<u8>>>(2).is_none());
        assert!(row.get::<_, Option<String>>(3).is_none());

        // History retains all active uploads and only the newest 100 terminals.
        enqueue(&mut second, &[file("still-queued.csv")]).await.unwrap();
        second.batch_execute(
            "INSERT INTO statement_import_job (filename, size_bytes, status, finished_at)
             SELECT 'done.csv', 1, 'succeeded', now() FROM generate_series(1, 105)"
        ).await.unwrap();
        assert_eq!(list(&second).await.unwrap().jobs.len(), 101);
        prune(&second).await.unwrap();
        let count: i64 = second.query_one("SELECT count(*) FROM statement_import_job", &[]).await.unwrap().get(0);
        assert_eq!(count, 101);
        second.batch_execute(&format!("SET search_path TO public; DROP SCHEMA {schema} CASCADE"))
            .await.unwrap();
    }
}
